//! Implementation runs for non-Helion families (iCE40, ECP5, 7-series).
//!
//! The run worker synthesizes with helion-sv as usual; when the project part
//! belongs to another family (`helion_targets::family_of`), this module takes
//! over instead of the HL10T place/route/bitgen: it applies the project's pin
//! constraints as `LOC`, runs that family's pack → place → route → bitstream
//! through `helion_targets::implement`, and writes the family's bitstream
//! artifact (IceStorm `.asc`, Trellis `.config`, FASM) next to the run's
//! checkpoints. Utilization, DRC and the die/placement come from the family
//! crate (helion-targets `reports`), timing from the family's delay model;
//! there is no power model for these families, so no power report.
//!
//! 7-series parts (the ZedBoard's xc7z020 among them) skip all of the above
//! and run [`run_xdc_flow`]: the HDL + XDC flow `zedbit` runs, with its own
//! synthesis and timing, writing the real `.bit`.

use std::path::{Path, PathBuf};

use crate::runs::RunArtifacts;
use crate::state::{DrcItem, DrcSev, MsgSev, TimingState, UtilState};

/// Family die and (after implementation) placement for the Device view.
#[derive(Clone, Debug)]
pub struct FamilyDie {
    pub grid: helion_targets::reports::DieGrid,
    /// Placed primitives (empty for synthesis-only runs).
    pub placed: Vec<helion_targets::reports::PlacedPrim>,
}

/// Utilization rows from the family packer (helion-targets), mapped onto the
/// six report rows with the family's primitive names.
pub fn family_util(design: &helion_ir::Design, part: &str) -> Result<UtilState, String> {
    let u = helion_targets::reports::utilization(design, part)?;
    let pick = |i: usize| u.rows.get(i).map(|r| (r.resource.to_string(), r.used as u32, r.available as u32));
    // Row order from helion-targets: LUT, register, RAM, I/O, then DSP
    // (ECP5 only), PLL, global clock.
    let (lut, ff, ram, io) = (pick(0), pick(1), pick(2), pick(3));
    let dsp = u.rows.iter().find(|r| r.resource.starts_with("MULT") || r.resource == "MAC16");
    let glb = u.rows.iter().find(|r| r.resource.starts_with("Global"));
    let mut st = UtilState { design: helion_ir::resolve(design.name).to_string(), family: u.family.label().to_string(), ..UtilState::default() };
    let mut labels = Vec::new();
    for (slot, row) in [lut, ff, ram, io].into_iter().enumerate() {
        let (label, used, avail) = row.unwrap_or_default();
        labels.push(label);
        match slot {
            0 => (st.luts_used, st.luts_avail) = (used, avail),
            1 => (st.regs_used, st.regs_avail) = (used, avail),
            2 => (st.bram_used, st.bram_avail) = (used, avail),
            _ => (st.io_used, st.io_avail) = (used, avail),
        }
    }
    match dsp {
        Some(r) => {
            labels.push(r.resource.to_string());
            (st.dsp_used, st.dsp_avail) = (r.used as u32, r.available as u32);
        }
        None => labels.push(format!("DSP (none on {part})")),
    }
    let g = glb.map(|r| (r.resource.to_string(), r.used as u32, r.available as u32)).unwrap_or_default();
    labels.push(g.0);
    (st.bufg_used, st.bufg_avail) = (g.1, g.2);
    st.family_labels = labels;
    Ok(st)
}

/// Family DRC rows (capacity, package pins, packer acceptance).
pub fn family_drc(design: &helion_ir::Design, part: &str) -> Result<Vec<DrcItem>, String> {
    Ok(helion_targets::reports::drc(design, part)?
        .into_iter()
        .map(|d| DrcItem {
            id: d.id.to_string(),
            severity: match d.severity {
                helion_targets::reports::DrcSeverity::Error => DrcSev::Error,
                helion_targets::reports::DrcSeverity::Warning => DrcSev::Warning,
            },
            objects: d.objects,
            message: d.message,
        })
        .collect())
}

/// Logic-level timing with the family's delay model (no placement-aware
/// routing estimate: the family flows do not export wire delays).
pub fn family_timing(
    design: &helion_ir::Design,
    part: &str,
    xdc: &helion_sta::Constraints,
    clks: &[helion_sta::Clock],
    send: &dyn Fn(String, MsgSev),
) -> Option<TimingState> {
    let Some((model, note)) = helion_targets::reports::timing_model(part) else {
        send(format!("report_timing: no delay model for {part}"), MsgSev::Warn);
        return None;
    };
    let t = match helion_sta::report_timing_with_model(design, clks, &model) {
        Ok(t) => t,
        Err(e) => {
            send(format!("WARN: report_timing skipped: {e}"), MsgSev::Warn);
            return None;
        }
    };
    let summary = helion_sta::report_timing_summary(clks, xdc, Some(&t));
    send(
        format!(
            "report_timing WNS_PS={} TNS_PS={} endpoints={} ({note})",
            summary.wns_ps.map(|w| w.to_string()).unwrap_or_else(|| "-".into()),
            summary.tns_ps,
            summary.endpoints
        ),
        MsgSev::Info,
    );
    let mut st = crate::runs::timing_state_from(&summary, Some(&t), design, None, xdc);
    st.model = note.to_string();
    Some(st)
}

/// Synthesis-run reports for a family part: family utilization, family
/// timing model, die geometry. No power model exists for these families.
pub fn family_synth_reports(
    design: &helion_ir::Design,
    xdc: &helion_sta::Constraints,
    clks: &[helion_sta::Clock],
    part: &str,
    art: &mut RunArtifacts,
    send: &dyn Fn(String, MsgSev),
) {
    art.timing = family_timing(design, part, xdc, clks, send);
    match family_util(design, part) {
        Ok(u) => {
            send(format!("report_utilization {} {}={}/{} (family packer)", u.family, u.family_labels[0], u.luts_used, u.luts_avail), MsgSev::Info);
            art.util = Some(u);
        }
        Err(e) => send(format!("WARN: utilization skipped: {e}"), MsgSev::Warn),
    }
    match helion_targets::reports::die(part) {
        Ok(grid) => art.family_die = Some(FamilyDie { grid, placed: Vec::new() }),
        Err(e) => send(format!("WARN: die geometry unavailable: {e}"), MsgSev::Warn),
    }
    send(format!("report_power: not available for {part} (no power model for this family)"), MsgSev::Info);
}

/// The non-Helion family of `part`, if it has one.
pub fn foreign_family(part: &str) -> Option<helion_targets::Family> {
    helion_targets::family_of(part).filter(|f| *f != helion_targets::Family::Helion)
}

/// Run the family flow on `design` and write its artifact under `out_dir`.
pub fn run_family(
    mut design: helion_ir::Design,
    part: &str,
    package_pins: &[(String, String)],
    out_dir: &Path,
    xdc: &helion_sta::Constraints,
    clks: &[helion_sta::Clock],
    send: &dyn Fn(String, MsgSev),
) -> Result<RunArtifacts, String> {
    let family = foreign_family(part).ok_or_else(|| format!("{part} is not a family part"))?;
    let missing = helion_targets::apply_pin_constraints(&mut design, package_pins);
    if !package_pins.is_empty() {
        send(
            format!(
                "set_property PACKAGE_PIN applied={}{}",
                package_pins.len() - missing.len(),
                if missing.is_empty() {
                    String::new()
                } else {
                    format!(" (no such port: {})", missing.join(", "))
                }
            ),
            if missing.is_empty() { MsgSev::Info } else { MsgSev::Warn },
        );
    }
    send(format!("launch_runs impl_1 -part {part} ({})", family.label()), MsgSev::Info);
    let out = helion_targets::implement(&design, part, &mut |line| send(line.to_string(), MsgSev::Info))?;
    let util = family_util(&design, part).map_err(|e| send(format!("WARN: utilization skipped: {e}"), MsgSev::Warn)).ok();
    let drc = family_drc(&design, part).map_err(|e| send(format!("WARN: DRC skipped: {e}"), MsgSev::Warn)).ok();
    if let Some(d) = &drc {
        let errors = d.iter().filter(|x| x.severity == DrcSev::Error).count();
        send(format!("report_drc {} violation(s), {errors} error(s)", d.len()), if errors > 0 { MsgSev::Warn } else { MsgSev::Info });
    }
    let timing = family_timing(&design, part, xdc, clks, send);
    let family_die = match (helion_targets::reports::die(part), helion_targets::reports::placement(&design, part)) {
        (Ok(grid), Ok(placed)) => Some(FamilyDie { grid, placed }),
        (Ok(grid), Err(e)) => {
            send(format!("WARN: placement view unavailable: {e}"), MsgSev::Warn);
            Some(FamilyDie { grid, placed: Vec::new() })
        }
        (Err(e), _) => {
            send(format!("WARN: die geometry unavailable: {e}"), MsgSev::Warn);
            None
        }
    };
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let top = helion_ir::resolve(design.name).to_string();
    let path: PathBuf = out_dir.join(format!("{top}.{}", out.ext));
    std::fs::write(&path, &out.text).map_err(|e| format!("{}: {e}", path.display()))?;
    let (_, what) = family.artifact();
    send(format!("write_bitstream {} ({what})", path.display()), MsgSev::Info);
    if let Some(why) = helion_hw::lattice_program_reason(part, Some(&path)) {
        send(format!("WARN: {why}"), MsgSev::Warn);
    }
    Ok(RunArtifacts {
        cells: design.cells.len(),
        part: part.to_string(),
        summary: format!(
            "{} cells, {} {} written to {}",
            design.cells.len(),
            family.label(),
            out.ext,
            path.display()
        ),
        design: Some(design),
        util,
        drc,
        timing,
        family_die,
        // Program Device then names the pack-and-load commands instead of
        // "No bitstream yet" sending the user back to Generate.
        bit_file: Some(path),
        ..RunArtifacts::default()
    })
}

/// True for 7-series parts, which build with the HDL + XDC flow (`zedbit`'s).
pub fn is_xilinx7(part: &str) -> bool {
    helion_targets::family_of(part) == Some(helion_targets::Family::Xilinx7)
}

/// HDL sources, constraint files and top of a run source: a `.prj` lists
/// them; a single HDL file uses its sibling `<stem>.xdc`.
fn xdc_inputs(source: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>, Option<String>), String> {
    if source.extension().is_some_and(|e| e.eq_ignore_ascii_case("prj")) {
        let text = std::fs::read_to_string(source).map_err(|e| format!("{}: {e}", source.display()))?;
        let prj = helion_proj::load_prj(&text)?;
        let res = |v: &[String]| v.iter().map(|s| helion_proj::resolve_prj_path(source, s)).collect::<Vec<_>>();
        let xdc: Vec<PathBuf> = res(&prj.constraint_files).into_iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xdc"))).collect();
        return Ok((res(&prj.sources), xdc, prj.top));
    }
    let xdc = source.with_extension("xdc");
    Ok((vec![source.to_path_buf()], if xdc.is_file() { vec![xdc] } else { Vec::new() }, None))
}

/// Utilization of a packed 7-series design against the device's sites.
fn x7_util(p: &helion_targets::xdc_flow_types::Packed7, dev: &helion_targets::xdc_flow_types::Device, design: &str) -> UtilState {
    let slices = dev.slices().len() as u32;
    UtilState {
        luts_used: p.luts.len() as u32,
        luts_avail: slices * 4,
        regs_used: p.ffs.len() as u32,
        regs_avail: slices * 8,
        bram_used: p.brams.len() as u32,
        bram_avail: dev.bram36_sites().len() as u32,
        io_used: p.iobs.len() as u32,
        io_avail: dev.iob_sites().len() as u32,
        dsp_used: p.dsps.len() as u32,
        dsp_avail: dev.dsp48_sites().len() as u32,
        bufg_used: p.bufgs.len() as u32,
        bufg_avail: dev.bufg_sites().len() as u32,
        design: design.to_string(),
        family: helion_targets::Family::Xilinx7.label().to_string(),
        family_labels: ["LUT6", "FF", "RAMB36/18", "IOB", "DSP48E1", "BUFG"].map(String::from).to_vec(),
        ..UtilState::default()
    }
}

/// The flow's routed timing as the Timing view's summary, clocks and worst
/// path (one row per path step, with its routed wire delay).
fn x7_timing(b: &helion_targets::xdc_flow::Built) -> TimingState {
    let a = &b.timing;
    let mut clocks: Vec<crate::state::TimingClock> = b
        .periods
        .iter()
        .map(|(net, &ps)| crate::state::TimingClock { name: net.clone(), period_ps: ps as u64, source: net.clone(), generated: false, master: None })
        .collect();
    clocks.sort_by(|x, y| x.name.cmp(&y.name));
    let mut pins = Vec::new();
    let mut prev = 0i64;
    for st in &a.worst_path {
        let at = st.arrival_ps as i64;
        if st.wire_ps > 0 {
            pins.push(crate::state::TimingPinRow { cell: st.what.clone(), delay_type: "net".into(), incr_ps: st.wire_ps as i64, path_ps: prev + st.wire_ps as i64, ..Default::default() });
        }
        pins.push(crate::state::TimingPinRow { cell: st.what.clone(), delay_type: "cell".into(), incr_ps: at - prev - st.wire_ps as i64, path_ps: at, ..Default::default() });
        prev = at;
    }
    let paths = match (a.worst_path.first(), a.worst_path.last()) {
        (Some(f), Some(l)) => vec![crate::state::TimingPathRow {
            name: "Path 1 (worst)".into(),
            from: f.what.clone(),
            to: l.what.clone(),
            slack_ps: a.wns_ps,
            delay_ps: l.arrival_ps as i64,
            pins,
            ..Default::default()
        }],
        _ => Vec::new(),
    };
    TimingState {
        wns_ps: Some(a.wns_ps),
        tns_ps: a.wns_ps.min(0),
        failing_endpoints: a.failing_endpoints,
        failing_setup: a.failing_endpoints,
        endpoints: a.endpoints,
        has_run: true,
        clocks,
        paths,
        ..TimingState::default()
    }
}

/// Synthesis-only run on a 7-series part: the flow's front end, then the
/// packer for resource counts (no placement, so no die and no timing).
pub fn run_xdc_synth(source: &Path, part: &str, send: &dyn Fn(String, MsgSev)) -> Result<RunArtifacts, String> {
    use helion_targets::xdc_flow_types::Backend;
    let part = helion_targets::canonical_part(part);
    let (srcs, _, top) = xdc_inputs(source)?;
    if srcs.is_empty() {
        return Err(format!("{}: no HDL sources", source.display()));
    }
    let design_arg = srcs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(",");
    let mut log = |l: &str| send(l.to_string(), MsgSev::Info);
    let (design, _) = helion_targets::xdc_flow::synth(&design_arg, top.as_deref(), "auto", &mut log)?;
    let be = helion_targets::xdc_flow_types::Xilinx7Backend::open(part)?;
    let packed = be.pack(&design)?;
    let stem = helion_ir::resolve(design.name).to_string();
    let util = x7_util(&packed, be.device(), &stem);
    send(format!("report_utilization {} LUT6={}/{} FF={}/{} (7-series packer)", part, util.luts_used, util.luts_avail, util.regs_used, util.regs_avail), MsgSev::Info);
    Ok(RunArtifacts {
        cells: design.cells.len(),
        part: part.to_string(),
        summary: format!("{} LUTs, {} FFs after packing on {part}", packed.luts.len(), packed.ffs.len()),
        util: Some(util),
        design: Some(design),
        ..RunArtifacts::default()
    })
}

/// Implement `source` on a 7-series part with `helion_targets::xdc_flow`
/// (synthesis, pins from the XDC, pack, place, route, timing, `.bit`), the
/// flow `zedbit` runs, and report it to the IDE: utilization, slack, the
/// die with every placed primitive, and the netlist for the schematic.
pub fn run_xdc_flow(source: &Path, part: &str, out_dir: &Path, send: &dyn Fn(String, MsgSev)) -> Result<RunArtifacts, String> {
    let part = helion_targets::canonical_part(part);
    let (srcs, xdcs, top) = xdc_inputs(source)?;
    if srcs.is_empty() {
        return Err(format!("{}: no HDL sources", source.display()));
    }
    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let xdc = match xdcs.as_slice() {
        [] => return Err(format!("{part}: no .xdc constraints; every top-level port needs a PACKAGE_PIN (add one with read_xdc in the project, or put <name>.xdc next to the source)")),
        [one] => one.clone(),
        many => {
            // xdc_flow reads one file; several are read in project order.
            let mut text = String::new();
            for f in many {
                text.push_str(&std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?);
                text.push('\n');
            }
            let joined = out_dir.join("constraints.xdc");
            std::fs::write(&joined, text).map_err(|e| format!("{}: {e}", joined.display()))?;
            joined
        }
    };
    let stem = top.clone().or_else(|| srcs[0].file_stem().and_then(|s| s.to_str()).map(str::to_string)).unwrap_or_else(|| "design".into());
    let bit = out_dir.join(format!("{stem}.bit"));
    let design_arg = srcs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(",");
    send(format!("launch_runs impl_1 -part {part} (Xilinx 7-Series, xdc_flow) read_xdc {}", xdc.display()), MsgSev::Info);
    let mut log = |l: &str| send(l.to_string(), MsgSev::Info);
    let b = helion_targets::xdc_flow::run(&design_arg, &xdc.display().to_string(), &bit.display().to_string(), top.as_deref(), false, "auto", part, None, &mut log)?
        .ok_or("xdc_flow stopped before routing")?;
    let view = helion_targets::view_xilinx7::from_built(&b);
    let dev = b.backend.device();
    let p = &b.routed.placed.packed;
    let util = x7_util(p, dev, &stem);
    let timing = x7_timing(&b);
    send(format!("report_timing_summary WNS={:.3} ns (period {:.3} ns)", b.slack_ns, b.period_ns), MsgSev::Info);
    send(format!("zedbit: summary {}", b.summary), MsgSev::Info);
    if !p.ps7s.is_empty() {
        send("WARN: this design uses the Zynq PS7; Program Device loads the PL only. Start the PS (FCLK, AXI) separately, e.g. openocd with tools/zynq/ps7_axi_check.tcl.".into(), MsgSev::Warn);
    }
    let bytes = std::fs::metadata(&bit).map(|m| m.len() as usize).unwrap_or(0);
    send(format!("write_bitstream {} ({bytes} bytes)", bit.display()), MsgSev::Info);
    let family_die = Some(FamilyDie {
        grid: helion_targets::reports::xilinx_die(dev, part),
        placed: helion_targets::reports::xilinx_placement(&b.routed, dev),
    });
    Ok(RunArtifacts {
        cells: b.design.cells.len(),
        part: part.to_string(),
        summary: format!("{} LUTs, {} FFs, WNS {:.3} ns; {} written", p.luts.len(), p.ffs.len(), b.slack_ns, bit.display()),
        util: Some(util),
        timing: Some(timing),
        family_die,
        design: Some(b.design),
        bit_file: Some(bit.clone()),
        view: Some(view),
        ..RunArtifacts::default()
    })
}

/// Scene/test fixture: implement `examples/counter.sv` on `part` through the
/// real family flow (temp output dir) and apply the run's reports.
pub fn seed_family_fixture(s: &mut crate::state::AppState, part: &str) -> Result<(), String> {
    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
    let design = helion_sv::synth_sv_path(&src)?;
    // One scratch dir per call: scenes for the same part render in parallel
    // in the snapshot tests, and a shared dir was deleted under a live run.
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "helion-rune-family-{}-{part}-{seq}",
        std::process::id()
    ));
    let clk = helion_sta::Clock {
        name: "clk".into(),
        period_ps: 10_000,
        source: "clk".into(),
        generated: false,
        master: None,
        divide_by: 1,
        multiply_by: 1,
        invert: false,
        edges: Vec::new(),
    };
    // counter.sv ports on the family's breakout-board pins.
    let pins: Vec<(String, String)> = if part.to_ascii_lowercase().starts_with("ice40") {
        vec![("clk".into(), "J3".into()), ("led".into(), "B5".into())]
    } else {
        vec![("clk".into(), "A10".into()), ("led".into(), "A12".into())]
    };
    let art = run_family(design, part, &pins, &dir, &helion_sta::Constraints::default(), &[clk], &|_, _| {})?;
    s.project.part = part.to_string();
    s.flow.impl_run = crate::state::RunStatus::Complete;
    crate::runs::apply_artifacts(s, art);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Scene fixture: the view contract of a real ZedBoard blinky implementation
/// (`examples/zedboard_blinky.prj`), run once per process and shared, since
/// the flow takes most of a minute.
pub fn seed_xilinx7_view_fixture(s: &mut crate::state::AppState) -> Result<(), String> {
    static VIEW: std::sync::OnceLock<Result<helion_targets::view::ImplView, String>> = std::sync::OnceLock::new();
    let v = VIEW.get_or_init(|| {
        if !helion_targets::zynq_db_present() {
            return Err("no zynq7 X-Ray database".into());
        }
        let prj = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/zedboard_blinky.prj");
        let dir = std::env::temp_dir().join(format!("helion-rune-view-fixture-{}", std::process::id()));
        let art = run_xdc_flow(&prj, "xc7z020", &dir, &|_, _| {});
        let _ = std::fs::remove_dir_all(&dir);
        art?.view.ok_or_else(|| "the run produced no view".to_string())
    });
    s.project.part = helion_targets::ZEDBOARD_PART.to_string();
    s.runs.impl_view = Some(v.clone()?);
    s.flow.impl_run = crate::state::RunStatus::Complete;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zedboard_blinky_project_builds_a_bit_with_the_xdc_flow() {
        if !helion_targets::zynq_db_present() {
            eprintln!("skipped: no zynq7 X-Ray database");
            return;
        }
        let prj = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/zedboard_blinky.prj");
        let dir = std::env::temp_dir().join(format!("helion-rune-xdc-{}", std::process::id()));
        let art = run_xdc_flow(&prj, "xc7z020", &dir, &|_, _| {}).expect("xdc flow");
        let bit = std::fs::read(dir.join("zedboard_blinky.bit")).expect(".bit written");
        let _ = std::fs::remove_dir_all(&dir);
        // Sync word of a 7-series configuration stream.
        assert!(bit.windows(4).any(|w| w == [0xAA, 0x99, 0x55, 0x66]), "no sync word in {} bytes", bit.len());
        assert_eq!(art.part, "xc7z020clg484-1");
        let u = art.util.expect("utilization");
        assert_eq!((u.io_used, u.regs_used), (9, 27), "clk + 8 LEDs, 27-bit counter");
        let t = art.timing.expect("timing");
        assert!(t.wns_ps.expect("wns") >= 0);
        assert_eq!(t.clocks.len(), 1, "one 100 MHz clock: {:?}", t.clocks);
        assert_eq!(t.clocks[0].period_ps, 10_000);
        assert!(t.endpoints >= 27 && !t.paths.is_empty() && !t.paths[0].pins.is_empty());
        let syn = run_xdc_synth(&prj, "xc7z020", &|_, _| {}).expect("synth run");
        assert_eq!(syn.util.expect("util").luts_avail, 53_200);
        // Reports that need an HL10 model refuse the part instead of using HL10T numbers.
        assert!(crate::runs::resolve_device("xc7z020", &|_, _| {}).is_err());
        let die = art.family_die.expect("die");
        assert!(die.grid.count(helion_targets::reports::TileClass::Logic) > 1000);
        assert_eq!(die.placed.iter().filter(|p| p.class == helion_targets::reports::TileClass::Io).count(), 9);
        assert!(art.design.is_some(), "netlist for the schematic");
    }

    /// Guard: a ZedBoard implementation run fills the view contract from the
    /// run itself (die, placement, routing, usage, timing, IO, clocks).
    #[test]
    fn zedboard_run_fills_the_impl_view_from_the_run() {
        if !helion_targets::zynq_db_present() {
            eprintln!("skipped: no zynq7 X-Ray database");
            return;
        }
        let prj = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/zedboard_blinky.prj");
        let dir = std::env::temp_dir().join(format!("helion-rune-view-{}", std::process::id()));
        let art = run_xdc_flow(&prj, "xc7z020", &dir, &|_, _| {}).expect("xdc flow");
        let _ = std::fs::remove_dir_all(&dir);
        let v = art.view.expect("impl view");
        assert_eq!(v.part, "xc7z020clg484-1");
        assert!(v.tiles.len() > 10_000 && v.cols > 0 && v.rows > 0, "{} tiles", v.tiles.len());
        let regions = v.clock_regions();
        assert_eq!(regions.len(), 6, "xc7z020 has six clock regions in the database: {regions:?}");
        assert!(v.provenance["tiles"].method.contains("tilegrid.json"), "regions read, not derived");
        assert!(v.cells.iter().filter(|c| c.kind == "FF").count() >= 27, "27-bit counter placed");
        assert!(v.cells.iter().all(|c| !c.site.is_empty()));
        let routed: Vec<_> = v.nets.iter().filter(|n| !n.hops.is_empty()).collect();
        assert!(!routed.is_empty(), "routed nets with hops");
        assert!(routed.iter().any(|n| n.driver.is_some() && !n.loads.is_empty()), "router pins on nets");
        assert!(v.usage.iter().any(|u| u.sites_used > 0), "per-tile site usage");
        assert!(v.usage.iter().all(|u| u.sites_used <= u.sites_capacity), "sites counted once per site");
        assert!(v.usage.iter().any(|u| u.overflow.is_some_and(|o| o.h_used + o.v_used > 0)), "per-tile wire usage");
        let t = v.timing.as_ref().expect("timing");
        assert!(t.total_endpoints >= 27 && t.endpoints.len() == t.total_endpoints, "every endpoint's slack");
        assert!(t.endpoints.iter().all(|e| e.x.is_some()), "endpoints land on the die");
        assert!(t.whs_ps.is_none() && v.missing.contains_key("hold"), "hold is named missing, not zero");
        let io = v.io.as_ref().expect("io");
        assert_eq!(io.pins.len(), 9, "clk + 8 LEDs");
        assert!(io.pins.iter().all(|p| p.bank.is_some()), "bank from the package table");
        let c = v.clocks.as_ref().expect("clocks");
        assert_eq!(c.clocks.len(), 1);
        assert!(c.clocks[0].loads >= 27 && !c.clocks[0].regions.is_empty() && c.clocks[0].buffer.is_some(), "{:?}", c.clocks[0]);
        assert!(c.bufgs_used >= 1 && c.bufgs_available == 32);
        assert!(!v.control_sets.is_empty() && !v.high_fanout.is_empty());
        assert!(v.missing.contains_key("power"), "power names its missing input");
    }

    #[test]
    fn ice40_run_writes_an_asc_with_project_pins() {
        let d = helion_sv::synth_sv(
            "module blinky(input wire clk, output wire led);\n reg [7:0] c = 0;\n always @(posedge clk) c <= c + 8'd1;\n assign led = c[7];\nendmodule\n",
            "blinky.sv",
        )
        .unwrap();
        let dir = std::env::temp_dir().join(format!("helion-fam-{}", std::process::id()));
        let lines = std::cell::RefCell::new(Vec::new());
        let art = run_family(
            d,
            "ice40-hx8k",
            &[("clk".into(), "J3".into()), ("led".into(), "B5".into())],
            &dir,
            &helion_sta::Constraints::default(),
            &[helion_sta::Clock {
                name: "clk".into(),
                period_ps: 10_000,
                source: "clk".into(),
                generated: false,
                master: None,
                divide_by: 1,
                multiply_by: 1,
                invert: false,
                edges: Vec::new(),
            }],
            &|l, _| lines.borrow_mut().push(l),
        )
        .unwrap();
        assert!(art.summary.contains(".asc") || art.summary.contains("asc written"), "{}", art.summary);
        assert!(dir.join("blinky.asc").is_file());
        assert_eq!(art.bit_file.as_deref(), Some(dir.join("blinky.asc").as_path()), "Program Device sees the .asc");
        assert!(lines.borrow().iter().any(|l| l.starts_with("WARN:") && l.contains("G25") && l.contains("icepack")));
        assert!(lines.borrow().iter().any(|l| l.starts_with("route")));
        assert!(foreign_family("HL10T-C32-1").is_none());
        // Reports reflect the iCE40 part, not the HL10T model.
        let u = art.util.as_ref().expect("family utilization");
        assert_eq!(u.luts_avail, 7680);
        assert_eq!(u.rows()[0].0, "LUT4");
        assert_eq!(u.regs_used, 8);
        let t = art.timing.as_ref().expect("family timing");
        assert!(t.model.contains("iCE40"), "{}", t.model);
        assert!(art.drc.as_ref().is_some_and(|d| !d.iter().any(|x| x.severity == DrcSev::Error)));
        let die = art.family_die.as_ref().expect("die");
        assert_eq!((die.grid.cols, die.grid.rows), (34, 34));
        assert!(die.placed.iter().any(|p| p.name == "led" && (p.x, p.y) == (7, 33)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
