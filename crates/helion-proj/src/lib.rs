//! Dual-mode Session, checkpoints `.hckp`, object query, opt, ECO.

use helion_bits::{bitgen, eco_lut, Bitstream};
use helion_device::Device;
use helion_ir::{CellKind, Design};
use helion_pack::pack;
use helion_place::place;
use helion_route::{route, Routed};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Project,
    NonProject,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub mode: Mode,
    pub design: Option<Design>,
    pub bitstream: Option<Bitstream>,
    pub routed: Option<Routed>,
}

impl Session {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            design: None,
            bitstream: None,
            routed: None,
        }
    }

    pub fn impl_design(&mut self, d: Design, dev: &Device) -> Result<(), String> {
        let packed = pack(&d, dev)?;
        let placed = place(&packed, dev)?;
        let routed = route(&placed, dev)?;
        let bits = bitgen(dev, &routed)?;
        self.design = Some(d);
        self.bitstream = Some(bits);
        self.routed = Some(routed);
        Ok(())
    }

    pub fn eco(&mut self, dev: &Device, cell: &str, new_init: u64) -> Result<(), String> {
        let r = self.routed.as_ref().ok_or("eco: not implemented")?;
        let bits = eco_lut(dev, r, cell, new_init)?;
        if let Some(rt) = self.routed.as_mut() {
            if let Some(lf) = rt
                .placed
                .packed
                .lutffs
                .iter_mut()
                .find(|l| l.lut_cell == cell || l.ff_cell == cell)
            {
                lf.init = new_init;
            }
        }
        self.bitstream = Some(bits);
        Ok(())
    }

    pub fn blinky_hash(&self) -> Option<u32> {
        self.bitstream.as_ref().map(|b| {
            let mut h = b.idcode;
            for ((bl, maj, min), w) in &b.frames {
                h ^= helion_bits::crc32c(&[*bl, *min]);
                h ^= *maj as u32;
                h = h.wrapping_mul(0x9E37_79B9) ^ helion_bits::crc32c(&w.to_le_bytes());
            }
            h
        })
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"HCKP");
        v.push(match self.mode {
            Mode::Project => 1,
            Mode::NonProject => 2,
        });
        if let Some(h) = self.blinky_hash() {
            v.extend_from_slice(&h.to_le_bytes());
        }
        v
    }

    pub fn restore(bytes: &[u8]) -> Result<(Mode, u32), String> {
        if bytes.len() < 9 || &bytes[0..4] != b"HCKP" {
            return Err("bad hckp".into());
        }
        let mode = match bytes[4] {
            1 => Mode::Project,
            2 => Mode::NonProject,
            _ => return Err("bad mode".into()),
        };
        let hash = u32::from_le_bytes(bytes[5..9].try_into().unwrap());
        Ok((mode, hash))
    }
}

pub fn get_cells(d: &Design, filter: Option<&str>) -> Vec<String> {
    d.cells
        .iter()
        .filter(|c| filter.map(|f| c.name.contains(f)).unwrap_or(true))
        .map(|c| c.name.clone())
        .collect()
}

pub fn get_nets(d: &Design, filter: Option<&str>) -> Vec<String> {
    d.nets
        .iter()
        .filter(|n| filter.map(|f| n.name.contains(f)).unwrap_or(true))
        .map(|n| n.name.clone())
        .collect()
}

pub fn get_pins(d: &Design, cell: &str) -> Vec<String> {
    d.nets
        .iter()
        .flat_map(|n| {
            n.endpoints
                .iter()
                .filter(|e| e.cell == cell)
                .map(|e| format!("{}/{}", e.cell, e.pin))
        })
        .collect()
}

/// Drop const-0 LUT+FF pairs that do not drive an IOB.
/// Vivado-like project file: `part`, `read_sv`, `read_vhdl`, `read_c`, `create_clock`, `set_property PACKAGE_PIN`.
#[derive(Clone, Debug, Default)]
pub struct ProjectFile {
    pub part: String,
    pub sources: Vec<String>,
    pub sdc: Vec<String>,
    pub package_pins: Vec<(String, String)>,
}

pub fn load_prj(text: &str) -> Result<ProjectFile, String> {
    let mut p = ProjectFile {
        part: "HL10T-C32-1".into(),
        ..Default::default()
    };
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut toks = line.split_whitespace();
        let Some(cmd) = toks.next() else { continue };
        match cmd {
            "part" => {
                if let Some(v) = toks.next() {
                    p.part = v.to_string();
                }
            }
            "read_sv" | "read_vhdl" | "read_c" | "read_verilog" | "sv" | "vhdl" | "c" => {
                if let Some(v) = toks.next() {
                    p.sources.push(v.to_string());
                }
            }
            "create_clock" => p.sdc.push(line.to_string()),
            "set_property" => {
                let rest: Vec<&str> = toks.collect();
                // set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]
                if rest.first().copied() == Some("PACKAGE_PIN") && rest.len() >= 2 {
                    let site = rest[1].to_string();
                    let joined = rest[2..].join(" ");
                    let port = joined
                        .split_once("get_ports")
                        .and_then(|(_, r)| {
                            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .find(|s| !s.is_empty())
                        })
                        .unwrap_or("")
                        .to_string();
                    if !port.is_empty() {
                        p.package_pins.push((port, site));
                    }
                }
            }
            _ => {}
        }
    }
    if p.sources.is_empty() {
        return Err("project has no sources".into());
    }
    Ok(p)
}

pub fn opt_design(d: &mut Design) -> usize {
    let iob_nets: std::collections::HashSet<String> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut))
        .filter_map(|c| d.net_on(&c.name, "I").map(|s| s.to_string()))
        .collect();
    let mut drop = Vec::new();
    for c in &d.cells {
        let CellKind::Lut6 { init: 0 } = c.kind else {
            continue;
        };
        let Some(o) = d.net_on(&c.name, "O") else {
            continue;
        };
        let Some(ff) = d.cells.iter().find(|f| {
            matches!(f.kind, CellKind::Hff) && d.net_on(&f.name, "D") == Some(o)
        }) else {
            continue;
        };
        let q = d.net_on(&ff.name, "Q").unwrap_or("");
        if iob_nets.contains(q) {
            continue;
        }
        drop.push(c.name.clone());
        drop.push(ff.name.clone());
    }
    let n = drop.len() / 2;
    d.cells.retain(|c| !drop.contains(&c.name));
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::{CellKind, Design, PortDir};

    #[test]
    fn dual_mode_same_hash_and_ckpt() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut proj = Session::new(Mode::Project);
        let mut np = Session::new(Mode::NonProject);
        proj.impl_design(Design::structural_blinky(), &dev).unwrap();
        np.impl_design(Design::structural_blinky(), &dev).unwrap();
        assert_eq!(proj.blinky_hash(), np.blinky_hash());
        let ck = proj.checkpoint();
        let (mode, h) = Session::restore(&ck).unwrap();
        assert_eq!(mode, Mode::Project);
        assert_eq!(Some(h), proj.blinky_hash());
        let cells = get_cells(proj.design.as_ref().unwrap(), Some("lut"));
        assert_eq!(cells, vec!["u_lut"]);
        assert!(get_nets(proj.design.as_ref().unwrap(), Some("q")).contains(&"q".into()));
        assert!(get_pins(proj.design.as_ref().unwrap(), "u_lut").iter().any(|p| p.ends_with("/I0")));
    }

    #[test]
    fn opt_drops_dead_const0_and_eco_changes_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.add_cell("dead_lut", CellKind::Lut6 { init: 0 });
        d.add_cell("dead_ff", CellKind::Hff);
        d.connect("clk", "dead_ff", "CLK");
        d.connect("dead_d", "dead_lut", "O");
        d.connect("dead_d", "dead_ff", "D");
        d.connect("dead_q", "dead_ff", "Q");
        d.connect("dead_q", "dead_lut", "I0");
        let before = d.cells.len();
        let n = opt_design(&mut d);
        assert_eq!(n, 1);
        assert!(d.cells.len() < before);
        assert!(d.cell("u_lut").is_some());
        assert!(d.cell("dead_lut").is_none());

        let mut s = Session::new(Mode::NonProject);
        s.impl_design(Design::structural_blinky(), &dev).unwrap();
        let h0 = s.blinky_hash();
        s.eco(&dev, "u_lut", 0xAAAA_AAAA_AAAA_AAAA).unwrap();
        assert_ne!(s.blinky_hash(), h0, "ECO must change bitstream hash");
        let _ = PortDir::In;
    }

    #[test]
    fn project_file_parses_vivado_shaped_commands() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/blinky.sv
create_clock -period 10.000 [get_ports clk]
set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]
"#,
        )
        .unwrap();
        assert_eq!(prj.part, "HL10T-C32-1");
        assert_eq!(prj.sources, vec!["examples/blinky.sv"]);
        assert_eq!(prj.sdc.len(), 1);
        assert_eq!(prj.package_pins, vec![("led".into(), "IOB_X2Y0".into())]);
        assert!(load_prj("part X\n").is_err());
    }
}
