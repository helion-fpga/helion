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
        ..RunArtifacts::default()
    })
}

/// Scene/test fixture: implement `examples/counter.sv` on `part` through the
/// real family flow (temp output dir) and apply the run's reports.
pub fn seed_family_fixture(s: &mut crate::state::AppState, part: &str) -> Result<(), String> {
    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
    let design = helion_sv::synth_sv_path(&src)?;
    let dir = std::env::temp_dir().join(format!("helion-rune-family-{}-{part}", std::process::id()));
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

#[cfg(test)]
mod tests {
    use super::*;

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
