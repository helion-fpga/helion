//! UG909 Dynamic Function eXchange (partial reconfiguration) flow.
//!
//! A [`DfxPartition`] binds a hierarchy instance of the static design to a
//! `.prj` pblock (the reconfigurable partition) and lists reconfigurable
//! module variants, each a set of HDL files defining the partition's
//! module. [`run_dfx`] implements one configuration per module:
//!
//! 1. synthesize static sources + that module's sources;
//! 2. place with the partition pblock as an exclusive region owning every
//!    cell of the instance (`helion_place::place_with_regions`); module
//!    two onward first reuse the previous configuration's sites for every
//!    same-named static cell (incremental placement), so the static side
//!    stays put;
//! 3. route, then register the routed configuration as a
//!    `helion_fabric::ReconfigModule` (the first one also locks the
//!    partition-pin boundary anchors);
//! 4. run the UG909 DRC deck (`helion_fabric::check_dfx_drc`, HDPR-1..8);
//! 5. write each module's partial bitstream (`ReconfigPartition::bitgen_rm`,
//!    only the frames of the pblock's columns) plus the full bitstream of
//!    the first configuration.
//!
//! Static routing is re-run per configuration (the router has no locked
//! static-route mode); frames outside the partition never enter a
//! partial, and HDPR-6/7 check the boundary pins every module shares.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::{ImplStrategy, Mode, PrjPblock, Session, pblock_regions};
use helion_device::Device;
use helion_fabric::{IsolationValue, ReconfigModule, ReconfigPartition, check_dfx_drc};
use helion_ir::resolve;

/// One reconfigurable module variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DfxModule {
    pub name: String,
    pub sources: Vec<PathBuf>,
}

/// A reconfigurable partition: pblock + the instance it holds + variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DfxPartition {
    pub name: String,
    pub pblock: PrjPblock,
    /// Hierarchy instance (in the static top) that is reconfigured.
    pub instance: String,
    pub modules: Vec<DfxModule>,
}

/// Per-module result.
#[derive(Clone, Debug)]
pub struct DfxModuleResult {
    pub module: String,
    pub rm_cells: usize,
    pub pbits: PathBuf,
    pub frames: usize,
    pub bytes: usize,
}

/// One UG909 DRC finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DfxFinding {
    pub code: String,
    pub objects: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct DfxResult {
    pub partition: String,
    pub full_bit: PathBuf,
    pub partpins: usize,
    pub modules: Vec<DfxModuleResult>,
    pub drc: Vec<DfxFinding>,
    pub log: Vec<String>,
}

fn synth(files: &[PathBuf]) -> Result<helion_ir::Design, String> {
    let refs: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
    helion_sv::synth_sv_files(&refs)
}

/// Implement every module of `part` over `static_sources` and write the
/// partial bitstreams into `out_dir`.
pub fn run_dfx(
    dev: &Device,
    static_sources: &[PathBuf],
    part: &DfxPartition,
    out_dir: &Path,
) -> Result<DfxResult, String> {
    if part.modules.is_empty() {
        return Err(format!("dfx {}: no reconfigurable modules", part.name));
    }
    if !part.pblock.ranged {
        return Err(format!("dfx {}: pblock {} has no range", part.name, part.pblock.name));
    }
    std::fs::create_dir_all(out_dir).map_err(|e| format!("dfx {}: {e}", out_dir.display()))?;
    let pb = &part.pblock;
    let mut rp = ReconfigPartition::new(
        part.name.clone(),
        helion_bits::Pblock::new(pb.name.clone(), pb.x0, pb.y0, pb.x1, pb.y1),
    );
    let mut log = Vec::new();
    let mut prev_placed: Option<helion_place::Placed> = None;
    let mut full_bit = PathBuf::new();
    let mut counts = Vec::new();
    let opts = ImplStrategy::Default.place_opts();
    let member = PrjPblock {
        cells: vec![part.instance.clone()],
        ..pb.clone()
    };
    for (mi, m) in part.modules.iter().enumerate() {
        let mut files = static_sources.to_vec();
        files.extend(m.sources.iter().cloned());
        let design = synth(&files).map_err(|e| format!("dfx {} synth: {e}", m.name))?;
        if !design.instances.iter().any(|i| resolve(i.name) == part.instance) {
            return Err(format!(
                "dfx {}: the static design has no instance {}",
                part.name, part.instance
            ));
        }
        let mut regions = pblock_regions(&design, std::slice::from_ref(&member));
        for r in &mut regions {
            r.exclusive = true;
        }
        let rm_cells: BTreeSet<String> = design
            .cells
            .iter()
            .map(|c| resolve(c.name).to_string())
            .filter(|c| regions.iter().any(|r| r.lists(c)))
            .collect();
        if rm_cells.is_empty() {
            return Err(format!("dfx {}: module {} put no cells in {}", part.name, m.name, part.instance));
        }
        let mut s = Session::new(Mode::Project);
        s.synth_design(design);
        match &prev_placed {
            None => {
                s.place_design_with_pblocks(dev, opts, std::slice::from_ref(&member))
                    .map_err(|e| format!("dfx {} place: {e}", m.name))?;
                // Re-apply exclusivity (place_design_with_pblocks regions are shared).
                if let Some(pl) = s.placed.as_mut() {
                    helion_place::legalize_into_regions(pl, dev, &regions)
                        .map_err(|e| format!("dfx {} place: {e}", m.name))?;
                }
            }
            Some(prev) => {
                s.incremental_place(dev, prev)
                    .map_err(|e| format!("dfx {} place: {e}", m.name))?;
                if let Some(pl) = s.placed.as_mut() {
                    helion_place::legalize_into_regions(pl, dev, &regions)
                        .map_err(|e| format!("dfx {} place: {e}", m.name))?;
                }
            }
        }
        s.route_design(dev).map_err(|e| format!("dfx {} route: {e}", m.name))?;
        let routed = s.routed.clone().ok_or("dfx: route produced nothing")?;
        log.push(format!(
            "dfx {} module={} rm_cells={} overused={}",
            part.name,
            m.name,
            rm_cells.len(),
            routed.overused
        ));
        if mi == 0 {
            let pins = rp
                .lock_partpins_from_routed(&routed, &rm_cells, IsolationValue::Zero)
                .map_err(|e| format!("dfx {} partpins: {e}", m.name))?;
            log.push(format!("dfx {} partpins locked={}", part.name, pins.len()));
            let bits = s.write_bitstream(dev).map_err(|e| format!("dfx {} bitstream: {e}", m.name))?;
            full_bit = out_dir.join(format!("{}_{}_full.bit", part.name, m.name));
            std::fs::write(&full_bit, &bits.packets).map_err(|e| format!("{}: {e}", full_bit.display()))?;
        }
        prev_placed = s.placed.clone();
        counts.push(rm_cells.len());
        rp.add_module(ReconfigModule::new(m.name.clone(), rm_cells, routed));
    }
    let drc = check_dfx_drc(dev, std::slice::from_ref(&rp))
        .violations
        .into_iter()
        .map(|v| DfxFinding {
            code: v.code.to_string(),
            objects: v.objects,
            message: v.message,
        })
        .collect();
    let mut modules = Vec::new();
    for (m, n) in part.modules.iter().zip(counts) {
        let pbits = rp.bitgen_rm(dev, &m.name).map_err(|e| format!("dfx {}: {e}", m.name))?;
        let path = out_dir.join(format!("{}_{}_partial.pbits", part.name, m.name));
        std::fs::write(&path, &pbits.packets).map_err(|e| format!("{}: {e}", path.display()))?;
        log.push(format!(
            "write_bitstream -cell {} {} frames={} bytes={}",
            part.instance,
            path.display(),
            pbits.frames.len(),
            pbits.packets.len()
        ));
        modules.push(DfxModuleResult {
            module: m.name.clone(),
            rm_cells: n,
            pbits: path,
            frames: pbits.frames.len(),
            bytes: pbits.packets.len(),
        });
    }
    Ok(DfxResult {
        partition: part.name.clone(),
        full_bit,
        partpins: rp.partpins.len(),
        modules,
        drc,
        log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP: &str = "module top(input clk, output led);\n  reg [3:0] cnt = 0;\n  always @(posedge clk) cnt <= cnt + 1;\n  wire y;\n  rm u_rm (.clk(clk), .a(cnt[3]), .y(y));\n  assign led = y;\nendmodule\n";
    const RM_INV: &str = "module rm(input clk, input a, output reg y);\n  always @(posedge clk) y <= ~a;\nendmodule\n";
    const RM_BUF: &str = "module rm(input clk, input a, output reg y);\n  reg t;\n  always @(posedge clk) begin t <= a; y <= t; end\nendmodule\n";

    #[test]
    fn two_modules_produce_partial_bitstreams_inside_the_partition() {
        let dir = std::env::temp_dir().join(format!("helion-dfx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let top = dir.join("top.v");
        let inv = dir.join("rm_inv.v");
        let buf = dir.join("rm_buf.v");
        std::fs::write(&top, TOP).unwrap();
        std::fs::write(&inv, RM_INV).unwrap();
        std::fs::write(&buf, RM_BUF).unwrap();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let part = DfxPartition {
            name: "rp0".into(),
            pblock: PrjPblock {
                name: "pblock_rp0".into(),
                x0: 6,
                y0: 5,
                x1: 8,
                y1: 8,
                ranged: true,
                cells: Vec::new(),
            },
            instance: "u_rm".into(),
            modules: vec![
                DfxModule { name: "inv".into(), sources: vec![inv] },
                DfxModule { name: "buf".into(), sources: vec![buf] },
            ],
        };
        let r = run_dfx(&dev, &[top], &part, &dir.join("out")).unwrap();
        assert_eq!(r.modules.len(), 2);
        for m in &r.modules {
            assert!(m.rm_cells > 0, "{m:?}");
            assert!(m.frames > 0);
            assert!(std::fs::metadata(&m.pbits).unwrap().len() > 0);
        }
        assert!(r.full_bit.is_file());
        assert_ne!(
            std::fs::read(&r.modules[0].pbits).unwrap(),
            std::fs::read(&r.modules[1].pbits).unwrap(),
            "different modules configure the partition differently"
        );
        assert!(
            !r.drc.iter().any(|f| f.code == "HDPR-5"),
            "every RM cell sits inside the partition: {:?}",
            r.drc
        );
    }

    #[test]
    fn missing_instance_is_an_error() {
        let dir = std::env::temp_dir().join(format!("helion-dfx-miss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let top = dir.join("top.v");
        std::fs::write(&top, TOP).unwrap();
        let inv = dir.join("rm_inv.v");
        std::fs::write(&inv, RM_INV).unwrap();
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let part = DfxPartition {
            name: "rp0".into(),
            pblock: PrjPblock { name: "p".into(), x0: 6, y0: 5, x1: 8, y1: 8, ranged: true, cells: vec![] },
            instance: "u_nope".into(),
            modules: vec![DfxModule { name: "inv".into(), sources: vec![inv] }],
        };
        assert!(run_dfx(&dev, &[top], &part, &dir).unwrap_err().contains("no instance"));
    }
}
