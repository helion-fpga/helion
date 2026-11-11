//! Export Hardware (`write_hw_platform`) and Archive Project
//! (`archive_project`) dialogs plus their Tcl verbs.
//!
//! Export Hardware gathers what the session really has — the last
//! implementation run's bitstream packets, the open block design's emitted
//! HDL, address map and processor, the project part and board, and the
//! project's constraint files — and hands it to
//! `helion_proj::hw_platform::write_hw_platform`. Archive Project calls
//! `helion_proj::archive::archive_project` on the open `.prj`.

use std::path::{Path, PathBuf};

use rune::{El, TextEdit, col, row};
use rune_kit::{Checkbox, Dialog, Kit};

use crate::state::{AppState, ToastKind};
use crate::theme::VivadoTheme;
use crate::views::cad;
use crate::views::dialogs::DialogId;
use crate::views::ip_dialogs::{edit_box, field, footer, hrule};

/// Dialog drafts and last outcomes.
#[derive(Clone, Debug)]
pub struct HandoffState {
    pub hw_path: TextEdit,
    pub hw_bitstream: bool,
    pub hw_last: Option<Result<String, String>>,
    pub ar_path: TextEdit,
    pub ar_runs: bool,
    pub ar_last: Option<Result<String, String>>,
}

impl Default for HandoffState {
    fn default() -> Self {
        Self {
            hw_path: TextEdit::new(""),
            hw_bitstream: true,
            hw_last: None,
            ar_path: TextEdit::new(""),
            ar_runs: false,
            ar_last: None,
        }
    }
}

/// The open `.prj`, if the project lives on disk.
pub fn project_prj(s: &AppState) -> Option<PathBuf> {
    s.runs
        .source
        .clone()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("prj")) && p.is_file())
}

/// Board recorded in the `.prj` (`# board NAME`, written by New Project).
pub fn project_board(s: &AppState) -> Option<String> {
    let text = std::fs::read_to_string(project_prj(s)?).ok()?;
    text.lines()
        .find_map(|l| l.trim().strip_prefix("# board ").map(|b| b.trim().to_string()))
        .filter(|b| !b.is_empty())
}

fn out_dir(s: &AppState) -> PathBuf {
    match project_prj(s).and_then(|p| p.parent().map(Path::to_path_buf)) {
        Some(d) => d,
        None => std::env::temp_dir().join("helion-handoff").join(if s.project.name.is_empty() {
            "project"
        } else {
            &s.project.name
        }),
    }
}

fn platform_name(s: &AppState) -> String {
    s.ip.bd
        .as_ref()
        .filter(|b| b.ok() && !b.sys.cells.is_empty())
        .map(|b| b.name().to_string())
        .or_else(|| s.project.top.clone())
        .unwrap_or_else(|| s.project.name.clone())
}

fn bitstream_bytes(s: &AppState) -> Option<Vec<u8>> {
    s.bitstream
        .bits
        .as_ref()
        .filter(|b| !b.packets.is_empty())
        .map(|b| b.packets.clone())
}

/// Why Export Hardware is unavailable.
pub fn export_reason(s: &AppState) -> Option<String> {
    let has_bd = s.ip.bd.as_ref().is_some_and(|b| b.ok() && !b.sys.cells.is_empty());
    if !has_bd && bitstream_bytes(s).is_none() {
        return Some("Nothing to export: run implementation for a bitstream or build a valid block design.".into());
    }
    None
}

/// Open the Export Hardware dialog with a default output path.
pub fn open_export(s: &mut AppState) {
    if s.handoff.hw_path.text.trim().is_empty() {
        let p = out_dir(s).join(format!("{}.hwp", platform_name(s)));
        s.handoff.hw_path = TextEdit::new(p.display().to_string());
    }
    s.handoff.hw_bitstream = bitstream_bytes(s).is_some();
    s.handoff.hw_last = None;
    s.dialogs.open(DialogId::ExportHardware);
}

/// Write the hardware platform hand-off to `out`.
pub fn export_hw(s: &mut AppState, out: &Path, with_bitstream: bool) -> Result<String, String> {
    if let Some(r) = export_reason(s) {
        return Err(format!("write_hw_platform: {r}"));
    }
    let name = platform_name(s);
    let stage = crate::views::ip_model::gen_dir(s).join("hw").join(&name);
    std::fs::create_dir_all(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    let mut p = helion_proj::hw_platform::HwPlatform {
        name: name.clone(),
        top: name.clone(),
        part: s.project.part.clone(),
        family: helion_targets::family_of(&s.project.part)
            .map(|f| f.label().to_string())
            .unwrap_or_else(|| s.project.family.clone()),
        board: project_board(s),
        ..Default::default()
    };
    if with_bitstream {
        let bytes = bitstream_bytes(s).ok_or("no bitstream in this session (run implementation first)")?;
        let bit = stage.join(format!("{name}.bit"));
        std::fs::write(&bit, bytes).map_err(|e| format!("{}: {e}", bit.display()))?;
        p.bitstream = Some(bit);
    }
    if let Some(bd) = s.ip.bd.as_ref().filter(|b| !b.sys.cells.is_empty())
        && let Ok(emit) = &bd.emit
    {
        let top = stage.join(format!("{}.sv", bd.name()));
        std::fs::write(&top, &emit.sv).map_err(|e| format!("{}: {e}", top.display()))?;
        p.bd_sources.push(top);
        p.bd_sources.extend(emit.sources.iter().cloned());
        p.top = bd.name().to_string();
        for seg in bd.sys.address_map() {
            let vlnv = bd.sys.cell(&seg.inst).map(|c| c.model.core.vlnv()).unwrap_or_default();
            p.address_map.push(helion_proj::hw_platform::HwSegment {
                cell: seg.inst,
                vlnv,
                interface: "s_mm".into(),
                base: seg.base,
                range: seg.range,
            });
        }
        if let Some(m) = emit.interconnect.as_ref().and_then(|i| i.master.clone()).and_then(|m| bd.sys.cell(&m)) {
            let reset = m
                .params
                .get("PROGADDR_RESET")
                .cloned()
                .or_else(|| m.model.param("PROGADDR_RESET").map(|x| x.default.clone()))
                .and_then(|v| helion_ipxact::parse_param_number(&v))
                .unwrap_or(0) as u64;
            p.processor = Some(helion_proj::hw_platform::HwProcessor {
                cell: m.inst.clone(),
                vlnv: m.model.core.vlnv(),
                reset_vector: reset,
                params: m.params.clone(),
            });
        }
    }
    for f in s
        .project_files
        .files
        .iter()
        .filter(|f| f.set == crate::project::FileSet::Constraints)
    {
        if let Some(path) = f.path.as_ref().filter(|p| p.is_file()) {
            p.constraints.push(path.clone());
        }
    }
    let rep = helion_proj::hw_platform::write_hw_platform(&p, out)?;
    Ok(format!(
        "write_hw_platform {} files={} bytes={} bitstream={} peripherals={}",
        rep.path.display(),
        rep.files,
        rep.bytes,
        u8::from(rep.has_bitstream),
        p.address_map.len()
    ))
}

/// Why Archive Project is unavailable.
pub fn archive_reason(s: &AppState) -> Option<String> {
    if project_prj(s).is_none() {
        return Some("Archive needs a project saved on disk (.prj).".into());
    }
    None
}

/// Open the Archive Project dialog with a default output path.
pub fn open_archive(s: &mut AppState) {
    if s.handoff.ar_path.text.trim().is_empty() {
        let name = if s.project.name.is_empty() { "project" } else { &s.project.name };
        let p = out_dir(s).join(format!("{name}_archive.zip"));
        s.handoff.ar_path = TextEdit::new(p.display().to_string());
    }
    s.handoff.ar_last = None;
    s.dialogs.open(DialogId::ArchiveProject);
}

/// Archive the open project to `out`.
pub fn archive(s: &AppState, out: &Path, include_runs: bool) -> Result<String, String> {
    if let Some(r) = archive_reason(s) {
        return Err(format!("archive_project: {r}"));
    }
    let prj = project_prj(s).expect("checked");
    let runs = crate::runs::checkpoint_root(s);
    let rep = helion_proj::archive::archive_project(
        &prj,
        out,
        &helion_proj::archive::ArchiveOptions {
            runs_dir: (include_runs && runs.is_dir()).then_some(runs),
        },
    )?;
    Ok(format!(
        "archive_project {} entries={} bytes={} sources={} constraints={} ip={} run_files={}{}",
        rep.zip.display(),
        rep.entries,
        rep.bytes,
        rep.sources,
        rep.constraints,
        rep.ip_packages,
        rep.run_files,
        if rep.missing.is_empty() {
            String::new()
        } else {
            format!(" missing={}", rep.missing.join(","))
        }
    ))
}

fn console(s: &mut AppState, r: &Result<String, String>) {
    let (text, sev) = match r {
        Ok(v) => (v.clone(), crate::state::MsgSev::Info),
        Err(e) => (format!("ERROR: {e}"), crate::state::MsgSev::Error),
    };
    s.console.lines.push(crate::state::ConsoleLine { text, sev });
}

fn summary_row(label: &str, value: String, t: &VivadoTheme) -> El {
    cad::kv(label, cad::cut(value, VivadoTheme::FONT_UI, t.text), 120.0, t)
}

fn export_dialog(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme, kit: &Kit) {
    let bd = s.ip.bd.as_ref().filter(|b| b.ok() && !b.sys.cells.is_empty());
    let bits = bitstream_bytes(s);
    let mut body = col()
        .key("hw-body")
        .w_full()
        .gap(4.0)
        .padding(10.0)
        .child(field(
            "Output file",
            edit_box("hw-path", &s.handoff.hw_path, "design.hwp", kit, |s: &mut AppState| &mut s.handoff.hw_path),
            t,
        ))
        .child(summary_row("Part", s.project.part.clone(), t))
        .child(summary_row("Board", project_board(s).unwrap_or_else(|| "none recorded".into()), t))
        .child(summary_row(
            "Block design",
            match bd {
                Some(b) => format!(
                    "{} · {} cells · {} mapped peripherals",
                    b.name(),
                    b.sys.cells.len(),
                    b.sys.address_map().len()
                ),
                None => "none (bitstream-only platform)".into(),
            },
            t,
        ));
    let bit_label = match &bits {
        Some(b) => format!("Include bitstream ({} bytes from {})", b.len(), s.bitstream.source),
        None => "Include bitstream (none: run implementation first)".into(),
    };
    let mut cb = Checkbox::new(s.handoff.hw_bitstream && bits.is_some()).label(&bit_label);
    if bits.is_some() {
        cb = cb.on_toggle::<AppState>(|s: &mut AppState| s.handoff.hw_bitstream = !s.handoff.hw_bitstream);
    }
    body = body.child(row().key("hw-bitstream").child(cb.build(kit)));
    body = body.child(
        cad::dim("Writes platform.json (format helion-hw-platform v1), the bitstream, block-design HDL, constraints and sw/helion_platform.h.", t)
            .wrap()
            .w_full(),
    );
    let reason = export_reason(s).or_else(|| {
        s.handoff.hw_path.text.trim().is_empty().then(|| "Choose an output file.".to_string())
    });
    let status = s.handoff.hw_last.as_ref().map(|r| match r {
        Ok(v) => (v.as_str(), false),
        Err(e) => (e.as_str(), true),
    });
    let panel = col().w_full().child(body).child(hrule(t)).child(footer(
        "hw",
        "Export",
        reason.as_deref(),
        status,
        t,
        kit,
        |s: &mut AppState| {
            let out = PathBuf::from(s.handoff.hw_path.text.trim());
            let with_bit = s.handoff.hw_bitstream && bitstream_bytes(s).is_some();
            let r = export_hw(s, &out, with_bit);
            console(s, &r);
            if let Ok(v) = &r {
                s.toasts.push(ToastKind::Success, "Hardware exported", v);
                s.dialogs.close();
            }
            s.handoff.hw_last = Some(r);
        },
    ));
    Dialog::new(DialogId::ExportHardware.key(), "Export Hardware Platform")
        .open(true)
        .width(620.0)
        .fit()
        .on_close(|s: &mut AppState| s.dialogs.close())
        .child(panel)
        .build(kit, layer);
}

fn archive_dialog(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme, kit: &Kit) {
    let runs = crate::runs::checkpoint_root(s);
    let mut cb = Checkbox::new(s.handoff.ar_runs && runs.is_dir()).label("Include run results and checkpoints");
    if runs.is_dir() {
        cb = cb.on_toggle::<AppState>(|s: &mut AppState| s.handoff.ar_runs = !s.handoff.ar_runs);
    }
    let body = col()
        .key("ar-body")
        .w_full()
        .gap(4.0)
        .padding(10.0)
        .child(field(
            "Archive file",
            edit_box("ar-path", &s.handoff.ar_path, "project_archive.zip", kit, |s: &mut AppState| &mut s.handoff.ar_path),
            t,
        ))
        .child(summary_row(
            "Project",
            project_prj(s).map(|p| p.display().to_string()).unwrap_or_else(|| "not on disk".into()),
            t,
        ))
        .child(row().key("ar-runs").gap(8.0).items_center().child(cb.build(kit)).child(cad::dim(
            if runs.is_dir() { runs.display().to_string() } else { "no run results yet".into() },
            t,
        )))
        .child(
            cad::dim("Sources, constraints and IP packages are copied in and every path in the .prj is rewritten relative to the archive.", t)
                .wrap()
                .w_full(),
        );
    let reason = archive_reason(s).or_else(|| {
        s.handoff.ar_path.text.trim().is_empty().then(|| "Choose an archive file.".to_string())
    });
    let status = s.handoff.ar_last.as_ref().map(|r| match r {
        Ok(v) => (v.as_str(), false),
        Err(e) => (e.as_str(), true),
    });
    let panel = col().w_full().child(body).child(hrule(t)).child(footer(
        "ar",
        "Archive",
        reason.as_deref(),
        status,
        t,
        kit,
        |s: &mut AppState| {
            let out = PathBuf::from(s.handoff.ar_path.text.trim());
            let r = archive(s, &out, s.handoff.ar_runs);
            console(s, &r);
            if let Ok(v) = &r {
                s.toasts.push(ToastKind::Success, "Project archived", v);
                s.dialogs.close();
            }
            s.handoff.ar_last = Some(r);
        },
    ));
    Dialog::new(DialogId::ArchiveProject.key(), "Archive Project")
        .open(true)
        .width(620.0)
        .fit()
        .on_close(|s: &mut AppState| s.dialogs.close())
        .child(panel)
        .build(kit, layer);
}

/// Dispatch (called from `dialogs::push_dialogs`).
pub fn push(id: DialogId, layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme, kit: &Kit) {
    match id {
        DialogId::ExportHardware => export_dialog(layer, s, t, kit),
        DialogId::ArchiveProject => archive_dialog(layer, s, t, kit),
        _ => {}
    }
}

/// Tcl `write_hw_platform ?-include_bit? ?path?` / `archive_project ?-include_runs? ?path?`.
pub fn tcl_verb(s: &mut AppState, name: &str, args: &[String]) -> Result<String, String> {
    let flag = |f: &str| args.iter().any(|a| a == f);
    let path = args.iter().find(|a| !a.starts_with('-')).map(PathBuf::from);
    match name {
        "write_hw_platform" => {
            let out = path.unwrap_or_else(|| out_dir(s).join(format!("{}.hwp", platform_name(s))));
            let with_bit = flag("-include_bit") || (bitstream_bytes(s).is_some() && !flag("-no_bit"));
            export_hw(s, &out, with_bit)
        }
        "archive_project" => {
            let name = if s.project.name.is_empty() { "project".to_string() } else { s.project.name.clone() };
            let out = path.unwrap_or_else(|| out_dir(s).join(format!("{name}_archive.zip")));
            archive(s, &out, flag("-include_runs"))
        }
        _ => Err(format!("{name}: not a hand-off verb")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_needs_content_and_writes_bd_platform() {
        let mut s = crate::init();
        s.project.name = format!("hwp-{}", std::process::id());
        assert!(export_reason(&s).is_some());
        crate::views::ip_model::create_bd_cell(&mut s, "h_rv32_hb1").unwrap();
        crate::views::ip_model::create_bd_cell(&mut s, "h_uart").unwrap();
        crate::views::ip_model::apply_automation(&mut s).unwrap();
        assert!(export_reason(&s).is_none());
        let out = std::env::temp_dir().join(format!("hwp-test-{}/p.hwp", std::process::id()));
        let msg = export_hw(&mut s, &out, false).unwrap();
        assert!(msg.contains("peripherals=1"), "{msg}");
        let entries = helion_proj::zip::read_zip(&std::fs::read(&out).unwrap()).unwrap();
        let manifest = String::from_utf8(entries[0].1.clone()).unwrap();
        assert!(manifest.contains("\"cell\": \"h_rv32_hb1_0\""));
        assert!(manifest.contains("\"base\": \"0x40000000\""));
        assert!(entries.iter().any(|(n, _)| n.ends_with("picorv32.v")));
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
        let _ = std::fs::remove_dir_all(crate::views::ip_model::gen_dir(&s));
    }

    #[test]
    fn archive_requires_a_project_on_disk_and_writes_a_zip() {
        let mut s = crate::init();
        assert!(archive_reason(&s).is_some(), "in-memory demo cannot be archived");
        let dir = std::env::temp_dir().join(format!("ar-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("top.v"), "module top(input a, output y); assign y = a; endmodule\n").unwrap();
        let prj = dir.join("p.prj");
        std::fs::write(&prj, format!("part HL10T-C32-1\n# board HB1\nread_sv {}\n", dir.join("top.v").display())).unwrap();
        s.runs.source = Some(prj);
        assert_eq!(project_board(&s).as_deref(), Some("HB1"));
        let out = dir.join("out.zip");
        let msg = tcl_verb(&mut s, "archive_project", &[out.display().to_string()]).unwrap();
        assert!(msg.contains("sources=1"), "{msg}");
        assert!(out.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
