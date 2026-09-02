//! Dual-mode Session, checkpoints `.hckp`, object query, opt, ECO.

use helion_bits::{bitgen, bitgen_pblock, eco_lut, Bitstream};
use helion_device::Device;
use helion_ir::{CellKind, Design, PortDir};
use helion_pack::{pack, Packed};
use helion_place::{place_in_region, place_incremental, place_with, PlaceOpts, Placed};
use helion_route::{route_with, RouteOpts, Routed, HOP_DELAY_PS};
use helion_sta::{create_clock, report_timing_routed};
use helion_hw::prog_sim;
use helion_debug::insert_ila;

/// UG986 Lab 1 Helion equivalents of implementation strategies.
/// Not Vivado strategy trademarks: same *kind* of lever (timing vs runtime vs phys).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImplStrategy {
    /// Timing-driven place + full PathFinder. Gold WNS path (`impl_1`).
    Default,
    /// Same engine as Default (timing-driven family).
    TimingExplore,
    /// Wirelength place + 1 PathFinder iter (faster, worse WNS).
    RuntimeOpt,
    /// Timing-driven place + directed extra hops (phys-opt detours).
    PhysOpt,
}

impl ImplStrategy {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" | "impl_1" => Ok(Self::Default),
            "timingexplore" | "timing_explore" | "explore" => Ok(Self::TimingExplore),
            "runtimeopt" | "runtime_opt" | "runtime" => Ok(Self::RuntimeOpt),
            "physopt" | "phys_opt" | "phys" => Ok(Self::PhysOpt),
            other => Err(format!("unknown impl strategy {other}")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::TimingExplore => "TimingExplore",
            Self::RuntimeOpt => "RuntimeOpt",
            Self::PhysOpt => "PhysOpt",
        }
    }

    pub fn place_opts(self) -> PlaceOpts {
        match self {
            Self::RuntimeOpt => PlaceOpts { timing_weight: 0.0 },
            _ => PlaceOpts { timing_weight: 0.75 },
        }
    }

    pub fn route_opts(self) -> RouteOpts {
        match self {
            Self::RuntimeOpt => RouteOpts {
                max_iters: 1,
                extra_hops: 0,
            },
            Self::PhysOpt => RouteOpts {
                max_iters: 8,
                extra_hops: 8,
            },
            _ => RouteOpts::default(),
        }
    }
}

/// UG986 Lab 2 Incremental Reuse Report (cells/nets/ports from HNF names).
#[derive(Clone, Debug, Default)]
pub struct ReuseReport {
    pub cells: usize,
    pub reused_cells: usize,
    pub nets: usize,
    pub reused_nets: usize,
    pub ports: usize,
    pub reused_ports: usize,
}

impl ReuseReport {
    pub fn cell_pct(&self) -> u32 {
        if self.cells == 0 {
            0
        } else {
            (self.reused_cells * 100 / self.cells) as u32
        }
    }

    pub fn text(&self) -> String {
        format!(
            "reuse cells={}/{} ({pct}%) nets={}/{} ports={}/{}",
            self.reused_cells,
            self.cells,
            self.reused_nets,
            self.nets,
            self.reused_ports,
            self.ports,
            pct = self.cell_pct()
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Project,
    NonProject,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub mode: Mode,
    pub design: Option<Design>,
    pub packed: Option<Packed>,
    pub placed: Option<Placed>,
    pub bitstream: Option<Bitstream>,
    pub routed: Option<Routed>,
    /// Last complete placement, used by incremental_impl (UG986 Lab 2).
    pub impl_checkpoint: Option<Placed>,
    pub hw_open: bool,
    pub programmed: bool,
    pub part: String,
}

impl Session {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            design: None,
            packed: None,
            placed: None,
            bitstream: None,
            routed: None,
            impl_checkpoint: None,
            hw_open: false,
            programmed: false,
            part: "HL10T-C32-1".into(),
        }
    }

    pub fn synth_design(&mut self, d: Design) {
        self.design = Some(d);
        self.reset_impl();
    }

    /// Drop place/route/bitstream (Vivado `reset_run impl_1`). Keeps the synth netlist.
    pub fn reset_impl(&mut self) {
        self.packed = None;
        self.placed = None;
        self.routed = None;
        self.bitstream = None;
        self.impl_checkpoint = None;
        self.programmed = false;
    }

    pub fn write_checkpoint(&mut self) -> Result<String, String> {
        let p = self.placed.as_ref().ok_or("write_checkpoint: not placed")?;
        self.impl_checkpoint = Some(p.clone());
        Ok(format!(
            "write_checkpoint lutff={}",
            p.lutff_sites.len()
        ))
    }

    /// Drop the synth netlist and every impl artifact (Vivado `reset_run synth_1`).
    pub fn reset_synth(&mut self) {
        self.design = None;
        self.reset_impl();
    }

    pub fn opt_design_step(&mut self) -> Result<usize, String> {
        let d = self.design.as_mut().ok_or("opt_design: no design")?;
        Ok(opt_design(d))
    }

    pub fn place_design(&mut self, dev: &Device) -> Result<(), String> {
        self.place_design_with(dev, ImplStrategy::Default.place_opts())
    }

    pub fn place_design_with(&mut self, dev: &Device, opts: PlaceOpts) -> Result<(), String> {
        let d = self.design.as_ref().ok_or("place_design: no design")?;
        let packed = pack(d, dev)?;
        // Default timing_weight 0.75 matches `helion run` / QoR gold (9640 ps).
        let placed = place_with(&packed, dev, opts)?;
        self.packed = Some(packed);
        self.placed = Some(placed);
        self.routed = None;
        self.bitstream = None;
        Ok(())
    }

    /// UG893 `create_pblock`/`resize_pblock`: place into a HAD rectangle.
    pub fn place_pblock(
        &mut self,
        dev: &Device,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    ) -> Result<(), String> {
        let d = self.design.as_ref().ok_or("place_pblock: no design")?;
        let packed = pack(d, dev)?;
        let placed = place_in_region(
            &packed,
            dev,
            ImplStrategy::Default.place_opts(),
            x0,
            y0,
            x1,
            y1,
        )?;
        self.packed = Some(packed);
        self.placed = Some(placed);
        self.routed = None;
        self.bitstream = None;
        Ok(())
    }

    /// Partial bitstream for Pblock sites (`helion-bits::bitgen_pblock`).
    pub fn write_pblock_bitstream(
        &self,
        dev: &Device,
        sites: &[(u32, u32)],
    ) -> Result<Bitstream, String> {
        let routed = self.routed.as_ref().ok_or("write_pblock: not routed")?;
        bitgen_pblock(dev, routed, sites)
    }

    pub fn route_design(&mut self, dev: &Device) -> Result<(), String> {
        self.route_design_with(dev, RouteOpts::default())
    }

    pub fn route_design_with(&mut self, dev: &Device, opts: RouteOpts) -> Result<(), String> {
        let placed = self.placed.as_ref().ok_or("route_design: not placed")?;
        let routed = route_with(placed, dev, opts)?;
        self.routed = Some(routed);
        self.bitstream = None;
        Ok(())
    }

    /// Full impl with a Lab 1 strategy. Assumes `design` is already synthesized.
    pub fn impl_with_strategy(&mut self, dev: &Device, strategy: ImplStrategy) -> Result<(), String> {
        if strategy == ImplStrategy::PhysOpt {
            let _ = self.opt_design_step()?;
        }
        self.place_design_with(dev, strategy.place_opts())?;
        self.route_design_with(dev, strategy.route_opts())?;
        self.write_bitstream(dev)?;
        Ok(())
    }

    /// UG986 Lab 2: place the current netlist reusing `prev` sites for named cells.
    pub fn incremental_place(
        &mut self,
        dev: &Device,
        prev: &Placed,
    ) -> Result<ReuseReport, String> {
        let d = self.design.as_ref().ok_or("incremental_place: no design")?;
        let packed = pack(d, dev)?;
        let (placed, reused_lutff) = place_incremental(
            &packed,
            dev,
            prev,
            PlaceOpts { timing_weight: 0.75 },
        )?;
        let prev_cells: std::collections::HashSet<&str> = prev
            .packed
            .lutffs
            .iter()
            .flat_map(|l| [l.lut_cell.as_str(), l.ff_cell.as_str()])
            .chain(prev.packed.iobs.iter().map(|i| i.cell.as_str()))
            .collect();
        let reused_cells = d
            .cells
            .iter()
            .filter(|c| prev_cells.contains(c.name.as_str()))
            .count();
        let prev_nets: std::collections::HashSet<&str> =
            prev.packed.lutffs.iter().map(|l| l.q_net.as_str()).collect();
        let reused_nets = d
            .nets
            .iter()
            .filter(|n| prev_nets.contains(n.name.as_str()))
            .count();
        let report = ReuseReport {
            cells: d.cells.len(),
            reused_cells,
            nets: d.nets.len(),
            reused_nets,
            ports: d.ports.len(),
            reused_ports: d.ports.len(),
        };
        let _ = reused_lutff;
        self.packed = Some(packed);
        self.placed = Some(placed);
        self.routed = None;
        self.bitstream = None;
        Ok(report)
    }

    /// UG986 Lab 3: drop an IOB net's route (delay 0).
    pub fn unroute_net(&mut self, net: &str) -> Result<String, String> {
        let r = self.routed.as_mut().ok_or("unroute_net: not routed")?;
        let idx = r
            .placed
            .packed
            .iobs
            .iter()
            .position(|i| i.from_net == net || i.cell == net)
            .ok_or_else(|| format!("unroute_net: no IOB net {net}"))?;
        if let Some(io) = r.iob_src.get_mut(idx) {
            io.hops = 0;
            io.delay_ps = 0;
        }
        self.bitstream = None;
        Ok(format!("unroute_net {net}"))
    }

    /// UG986 Lab 3: add FIXED_ROUTE extra hops (delay) on an IOB net.
    pub fn fix_route(&mut self, net: &str, extra_hops: u32) -> Result<String, String> {
        let r = self.routed.as_mut().ok_or("fix_route: not routed")?;
        let idx = r
            .placed
            .packed
            .iobs
            .iter()
            .position(|i| i.from_net == net || i.cell == net)
            .ok_or_else(|| format!("fix_route: no IOB net {net}"))?;
        if let Some(io) = r.iob_src.get_mut(idx) {
            io.hops += extra_hops;
            io.delay_ps += extra_hops as i64 * HOP_DELAY_PS;
        }
        self.bitstream = None;
        Ok(format!(
            "fix_route {net} extra_hops={extra_hops} delay_ps={}",
            r.iob_src.get(idx).map(|i| i.delay_ps).unwrap_or(0)
        ))
    }

    /// UG986 Lab 4 Check ECO: cells in the netlist that are not in the last placement.
    pub fn check_eco(&self) -> Result<String, String> {
        let d = self.design.as_ref().ok_or("check_eco: no design")?;
        let placed = self.placed.as_ref().ok_or("check_eco: not placed")?;
        let have: std::collections::HashSet<&str> = placed
            .packed
            .lutffs
            .iter()
            .map(|l| l.lut_cell.as_str())
            .chain(placed.packed.lutffs.iter().map(|l| l.ff_cell.as_str()))
            .chain(placed.packed.iobs.iter().map(|i| i.cell.as_str()))
            .collect();
        let missing: Vec<&str> = d
            .cells
            .iter()
            .map(|c| c.name.as_str())
            .filter(|n| !have.contains(n))
            .collect();
        Ok(format!(
            "check_eco missing={} {}",
            missing.len(),
            missing.join(",")
        ))
    }

    /// UG986 Lab 4: insert a LUT+FF pair named like ECO_LUT3 so pack can place it.
    pub fn insert_eco_lut(&mut self, name: &str, init: u64) -> Result<String, String> {
        let d = self.design.as_mut().ok_or("insert_eco_lut: no design")?;
        if d.cells.iter().any(|c| c.name == name) {
            return Err(format!("insert_eco_lut: {name} exists"));
        }
        let ff = format!("{name}_ff");
        d.add_cell(name, CellKind::Lut6 { init });
        d.add_cell(&ff, CellKind::Hff);
        if !d.ports.iter().any(|p| p.name == "clk") {
            d.add_port("clk", PortDir::In);
        }
        d.connect("clk", &ff, "CLK");
        d.connect(format!("{name}_d"), name, "O");
        d.connect(format!("{name}_d"), &ff, "D");
        d.connect(format!("{name}_q"), &ff, "Q");
        Ok(format!("insert_eco_lut {name} init={init:#x}"))
    }

    pub fn write_bitstream(&mut self, dev: &Device) -> Result<&Bitstream, String> {
        let routed = self.routed.as_ref().ok_or("write_bitstream: not routed")?;
        let bits = bitgen(dev, routed)?;
        self.bitstream = Some(bits);
        Ok(self.bitstream.as_ref().unwrap())
    }

    pub fn write_hnf(&self) -> Result<String, String> {
        Ok(self.design.as_ref().ok_or("write_hnf: no design")?.to_hnf())
    }

    pub fn report_timing(&self, dev: &Device) -> Result<String, String> {
        let _ = dev;
        let d = self.design.as_ref().ok_or("report_timing: no design")?;
        let r = self.routed.as_ref().ok_or("report_timing: not routed")?;
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let t = report_timing_routed(d, r, &clks)?;
        Ok(format!(
            "report_timing {} WNS_PS={} TNS_PS={} SETUP_PS={} HOLD_PS={} HOLD_SLACK_PS={} endpoints={} r2r_ps={} iob_ps={} route_ps={}",
            d.name, t.wns_ps, t.tns_ps, t.setup_ps, t.hold_ps, t.hold_slack_ps, t.endpoints, t.r2r_ps, t.iob_ps, t.route_ps
        ))
    }

    pub fn report_utilization(&self, dev: &Device) -> Result<String, String> {
        let p = self
            .placed
            .as_ref()
            .map(|pl| &pl.packed)
            .or(self.packed.as_ref())
            .ok_or("report_utilization: not packed")?;
        Ok(format!(
            "report_utilization LUTFF={}/{} IOB={}/{} BRAM={}/{} DSP={}/{}",
            p.lutffs.len(),
            dev.lut6_count(),
            p.iobs.len(),
            dev.iob_sites().count(),
            p.brams.len(),
            dev.n_bram,
            p.macs.len(),
            dev.n_dsp
        ))
    }

    pub fn open_hw_manager(&mut self) {
        self.hw_open = true;
    }

    pub fn program_hw(&mut self, dev: &Device) -> Result<String, String> {
        if !self.hw_open {
            return Err("program_hw: open_hw_manager first".into());
        }
        let bits = self.bitstream.as_ref().ok_or("program_hw: no bitstream")?;
        let st = prog_sim(dev, bits)?;
        self.programmed = true;
        Ok(format!(
            "program_hw DONE={} GWE={} CRC_ERR={}",
            st.done as u8, st.gwe as u8, st.crc_err as u8
        ))
    }

    pub fn mark_debug(&mut self, net: &str) -> Result<(), String> {
        let d = self.design.as_mut().ok_or("mark_debug: no design")?;
        d.mark_debug(net)?;
        insert_ila(d, net)?;
        Ok(())
    }

    pub fn set_property(&mut self, key: &str, val: &str, obj: &str) -> Result<(), String> {
        let d = self.design.as_mut().ok_or("set_property: no design")?;
        if key.eq_ignore_ascii_case("DONT_TOUCH") || key.eq_ignore_ascii_case("keep") {
            d.set_cell_attr(obj, "DONT_TOUCH", val)?;
        } else if key.eq_ignore_ascii_case("mark_debug") {
            d.set_net_attr(obj, "mark_debug", val)?;
        } else if key.eq_ignore_ascii_case("LOC") || key.eq_ignore_ascii_case("PACKAGE_PIN") {
            d.set_loc(obj, val)?;
        } else {
            d.set_cell_attr(obj, key, val)?;
        }
        Ok(())
    }

    pub fn impl_design(&mut self, d: Design, dev: &Device) -> Result<(), String> {
        self.synth_design(d);
        self.place_design(dev)?;
        self.route_design(dev)?;
        self.write_bitstream(dev)?;
        Ok(())
    }

    pub fn restore_session(bytes: &[u8], dev: &Device) -> Result<Self, String> {
        let (mode, hash, design) = Self::restore_with_ir(bytes)?;
        let mut s = Self::new(mode);
        s.part = dev.part.clone();
        if let Some(d) = design {
            s.impl_design(d, dev)?;
        }
        let h2 = s.blinky_hash().unwrap_or(0);
        if h2 != hash {
            return Err(format!(
                "hckp restore hash mismatch stored {hash:#x} got {h2:#x}"
            ));
        }
        Ok(s)
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
        let h = self.blinky_hash().unwrap_or(0);
        v.extend_from_slice(&h.to_le_bytes());
        if let Some(d) = &self.design {
            let hnf = d.to_hnf();
            let b = hnf.as_bytes();
            v.extend_from_slice(&(b.len() as u32).to_le_bytes());
            v.extend_from_slice(b);
        }
        v
    }

    pub fn restore(bytes: &[u8]) -> Result<(Mode, u32), String> {
        let (m, h, _) = Self::restore_with_ir(bytes)?;
        Ok((m, h))
    }

    pub fn restore_with_ir(bytes: &[u8]) -> Result<(Mode, u32, Option<Design>), String> {
        if bytes.len() < 9 || &bytes[0..4] != b"HCKP" {
            return Err("bad hckp".into());
        }
        let mode = match bytes[4] {
            1 => Mode::Project,
            2 => Mode::NonProject,
            _ => return Err("bad mode".into()),
        };
        let hash = u32::from_le_bytes(bytes[5..9].try_into().unwrap());
        let design = if bytes.len() >= 13 {
            let n = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
            if bytes.len() >= 13 + n {
                let text = std::str::from_utf8(&bytes[13..13 + n]).map_err(|e| e.to_string())?;
                Some(Design::from_hnf(text)?)
            } else {
                None
            }
        } else {
            None
        };
        Ok((mode, hash, design))
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
        if c.attrs.flag("DONT_TOUCH") || c.attrs.flag("keep") || ff.attrs.flag("DONT_TOUCH") {
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
        let (_, _, ir) = Session::restore_with_ir(&ck).unwrap();
        let ir = ir.expect("checkpoint must embed HNF");
        assert_eq!(ir.name, "blinky");
        assert_eq!(ir.lut_inits(), Design::structural_blinky().lut_inits());
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

        let mut kept = Design::structural_blinky();
        kept.add_cell("dead_lut", CellKind::Lut6 { init: 0 });
        kept.add_cell("dead_ff", CellKind::Hff);
        kept.connect("clk", "dead_ff", "CLK");
        kept.connect("dead_d", "dead_lut", "O");
        kept.connect("dead_d", "dead_ff", "D");
        kept.connect("dead_q", "dead_ff", "Q");
        kept.connect("dead_q", "dead_lut", "I0");
        kept.dont_touch("dead_lut").unwrap();
        let before = kept.cells.len();
        assert_eq!(opt_design(&mut kept), 0);
        assert_eq!(kept.cells.len(), before, "DONT_TOUCH must survive opt");

        let mut s = Session::new(Mode::NonProject);
        s.impl_design(Design::structural_blinky(), &dev).unwrap();
        let h0 = s.blinky_hash();
        s.eco(&dev, "u_lut", 0xAAAA_AAAA_AAAA_AAAA).unwrap();
        assert_ne!(s.blinky_hash(), h0, "ECO must change bitstream hash");
        let _ = PortDir::In;
    }

    #[test]
    fn tcl_session_steps_hit_engines_and_hckp_restores_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::NonProject);
        s.synth_design(Design::structural_counter());
        assert!(s.design.is_some());
        assert!(s.placed.is_none(), "synth must not place");
        let n = s.opt_design_step().unwrap();
        let _ = n;
        s.place_design(&dev).unwrap();
        assert!(s.placed.is_some());
        assert!(s.routed.is_none(), "place_design must not route");
        s.route_design(&dev).unwrap();
        assert!(s.routed.is_some());
        assert!(s.bitstream.is_none(), "route_design must not bitgen");
        let bits = s.write_bitstream(&dev).unwrap();
        assert!(!bits.frames.is_empty());
        let hnf = s.write_hnf().unwrap();
        assert!(hnf.starts_with("HNF 1"));
        let t = s.report_timing(&dev).unwrap();
        assert!(t.contains("WNS_PS="), "{t}");
        assert!(t.contains("HOLD_PS="), "{t}");
        assert!(!t.contains("report_timing ok"), "must hit STA engine: {t}");
        let u = s.report_utilization(&dev).unwrap();
        assert!(u.contains("LUTFF=4/8192"), "{u}");
        s.set_property("DONT_TOUCH", "true", "u_lut0").unwrap();
        assert!(s.design.as_ref().unwrap().cell("u_lut0").unwrap().attrs.flag("DONT_TOUCH"));
        s.open_hw_manager();
        let hw = s.program_hw(&dev).unwrap();
        assert!(hw.contains("DONE=1"), "{hw}");
        let h0 = s.blinky_hash().unwrap();
        let ck = s.checkpoint();
        let s2 = Session::restore_session(&ck, &dev).unwrap();
        assert_eq!(s2.blinky_hash(), Some(h0), ".hckp restore must match bitstream hash");
        let die = dev.report_die();
        assert!(die.contains("HL10T-C32-1"));
        s.mark_debug("q3").unwrap();
        assert!(s.design.as_ref().unwrap().net("q3").unwrap().attrs.flag("mark_debug"));
        assert!(get_cells(s.design.as_ref().unwrap(), None).iter().any(|c| c.contains("lut")));
        assert!(!get_pins(s.design.as_ref().unwrap(), "u_lut0").is_empty());
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

    #[test]
    fn strategies_move_wns_and_incremental_reuses_cells() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut def = Session::new(Mode::NonProject);
        def.synth_design(Design::structural_counter());
        def.impl_with_strategy(&dev, ImplStrategy::Default).unwrap();
        let t_def = def.report_timing(&dev).unwrap();
        let mut rt = Session::new(Mode::NonProject);
        rt.synth_design(Design::structural_counter());
        rt.impl_with_strategy(&dev, ImplStrategy::RuntimeOpt).unwrap();
        let t_rt = rt.report_timing(&dev).unwrap();
        assert_ne!(t_def, t_rt, "RuntimeOpt WNS must differ from Default: {t_def} vs {t_rt}");
        let mut phys = Session::new(Mode::NonProject);
        phys.synth_design(Design::structural_counter());
        phys.impl_with_strategy(&dev, ImplStrategy::PhysOpt).unwrap();
        let t_phys = phys.report_timing(&dev).unwrap();
        assert_ne!(t_phys, t_def, "PhysOpt extra hops must move WNS: {t_phys} vs {t_def}");

        let prev = def.placed.clone().unwrap();
        let reuse = def.incremental_place(&dev, &prev).unwrap();
        assert_eq!(reuse.cell_pct(), 100, "{}", reuse.text());
        def.insert_eco_lut("ECO_LUT3", 0x8).unwrap();
        let chk = def.check_eco().unwrap();
        assert!(chk.contains("ECO_LUT3"), "{chk}");
        let reuse2 = def.incremental_place(&dev, &prev).unwrap();
        assert!(reuse2.reused_cells < reuse2.cells, "{}", reuse2.text());
        assert!(reuse2.reused_cells > 0, "{}", reuse2.text());
        def.route_design(&dev).unwrap();
        let led = def
            .placed
            .as_ref()
            .unwrap()
            .packed
            .iobs
            .first()
            .unwrap()
            .from_net
            .clone();
        let before = def.routed.as_ref().unwrap().iob_src[0].delay_ps;
        def.fix_route(&led, 3).unwrap();
        assert_eq!(
            def.routed.as_ref().unwrap().iob_src[0].delay_ps,
            before + 3 * helion_route::HOP_DELAY_PS
        );
        def.unroute_net(&led).unwrap();
        assert_eq!(def.routed.as_ref().unwrap().iob_src[0].delay_ps, 0);
    }
}
