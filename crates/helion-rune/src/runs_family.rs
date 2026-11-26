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

/// The Timing view's state from an implementation run's view contract: the
/// placed-graph analysis the contract carries, so the Timing pane, the
/// status bar and the contract reports state one WNS.
fn view_timing(v: &helion_targets::view::ImplView, part: &str, send: &dyn Fn(String, MsgSev)) -> Option<TimingState> {
    let Some(t) = &v.timing else {
        let why = v.missing.get("timing").map(String::as_str).unwrap_or("no timing in this run");
        send(format!("report_timing: not computed for {part}: {why}"), MsgSev::Warn);
        return None;
    };
    let model = v.provenance.get("timing").map(|p| p.method.clone()).unwrap_or_default();
    send(format!("report_timing WNS_PS={} TNS_PS={} endpoints={} ({} placed logic graph)", t.wns_ps, t.tns_ps, t.total_endpoints, v.family.label()), MsgSev::Info);
    let clocks = v
        .clocks
        .iter()
        .flat_map(|c| &c.clocks)
        .filter(|c| c.period_ps > 0)
        .map(|c| crate::state::TimingClock { name: c.name.clone(), period_ps: c.period_ps as u64, source: c.name.clone(), generated: false, master: None })
        .collect();
    let paths = t
        .worst_path
        .iter()
        .map(|p| crate::state::TimingPathRow { name: "Path 1 (worst)".into(), from: p.from.clone(), to: p.to.clone(), slack_ps: p.slack_ps, delay_ps: p.requirement_ps - p.slack_ps, ..Default::default() })
        .collect();
    Some(TimingState {
        wns_ps: Some(t.wns_ps),
        tns_ps: t.tns_ps,
        failing_endpoints: t.failing_endpoints,
        failing_setup: t.failing_endpoints,
        endpoints: t.total_endpoints,
        has_run: true,
        clocks,
        paths,
        model: format!("{}: {model}", v.family.label()),
        ..TimingState::default()
    })
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
    let periods: Vec<(String, u32)> = clks.iter().map(|c| (if c.source.is_empty() { c.name.clone() } else { c.source.clone() }, c.period_ps as u32)).collect();
    let (out, view) = helion_targets::implement_view(&design, part, &periods, &mut |line| send(line.to_string(), MsgSev::Info))?;
    let util = family_util(&design, part).map_err(|e| send(format!("WARN: utilization skipped: {e}"), MsgSev::Warn)).ok();
    let drc = family_drc(&design, part).map_err(|e| send(format!("WARN: DRC skipped: {e}"), MsgSev::Warn)).ok();
    if let Some(d) = &drc {
        let errors = d.iter().filter(|x| x.severity == DrcSev::Error).count();
        send(format!("report_drc {} violation(s), {errors} error(s)", d.len()), if errors > 0 { MsgSev::Warn } else { MsgSev::Info });
    }
    let timing = match &view {
        Some(v) => view_timing(v, part, send),
        None => family_timing(&design, part, xdc, clks, send),
    };
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
        view,
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
    let (design, _, _) = helion_targets::xdc_flow::synth(&design_arg, top.as_deref(), "auto", &mut log)?;
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
    // A real run's timing state carries WNS to the status bar; the fixture takes it from the view.
    s.timing.wns_ps = s.runs.impl_view.as_ref().and_then(|v| v.timing.as_ref()).map(|t| t.wns_ps);
    s.flow.impl_run = crate::state::RunStatus::Complete;
    Ok(())
}

/// A real HL10 implementation of `examples/board_counter.sv` at 100 MHz:
/// its reset and enable pads reach the logic over interconnect wires.
pub fn hl10_board_counter_run() -> Result<RunArtifacts, String> {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("helion-rune-hl10-view-{}-{}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let prj = dir.join("board_counter.prj");
    let sv = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/board_counter.sv");
    let art = std::fs::write(&prj, format!("part HL10T-C32-1\nread_sv {}\ncreate_clock -period 10.000 [get_ports clk]\n", sv.display()))
        .map_err(|e| format!("{}: {e}", prj.display()))
        .and_then(|()| crate::runs::run_blocking(crate::runs::RunKind::Impl, &prj, "HL10T-C32-1", &|_, _| {}));
    let _ = std::fs::remove_dir_all(&dir);
    art
}

/// Scene fixture: that HL10 run applied to the session, as a finished
/// implementation run leaves it.
pub fn seed_hl10_view_fixture(s: &mut crate::state::AppState) -> Result<(), String> {
    let art = hl10_board_counter_run()?;
    s.project.part = "HL10T-C32-1".to_string();
    s.flow.impl_run = crate::state::RunStatus::Complete;
    crate::runs::apply_artifacts(s, art);
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

    /// Guard: the view fixture the scenes draw carries the run's WNS to the
    /// status bar, as a finished run's timing state does.
    #[test]
    fn the_view_fixture_gives_the_status_bar_the_runs_wns() {
        if !helion_targets::zynq_db_present() {
            eprintln!("skipped: no zynq7 X-Ray database");
            return;
        }
        let mut s = crate::init();
        seed_xilinx7_view_fixture(&mut s).expect("fixture");
        let wns = s.runs.impl_view.as_ref().and_then(|v| v.timing.as_ref()).map(|t| t.wns_ps);
        assert!(wns.is_some(), "the run has timing");
        assert_eq!(s.timing.wns_ps, wns);
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
        assert!(t.whs_ps.is_some() && t.ths_ps.is_some() && t.worst_hold_path.is_some(), "hold is computed for this run");
        let io = v.io.as_ref().expect("io");
        assert_eq!(io.pins.len(), 9, "clk + 8 LEDs");
        assert!(io.pins.iter().all(|p| p.bank.is_some()), "bank from the package table");
        let c = v.clocks.as_ref().expect("clocks");
        assert_eq!(c.clocks.len(), 1);
        assert!(c.clocks[0].loads >= 27 && !c.clocks[0].regions.is_empty() && c.clocks[0].buffer.is_some(), "{:?}", c.clocks[0]);
        assert!(c.bufgs_used >= 1 && c.bufgs_available == 32);
        assert!(!v.control_sets.is_empty() && !v.high_fanout.is_empty());
        let p = v.power.as_ref().expect("power estimate");
        assert!(p.dynamic_mw > 0.0 && p.static_mw > 0.0 && !p.tiles.is_empty() && p.method.contains("fit error"), "{p:?}");
        assert!(v.resources.iter().any(|r| r.name == "Slice Registers" && r.used >= 27.0), "utilization rows");
        assert_eq!(v.congestion.len(), 2, "H and V congestion levels");
    }

    /// One real run of the ZedBoard OLED demo, shared by the schematic and
    /// board guards (`None` without the X-Ray database).
    fn oled_view() -> Option<&'static helion_targets::view::ImplView> {
        static RUN: std::sync::OnceLock<Option<helion_targets::view::ImplView>> = std::sync::OnceLock::new();
        RUN.get_or_init(|| {
            if !helion_targets::zynq_db_present() {
                eprintln!("skipped: no zynq7 X-Ray database");
                return None;
            }
            let prj = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/zedboard_oled_demo.prj");
            let dir = std::env::temp_dir().join(format!("helion-rune-oled-{}", std::process::id()));
            let art = run_xdc_flow(&prj, "xc7z020", &dir, &|_, _| {}).expect("xdc flow");
            let _ = std::fs::remove_dir_all(&dir);
            Some(art.view.expect("impl view"))
        })
        .as_ref()
    }

    /// Guard: the RTL schematic is the v2 front end's elaborated netlist of
    /// this run: word registers and operators, the design's own ports.
    #[test]
    fn oled_run_fills_the_rtl_schematic() {
        use helion_targets::view::NetlistStage;
        let Some(v) = oled_view() else { return };
        let n = v.netlist(NetlistStage::Rtl).expect("RTL netlist");
        assert_eq!(n.top, "zedboard_oled_demo");
        assert_eq!(n.ports.len(), 8, "clk, six OLED pins, led");
        let kind = |c: &str| n.cells.iter().find(|x| x.name == c).map(|x| x.kind.as_str());
        for r in ["wait_cnt", "rom_idx", "bx", "by", "frame"] {
            assert_eq!(kind(r), Some("RTL_REG"), "{r} is one word register");
        }
        let k = n.kinds();
        assert!(k.get("RTL_ADD").copied().unwrap_or(0) + k.get("RTL_SUB").copied().unwrap_or(0) >= 3, "counters add: {k:?}");
        assert!(k.get("RTL_MUX").copied().unwrap_or(0) >= 1 && k.get("RTL_EQ").copied().unwrap_or(0) >= 1, "{k:?}");
        assert!(!k.keys().any(|t| t.starts_with("LUT") || t.starts_with("FD")), "no mapped primitives at RTL: {k:?}");
        let clk = n.nets.iter().find(|x| x.name == "clk").expect("clk net");
        assert_eq!(clk.drivers, vec!["clk".to_string()], "the port drives clk");
        assert!(clk.loads.iter().filter(|l| l.ends_with("/C")).count() >= 5, "registers clocked by clk");
        assert!(n.ports.iter().all(|p| !p.nets.is_empty()), "every port is wired");
    }

    /// Guard: the synthesized schematic is the mapped netlist the packer read.
    #[test]
    fn oled_run_fills_the_synthesized_schematic() {
        use helion_targets::view::NetlistStage;
        let Some(v) = oled_view() else { return };
        let n = v.netlist(NetlistStage::Synthesized).expect("synthesized netlist");
        let k = n.kinds();
        let luts: usize = k.iter().filter(|(t, _)| t.starts_with("LUT")).map(|(_, c)| c).sum();
        let ffs: usize = k.iter().filter(|(t, _)| t.starts_with("FD")).map(|(_, c)| c).sum();
        assert!(luts >= 50 && ffs >= 60, "{k:?}");
        assert!(!k.keys().any(|t| t.starts_with("RTL_")), "no RTL operators after mapping: {k:?}");
        assert_eq!(ffs, v.cells.iter().filter(|c| c.kind == "FF").count(), "every flop the placer placed");
        let undriven = n.nets.iter().filter(|x| x.drivers.is_empty() && !x.loads.is_empty()).count();
        assert_eq!(undriven, 0, "every used net has a driver: {:?}", n.nets.iter().filter(|x| x.drivers.is_empty()).take(5).collect::<Vec<_>>());
        assert!(n.cells.iter().all(|c| c.site.is_none()), "no placement before implementation");
    }

    /// Guard: the implemented schematic is the synthesized one with the
    /// placement's site and BEL on every LUT and flop.
    #[test]
    fn oled_run_fills_the_implemented_schematic() {
        use helion_targets::view::NetlistStage;
        let Some(v) = oled_view() else { return };
        let syn = v.netlist(NetlistStage::Synthesized).expect("synthesized");
        let n = v.netlist(NetlistStage::Implemented).expect("implemented netlist");
        assert!(n.cells.len() >= syn.cells.len());
        for c in n.cells.iter().filter(|c| c.kind.starts_with("FD")) {
            let site = c.site.as_deref().unwrap_or_else(|| panic!("{} unplaced", c.name));
            assert!(site.starts_with("SLICE_") && c.bel.is_some(), "{}: {site} {:?}", c.name, c.bel);
            assert!(v.cells.iter().any(|p| p.cell == c.name && p.site == site), "{} site from the run", c.name);
        }
        assert!(n.cells.iter().filter(|c| c.kind.starts_with("LUT")).all(|c| c.site.is_some()), "every LUT placed");
        assert!(v.provenance[NetlistStage::Implemented.section()].method.contains("placed under their own name"));
    }

    /// Guard: the board panel puts the OLED demo's ports on the right
    /// ZedBoard parts and lists what it leaves free.
    #[test]
    fn oled_run_maps_its_ports_onto_the_zedboard() {
        let Some(v) = oled_view() else { return };
        let b = v.board.as_ref().expect("board panel");
        assert_eq!(b.board, "Avnet/Digilent ZedBoard");
        let at = |p: &str| b.part_of(p).map(|(g, i)| (g.kind.as_str(), i.label.as_str(), i.pin.as_str()));
        assert_eq!(at("clk"), Some(("Clock", "GCLK", "Y9")));
        assert_eq!(at("led"), Some(("LED", "LD0", "T22")));
        for (port, label, pin) in [("oled_dc", "OLED-DC", "U10"), ("oled_res", "OLED-RES", "U9"), ("oled_sclk", "OLED-SCLK", "AB12"), ("oled_sdin", "OLED-SDIN", "AA12"), ("oled_vbat", "OLED-VBAT", "U11"), ("oled_vdd", "OLED-VDD", "U12")] {
            assert_eq!(at(port), Some(("OLED", label, pin)), "{port}");
        }
        assert_eq!(b.used().count(), 8);
        assert!(b.off_board.is_empty(), "{:?}", b.off_board);
        let free = |k: &str| b.groups.iter().find(|g| g.kind == k).map(|g| g.items.iter().filter(|i| i.port.is_none()).count());
        assert_eq!((free("LED"), free("Switch"), free("Button"), free("Pmod JA")), (Some(7), Some(8), Some(5), Some(8)));
    }

    /// Checks every family's filled contract shares: a die, placed cells on
    /// it, routed nets with hops, per-tile site and wire usage, every
    /// endpoint's slack on the die, clocks, IO, the DRC deck, the
    /// schematic stages, and a named reason for whatever is absent.
    fn assert_view_is_filled(v: &helion_targets::view::ImplView, ports: usize) {
        use helion_targets::view::NetlistStage;
        assert!(!v.tiles.is_empty() && v.cols > 0 && v.rows > 0);
        let on_die: std::collections::HashSet<&str> = v.tiles.iter().map(|t| t.name.as_str()).collect();
        let sites: std::collections::HashSet<&str> = v.tiles.iter().flat_map(|t| t.sites.iter().map(String::as_str)).collect();
        assert!(v.cells.iter().filter(|c| c.kind == "FF").count() >= 4, "the counter's flops are placed");
        for c in &v.cells {
            assert!(c.tile.as_deref().is_some_and(|t| on_die.contains(t)), "{c:?} is off the die");
            assert!(sites.contains(c.site.as_str()), "{c:?}: no such site in its tile");
        }
        let routed: Vec<_> = v.nets.iter().filter(|n| !n.hops.is_empty()).collect();
        assert!(!routed.is_empty(), "routed nets with hops");
        assert!(routed.iter().all(|n| n.hops.iter().all(|h| on_die.contains(h.tile.as_str()))), "every hop names a tile of the die");
        assert!(v.nets.iter().any(|n| n.driver.is_some() && !n.loads.is_empty()), "driver and loads on nets");
        assert!(v.usage.iter().any(|u| u.sites_used > 0) && v.usage.iter().all(|u| u.sites_used <= u.sites_capacity), "per-tile site usage within capacity");
        assert!(v.usage.iter().any(|u| u.overflow.is_some_and(|o| o.h_used + o.v_used > 0)), "per-tile wire usage");
        assert!(v.usage.iter().filter_map(|u| u.overflow).all(|o| o.h_used <= o.h_capacity && o.v_used <= o.v_capacity), "a routed design is within wire capacity");
        assert_eq!(v.congestion.len(), 2, "H and V congestion levels");
        let t = v.timing.as_ref().expect("timing");
        assert!(t.total_endpoints >= 4 && t.endpoints.len() == t.total_endpoints, "every endpoint's slack: {} of {}", t.endpoints.len(), t.total_endpoints);
        assert!(t.endpoints.iter().all(|e| e.x.is_some() && e.y.is_some()), "endpoints land on the die");
        assert_eq!(t.endpoints.iter().map(|e| e.slack_ps).min(), Some(t.wns_ps), "WNS is the worst endpoint");
        assert!(t.worst_path.is_some() && t.fmax_mhz.is_some());
        let c = v.clocks.as_ref().expect("clocks");
        assert_eq!(c.clocks.len(), 1, "{:?}", c.clocks);
        assert!(c.clocks[0].loads >= 4 && c.clocks[0].period_ps == 10_000 && c.clocks[0].buffer.is_some(), "{:?}", c.clocks[0]);
        assert!(c.bufgs_used >= 1 && c.bufgs_available >= c.bufgs_used);
        let io = v.io.as_ref().expect("io");
        assert_eq!(io.pins.len(), ports, "one pin per port bit: {:?}", io.pins);
        assert!(io.pins.iter().all(|p| !p.package_pin.is_empty() && (p.bank.is_some() || p.package_pin == "configuration port")), "{:?}", io.pins);
        assert!(io.package_pins >= io.pins.len());
        assert!(!v.control_sets.is_empty() && !v.high_fanout.is_empty() && !v.resources.is_empty());
        assert!(v.resources.iter().all(|r| r.used <= r.available as f64), "{:?}", v.resources);
        assert!(v.drc_rules.len() >= 9 && v.provenance.contains_key("drc"), "the DRC deck ran");
        let syn = v.netlist(NetlistStage::Synthesized).expect("synthesized netlist");
        let imp = v.netlist(NetlistStage::Implemented).expect("implemented netlist");
        assert!(syn.kinds().keys().any(|k| k.starts_with("LUT")) && imp.cells.iter().filter(|c| c.site.is_some()).count() >= 4);
        assert!(v.netlist(NetlistStage::Rtl).is_none() && v.missing[NetlistStage::Rtl.section()].contains("word-level"), "the missing RTL stage names why");
        for section in ["board"] {
            assert!(v.missing.get(section).is_some_and(|m| m.len() > 20), "{section} names its reason");
        }
        // Every report and device panel renders a table or a named reason.
        use crate::views::contract_views as cv;
        let text = format!("{}{}", cv::render_all_reports(v), cv::render_device_tab(v));
        assert!(!text.contains("not available") && !text.contains("placeholder"), "{text}");
        for r in cv::Report::ALL {
            let p = cv::report(v, r).render();
            assert!(p.contains("---") || p.contains("missing input:"), "{r:?}: {p}");
        }
    }

    fn counter_view(part: &str) -> helion_targets::view::ImplView {
        let mut s = crate::init();
        seed_family_fixture(&mut s, part).expect("family run");
        s.runs.impl_view.expect("impl view")
    }

    /// Guard: an iCE40 implementation run fills the view contract from the
    /// run itself, and names what iCE40 does not have.
    #[test]
    fn ice40_run_fills_the_impl_view_from_the_run() {
        let v = counter_view("ice40-hx8k");
        assert_eq!((v.family, v.cols, v.rows, v.row0_top), (helion_targets::Family::Ice40, 34, 34, false));
        assert_view_is_filled(&v, 2);
        let io = v.io.as_ref().unwrap();
        let pin = |port: &str| io.pins.iter().find(|p| p.port == port).unwrap();
        assert_eq!((pin("clk").package_pin.as_str(), pin("clk").bank, pin("clk").clock_capable), ("J3", Some(3), Some(true)));
        assert_eq!((pin("led").package_pin.as_str(), pin("led").bank, pin("led").clock_capable), ("B5", Some(0), Some(false)));
        assert!(v.clocks.as_ref().unwrap().clocks[0].buffer.as_deref().is_some_and(|b| b.starts_with("glb_netwk_")));
        assert!(v.clock_regions().is_empty() && v.missing["clock-regions"].contains("no clock regions"));
        assert!(v.missing["io-standard"].contains("VCCIO") && v.missing["power"].contains("no power model") && v.missing["hold"].contains("one corner"));
        assert!(v.power.is_none() && v.timing.as_ref().unwrap().whs_ps.is_none());
        assert!(v.provenance["timing"].method.contains("iCE40-HX8K delay model"));
        let ran = |id: &str| v.drc_rules.iter().find(|r| r.id == id).unwrap().not_run.is_none();
        assert!(ran("UCIO-1") && ran("LUTLP-1") && ran("RTSTAT-1") && ran("HLN-CLKPIN") && !ran("NSTD-1") && !ran("ZPS7-1"));
        assert!(v.drc.is_empty(), "{:?}", v.drc);
    }

    /// Guard: an ECP5 implementation run fills the view contract from the
    /// run itself: quadrants as clock regions, banks from the IO database.
    #[test]
    fn ecp5_run_fills_the_impl_view_from_the_run() {
        let v = counter_view("LFE5U-85F");
        assert_eq!((v.family, v.cols, v.rows, v.row0_top), (helion_targets::Family::Ecp5, 127, 96, true));
        assert_view_is_filled(&v, 2);
        assert_eq!(v.clock_regions(), vec!["LL", "LR", "UL", "UR"], "the four global clock quadrants");
        let c = &v.clocks.as_ref().unwrap().clocks[0];
        assert!(!c.regions.is_empty() && c.buffer.as_deref().is_some_and(|b| b.contains("DCC")), "{c:?}");
        let io = v.io.as_ref().unwrap();
        assert!(io.pins.iter().any(|p| p.port == "clk" && p.package_pin == "A10" && p.clock_capable == Some(true)), "{:?}", io.pins);
        assert!(io.pins.iter().any(|p| p.port == "led" && p.package_pin == "A12"));
        assert!(v.nets.iter().any(|n| n.kind == "clock-spine" && !n.hops.is_empty()), "the global clock's arcs");
        assert!(v.tiles_of(helion_targets::view::TileClass::Dsp).count() > 0 && v.tiles_of(helion_targets::view::TileClass::Ram).count() > 0);
        assert!(v.missing["io-standard"].contains("IO type") && v.missing["power"].contains("no power model"));
        assert!(v.drc.is_empty(), "{:?}", v.drc);
    }

    /// Guard: an HL10 implementation run (the run worker's own flow) fills
    /// the view contract from the routed design.
    #[test]
    fn hl10_run_fills_the_impl_view_from_the_run() {
        let art = hl10_board_counter_run().expect("HL10 run");
        let wns = art.timing.as_ref().and_then(|t| t.wns_ps).expect("the run's WNS");
        let v = art.view.expect("impl view");
        assert_eq!((v.family, v.part.as_str(), v.row0_top), (helion_targets::Family::Helion, "HL10T-C32-1", false));
        assert_view_is_filled(&v, 11);
        let t = v.timing.as_ref().unwrap();
        assert_eq!(t.wns_ps, wns, "the contract states the run's WNS");
        assert!(t.whs_ps.is_some(), "the HL10 model has a hold figure");
        assert!(v.cells.iter().any(|c| c.kind == "LUT" && c.site.contains("/BLE") && c.bel.as_deref() == Some("LUT")));
        assert!(v.nets.iter().any(|n| n.kind == "pips" && n.hops.iter().all(|h| h.wire.starts_with(['N', 'E', 'S', 'W']))), "nets on interconnect wires");
        let io = v.io.as_ref().unwrap();
        assert!(io.pins.iter().any(|p| p.port == "clk" && p.package_pin == "configuration port"), "the implicit user clock: {:?}", io.pins);
        assert_eq!(io.pins.iter().filter(|p| p.package_pin.starts_with("IOB_X") && p.bank.is_some()).count(), 10, "rst, en and eight LEDs on pads");
        let p = v.power.as_ref().expect("power from the HL10 model");
        assert!(p.total_mw > 0.0 && (p.total_mw - p.static_mw - p.dynamic_mw).abs() < 1e-6 && p.tiles.is_empty());
        assert!(v.missing["power-tiles"].contains("attributes nothing to a tile"));
        assert!(v.missing["clock-regions"].contains("no clock regions") && v.missing["clock-capable"].contains("any HL10 input pad"));
        // No IOSTANDARD and no pin in the project: reported as Vivado would.
        assert!(v.drc.iter().any(|d| d.rule == "NSTD-1" && d.message.contains("LVCMOS18")), "{:?}", v.drc);
        assert!(v.drc.iter().any(|d| d.rule == "UCIO-1" && d.message.contains("the flow chose IOB_X")), "{:?}", v.drc);
        assert!(!v.drc.iter().any(|d| d.rule == "UCIO-1" && d.message.starts_with("clk:")), "the implicit clock needs no pin: {:?}", v.drc);
        assert!(v.drc_rules.iter().find(|r| r.id == "HLN-CLKPIN").unwrap().not_run.is_some());
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
