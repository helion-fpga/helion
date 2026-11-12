//! IP Integrator model: the catalog (bundled `helion_ipxact::catalog()` plus
//! the project's user IP repository), the open block design (a
//! `helion_bd::BdSystem` with its emitted HDL and DRC), and the actions the
//! toolbar, dialogs and Tcl verbs drive.
//!
//! No engine logic lives here: port lists and parameters come from
//! `helion_ipxact::load_core_model` (the packaged RTL + `.helion`), connection
//! automation, address assignment, DRC and emission are `helion_bd::BdSystem`
//! / `helion_bd::emit_system`, and packaging is `helion_ipxact::package_user_ip`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use helion_bd::{BdSystem, SysDiagnostic, SystemEmit};
use helion_ipxact::MmRole;
use helion_ipxact::{CoreModel, IpCore};

use crate::state::{AppState, CenterPane};

/// Default block-design name (Vivado's `create_bd_design` default).
pub const BD_NAME: &str = "design_1";

/// Block-design tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BdTab {
    #[default]
    Diagram,
    AddressMap,
    Hdl,
}

/// The open block design: the backend system plus its last emission + DRC.
#[derive(Clone, Debug)]
pub struct BdModel {
    pub sys: BdSystem,
    pub emit: Result<SystemEmit, String>,
    pub diags: Vec<SysDiagnostic>,
}

impl BdModel {
    fn new(name: &str) -> Self {
        let mut m = Self {
            sys: BdSystem::new(name),
            emit: Err(String::new()),
            diags: Vec::new(),
        };
        m.refresh();
        m
    }

    /// Re-run DRC and emission after any edit.
    pub fn refresh(&mut self) {
        self.diags = self.sys.validate();
        self.emit = helion_bd::emit_system(&self.sys);
    }

    pub fn name(&self) -> &str {
        &self.sys.name
    }

    pub fn ok(&self) -> bool {
        self.emit.is_ok()
    }

    pub fn errors(&self) -> Vec<String> {
        self.diags.iter().filter(|d| d.error).map(|d| d.message.clone()).collect()
    }

    pub fn warnings(&self) -> Vec<String> {
        self.diags.iter().filter(|d| !d.error).map(|d| d.message.clone()).collect()
    }

    pub fn sv(&self) -> &str {
        self.emit.as_ref().map(|e| e.sv.as_str()).unwrap_or("")
    }
}

/// Customize IP dialog draft: one text field per `CONFIG.*` parameter plus
/// the instance name and (for Helion-MM slaves) the base address.
#[derive(Clone, Debug)]
pub struct CustomizeDraft {
    pub inst: String,
    pub name: rune::TextEdit,
    pub base: rune::TextEdit,
    pub params: Vec<(String, rune::TextEdit)>,
    pub error: Option<String>,
}

/// Package IP dialog draft.
#[derive(Clone, Debug)]
pub struct PackageDraft {
    /// Index into the project's design files.
    pub source: Option<usize>,
    pub top: rune::TextEdit,
    pub vendor: rune::TextEdit,
    pub library: rune::TextEdit,
    pub version: rune::TextEdit,
    /// Header analysis of the picked source (`Analyze`), or its error.
    pub preview: Option<Result<helion_ipxact::RtlModule, String>>,
    pub error: Option<String>,
}

impl Default for PackageDraft {
    fn default() -> Self {
        Self {
            source: None,
            top: rune::TextEdit::new(""),
            vendor: rune::TextEdit::new("user"),
            library: rune::TextEdit::new("ip"),
            version: rune::TextEdit::new("1.0"),
            preview: None,
            error: None,
        }
    }
}

/// Generated output products for one catalog core or block design.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedIp {
    pub path: PathBuf,
    pub bytes: usize,
}

/// IP Integrator state.
#[derive(Clone, Debug)]
pub struct IpState {
    pub catalog: Vec<IpCore>,
    /// Catalog cores with an RTL package (they can join a block design).
    pub packaged: BTreeSet<String>,
    /// Cores loaded from the user IP repository.
    pub user: BTreeSet<String>,
    pub filter: rune::TextEdit,
    pub selected: Option<String>,
    /// Selected block-design cell (instance name).
    pub selected_cell: Option<String>,
    pub bd: Option<BdModel>,
    pub tab: BdTab,
    pub generated: BTreeMap<String, GeneratedIp>,
    pub customize: Option<CustomizeDraft>,
    pub package: PackageDraft,
    /// Fallback edit target when a dialog field outlives its draft.
    pub scratch: rune::TextEdit,
}

impl Default for IpState {
    fn default() -> Self {
        let mut s = Self {
            catalog: Vec::new(),
            packaged: BTreeSet::new(),
            user: BTreeSet::new(),
            filter: rune::TextEdit::new(""),
            selected: None,
            selected_cell: None,
            bd: None,
            tab: BdTab::Diagram,
            generated: BTreeMap::new(),
            customize: None,
            package: PackageDraft::default(),
            scratch: rune::TextEdit::new(""),
        };
        s.load_catalog(None);
        s
    }
}

/// One catalog row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpRow {
    pub name: String,
    pub vlnv: String,
    pub bus: String,
    pub status: &'static str,
    pub user: bool,
}

impl IpState {
    /// Bundled catalog plus every package in `user_repo` (user cores win
    /// on a name clash so a re-packaged core replaces its old entry).
    pub fn load_catalog(&mut self, user_repo: Option<&Path>) {
        let repo = helion_ipxact::ip_repo_dir();
        let mut catalog = helion_ipxact::catalog();
        self.packaged = catalog
            .iter()
            .filter(|c| repo.join(&c.name).join(format!("{}.helion", c.name)).is_file())
            .map(|c| c.name.clone())
            .collect();
        self.user.clear();
        if let Some(dir) = user_repo {
            for m in helion_ipxact::scan_repository(dir) {
                catalog.retain(|c| c.name != m.core.name);
                self.packaged.insert(m.core.name.clone());
                self.user.insert(m.core.name.clone());
                catalog.push(m.core);
            }
        }
        self.catalog = catalog;
    }

    pub fn core(&self, name: &str) -> Option<&IpCore> {
        self.catalog.iter().find(|c| c.name == name)
    }

    pub fn in_bd(&self, name: &str) -> bool {
        self.bd
            .as_ref()
            .is_some_and(|b| b.sys.cells.iter().any(|c| c.model.core.name == name))
    }

    pub fn status(&self, name: &str) -> &'static str {
        if self.in_bd(name) {
            "In BD"
        } else if self.generated.contains_key(name) {
            "Generated"
        } else if !self.packaged.contains(name) {
            "Generator"
        } else {
            "Available"
        }
    }

    pub fn rows(&self) -> Vec<IpRow> {
        self.catalog
            .iter()
            .map(|c| IpRow {
                name: c.name.clone(),
                vlnv: c.vlnv(),
                bus: c.bus.clone(),
                status: self.status(&c.name),
                user: self.user.contains(&c.name),
            })
            .collect()
    }

    /// Rows matching the filter (name, VLNV, or bus; case-insensitive).
    pub fn filtered(&self) -> Vec<IpRow> {
        let q = self.filter.text.trim().to_ascii_lowercase();
        self.rows()
            .into_iter()
            .filter(|r| {
                q.is_empty()
                    || r.name.to_ascii_lowercase().contains(&q)
                    || r.vlnv.to_ascii_lowercase().contains(&q)
                    || r.bus.to_ascii_lowercase().contains(&q)
            })
            .collect()
    }

    pub fn cell(&self, inst: &str) -> Option<&helion_bd::SysCell> {
        self.bd.as_ref()?.sys.cell(inst)
    }
}

/// Where the project's user IP repository lives (`<project dir>/ip_repo`).
pub fn user_repo_dir(s: &AppState) -> PathBuf {
    let loc = PathBuf::from(&s.project.location);
    if !s.project.location.is_empty() && loc.is_dir() {
        loc.join("ip_repo")
    } else {
        std::env::temp_dir()
            .join("helion-ip-repo")
            .join(if s.project.name.is_empty() { "project" } else { &s.project.name })
    }
}

/// Reload the catalog (bundled + user repository).
pub fn refresh_catalog(s: &mut AppState) -> String {
    let repo = user_repo_dir(s);
    s.ip.load_catalog(Some(&repo));
    catalog_text(s)
}

fn with_bd<R>(s: &mut AppState, f: impl FnOnce(&mut BdModel) -> R) -> Result<R, String> {
    let bd = s
        .ip
        .bd
        .as_mut()
        .ok_or_else(|| "no block design open (create_bd first)".to_string())?;
    let r = f(bd);
    bd.refresh();
    Ok(r)
}

/// Create Block Design (`design_1`).
pub fn create_block_design(s: &mut AppState) -> Result<String, String> {
    if let Some(bd) = &s.ip.bd {
        return Err(format!("create_bd_design: {} is already open", bd.name()));
    }
    s.ip.bd = Some(BdModel::new(BD_NAME));
    s.navigate(CenterPane::Ip);
    Ok(format!("create_bd_design {BD_NAME}"))
}

fn find_core(s: &AppState, verb: &str, spec: &str) -> Result<IpCore, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(format!("{verb}: missing name"));
    }
    if let Ok(i) = spec.parse::<usize>() {
        return s
            .ip
            .catalog
            .get(i)
            .cloned()
            .ok_or_else(|| format!("{verb}: no row {spec}"));
    }
    s.ip.catalog
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(spec) || c.vlnv().eq_ignore_ascii_case(spec))
        .cloned()
        .ok_or_else(|| format!("{verb}: no IP {spec}"))
}

fn spec_or_selected(s: &AppState, spec: &str) -> String {
    if spec.trim().is_empty() {
        s.ip.selected.clone().unwrap_or_default()
    } else {
        spec.trim().to_string()
    }
}

/// Load a catalog core's model: user repository first, then bundled.
pub fn core_model(s: &AppState, name: &str) -> Result<CoreModel, String> {
    if s.ip.user.contains(name) {
        return helion_ipxact::load_core_model_in(&user_repo_dir(s), name);
    }
    helion_ipxact::load_core_model(name)
}

/// IP Catalog click / Tcl `select_ip`.
pub fn select_ip(s: &mut AppState, spec: &str) -> Result<String, String> {
    let core = find_core(s, "select_ip", spec)?;
    let status = s.ip.status(&core.name);
    s.ip.selected = Some(core.name.clone());
    s.navigate(CenterPane::Ip);
    Ok(format!(
        "ip {} VLNV={} BUS={} STATUS={status}",
        core.name,
        core.vlnv(),
        core.bus
    ))
}

/// Select a block-design cell (canvas / tables / Tcl `select_bd_cell`).
pub fn select_cell(s: &mut AppState, inst: &str) -> Result<String, String> {
    let core = s
        .ip
        .cell(inst)
        .map(|c| c.model.core.name.clone())
        .ok_or_else(|| format!("select_bd_cell: no cell {inst}"))?;
    s.ip.selected_cell = Some(inst.to_string());
    s.ip.selected = Some(core.clone());
    s.navigate(CenterPane::Ip);
    Ok(format!("select_bd_cell {inst} ({core})"))
}

/// Where output products are written (`<project>.gen/sources_1/...`).
pub fn gen_dir(s: &AppState) -> PathBuf {
    let loc = PathBuf::from(&s.project.location);
    let project = if s.project.name.is_empty() {
        "project".to_string()
    } else {
        s.project.name.clone()
    };
    if !s.project.location.is_empty() && loc.is_dir() {
        loc.join(format!("{project}.gen")).join("sources_1")
    } else {
        std::env::temp_dir()
            .join("helion-ip")
            .join(&project)
            .join("sources_1")
    }
}

/// Where Generate Output Products writes a catalog core's IP-XACT.
pub fn output_dir(s: &AppState, core: &str) -> PathBuf {
    gen_dir(s).join("ip").join(core)
}

/// Why Generate Output Products (catalog core) is unavailable.
pub fn generate_block_reason(s: &AppState) -> Option<String> {
    let Some(name) = &s.ip.selected else {
        return Some("Select an IP core in the catalog first.".into());
    };
    match s.ip.core(name) {
        Some(c) if c.bus.to_ascii_lowercase().contains("axi") => Some(format!(
            "{} uses AXI; Helion emits Helion-MM/ST only.",
            c.name
        )),
        Some(_) => None,
        None => Some(format!("{name} is no longer in the catalog.")),
    }
}

/// Generate Output Products for a catalog core: its IP-XACT component XML
/// (full ports/params/bus interfaces when packaged, VLNV-only otherwise).
pub fn generate_ip(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec_or_selected(s, spec);
    let core = find_core(s, "generate_ip", &spec)?;
    if core.bus.to_ascii_lowercase().contains("axi") {
        return Err(format!("generate_ip: {} bus {} is not Helion-MM/ST", core.name, core.bus));
    }
    let xml = match core_model(s, &core.name) {
        Ok(m) => helion_ipxact::component_xml(&m, &BTreeMap::new()),
        Err(_) => helion_ipxact::to_xml(&core),
    };
    let dir = output_dir(s, &core.name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("generate_ip: {}: {e}", dir.display()))?;
    let path = dir.join("component.xml");
    std::fs::write(&path, &xml).map_err(|e| format!("generate_ip: {}: {e}", path.display()))?;
    s.ip.generated.insert(
        core.name.clone(),
        GeneratedIp {
            path: path.clone(),
            bytes: xml.len(),
        },
    );
    s.ip.selected = Some(core.name.clone());
    s.navigate(CenterPane::Ip);
    Ok(format!(
        "generate_target {} VLNV={} BUS={} xml_bytes={} path={}",
        core.name,
        core.vlnv(),
        core.bus,
        xml.len(),
        path.display()
    ))
}

/// Why Add to Block Design is unavailable (`None` = enabled).
pub fn add_block_reason(s: &AppState) -> Option<String> {
    let Some(name) = &s.ip.selected else {
        return Some("Select an IP core in the catalog first.".into());
    };
    let core = s.ip.core(name)?;
    if core.bus != "Helion-MM" && core.bus != "Helion-ST" {
        return Some(format!(
            "{} uses {}; only Helion-MM/ST cores join a block design.",
            core.name, core.bus
        ));
    }
    if !s.ip.packaged.contains(name) {
        return Some(format!(
            "{name} is generator-only (no packaged RTL), so it cannot be instantiated."
        ));
    }
    None
}

/// Add to Block Design / Tcl `create_bd_cell` (creates the BD if needed).
pub fn create_bd_cell(s: &mut AppState, spec: &str) -> Result<String, String> {
    let spec = spec_or_selected(s, spec);
    let core = find_core(s, "create_bd_cell", &spec)?;
    let model = core_model(s, &core.name).map_err(|e| format!("create_bd_cell: {e}"))?;
    if s.ip.bd.is_none() {
        create_block_design(s)?;
    }
    let inst = with_bd(s, |bd| bd.sys.add_cell(model))?.map_err(|e| format!("create_bd_cell: {e}"))?;
    s.ip.selected = Some(core.name.clone());
    s.ip.selected_cell = Some(inst.clone());
    s.navigate(CenterPane::Ip);
    Ok(format!("create_bd_cell {} {inst}", core.vlnv()))
}

/// Remove the selected (or named) cell.
pub fn delete_bd_cell(s: &mut AppState, inst: &str) -> Result<String, String> {
    let inst = if inst.trim().is_empty() {
        s.ip.selected_cell.clone().unwrap_or_default()
    } else {
        inst.trim().to_string()
    };
    with_bd(s, |bd| bd.sys.remove_cell(&inst))??;
    if s.ip.selected_cell.as_deref() == Some(inst.as_str()) {
        s.ip.selected_cell = None;
    }
    Ok(format!("delete_bd_objs {inst}"))
}

/// Why Run Connection Automation is unavailable.
pub fn automation_block_reason(s: &AppState) -> Option<String> {
    let Some(bd) = &s.ip.bd else {
        return Some("Create a block design first.".into());
    };
    if bd.sys.cells.is_empty() {
        return Some("Add IP to the block design first.".into());
    }
    if bd.sys.plan_automation().cells.is_empty() {
        return Some("Every cell is already connected.".into());
    }
    None
}

/// Run Connection Automation (`apply_bd_automation`): returns the summary.
pub fn apply_automation(s: &mut AppState) -> Result<String, String> {
    let plan = with_bd(s, |bd| bd.sys.apply_automation())?;
    if plan.cells.is_empty() {
        return Err("apply_bd_automation: nothing to connect".into());
    }
    let ic = s
        .ip
        .bd
        .as_ref()
        .and_then(|b| b.emit.as_ref().ok())
        .and_then(|e| e.interconnect.as_ref())
        .map(|i| format!(" interconnect={} slaves={}", i.module, i.slaves.len()))
        .unwrap_or_default();
    Ok(format!(
        "apply_bd_automation cells={} steps={}{ic}{}",
        plan.cells.len(),
        plan.steps.len(),
        if plan.skipped.is_empty() {
            String::new()
        } else {
            format!(" skipped={}", plan.skipped.len())
        }
    ))
}

/// `assign_bd_address`.
pub fn assign_addresses(s: &mut AppState) -> Result<String, String> {
    let done = with_bd(s, |bd| bd.sys.assign_addresses())?;
    Ok(format!(
        "assign_bd_address {}",
        if done.is_empty() {
            "(all slaves already mapped)".to_string()
        } else {
            done.iter()
                .map(|(i, b)| format!("{i}={b:#010x}"))
                .collect::<Vec<_>>()
                .join(" ")
        }
    ))
}

/// `set_property CONFIG.<name> <value> [get_bd_cells <inst>]`.
pub fn set_cell_param(s: &mut AppState, inst: &str, name: &str, value: &str) -> Result<String, String> {
    let name = name.trim_start_matches("CONFIG.");
    let lit = with_bd(s, |bd| bd.sys.set_param(inst, name, value))??;
    Ok(format!("set_property CONFIG.{name} {lit} [get_bd_cells {inst}]"))
}

/// Open the Customize IP dialog for a cell.
pub fn open_customize(s: &mut AppState, inst: &str) -> Result<(), String> {
    let cell = s
        .ip
        .cell(inst)
        .cloned()
        .ok_or_else(|| format!("no cell {inst}"))?;
    let params = cell
        .model
        .params
        .iter()
        .map(|p| {
            let v = cell.params.get(&p.name).cloned().unwrap_or_else(|| p.default.clone());
            (p.name.clone(), rune::TextEdit::new(&v))
        })
        .collect();
    s.ip.customize = Some(CustomizeDraft {
        inst: inst.to_string(),
        name: rune::TextEdit::new(&cell.inst),
        base: rune::TextEdit::new(
            &cell.base.map(|b| format!("{b:#010x}")).unwrap_or_default(),
        ),
        params,
        error: None,
    });
    s.ip.selected_cell = Some(inst.to_string());
    s.dialogs.open(crate::views::dialogs::DialogId::IpCustomize);
    Ok(())
}

/// Live validation of the Customize draft (`None` = OK to apply).
pub fn customize_error(s: &AppState) -> Option<String> {
    let d = s.ip.customize.as_ref()?;
    let cell = s.ip.cell(&d.inst)?;
    for (name, edit) in &d.params {
        if let Some(p) = cell.model.param(name)
            && let Err(e) = p.validate(&edit.text)
        {
            return Some(e);
        }
    }
    let to = d.name.text.trim();
    if to.is_empty()
        || !to.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || to.starts_with(|c: char| c.is_ascii_digit())
    {
        return Some(format!("Instance name '{to}' is not an HDL identifier."));
    }
    if to != d.inst && s.ip.cell(to).is_some() {
        return Some(format!("Instance name '{to}' is already used."));
    }
    if cell.role() == Some(MmRole::Slave) && !d.base.text.trim().is_empty() {
        match helion_ipxact::parse_param_number(d.base.text.trim()) {
            Some(b) if b >= 0 && (b as u64) % helion_bd::MM_APERTURE == 0 => {}
            _ => return Some("Base address must be a 4 KiB-aligned hex value.".into()),
        }
    }
    None
}

/// Apply the Customize draft atomically (on a clone, then commit).
pub fn apply_customize(s: &mut AppState) -> Result<String, String> {
    let d = s.ip.customize.clone().ok_or("no customize dialog open")?;
    if let Some(e) = customize_error(s) {
        if let Some(c) = s.ip.customize.as_mut() {
            c.error = Some(e.clone());
        }
        return Err(e);
    }
    let bd = s.ip.bd.as_ref().ok_or("no block design open")?;
    let mut sys = bd.sys.clone();
    let mut changes = Vec::new();
    for (name, edit) in &d.params {
        let cell = sys.cell(&d.inst).ok_or("cell vanished")?;
        let p = cell.model.param(name).ok_or("param vanished")?;
        let current = cell.params.get(name).cloned().unwrap_or_else(|| p.default.clone());
        if p.validate(&current) != p.validate(&edit.text) {
            let lit = sys.set_param(&d.inst, name, &edit.text)?;
            changes.push(format!("CONFIG.{name}={lit}"));
        }
    }
    let base_txt = d.base.text.trim();
    if !base_txt.is_empty() {
        let b = helion_ipxact::parse_param_number(base_txt).ok_or("bad base address")? as u64;
        if sys.cell(&d.inst).and_then(|c| c.base) != Some(b) {
            sys.set_base(&d.inst, b)?;
            changes.push(format!("base={b:#010x}"));
        }
    }
    let to = d.name.text.trim().to_string();
    if to != d.inst {
        sys.rename(&d.inst, &to)?;
        changes.push(format!("name={to}"));
    }
    if let Some(bd) = s.ip.bd.as_mut() {
        bd.sys = sys;
        bd.refresh();
    }
    s.ip.selected_cell = Some(to.clone());
    s.ip.customize = None;
    s.dialogs.close();
    Ok(format!(
        "customize {to} {}",
        if changes.is_empty() { "(no changes)".to_string() } else { changes.join(" ") }
    ))
}

/// Why Generate Block Design is unavailable.
pub fn generate_bd_reason(s: &AppState) -> Option<String> {
    let Some(bd) = &s.ip.bd else {
        return Some("Create a block design first.".into());
    };
    if bd.sys.cells.is_empty() {
        return Some("The block design has no cells.".into());
    }
    match &bd.emit {
        Ok(_) => None,
        Err(e) => Some(format!("Fix the block design first: {e}")),
    }
}

/// Generate Block Design output products: the emitted top (`<bd>.sv`, with
/// the interconnect), one `component.xml` per cell (customized parameter
/// values), and the core RTL; adds the top and core files to the project.
pub fn generate_bd(s: &mut AppState) -> Result<String, String> {
    if let Some(r) = generate_bd_reason(s) {
        return Err(format!("generate_target: {r}"));
    }
    let bd = s.ip.bd.as_ref().expect("checked");
    let emit = bd.emit.clone().expect("checked");
    let sys = bd.sys.clone();
    let dir = gen_dir(s).join("bd").join(&sys.name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let top = dir.join(format!("{}.sv", sys.name));
    std::fs::write(&top, &emit.sv).map_err(|e| format!("{}: {e}", top.display()))?;
    for c in &sys.cells {
        let ip_dir = dir.join("ip").join(&c.inst);
        std::fs::create_dir_all(&ip_dir).map_err(|e| format!("{}: {e}", ip_dir.display()))?;
        std::fs::write(
            ip_dir.join("component.xml"),
            helion_ipxact::component_xml(&c.model, &c.params),
        )
        .map_err(|e| format!("{}: {e}", ip_dir.display()))?;
    }
    let mut files = vec![top.clone()];
    files.extend(emit.sources.iter().cloned());
    let (n, prj) = crate::project::commit_add_sources(s, &files, crate::project::FileSet::Design)?;
    s.ip.generated.insert(
        sys.name.clone(),
        GeneratedIp {
            path: top.clone(),
            bytes: emit.sv.len(),
        },
    );
    Ok(format!(
        "generate_target {} top={} cells={} added={n}{}",
        sys.name,
        top.display(),
        sys.cells.len(),
        prj.map(|p| format!(" prj={}", p.display())).unwrap_or_default()
    ))
}

/// Analyze the Package IP source (header + inferred buses).
pub fn analyze_package_source(s: &mut AppState) {
    let pk = &s.ip.package;
    let Some(path) = pk
        .source
        .and_then(|i| s.project_files.files.get(i))
        .and_then(|f| f.path.clone())
    else {
        s.ip.package.preview = Some(Err("Pick a design source file first.".into()));
        return;
    };
    let top = pk.top.text.trim().to_string();
    let r = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}", path.display()))
        .and_then(|t| {
            helion_ipxact::parse_rtl_module(&t, if top.is_empty() { None } else { Some(&top) })
        });
    if let Ok(m) = &r
        && top.is_empty()
    {
        s.ip.package.top = rune::TextEdit::new(&m.name);
    }
    s.ip.package.preview = Some(r);
}

/// Package IP: write the module into the user repository and reload the
/// catalog so it appears (and can join a block design).
pub fn package_ip(s: &mut AppState) -> Result<String, String> {
    let pk = s.ip.package.clone();
    let path = pk
        .source
        .and_then(|i| s.project_files.files.get(i))
        .and_then(|f| f.path.clone())
        .ok_or("Pick a design source file on disk first.")?;
    let top = pk.top.text.trim().to_string();
    if top.is_empty() {
        return Err("Top module is required (Analyze fills it in).".into());
    }
    let repo = user_repo_dir(s);
    let model = helion_ipxact::package_user_ip(
        &helion_ipxact::PackageRequest {
            source: path,
            top,
            vendor: pk.vendor.text.trim().to_string(),
            library: pk.library.text.trim().to_string(),
            version: pk.version.text.trim().to_string(),
        },
        &repo,
    )?;
    refresh_catalog(s);
    s.ip.selected = Some(model.core.name.clone());
    s.ip.package = PackageDraft::default();
    Ok(format!(
        "ipx::package_project {} bus={} params={} repo={}",
        model.core.vlnv(),
        model.core.bus,
        model.params.len(),
        repo.display()
    ))
}

/// Tcl `ip_catalog` text.
pub fn catalog_text(s: &AppState) -> String {
    let rows = s.ip.rows();
    let mut out = format!("ip_catalog n={}", rows.len());
    for r in &rows {
        out.push_str(&format!(
            "\nNAME={} VLNV={} BUS={} STATUS={}{}",
            r.name,
            r.vlnv,
            r.bus,
            r.status,
            if r.user { " REPO=user" } else { "" }
        ));
    }
    out
}

/// One generated-HDL instance row (from the emitted structure).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HdlRow {
    pub instance: String,
    pub module: String,
    pub bus: String,
    pub params: String,
}

/// Instances the emitted top contains (cells plus the interconnect).
pub fn hdl_rows(s: &AppState) -> Vec<HdlRow> {
    let Some(bd) = &s.ip.bd else { return Vec::new() };
    let Ok(emit) = &bd.emit else { return Vec::new() };
    let mut rows: Vec<HdlRow> = bd
        .sys
        .cells
        .iter()
        .map(|c| HdlRow {
            instance: c.inst.clone(),
            module: c.model.module.name.clone(),
            bus: c.model.core.bus.clone(),
            params: c
                .model
                .validate_overrides(&c.params)
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
        })
        .collect();
    if let Some(ic) = &emit.interconnect {
        rows.push(HdlRow {
            instance: ic.inst.clone(),
            module: ic.module.clone(),
            bus: "Helion-MM".into(),
            params: format!("{} slave(s)", ic.slaves.len()),
        });
    }
    rows
}

/// Tcl `bd_hdl` text.
pub fn hdl_text(s: &AppState) -> String {
    let rows = hdl_rows(s);
    let mut out = format!("bd_hdl n={}", rows.len());
    for r in &rows {
        out.push_str(&format!(
            "\nINSTANCE={} MODULE={} BUS={} PARAMS={}",
            r.instance, r.module, r.bus, r.params
        ));
    }
    out
}

/// Ensure a block design exists (Tcl `bd_drawing`/`bd_hdl`).
pub fn ensure_bd(s: &mut AppState) {
    if s.ip.bd.is_none() {
        let _ = create_block_design(s);
    }
    s.navigate(CenterPane::Ip);
}

/// Tcl `validate_bd_design`.
pub fn validate_text(s: &AppState) -> Result<String, String> {
    let bd = s.ip.bd.as_ref().ok_or("no block design open")?;
    let mut out = format!(
        "validate_bd_design {} errors={} warnings={}",
        bd.name(),
        bd.errors().len(),
        bd.warnings().len()
    );
    for d in &bd.diags {
        out.push_str(&format!(
            "\n{} {}",
            if d.error { "ERROR:" } else { "WARNING:" },
            d.message
        ));
    }
    Ok(out)
}
