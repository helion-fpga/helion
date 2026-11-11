//! Implementation runs for non-Helion families (iCE40, ECP5, 7-series).
//!
//! The run worker synthesizes with helion-sv as usual; when the project part
//! belongs to another family (`helion_targets::family_of`), this module takes
//! over instead of the HL10T place/route/bitgen: it applies the project's pin
//! constraints as `LOC`, runs that family's pack → place → route → bitstream
//! through `helion_targets::implement`, and writes the family's bitstream
//! artifact (IceStorm `.asc`, Trellis `.config`, FASM) next to the run's
//! checkpoints. Reports that need the HL10T device model (timing, power,
//! utilization on HAD sites) are not produced for these parts.

use std::path::{Path, PathBuf};

use crate::runs::RunArtifacts;
use crate::state::MsgSev;

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
        ..RunArtifacts::default()
    })
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
            &|l, _| lines.borrow_mut().push(l),
        )
        .unwrap();
        assert!(art.summary.contains(".asc") || art.summary.contains("asc written"), "{}", art.summary);
        assert!(dir.join("blinky.asc").is_file());
        assert!(lines.borrow().iter().any(|l| l.starts_with("route")));
        assert!(foreign_family("HL10T-C32-1").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
