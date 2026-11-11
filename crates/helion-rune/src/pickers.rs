//! Native file pickers shared by every view: Open Project, Add Sources,
//! bitstream Browse, and the XDC/export path fields.
//!
//! All dialogs go through Rune's [`Tx`](rune::Tx) file-dialog requests, so the
//! window shows a REAL native dialog (a sheet on macOS, owned by the window
//! on Windows, the desktop portal chooser on Linux) and the answer callback
//! consumes the chosen path. This crate never touches a dialog crate itself;
//! Rune owns the native backend behind its `native-file-dialog` feature
//! (enabled on the `rune` dependency in this crate's `Cargo.toml`).
//!
//! Dialog titles and filters mirror `helion-gui` read-only:
//! - [`HDL_EXTENSIONS`] mirrors `helion-gui/src/open_dialog.rs`
//!   (`HDL_EXTENSIONS`) and its `"Open HDL / Project"` title.
//! - [`RTL_EXTENSIONS`] / [`CONSTRAINT_EXTENSIONS`] mirror the `"Add Sources"`
//!   / `"Add Constraints"` native filters in
//!   `helion-gui/src/bin/helion-ide.rs` (`pick_rtl_dialog`,
//!   `pick_constraint_dialog`).
//! - [`project_directory`] mirrors its `"Project Directory"` folder title.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/file_dialog.rs`: `Tx::open_file/save_file/open_folder`,
//!   `FileDialogOptions::filter`, `FileDialogResult`.
//! - `crates/rune/src/runtime/input/file_dialog.rs`:
//!   `Runtime::file_dialog/resolve_file_dialog` (the test seam: handlers raise
//!   `Pending`, tests answer it as if the user had picked).

use std::path::{Path, PathBuf};

use rune::{FileDialogOptions, FileDialogResult, Tx};

use crate::state::{AppState, ConsoleLine, MsgSev, ToastKind};

/// Extensions the Open dialog accepts (mirrors `helion-gui` `HDL_EXTENSIONS`).
pub const HDL_EXTENSIONS: &[&str] = &["sv", "v", "svh", "vhd", "vhdl", "sdc", "xdc", "prj"];
/// RTL filter for the Design Sources kind (mirrors `pick_rtl_dialog`).
pub const RTL_EXTENSIONS: &[&str] = &["sv", "v", "vhd", "vhdl"];
/// Constraints filter (mirrors `pick_constraint_dialog`).
pub const CONSTRAINT_EXTENSIONS: &[&str] = &["sdc", "xdc"];
/// Simulation-sources filter (RTL plus headers).
pub const SIM_EXTENSIONS: &[&str] = &["sv", "v", "svh"];
/// Incremental-checkpoint filter (`.hck2` disk checkpoints plus `.dcp`).
pub const CHECKPOINT_EXTENSIONS: &[&str] = &["hck2", "dcp"];
/// `tcl.pre` / `tcl.post` hook-script filter.
pub const TCL_EXTENSIONS: &[&str] = &["tcl"];
/// Program-device bitstream filter.
pub const BITSTREAM_EXTENSIONS: &[&str] = &["hbits", "bit", "bin"];
/// Constraints-wizard export filter.
pub const XDC_EXTENSIONS: &[&str] = &["xdc", "sdc"];

/// Add Sources kinds: `(label, fileset, extensions)`, in dialog order.
pub const ADD_SOURCES_KINDS: &[(&str, &str, &[&str])] = &[
    ("Design Sources", "sources_1", RTL_EXTENSIONS),
    ("Constraints", "constrs_1", CONSTRAINT_EXTENSIONS),
    ("Simulation Sources", "sim_1", SIM_EXTENSIONS),
];

/// Clamp a kind index onto [`ADD_SOURCES_KINDS`] (UI state never panics).
pub fn add_sources_kind(i: usize) -> (&'static str, &'static str, &'static [&'static str]) {
    ADD_SOURCES_KINDS
        .get(i)
        .copied()
        .unwrap_or(ADD_SOURCES_KINDS[0])
}

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: String) {
    s.console.lines.push(ConsoleLine {
        text: tcl,
        sev: MsgSev::Info,
    });
}

/// Report a picker or consume failure: console error line plus error toast,
/// so the reason is visible wherever the user looks.
fn fail(s: &mut AppState, title: &str, detail: &str) {
    s.console.lines.push(ConsoleLine {
        text: format!("ERROR: {detail}"),
        sev: MsgSev::Error,
    });
    s.toasts.push(ToastKind::Error, title, detail);
}

/// Raise the Open Project dialog (menu, toolbar, and start page share it).
pub fn open_project(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Open HDL / Project".into(),
        ..Default::default()
    }
    .filter("HDL", HDL_EXTENSIONS);
    tx.open_file(options, |s: &mut AppState, result, _| {
        apply_open_project(s, result)
    });
}

/// File stem (`demo` for `demo.prj`), falling back to the full file name.
fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

/// Parent directory display, falling back to the temp dir when absent.
fn parent_dir(path: &Path) -> String {
    path.parent()
        .map(|p| p.display().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| std::env::temp_dir().display().to_string())
}

/// Consume the Open answer: `.prj` parses through the real
/// `helion_proj::load_prj` and reseeds the project; an HDL source loads its
/// real contents into an editor tab. Cancel is silent; errors toast.
fn apply_open_project(s: &mut AppState, result: FileDialogResult) {
    let path = match result {
        Err(e) => return fail(s, "Open Project", &e.to_string()),
        Ok(None) => return,
        Ok(Some(p)) => p,
    };
    let verb = format!("open_project {}", path.display());
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "prj" {
        let prj = match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| helion_proj::load_prj(&text))
        {
            Err(e) => return fail(s, "Open Project", &format!("{}: {e}", path.display())),
            Ok(prj) => prj,
        };
        let name = prj.project_name.clone().unwrap_or_else(|| stem(&path));
        let location = prj.project_dir.clone().unwrap_or_else(|| parent_dir(&path));
        s.reseed_for_new_project(name.clone(), location, prj.part.clone(), prj.top.clone());
        s.navigate(crate::state::CenterPane::Summary);
        echo(s, verb);
        s.toasts.push(ToastKind::Success, "Project opened", &name);
    } else {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| stem(&path));
        let text = match std::fs::read_to_string(&path) {
            Err(e) => {
                return fail(
                    s,
                    "Open Project",
                    &format!("cannot read {}: {e}", path.display()),
                );
            }
            Ok(text) => text,
        };
        crate::views::editor::open_file_tab(s, name.clone(), text, path);
        s.navigate(crate::state::CenterPane::Editor);
        echo(s, verb);
        s.toasts.push(ToastKind::Success, "Opened", &name);
    }
}

/// Raise the Add Sources picker for one [`ADD_SOURCES_KINDS`] entry.
pub fn add_sources(tx: &mut Tx, kind: usize) {
    let (label, _, exts) = add_sources_kind(kind);
    let options = FileDialogOptions {
        title: format!("Add {label}"),
        ..Default::default()
    }
    .filter(label, exts);
    tx.open_file(options, move |s: &mut AppState, result, _| {
        apply_add_sources(s, result)
    });
}

/// Consume the Add Sources answer: record the path (deduped) and echo
/// `add_files`; the dialog footer echoes the full fileset verb on Add.
fn apply_add_sources(s: &mut AppState, result: FileDialogResult) {
    let path = match result {
        Err(e) => return fail(s, "Add Sources", &e.to_string()),
        Ok(None) => return,
        Ok(Some(p)) => p,
    };
    let picked = path.display().to_string();
    if !s.dialogs.add_sources.files.contains(&picked) {
        s.dialogs.add_sources.files.push(picked.clone());
    }
    echo(s, format!("add_files {{{picked}}}"));
}

/// Raise the bitstream picker for the Hardware Manager Browse button.
pub fn bitstream(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Bitstream File".into(),
        ..Default::default()
    }
    .filter("Bitstream", BITSTREAM_EXTENSIONS);
    tx.open_file(options, |s: &mut AppState, result, _| {
        apply_bitstream(s, result)
    });
}

/// Consume the bitstream answer: point PROGRAM.FILE at the picked file.
fn apply_bitstream(s: &mut AppState, result: FileDialogResult) {
    let path = match result {
        Err(e) => return fail(s, "Bitstream File", &e.to_string()),
        Ok(None) => return,
        Ok(Some(p)) => p,
    };
    echo(
        s,
        format!(
            "set_property PROGRAM.FILE {{{}}} [current_hw_device]",
            path.display()
        ),
    );
    s.program.file = Some(path);
}

/// Raise the New Project location folder picker.
pub fn project_directory(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Project Directory".into(),
        ..Default::default()
    };
    tx.open_folder(options, |s: &mut AppState, result, _| {
        apply_project_directory(s, result)
    });
}

/// Consume the location answer: fill the wizard field, clear its error.
fn apply_project_directory(s: &mut AppState, result: FileDialogResult) {
    let path = match result {
        Err(e) => return fail(s, "Project Directory", &e.to_string()),
        Ok(None) => return,
        Ok(Some(p)) => p,
    };
    s.dialogs.create.location = rune::TextEdit::new(path.display().to_string());
    s.dialogs.create.error = None;
}

/// Raise the incremental-checkpoint picker (Settings, Synthesis page).
pub fn checkpoint(tx: &mut Tx) {
    let options = FileDialogOptions {
        title: "Open Checkpoint".into(),
        ..Default::default()
    }
    .filter("Checkpoint", CHECKPOINT_EXTENSIONS);
    tx.open_file(options, |s: &mut AppState, result, _| {
        apply_checkpoint(s, result)
    });
}

/// Consume the checkpoint answer: record the path and echo `open_checkpoint`.
fn apply_checkpoint(s: &mut AppState, result: FileDialogResult) {
    let path = match result {
        Err(e) => return fail(s, "Open Checkpoint", &e.to_string()),
        Ok(None) => return,
        Ok(Some(p)) => p,
    };
    s.dialogs.settings.incr_checkpoint = path.display().to_string();
    echo(
        s,
        format!("open_checkpoint {{{}}}", s.dialogs.settings.incr_checkpoint),
    );
}

/// Raise a `tcl.pre` (`pre` = true) or `tcl.post` hook-script picker.
pub fn tcl_hook(tx: &mut Tx, pre: bool) {
    let hook = if pre { "tcl.pre" } else { "tcl.post" };
    let options = FileDialogOptions {
        title: format!("Select {hook} Script"),
        ..Default::default()
    }
    .filter("Tcl", TCL_EXTENSIONS);
    tx.open_file(options, move |s: &mut AppState, result, _| {
        apply_tcl_hook(s, result, pre)
    });
}

/// Consume a hook-script answer: record the path and echo `add_files`.
fn apply_tcl_hook(s: &mut AppState, result: FileDialogResult, pre: bool) {
    let path = match result {
        Err(e) => return fail(s, "Hook Script", &e.to_string()),
        Ok(None) => return,
        Ok(Some(p)) => p,
    };
    let picked = path.display().to_string();
    if pre {
        s.dialogs.settings.tcl_pre = picked.clone();
    } else {
        s.dialogs.settings.tcl_post = picked.clone();
    }
    echo(s, format!("add_files -fileset utils_1 {{{picked}}}"));
}

/// Raise the Constraints-wizard Save dialog; `lines` is the generated SDC the
/// answer writes to disk.
pub fn export_xdc(tx: &mut Tx, lines: Vec<String>) {
    let options = FileDialogOptions {
        title: "Export Constraints".into(),
        file_name: Some("project1.xdc".into()),
        ..Default::default()
    }
    .filter("XDC", XDC_EXTENSIONS);
    tx.save_file(options, move |s: &mut AppState, result, _| {
        apply_export_xdc(s, result, &lines)
    });
}

/// Consume the export answer: write the SDC to the chosen path for real and
/// echo `write_xdc`; cancel writes nothing.
fn apply_export_xdc(s: &mut AppState, result: FileDialogResult, lines: &[String]) {
    let path: PathBuf = match result {
        Err(e) => return fail(s, "Export Constraints", &e.to_string()),
        Ok(None) => return,
        Ok(Some(p)) => p,
    };
    let mut text = lines.join("\n");
    text.push('\n');
    if let Err(e) = std::fs::write(&path, text) {
        return fail(
            s,
            "Export Constraints",
            &format!("cannot write {}: {e}", path.display()),
        );
    }
    echo(s, format!("write_xdc {{{}}}", path.display()));
    s.toasts.push(
        ToastKind::Success,
        "Constraints exported",
        &path.display().to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_table_covers_design_constraints_and_sim() {
        assert_eq!(ADD_SOURCES_KINDS.len(), 3);
        assert_eq!(add_sources_kind(0).1, "sources_1");
        assert_eq!(add_sources_kind(1).1, "constrs_1");
        assert_eq!(add_sources_kind(2).1, "sim_1");
        assert_eq!(
            add_sources_kind(99).1,
            "sources_1",
            "out-of-range clamps, never panics"
        );
    }

    #[test]
    fn cancel_answers_leave_state_untouched() {
        let mut s = AppState::demo();
        let before = format!("{:?}", s.dialogs.add_sources.files);
        apply_add_sources(&mut s, Ok(None));
        apply_bitstream(&mut s, Ok(None));
        apply_project_directory(&mut s, Ok(None));
        apply_checkpoint(&mut s, Ok(None));
        apply_tcl_hook(&mut s, Ok(None), true);
        apply_export_xdc(&mut s, Ok(None), &["create_clock".into()]);
        apply_open_project(&mut s, Ok(None));
        assert_eq!(format!("{:?}", s.dialogs.add_sources.files), before);
        assert!(s.toasts.items.is_empty(), "cancel must not toast");
    }

    #[test]
    fn error_answers_surface_a_visible_reason() {
        use rune::FileDialogError;
        let mut s = AppState::demo();
        let err = || Err(FileDialogError("no display".into()));
        apply_open_project(&mut s, err());
        apply_add_sources(&mut s, err());
        apply_export_xdc(&mut s, err(), &[]);
        assert_eq!(s.toasts.items.len(), 3);
        assert!(s.toasts.items.iter().all(|t| t.kind == ToastKind::Error));
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.contains("no display"))
        );
    }

    #[test]
    fn prj_answer_parses_part_and_top_from_the_real_file() {
        let path =
            std::env::temp_dir().join(format!("helion-pick-{}-unit.prj", std::process::id()));
        std::fs::write(&path, "part HL10T-DSP1\ntop dsp_top\nread_sv top.v\n").expect("fixture");
        let mut s = AppState::demo();
        s.empty_project();
        apply_open_project(&mut s, Ok(Some(path.clone())));
        assert_eq!(s.project.part, "HL10T-DSP1");
        assert_eq!(s.project.top.as_deref(), Some("dsp_top"));
        assert_eq!(s.center, crate::state::CenterPane::Summary);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_source_answer_reports_instead_of_opening_a_tab() {
        let mut s = AppState::demo();
        let tabs = s.editor.tabs.clone();
        apply_open_project(&mut s, Ok(Some("/definitely/not/here.v".into())));
        assert_eq!(s.editor.tabs, tabs, "unreadable file must not open a tab");
        assert!(s.toasts.items.iter().any(|t| t.kind == ToastKind::Error));
    }

    #[test]
    fn add_sources_answer_dedupes_and_echoes() {
        let mut s = AppState::demo();
        apply_add_sources(&mut s, Ok(Some("/tmp/a.v".into())));
        apply_add_sources(&mut s, Ok(Some("/tmp/a.v".into())));
        assert_eq!(s.dialogs.add_sources.files, vec!["/tmp/a.v".to_string()]);
        assert!(s.console.lines.iter().any(|l| l.text.contains("add_files")));
    }
}
