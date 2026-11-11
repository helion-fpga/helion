//! Implementable-part registry and family flow dispatch.
//!
//! The New Project part picker must list exactly the parts a backend can
//! take from synthesis to a bitstream, and runs must dispatch to that
//! family's backend. This crate is the single answer to both questions:
//!
//! - **Helion** (`HL10*`): every HAD part under `devices/helion/parts` that
//!   loads. The HL10T flow lives in helion-pack/place/route (the run worker
//!   drives it directly); this crate only lists the parts.
//! - **iCE40** (`ice40-hx8k`): IceStorm database embedded in helion-ice40;
//!   output is IceStorm `.asc` text (pack to `.bin` with `icepack`).
//! - **ECP5** (`LFE5U-85F`): Trellis database embedded in helion-ecp5;
//!   output is Trellis text config (`ecppack` makes the `.bit`).
//! - **Xilinx 7** (`xc7a35t`): needs the Project X-Ray database on disk
//!   (`$HELION_XILINX_DB` or `~/workspace/xilinx-dbsrc`). Without it the
//!   backend falls back to a two-pin synthetic model that cannot produce a
//!   usable bitstream, so the part is listed only when the database exists.

use helion_backend::Backend;
use helion_ir::Design;

/// Device family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    Helion,
    Ice40,
    Ecp5,
    Xilinx7,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Helion => "Helion",
            Family::Ice40 => "Lattice iCE40",
            Family::Ecp5 => "Lattice ECP5",
            Family::Xilinx7 => "Xilinx 7-Series",
        }
    }

    /// Bitstream artifact the family flow writes.
    pub fn artifact(self) -> (&'static str, &'static str) {
        match self {
            Family::Helion => ("bit", "Helion bitstream"),
            Family::Ice40 => ("asc", "IceStorm ASCII config (icepack → .bin)"),
            Family::Ecp5 => ("config", "Trellis text config (ecppack → .bit)"),
            Family::Xilinx7 => ("fasm", "FASM (xcfasm/frames2bit → .bit)"),
        }
    }
}

/// One part a backend can implement end to end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetPart {
    pub part: String,
    pub family: Family,
    /// Short resource summary for the picker.
    pub summary: String,
}

/// Root of the Project X-Ray database the xc7a35t backend needs.
pub fn xilinx_db_present() -> bool {
    helion_xilinx7::device::default_db_root()
        .join("artix7/xc7a50t/tilegrid.json")
        .is_file()
}

/// Every part some backend can take from netlist to bitstream on this
/// machine, grouped by family (Helion first), sorted within a family.
pub fn implementable_parts() -> Vec<TargetPart> {
    let mut out = Vec::new();
    for p in helion_device::Device::list_parts().unwrap_or_default() {
        if let Ok(d) = helion_device::Device::load_part(&p) {
            out.push(TargetPart {
                summary: format!(
                    "{} LUT6 · {} IOB · {} BRAM · {} DSP",
                    d.lut6_count(),
                    d.iob_sites().count(),
                    d.n_bram,
                    d.n_dsp
                ),
                part: p,
                family: Family::Helion,
            });
        }
    }
    out.push(TargetPart {
        part: "ice40-hx8k".into(),
        family: Family::Ice40,
        summary: "7680 LUT4 · 32 EBR · CT256 (IceStorm)".into(),
    });
    out.push(TargetPart {
        part: "LFE5U-85F".into(),
        family: Family::Ecp5,
        summary: "84K LUT4 · 208 EBR · 156 MULT18 (Trellis)".into(),
    });
    if xilinx_db_present() {
        out.push(TargetPart {
            part: "xc7a35t".into(),
            family: Family::Xilinx7,
            summary: "20800 LUT6 · 50 BRAM36 · csg324 (X-Ray)".into(),
        });
    }
    out
}

/// Family of a part name, or `None` for parts no backend implements.
pub fn family_of(part: &str) -> Option<Family> {
    implementable_parts()
        .into_iter()
        .find(|p| p.part.eq_ignore_ascii_case(part))
        .map(|p| p.family)
}

/// Apply `PACKAGE_PIN`/`LOC` constraints (port, pin) as port `LOC`
/// attributes, which every family packer honours. Returns the ports that do
/// not exist in the design (constraints for absent ports are reported, not
/// silently dropped).
pub fn apply_pin_constraints(design: &mut Design, pins: &[(String, String)]) -> Vec<String> {
    let mut missing = Vec::new();
    for (port, pin) in pins {
        if design.set_port_attr(port, "LOC", pin.clone()).is_err() {
            missing.push(port.clone());
        }
    }
    missing
}

/// Insert I/O buffers the family packers expect: helion-sv leaves top ports
/// as bare nets (the HL10T packer places its own IOBs), while the iCE40 /
/// ECP5 / 7-series packers map explicit `IobIn`/`IobOut` cells to pads. A
/// port that drives flip-flop clock pins also gets a `ClockBuf` so the
/// router promotes it to a global network. Ports that already have a pad
/// cell are left alone. Returns the number of buffers inserted.
pub fn insert_io_buffers(design: &mut Design) -> Result<usize, String> {
    use helion_ir::{CellKind, PortDir, intern, resolve};
    let mut added = 0;
    let ports: Vec<(String, PortDir)> = design
        .ports
        .iter()
        .map(|p| (resolve(p.name).to_string(), p.dir))
        .collect();
    for (port, dir) in ports {
        let Some(ni) = design.nets.iter().position(|n| resolve(n.name) == port) else {
            continue;
        };
        let has_pad = design.nets[ni].endpoints.iter().any(|e| resolve(e.pin) == "PAD");
        if has_pad {
            continue;
        }
        match dir {
            PortDir::In => {
                let is_clock = design.nets[ni]
                    .endpoints
                    .iter()
                    .any(|e| matches!(resolve(e.pin), "CLK" | "C" | "CK"));
                let inner = format!("{port}$pad_o");
                design.nets[ni].name = intern(&inner);
                design.rebuild_indexes();
                let ib = format!("{port}$ibuf");
                design.add_cell(&ib, CellKind::IobIn);
                design.connect(&port, &ib, "PAD");
                if is_clock {
                    let raw = format!("{port}$ibuf_o");
                    design.connect(&raw, &ib, "O");
                    let gb = format!("{port}$bufg");
                    design.add_cell(&gb, CellKind::ClockBuf);
                    design.connect(&raw, &gb, "I");
                    design.connect(&inner, &gb, "O");
                    added += 1;
                } else {
                    design.connect(&inner, &ib, "O");
                }
                added += 1;
            }
            PortDir::Out => {
                let inner = format!("{port}$pad_i");
                design.nets[ni].name = intern(&inner);
                design.rebuild_indexes();
                let ob = format!("{port}$obuf");
                design.add_cell(&ob, CellKind::IobOut);
                design.connect(&inner, &ob, "I");
                design.connect(&port, &ob, "PAD");
                added += 1;
            }
            _ => return Err(format!("port {port}: bidirectional ports are not supported by the family flows")),
        }
    }
    design.rebuild_indexes();
    Ok(added)
}

/// Output of a non-Helion family flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyBitstream {
    pub family: Family,
    pub part: String,
    /// File extension for the artifact (`asc`, `config`, `fasm`).
    pub ext: &'static str,
    /// The artifact text.
    pub text: String,
    /// Stage summary (`pack → place → route → bitstream`).
    pub stages: Vec<String>,
}

/// Run pack → place → route → bitstream for a non-Helion part. Each stage
/// is reported through `progress` as it finishes.
pub fn implement(
    design: &Design,
    part: &str,
    progress: &mut dyn FnMut(&str),
) -> Result<FamilyBitstream, String> {
    let family = family_of(part).ok_or_else(|| format!("no backend implements part {part}"))?;
    let mut owned = design.clone();
    let pads = insert_io_buffers(&mut owned)?;
    let design = &owned;
    let mut stages = Vec::new();
    let mut stage = |name: &str, detail: String, progress: &mut dyn FnMut(&str)| {
        let line = format!("{name}: {detail}");
        progress(&line);
        stages.push(line);
    };
    let (ext, _) = family.artifact();
    if pads > 0 {
        stage("io", format!("{pads} I/O buffer(s) inserted"), progress);
    }
    let text = match family {
        Family::Helion => {
            return Err(format!("{part} is a Helion part: run the HL10T flow"));
        }
        Family::Ice40 => {
            let b = helion_ice40::Ice40Backend::new(part);
            let packed = b.pack(design)?;
            stage("pack", format!("{} LUT, {} FF, {} IO", packed.luts.len(), packed.ffs.len(), packed.ios.len()), progress);
            let placed = b.place(&packed)?;
            stage("place", format!("{} LUT sites", placed.luts.len()), progress);
            let routed = b.route(&placed)?;
            stage("route", "routed".into(), progress);
            let asc = b.bitstream(&routed)?;
            stage("bitstream", format!("{} lines of .asc", asc.lines().count()), progress);
            asc
        }
        Family::Ecp5 => {
            let b = helion_ecp5::Ecp5Backend::open(part)?;
            let packed = b.pack(design)?;
            stage("pack", "packed".into(), progress);
            let placed = b.place(&packed)?;
            stage("place", "placed".into(), progress);
            let routed = b.route(&placed)?;
            stage("route", "routed".into(), progress);
            let cfg = b.bitstream(&routed)?;
            stage("bitstream", format!("{} lines of Trellis config", cfg.textcfg.lines().count()), progress);
            cfg.textcfg
        }
        Family::Xilinx7 => {
            let b = helion_xilinx7::Xilinx7Backend::open(part)?;
            let packed = b.pack(design)?;
            stage("pack", "packed".into(), progress);
            let placed = b.place(&packed)?;
            stage("place", "placed".into(), progress);
            let routed = b.route(&placed)?;
            stage("route", "routed".into(), progress);
            let bs = b.bitstream(&routed)?;
            stage("bitstream", format!("{} FASM features", bs.fasm.len()), progress);
            bs.fasm.join("\n")
        }
    };
    Ok(FamilyBitstream {
        family,
        part: part.to_string(),
        ext,
        text,
        stages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLINKY: &str = "module blinky(input wire clk, output wire led);\n  reg [7:0] c = 0;\n  always @(posedge clk) c <= c + 8'd1;\n  assign led = c[7];\nendmodule\n";

    fn synth() -> Design {
        helion_sv::synth_sv(BLINKY, "blinky.sv").expect("synth")
    }

    fn pinned(clk: &str, led: &str) -> Design {
        let mut d = synth();
        assert_eq!(insert_io_buffers(&mut d).unwrap(), 2, "clk pad + global (helion-sv already pads outputs)");
        assert_eq!(insert_io_buffers(&mut d).unwrap(), 0, "idempotent");
        let missing = apply_pin_constraints(
            &mut d,
            &[("clk".into(), clk.into()), ("led".into(), led.into()), ("nope".into(), "A1".into())],
        );
        assert_eq!(missing, vec!["nope".to_string()]);
        d
    }

    #[test]
    fn registry_lists_only_parts_that_load() {
        let parts = implementable_parts();
        assert!(parts.iter().any(|p| p.part == "HL10T-C32-1" && p.family == Family::Helion));
        assert_eq!(family_of("ice40-hx8k"), Some(Family::Ice40));
        assert_eq!(family_of("LFE5U-85F"), Some(Family::Ecp5));
        assert_eq!(family_of("LFE5U-45F"), None, "no Trellis DB vendored for 45F");
        assert_eq!(parts.iter().any(|p| p.part == "xc7a35t"), xilinx_db_present());
        for p in parts.iter().filter(|p| p.family == Family::Helion) {
            assert!(helion_device::Device::load_part(&p.part).is_ok());
        }
    }

    #[test]
    fn ice40_flow_runs_from_helion_sv_netlist() {
        let mut log = Vec::new();
        let out = implement(&pinned("J3", "B5"), "ice40-hx8k", &mut |l| log.push(l.to_string())).expect("ice40 flow");
        assert_eq!(out.ext, "asc");
        assert!(out.text.contains(".logic_tile"), "IceStorm asc body");
        assert_eq!(log.len(), 4, "{log:?}");
    }

    #[test]
    fn ecp5_flow_runs_from_helion_sv_netlist() {
        let out = implement(&pinned("A10", "A12"), "LFE5U-85F", &mut |_| {}).expect("ecp5 flow");
        assert_eq!(out.ext, "config");
        assert!(out.text.contains(".device LFE5U-85F"), "{}", &out.text[..out.text.len().min(200)]);
    }

    #[test]
    fn unknown_and_helion_parts_are_refused() {
        assert!(implement(&synth(), "xc9z999", &mut |_| {}).is_err());
        assert!(implement(&synth(), "HL10T-C32-1", &mut |_| {}).unwrap_err().contains("HL10T"));
    }
}
