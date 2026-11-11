//! Project file model behind the Sources column, Compile Order, the
//! properties card and the palette's file list.
//!
//! A `.prj` fills it from `read_sv` / `read_xdc` / `read_sdc` (paths resolved
//! the same way the run worker resolves them, `helion_proj::resolve_prj_path`).
//! Testbenches are classified into the simulation fileset by name. A bare
//! HDL file opened without a project, or a file added through Add Sources,
//! joins the matching fileset. Files live on disk; the demo/test project is
//! the one exception and is marked in-memory, never given a made-up path.

use std::path::{Path, PathBuf};

use crate::state::AppState;

/// Vivado fileset a file belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSet {
    Design,
    Constraints,
    Simulation,
}

impl FileSet {
    pub fn label(self) -> &'static str {
        match self {
            FileSet::Design => "sources_1",
            FileSet::Constraints => "constrs_1",
            FileSet::Simulation => "sim_1",
        }
    }
}

/// One project file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFile {
    /// Display name (file name).
    pub name: String,
    /// Disk path; `None` for the in-memory demo project.
    pub path: Option<PathBuf>,
    pub set: FileSet,
    /// First `module`/`entity` declared in the file (design + sim sources).
    pub module: Option<String>,
    /// Every module/entity the file declares (Locate and top detection).
    pub modules: Vec<String>,
    pub size: u64,
}

impl ProjectFile {
    /// Build from a disk path, classifying the fileset by extension/name and
    /// reading the declared module. Unreadable files keep size 0.
    pub fn from_path(path: &Path, set: Option<FileSet>) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let set = set.unwrap_or_else(|| classify(&name));
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let modules = if set == FileSet::Constraints {
            Vec::new()
        } else {
            declared_modules(&text)
        };
        Self {
            name,
            path: Some(path.to_path_buf()),
            set,
            module: modules.first().cloned(),
            modules,
            size,
        }
    }

    /// An in-memory file (demo project and tests only).
    pub fn in_memory(name: &str, set: FileSet, text: &str) -> Self {
        let modules = if set == FileSet::Constraints {
            Vec::new()
        } else {
            declared_modules(text)
        };
        Self {
            name: name.into(),
            path: None,
            set,
            module: modules.first().cloned(),
            modules,
            size: text.len() as u64,
        }
    }

    /// Language label for the Type field.
    pub fn lang(&self) -> &'static str {
        let n = self.name.to_ascii_lowercase();
        if n.ends_with(".sv") || n.ends_with(".svh") {
            "SystemVerilog"
        } else if n.ends_with(".v") || n.ends_with(".vh") {
            "Verilog"
        } else if n.ends_with(".vhd") || n.ends_with(".vhdl") {
            "VHDL"
        } else if n.ends_with(".xdc") {
            "XDC"
        } else if n.ends_with(".sdc") {
            "SDC"
        } else {
            "Text"
        }
    }

    /// Location text: the full disk path, or an honest in-memory marker.
    pub fn location(&self) -> String {
        match &self.path {
            Some(p) => p.display().to_string(),
            None => "In memory (not saved to disk)".into(),
        }
    }

    /// Tree label: `module (file)` for HDL, the file name otherwise.
    pub fn tree_label(&self) -> String {
        match &self.module {
            Some(m) => format!("{m} ({})", self.name),
            None => self.name.clone(),
        }
    }
}

/// Fileset for a file name: constraints by extension, testbenches by the
/// usual naming conventions, everything else is a design source.
pub fn classify(name: &str) -> FileSet {
    let n = name.to_ascii_lowercase();
    if n.ends_with(".xdc") || n.ends_with(".sdc") {
        return FileSet::Constraints;
    }
    let stem = n.rsplit_once('.').map(|(a, _)| a).unwrap_or(&n);
    if stem.ends_with("_tb")
        || stem.starts_with("tb_")
        || stem.ends_with("_test")
        || stem.contains("testbench")
        || stem == "tb"
    {
        FileSet::Simulation
    } else {
        FileSet::Design
    }
}

/// First `module NAME` (Verilog/SV) or `entity NAME is` (VHDL) declaration,
/// skipping `//` and `--` comment lines.
pub fn first_module(text: &str) -> Option<String> {
    declared_modules(text).into_iter().next()
}

/// Every `module NAME` / `entity NAME` declaration, in file order.
pub fn declared_modules(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim_start();
        if l.starts_with("//") || l.starts_with("--") {
            continue;
        }
        let lower = l.to_ascii_lowercase();
        let rest = if let Some(r) = lower.strip_prefix("module ") {
            Some(&l[l.len() - r.len()..])
        } else if let Some(r) = lower.strip_prefix("entity ") {
            Some(&l[l.len() - r.len()..])
        } else {
            None
        };
        if let Some(rest) = rest {
            let name: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// All files of the open project, in compile order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectFiles {
    pub files: Vec<ProjectFile>,
}

impl ProjectFiles {
    pub fn in_set(&self, set: FileSet) -> impl Iterator<Item = (usize, &ProjectFile)> {
        self.files
            .iter()
            .enumerate()
            .filter(move |(_, f)| f.set == set)
    }

    pub fn count(&self, set: FileSet) -> usize {
        self.in_set(set).count()
    }

    /// Index of the design top: the file declaring `top` when known, else
    /// the first design source.
    pub fn top_index(&self, top: Option<&str>) -> Option<usize> {
        if let Some(top) = top
            && let Some((i, _)) = self
                .in_set(FileSet::Design)
                .find(|(_, f)| f.modules.iter().any(|m| m == top))
        {
            return Some(i);
        }
        self.in_set(FileSet::Design).map(|(i, _)| i).next()
    }

    /// Index of the file that declares `module`, if any.
    pub fn defining(&self, module: &str) -> Option<usize> {
        self.files
            .iter()
            .position(|f| f.modules.iter().any(|m| m == module))
    }

    /// Add a file unless the same path (or in-memory name) is present;
    /// returns its index either way.
    pub fn add(&mut self, file: ProjectFile) -> usize {
        if let Some(i) = self
            .files
            .iter()
            .position(|f| f.path == file.path && f.name == file.name)
        {
            return i;
        }
        self.files.push(file);
        self.files.len() - 1
    }
}

/// The demo project's files (tests and headless scenes), mirroring the
/// demo editor buffers so sizes and modules are real for that content.
pub fn demo_files(s: &AppState) -> ProjectFiles {
    let mut files = ProjectFiles::default();
    files.add(ProjectFile::in_memory(
        "project1_demo.v",
        FileSet::Design,
        &s.editor.lines.join("\n"),
    ));
    files.add(ProjectFile::in_memory(
        "project1.xdc",
        FileSet::Constraints,
        &s.editor.xdc_lines.join("\n"),
    ));
    files.add(ProjectFile::in_memory(
        "project1_tb.v",
        FileSet::Simulation,
        &s.editor.tb_lines.join("\n"),
    ));
    files
}

/// Fill the project files from a parsed `.prj` and open the design top and
/// first constraint file in the editor.
pub fn load_from_prj(s: &mut AppState, prj_path: &Path, prj: &helion_proj::ProjectFile) {
    let mut files = ProjectFiles::default();
    for src in &prj.sources {
        let p = helion_proj::resolve_prj_path(prj_path, src);
        files.add(ProjectFile::from_path(&p, None));
    }
    for c in prj.constraint_files.iter().chain(prj.sdc.iter()) {
        // `sdc` holds inline SDC commands as well as paths: only real
        // constraint files join the fileset.
        let p = helion_proj::resolve_prj_path(prj_path, c);
        if p.is_file() {
            files.add(ProjectFile::from_path(&p, Some(FileSet::Constraints)));
        }
    }
    s.project_files = files;
    s.sources.selected = crate::views::sources::SourceFileId::Demo;
    let top = s.project.top.clone();
    if let Some(i) = s.project_files.top_index(top.as_deref()) {
        if s.project.top.is_none() {
            s.project.top = s.project_files.files[i].module.clone();
        }
        open_index(s, i);
    }
    let first_constraint = s.project_files.in_set(FileSet::Constraints).map(|(i, _)| i).next();
    if let Some(i) = first_constraint {
        open_index(s, i);
    }
    // Land on the design top, not the constraints tab opened last.
    if let Some(i) = s.project_files.top_index(top.as_deref()) {
        let name = s.project_files.files[i].name.clone();
        if let Some(tab) = s.editor.tabs.iter().position(|t| *t == name) {
            crate::views::editor::select_tab(s, tab);
        }
    }
}

/// Add one disk file to the open project (Add Sources, opening a bare HDL
/// file). Returns its index.
pub fn add_file(s: &mut AppState, path: &Path) -> usize {
    let i = s.project_files.add(ProjectFile::from_path(path, None));
    if s.project.top.is_none() && s.project_files.files[i].set == FileSet::Design {
        s.project.top = s.project_files.files[i].module.clone();
    }
    i
}

/// Add Sources commit: add each picked file to the project in the chosen
/// fileset and record it in the project file the flow reads, so the next
/// run elaborates it. A `.prj` project gets `read_sv`/`read_vhdl`/`read_xdc`
/// lines appended; a one-file HDL project is promoted to a `.prj` beside it.
/// Simulation sources stay session-side (the `.prj` has no sim fileset).
/// Returns how many files were added and the project file written, if any.
pub fn commit_add_sources(
    s: &mut AppState,
    paths: &[PathBuf],
    set: FileSet,
) -> Result<(usize, Option<PathBuf>), String> {
    let mut added = Vec::new();
    for p in paths {
        let before = s.project_files.files.len();
        let i = s.project_files.add(ProjectFile::from_path(p, Some(set)));
        if s.project_files.files.len() > before {
            added.push(i);
        }
        if s.project.top.is_none() && set == FileSet::Design {
            s.project.top = s.project_files.files[i].module.clone();
        }
    }
    if added.is_empty() || set == FileSet::Simulation {
        return Ok((added.len(), None));
    }
    let prj = match s.runs.source.clone() {
        Some(p) if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("prj")) => p,
        Some(p) if p.is_file() => promote_to_prj(s, &p)?,
        _ => return Ok((added.len(), None)),
    };
    let mut text = std::fs::read_to_string(&prj).map_err(|e| format!("{}: {e}", prj.display()))?;
    if !text.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    for i in &added {
        let f = &s.project_files.files[*i];
        let Some(path) = &f.path else { continue };
        let verb = match (f.set, f.lang()) {
            (FileSet::Constraints, "SDC") => "read_sdc",
            (FileSet::Constraints, _) => "read_xdc",
            (_, "VHDL") => "read_vhdl",
            _ => "read_sv",
        };
        text.push_str(&format!("{verb} {}\n", path.display()));
    }
    std::fs::write(&prj, text).map_err(|e| format!("{}: {e}", prj.display()))?;
    s.runs.source = Some(prj.clone());
    Ok((added.len(), Some(prj)))
}

/// Write `<stem>.prj` next to a one-file project's HDL source listing the
/// part and its current design + constraint files, and make it the run
/// input. An existing file of that name is never overwritten.
fn promote_to_prj(s: &AppState, hdl: &Path) -> Result<PathBuf, String> {
    let stem = hdl
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let dir = hdl.parent().unwrap_or(Path::new("."));
    let mut prj = dir.join(format!("{stem}.prj"));
    let mut n = 1;
    while prj.exists() {
        prj = dir.join(format!("{stem}_{n}.prj"));
        n += 1;
    }
    let mut text = format!(
        "# Helion project (created by Add Sources)\npart {}\n",
        s.project.part
    );
    if let Some(top) = &s.project.top {
        text.push_str(&format!("top {top}\n"));
    }
    text.push_str(&format!("read_sv {}\n", hdl.display()));
    std::fs::write(&prj, text).map_err(|e| format!("{}: {e}", prj.display()))?;
    Ok(prj)
}

/// Open project file `i` in an editor tab: activate an existing tab, else
/// read it from disk. In-memory files already have their buffers.
pub fn open_index(s: &mut AppState, i: usize) -> bool {
    let Some(f) = s.project_files.files.get(i).cloned() else {
        return false;
    };
    if let Some(tab) = s.editor.tabs.iter().position(|t| *t == f.name) {
        crate::views::editor::select_tab(s, tab);
        return true;
    }
    match &f.path {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(text) => {
                crate::views::editor::open_file_tab(s, f.name.clone(), text, p.clone());
                true
            }
            Err(e) => {
                s.toasts.push(
                    crate::state::ToastKind::Error,
                    "Couldn't open file",
                    &format!("{}: {e}", p.display()),
                );
                false
            }
        },
        None => {
            s.editor.tabs.push(f.name.clone());
            let tab = s.editor.tabs.len() - 1;
            crate::views::editor::select_tab(s, tab);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_by_extension_and_testbench_names() {
        assert_eq!(classify("top.sv"), FileSet::Design);
        assert_eq!(classify("pins.XDC"), FileSet::Constraints);
        assert_eq!(classify("clk.sdc"), FileSet::Constraints);
        assert_eq!(classify("counter_tb.sv"), FileSet::Simulation);
        assert_eq!(classify("tb_uart.v"), FileSet::Simulation);
        assert_eq!(classify("alu_testbench.vhd"), FileSet::Simulation);
    }

    #[test]
    fn first_module_reads_verilog_and_vhdl() {
        assert_eq!(
            first_module("// module fake\nmodule counter #(parameter W=4) (input clk);"),
            Some("counter".into())
        );
        assert_eq!(
            first_module("library ieee;\nentity blinky is\n port();"),
            Some("blinky".into())
        );
        assert_eq!(first_module("set_property PACKAGE_PIN"), None);
    }

    #[test]
    fn prj_load_resolves_sources_and_constraints_on_disk() {
        let prj = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.prj");
        let text = std::fs::read_to_string(&prj).unwrap();
        let parsed = helion_proj::load_prj(&text).unwrap();
        let mut s = crate::init();
        s.empty_project();
        s.reseed_for_new_project("counter".into(), "x".into(), parsed.part.clone(), None);
        load_from_prj(&mut s, &prj, &parsed);
        assert_eq!(s.project_files.count(FileSet::Design), 1);
        assert_eq!(s.project_files.count(FileSet::Constraints), 1);
        let top = &s.project_files.files[0];
        assert!(top.path.as_ref().unwrap().is_file());
        assert!(top.size > 0);
        assert_eq!(top.module.as_deref(), Some("counter"));
        assert_eq!(s.project.top.as_deref(), Some("counter"));
        assert!(s.editor.tabs.contains(&"counter.sv".to_string()));
        assert!(s.editor.tabs.contains(&"counter.sdc".to_string()));
        assert_eq!(s.editor.tabs[s.editor.active], "counter.sv");
    }

    #[test]
    fn add_dedupes_by_path() {
        let mut files = ProjectFiles::default();
        let p = PathBuf::from("/nope/a.sv");
        let a = files.add(ProjectFile::from_path(&p, None));
        let b = files.add(ProjectFile::from_path(&p, None));
        assert_eq!(a, b);
        assert_eq!(files.files.len(), 1);
    }
}
