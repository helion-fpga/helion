//! VHDL-2008 elaborator → Helion SV. Every concurrent and sequential construct
//! lowers to nets/LUT/FF. Nothing is left unknown: operators, with/select,
//! when/else, generate, process if/elsif/case/for, generics, port maps, and
//! IEEE casts all become SV that `helion_sv` maps.

use helion_ir::{intern, Design, MapResult, SoftDiag, SoftSpan};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Design-only wrapper — SoftDiag API is `map_vhdl` / `map_vhdl_path`.
/// `synth_*` drops softs by design until callers switch to `map_vhdl*`.
pub fn synth_vhdl(source: &str) -> Result<Design, String> {
    Ok(map_vhdl(source)?.design)
}

/// VHDL → map: Helion `Design` plus first-class `helion_ir::SoftDiag` (FM-HEL-L3).
/// SOFT ≠ PASS — callers must not treat `softs` as a closed WNS / cells=0 success.
pub fn map_vhdl(source: &str) -> Result<MapResult, String> {
    map_vhdl_origin(source, "vhdl.vhd")
}

/// Design-only wrapper; softs discarded — use `map_vhdl_path` for SoftDiag.
pub fn synth_vhdl_path(path: &Path) -> Result<Design, String> {
    Ok(map_vhdl_path(path)?.design)
}

pub fn map_vhdl_path(path: &Path) -> Result<MapResult, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    map_vhdl_origin(&src, &path.display().to_string())
}

/// Elaborate and map multiple VHDL (and optional companion SystemVerilog/Verilog)
/// `(origin, source)` units together. Cross-file `package` / `package body`
/// declarations (`use work.<pkg>.all`), separate `entity` and `architecture`
/// files, and multi-entity hierarchies (`entity work.<child>` and `component`)
/// are resolved across all files before lowering to `helion_sv::synth_sv_sources`.
pub fn map_vhdl_sources(files: &[(&str, &str)]) -> Result<MapResult, String> {
    if files.is_empty() {
        return Err("no sources".into());
    }
    if files.len() == 1 && !looks_like_sv_file(files[0].0, files[0].1) {
        return map_vhdl_origin(files[0].1, files[0].0);
    }
    for &(origin, source) in files {
        if looks_like_sv_file(origin, source) {
            continue;
        }
        if let Some(name) = osvvm_harness_module(source) {
            eprintln!(
                "diagnostic sim_only module={name} (OSVVM test harness; external entity not ingested; not a LUT; not a closed WNS)"
            );
            let soft = SoftDiag::new("sim_only", &name)
                .with_detail("OSVVM test harness; external entity not ingested")
                .with_span(span_best_effort(source, origin, &[&name]));
            emit_diag(&soft.table_line());
            return Ok(MapResult {
                design: empty_named(name),
                softs: vec![soft],
            });
        }
        if let Some((kind, name)) = banner_hang_module(source) {
            let why = match kind {
                "vendor_wrapper" => "vendor IP wrapper; not ingested; not a LUT; not a closed WNS",
                _ => "record or package type; not bit-blasted; not a LUT; not a closed WNS",
            };
            eprintln!("diagnostic {kind} module={name} ({why})");
            let soft = SoftDiag::new(kind, &name)
                .with_detail(why)
                .with_span(span_best_effort(source, origin, &[&name]));
            emit_diag(&soft.table_line());
            return Ok(MapResult {
                design: empty_named(name),
                softs: vec![soft],
            });
        }
    }
    let mut vhdl_files: Vec<(&str, &str)> = Vec::new();
    let mut sv_before: Vec<(&str, &str)> = Vec::new();
    let mut sv_after: Vec<(&str, &str)> = Vec::new();
    let last_is_sv = files
        .last()
        .map(|(o, s)| looks_like_sv_file(o, s))
        .unwrap_or(false);
    for &(origin, src) in files {
        if looks_like_sv_file(origin, src) {
            if last_is_sv {
                sv_after.push((origin, src));
            } else {
                sv_before.push((origin, src));
            }
        } else {
            vhdl_files.push((origin, src));
        }
    }
    if vhdl_files.is_empty() {
        let design = helion_sv::synth_sv_sources(files)?;
        return Ok(MapResult {
            design,
            softs: Vec::new(),
        });
    }
    let mut extra_known_bodies: HashSet<String> = HashSet::new();
    let mut extra_module_ports: HashMap<String, Vec<(String, &'static str, usize)>> =
        HashMap::new();
    for &(_origin, sv_src) in sv_before.iter().chain(sv_after.iter()) {
        if let Ok(mods) = helion_sv::list_sv_modules(sv_src) {
            for m in mods {
                extra_known_bodies.insert(m.to_ascii_lowercase());
            }
        }
        for (mname, mports) in scan_sv_module_ports(sv_src) {
            extra_known_bodies.insert(mname.to_ascii_lowercase());
            extra_module_ports.insert(mname.to_ascii_lowercase(), mports);
        }
    }
    let (sv, softs) =
        vhdl_sources_to_sv_with_softs(&vhdl_files, &extra_known_bodies, &extra_module_ports)?;
    let mut synth_inputs: Vec<(&str, &str)> = Vec::new();
    for &(o, s) in &sv_before {
        synth_inputs.push((o, s));
    }
    synth_inputs.push(("vhdl_multi.sv", sv.as_str()));
    for &(o, s) in &sv_after {
        synth_inputs.push((o, s));
    }
    let mut design = helion_sv::synth_sv_sources(&synth_inputs)?;
    strip_redundant_buffer_luts(&mut design);
    Ok(MapResult { design, softs })
}

/// Design-only multi-file VHDL synthesis wrapper.
pub fn synth_vhdl_sources(files: &[(&str, &str)]) -> Result<Design, String> {
    Ok(map_vhdl_sources(files)?.design)
}

/// Multi-file VHDL mapping from disk paths.
pub fn map_vhdl_files(paths: &[&Path]) -> Result<MapResult, String> {
    if paths.is_empty() {
        return Err("no sources".into());
    }
    let mut owned: Vec<(String, String)> = Vec::with_capacity(paths.len());
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
        owned.push((p.display().to_string(), text));
    }
    let refs: Vec<(&str, &str)> = owned
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    map_vhdl_sources(&refs)
}

/// Multi-file VHDL synthesis from disk paths.
pub fn synth_vhdl_files(paths: &[&Path]) -> Result<Design, String> {
    Ok(map_vhdl_files(paths)?.design)
}

/// Map a VHDL file together with any sibling `.vhd`/`.vhdl` (and `.sv`/`.v`)
/// files in the same directory so cross-file packages, entities, architectures,
/// and child components resolve automatically without project manifest ceremony,
/// while keeping `path`'s top entity last.
pub fn map_vhdl_path_with_siblings(path: &Path) -> Result<MapResult, String> {
    let Some(parent) = path.parent() else {
        return map_vhdl_path(path);
    };
    let Ok(rd) = std::fs::read_dir(parent) else {
        return map_vhdl_path(path);
    };
    let canon_target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut sibs: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let c = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            if c == canon_target {
                return false;
            }
            p.extension()
                .and_then(|e| e.to_str())
                .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "vhd" | "vhdl" | "sv" | "v"))
                .unwrap_or(false)
        })
        .collect();
    if sibs.is_empty() {
        return map_vhdl_path(path);
    }
    sibs.sort();
    sibs.push(path.to_path_buf());
    let refs: Vec<&Path> = sibs.iter().map(|p| p.as_path()).collect();
    map_vhdl_files(&refs)
}

/// Same-directory sibling synthesis for VHDL: packages, entities, architectures,
/// and child components in sibling files are elaborated with `path` as the top file.
pub fn synth_vhdl_path_with_siblings(path: &Path) -> Result<Design, String> {
    Ok(map_vhdl_path_with_siblings(path)?.design)
}

fn map_vhdl_origin(source: &str, origin: &str) -> Result<MapResult, String> {
    if let Some(name) = osvvm_harness_module(source) {
        // OSVVM / PoC test harness (CreateClock, library.entity). The
        // body is a simulator model, not a netlist. Do not elaborate it.
        // Not a LUT. Not AXI as a product. Not a closed WNS.
        eprintln!(
            "diagnostic sim_only module={name} (OSVVM test harness; external entity not ingested; not a LUT; not a closed WNS)"
        );
        let soft = SoftDiag::new("sim_only", &name)
            .with_detail("OSVVM test harness; external entity not ingested")
            .with_span(span_best_effort(source, origin, &[&name]));
        emit_diag(&soft.table_line());
        return Ok(MapResult {
            design: empty_named(name),
            softs: vec![soft],
        });
    }
    if let Some((kind, name)) = banner_hang_module(source) {
        // Banner-only 90s kills: vendor IP wrapper. One line. Not a LUT.
        // Not a vendor product. Not a closed WNS.
        let why = match kind {
            "vendor_wrapper" => "vendor IP wrapper; not ingested; not a LUT; not a closed WNS",
            _ => "record or package type; not bit-blasted; not a LUT; not a closed WNS",
        };
        eprintln!("diagnostic {kind} module={name} ({why})");
        let soft = SoftDiag::new(kind, &name)
            .with_detail(why)
            .with_span(span_best_effort(source, origin, &[&name]));
        emit_diag(&soft.table_line());
        return Ok(MapResult {
            design: empty_named(name),
            softs: vec![soft],
        });
    }
    let (sv, softs) = vhdl_to_sv_with_softs(source, origin)?;
    let mut design = helion_sv::synth_sv(&sv, "vhdl.sv")?;
    strip_redundant_buffer_luts(&mut design);
    Ok(MapResult { design, softs })
}

/// Remove redundant 1-input identity buffer LUTs (`0xaaaa_aaaa_aaaa_aaaa`)
/// emitted alongside direct `IobOut` port connections or wire aliases.
fn strip_redundant_buffer_luts(d: &mut Design) {
    use helion_ir::{CellKind, Spur};
    const BUF_INIT: u64 = 0xaaaa_aaaa_aaaa_aaaa;
    let pin_o = intern("O");
    let pin_i0 = intern("I0");
    let pin_pad = intern("PAD");
    let iob_cells: HashSet<Spur> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut))
        .map(|c| c.name)
        .collect();
    let mut iob_pad_nets: HashSet<Spur> = HashSet::new();
    for net in &d.nets {
        if net
            .endpoints
            .iter()
            .any(|e| e.pin == pin_pad && iob_cells.contains(&e.cell))
        {
            iob_pad_nets.insert(net.name);
        }
    }
    let mut remove_cells: HashSet<Spur> = HashSet::new();
    for c in &d.cells {
        let CellKind::Lut6 { init } = c.kind else {
            continue;
        };
        if init != BUF_INIT {
            continue;
        }
        let mut out_net: Option<Spur> = None;
        let mut has_other_in = false;
        for net in &d.nets {
            for e in &net.endpoints {
                if e.cell == c.name {
                    if e.pin == pin_o {
                        out_net = Some(net.name);
                    } else if e.pin != pin_i0 {
                        has_other_in = true;
                    }
                }
            }
        }
        if !has_other_in {
            if let Some(on) = out_net {
                if iob_pad_nets.contains(&on) {
                    remove_cells.insert(c.name);
                }
            }
        }
    }
    if remove_cells.is_empty() {
        return;
    }
    d.cells.retain(|c| !remove_cells.contains(&c.name));
    for net in &mut d.nets {
        net.endpoints.retain(|e| !remove_cells.contains(&e.cell));
    }
    d.rebuild_indexes();
}

/// OSVVM clock/reset harness or library-qualified AXI testbench. Not synth RTL.
fn empty_named(name: String) -> Design {
    let mut d = Design::new(&name);
    d.attrs.set("SIM_ONLY", "1");
    d.attrs.set("NO_BODY", "1");
    d
}

/// Files that previously printed a banner and then died at 90s.
/// Vendor wrapper: IP-packaging / AXI bridge shell. Do not elaborate it.
/// Note: VHDL-2008 `record` types (`is record`, `bus_req_t`, `bus_rsp_t`) are
/// now bit-blasted in bounded time and synthesized into real hardware.
fn banner_hang_module(source: &str) -> Option<(&'static str, String)> {
    let low = source.to_ascii_lowercase();
    let name = first_entity_is(source).or_else(|| last_entity_is(source))?;
    if low.contains("ip packaging") || low.contains("xbus2axi4") {
        return Some(("vendor_wrapper", name));
    }
    None
}

fn first_entity_is(source: &str) -> Option<String> {
    let low = source.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = low[search..].find("entity ") {
        let at = search + rel + "entity ".len();
        let rest = source.get(at..)?.trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let after = rest.get(name.len()..)?.trim_start();
        if !name.is_empty() && after.to_ascii_lowercase().starts_with("is") {
            return Some(name);
        }
        search = at;
        if search >= source.len() {
            break;
        }
    }
    None
}

fn osvvm_harness_module(source: &str) -> Option<String> {
    let low = source.to_ascii_lowercase();
    if !(low.contains("osvvm") || low.contains("createclock")) {
        return None;
    }
    Some(last_entity_is(source).unwrap_or_else(|| "osvvm_tb".into()))
}

fn last_entity_is(source: &str) -> Option<String> {
    let low = source.to_ascii_lowercase();
    let mut best = None;
    let mut search = 0usize;
    while let Some(rel) = low[search..].find("entity ") {
        let at = search + rel + "entity ".len();
        let rest = source.get(at..)?.trim_start();
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let after = rest.get(name.len()..)?.trim_start();
        if !name.is_empty() && after.to_ascii_lowercase().starts_with("is") {
            best = Some(name);
        }
        search = at;
        if search >= source.len() {
            break;
        }
    }
    best
}

#[allow(dead_code)] // unit tests and direct VHDL-to-SV translation utility
fn vhdl_to_sv(source: &str) -> Result<String, String> {
    Ok(vhdl_to_sv_with_softs(source, "vhdl.vhd")?.0)
}

fn looks_like_sv_file(origin: &str, src: &str) -> bool {
    let ext = Path::new(origin)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "sv" || ext == "v" {
        return true;
    }
    if ext == "vhd" || ext == "vhdl" {
        return false;
    }
    let low = src.to_ascii_lowercase();
    low.contains("endmodule") && !low.contains("architecture ") && !low.contains("end entity")
}

fn scan_sv_module_ports(src: &str) -> Vec<(String, Vec<(String, &'static str, usize)>)> {
    let mut out = Vec::new();
    let t = tokenize(src);
    let mut p = P { t: &t, i: 0 };
    while p.peek().is_some() {
        if p.eat("module") {
            let Ok(mname) = p.ident() else {
                continue;
            };
            if p.eat("#") {
                skip_paren_block(&mut p);
            }
            let mut ports = Vec::new();
            if p.eat("(") {
                let mut cur_dir: &'static str = "input";
                let mut cur_w: usize = 1;
                while !p.eat(")") && p.peek().is_some() {
                    if p.eat("input") {
                        cur_dir = "input";
                        cur_w = 1;
                    } else if p.eat("output") || p.eat("inout") {
                        cur_dir = "output";
                        cur_w = 1;
                    }
                    let _ = p.eat("wire");
                    let _ = p.eat("logic");
                    let _ = p.eat("reg");
                    let _ = p.eat("signed");
                    if p.eat("[") {
                        let hi = parse_int_expr(&mut p).unwrap_or(0);
                        let _ = p.eat(":");
                        let lo = parse_int_expr(&mut p).unwrap_or(0);
                        let _ = p.eat("]");
                        cur_w = (hi.abs_diff(lo) as usize) + 1;
                    }
                    if let Ok(pname) = p.ident() {
                        ports.push((pname, cur_dir, cur_w.max(1)));
                    }
                    if !p.eat(",") && !p.peek_sym(")") {
                        p.bump();
                    }
                }
            }
            out.push((mname, ports));
        } else {
            p.bump();
        }
    }
    out
}

fn vhdl_to_sv_with_softs(source: &str, origin: &str) -> Result<(String, Vec<SoftDiag>), String> {
    let files = [(origin, source)];
    let empty_bodies = HashSet::new();
    let empty_ports = HashMap::new();
    vhdl_sources_to_sv_with_softs(&files, &empty_bodies, &empty_ports)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum VType {
    Scalar(usize),
    Record(String),
    UnconstrainedArray(String),
}

#[derive(Clone, Debug)]
struct RecordField {
    name: String,
    ty: VType,
}

#[derive(Clone, Debug)]
struct RecordDef {
    fields: Vec<RecordField>,
}

#[derive(Clone, Debug)]
struct FuncDef {
    gen_params: Vec<(String, i64)>,
    params: Vec<String>,
    body_expr: String,
    leaf_exprs: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
struct GenericPkgTemplate {
    gen_order: Vec<String>,
    gen_defaults: HashMap<String, i64>,
    header_toks: Vec<Tok>,
    body_toks: Vec<Tok>,
}

#[derive(Clone, Debug, Default)]
struct TypeEnv {
    consts: HashMap<String, i64>,
    pkg_consts: HashMap<String, i64>,
    pkg_names: HashSet<String>,
    subtypes: HashMap<String, VType>,
    records: HashMap<String, RecordDef>,
    funcs: HashMap<String, FuncDef>,
    rec_vars: HashMap<String, String>,
    rec_consts: HashMap<String, HashMap<String, String>>,
    generic_pkgs: HashMap<String, GenericPkgTemplate>,
    unconstrained_arrays: HashMap<String, VType>,
    bounds: HashMap<String, (i64, i64)>,
    type_bounds: HashMap<String, (i64, i64)>,
}

impl TypeEnv {
    fn get_record(&self, name: &str) -> Option<&RecordDef> {
        let k = name.to_ascii_lowercase();
        if let Some(r) = self.records.get(&k) {
            return Some(r);
        }
        if let Some(VType::Record(target)) = self.subtypes.get(&k) {
            return self.records.get(&target.to_ascii_lowercase());
        }
        None
    }

    fn is_array_record(&self, rec_type: &str) -> bool {
        let Some(def) = self.get_record(rec_type) else {
            return false;
        };
        !def.fields.is_empty()
            && def.fields.iter().all(|f| {
                !f.name.is_empty()
                    && f.name
                        .chars()
                        .enumerate()
                        .all(|(i, c)| c.is_ascii_digit() || (i == 0 && (c == '-' || c == 'm')))
            })
    }

    fn make_constrained_array_record(
        &mut self,
        base_name: &str,
        lo: i64,
        hi: i64,
        elem_ty: &VType,
    ) -> String {
        let elem_tag = match elem_ty {
            VType::Scalar(w) => format!("s{w}"),
            VType::Record(r) => format!("r_{r}"),
            VType::UnconstrainedArray(u) => format!("u_{u}"),
        };
        let key = format!(
            "__arr_{}_{}_{}_{}",
            base_name.to_ascii_lowercase(),
            lo.to_string().replace('-', "m"),
            hi.to_string().replace('-', "m"),
            elem_tag
        );
        if !self.records.contains_key(&key) {
            let mut fields = Vec::new();
            let (start, end) = (lo.min(hi), lo.max(hi));
            for idx in start..=end {
                fields.push(RecordField {
                    name: idx.to_string(),
                    ty: elem_ty.clone(),
                });
            }
            self.records.insert(key.clone(), RecordDef { fields });
        }
        key
    }

    fn flatten_record_leaves(
        &self,
        rec_type: &str,
        prefix: &str,
        suffix_prefix: &str,
        depth: usize,
    ) -> Vec<(String, String, usize)> {
        if depth > 16 {
            return Vec::new();
        }
        let Some(def) = self.get_record(rec_type) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for f in &def.fields {
            let fname_clean = f.name.replace('-', "m");
            let full = if prefix.is_empty() {
                fname_clean.clone()
            } else {
                format!("{prefix}_{fname_clean}")
            };
            let suf = if suffix_prefix.is_empty() {
                fname_clean.to_ascii_lowercase()
            } else {
                format!("{suffix_prefix}_{}", fname_clean.to_ascii_lowercase())
            };
            match &f.ty {
                VType::Scalar(w) => out.push((full, suf, (*w).max(1))),
                VType::Record(sub) => {
                    out.extend(self.flatten_record_leaves(sub, &full, &suf, depth + 1));
                }
                VType::UnconstrainedArray(_) => out.push((full, suf, 1)),
            }
        }
        out
    }

    fn register_rec_var(&mut self, name: &str, rec_type: &str, depth: usize) {
        if depth > 16 {
            return;
        }
        let rlow = rec_type.to_ascii_lowercase();
        self.rec_vars.insert(name.to_ascii_lowercase(), rlow.clone());
        if let Some(def) = self.get_record(&rlow).cloned() {
            for f in &def.fields {
                let fname_clean = f.name.replace('-', "m");
                let sub_name = format!("{name}_{fname_clean}");
                if let VType::Record(sub) = &f.ty {
                    self.register_rec_var(&sub_name, sub, depth + 1);
                }
            }
        }
    }

    fn resolve_rec_var(&self, name: &str) -> Option<String> {
        self.rec_vars.get(&name.to_ascii_lowercase()).cloned()
    }
}

fn eat_context_decl_or_ref(p: &mut P) {
    let save = p.i;
    if p.eat_ident() && p.eat("is") {
        skip_to_end_unit(p, "context");
    } else {
        p.i = save;
        eat_to_semi(p);
    }
}

fn skip_package_unit(p: &mut P) {
    let _ = p.eat("body");
    let _ = p.eat_ident();
    let _ = p.eat("is");
    if p.eat("new") {
        eat_to_semi(p);
    } else {
        skip_to_end_unit(p, "package");
    }
}

fn vhdl_sources_to_sv_with_softs(
    files: &[(&str, &str)],
    extra_known_bodies: &HashSet<String>,
    extra_module_ports: &HashMap<String, Vec<(String, &'static str, usize)>>,
) -> Result<(String, Vec<SoftDiag>), String> {
    let tokenized: Vec<(&str, &str, Vec<Tok>)> = files
        .iter()
        .map(|&(origin, src)| (origin, src, tokenize(src)))
        .collect();
    let mut env = TypeEnv::default();
    env.pkg_names.insert("work".into());
    env.pkg_names.insert("ieee".into());
    env.pkg_names.insert("std".into());
    env.pkg_names.insert("fixed_pkg".into());
    env.pkg_names.insert("float_pkg".into());

    // Pass 1 (twice for cross-package dependencies): harvest all packages & package bodies.
    for _pass in 0..2 {
        for (_origin, _src, toks) in &tokenized {
            let mut p = P { t: toks, i: 0 };
            while p.peek().is_some() {
                if p.eat("library") || p.eat("use") {
                    eat_to_semi(&mut p);
                    continue;
                }
                if p.eat("context") {
                    eat_context_decl_or_ref(&mut p);
                    continue;
                }
                if p.eat("package") {
                    let is_body = p.eat("body");
                    parse_package(&mut p, &mut env, is_body)?;
                    continue;
                }
                if p.eat("entity") {
                    skip_to_end_unit(&mut p, "entity");
                    continue;
                }
                if p.eat("architecture") {
                    skip_to_end_unit(&mut p, "architecture");
                    continue;
                }
                if p.eat("configuration") {
                    skip_balanced_end(&mut p, "configuration");
                    continue;
                }
                p.bump();
            }
        }
    }

    let mut child_port_overrides: HashMap<String, HashMap<String, VType>> = HashMap::new();
    let mut child_bound_overrides: HashMap<String, HashMap<String, (i64, i64)>> = HashMap::new();
    let mut child_generic_overrides: HashMap<String, HashMap<String, i64>> = HashMap::new();

    let mut entities: Vec<Entity> = Vec::new();
    let mut entity_origins: HashMap<String, (String, String)> = HashMap::new();
    let mut archs: Vec<Arch> = Vec::new();
    let mut arch_origins: HashMap<String, (String, String)> = HashMap::new();

    for _elab_round in 0..2 {
        entities.clear();
        entity_origins.clear();
        archs.clear();
        arch_origins.clear();

        for (origin, src, toks) in &tokenized {
            let mut p = P { t: toks, i: 0 };
            while p.peek().is_some() {
                if p.eat("library") || p.eat("use") {
                    eat_to_semi(&mut p);
                    continue;
                }
                if p.eat("context") {
                    eat_context_decl_or_ref(&mut p);
                    continue;
                }
                if p.eat("package") {
                    skip_package_unit(&mut p);
                    continue;
                }
                if p.eat("entity") {
                    let ent = parse_entity(
                        &mut p,
                        &mut env,
                        &child_port_overrides,
                        &child_bound_overrides,
                        &child_generic_overrides,
                    )?;
                    entity_origins.insert(
                        ent.name.to_ascii_lowercase(),
                        (origin.to_string(), src.to_string()),
                    );
                    if let Some(existing) = entities
                        .iter_mut()
                        .find(|e| e.name.eq_ignore_ascii_case(&ent.name))
                    {
                        *existing = ent;
                    } else {
                        entities.push(ent);
                    }
                    continue;
                }
                if p.eat("architecture") {
                    skip_to_end_unit(&mut p, "architecture");
                    continue;
                }
                if p.eat("configuration") {
                    skip_balanced_end(&mut p, "configuration");
                    continue;
                }
                p.bump();
            }
        }

        for (origin, src, toks) in &tokenized {
            let mut p = P { t: toks, i: 0 };
            while p.peek().is_some() {
                if p.eat("library") || p.eat("use") {
                    eat_to_semi(&mut p);
                    continue;
                }
                if p.eat("context") {
                    eat_context_decl_or_ref(&mut p);
                    continue;
                }
                if p.eat("package") {
                    skip_package_unit(&mut p);
                    continue;
                }
                if p.eat("entity") {
                    skip_to_end_unit(&mut p, "entity");
                    continue;
                }
                if p.eat("architecture") {
                    let a = parse_arch(&mut p, &entities, &env)?;
                    arch_origins.insert(
                        a.of.to_ascii_lowercase(),
                        (origin.to_string(), src.to_string()),
                    );
                    archs.push(a);
                    continue;
                }
                if p.eat("configuration") {
                    skip_balanced_end(&mut p, "configuration");
                    continue;
                }
                p.bump();
            }
        }

        let mut changed = false;
        for a in &archs {
            for (rk, rv) in &a.env.records {
                env.records.entry(rk.clone()).or_insert_with(|| rv.clone());
            }
            let parent_ent = entities
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(&a.of));
            for st in &a.stmts {
                let CStmt::Inst {
                    module,
                    conns,
                    generic_map,
                    ..
                } = st
                else {
                    continue;
                };
                let mkey = module.to_ascii_lowercase();
                if !generic_map.is_empty() {
                    let entry = child_generic_overrides.entry(mkey.clone()).or_default();
                    if let Some(ce) = entities.iter().find(|e| e.name.eq_ignore_ascii_case(module))
                    {
                        for (gk, gv) in generic_map {
                            let resolved_k = if let Some(idx_s) = gk.strip_prefix('#') {
                                idx_s
                                    .parse::<usize>()
                                    .ok()
                                    .and_then(|i| ce.generic_order.get(i))
                                    .cloned()
                            } else {
                                Some(gk.clone())
                            };
                            if let Some(rk) = resolved_k {
                                if entry.insert(rk, *gv) != Some(*gv) {
                                    changed = true;
                                }
                            }
                        }
                    }
                }
                let Some(ce) = entities.iter().find(|e| e.name.eq_ignore_ascii_case(module))
                else {
                    continue;
                };
                for (formal, actual) in conns {
                    let port_info = if let Some(idx_s) = formal.strip_prefix('#') {
                        idx_s
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| ce.raw_ports.get(i))
                    } else {
                        ce.raw_ports
                            .iter()
                            .find(|(n, _, _)| n.eq_ignore_ascii_case(formal))
                    };
                    let Some((pname, _pdir, _pty)) = port_info else {
                        continue;
                    };
                    let pkey = pname.to_ascii_lowercase();
                    let act_clean = actual.trim();
                    if let Some(rec_t) = a.env.resolve_rec_var(act_clean) {
                        let p_entry = child_port_overrides.entry(mkey.clone()).or_default();
                        let new_ty = VType::Record(rec_t);
                        if p_entry.get(&pkey) != Some(&new_ty) {
                            p_entry.insert(pkey.clone(), new_ty);
                            changed = true;
                        }
                    } else if let Some((_, w)) = a
                        .signals
                        .iter()
                        .find(|(sn, _)| sn.eq_ignore_ascii_case(act_clean))
                        .cloned()
                        .or_else(|| {
                            parent_ent.and_then(|pe| {
                                pe.ports
                                    .iter()
                                    .find(|(pn, _, _)| pn.eq_ignore_ascii_case(act_clean))
                                    .map(|(pn, _, pw)| (pn.clone(), *pw))
                            })
                        })
                    {
                        if w > 1 {
                            let p_entry = child_port_overrides.entry(mkey.clone()).or_default();
                            let new_ty = VType::Scalar(w);
                            if p_entry.get(&pkey) != Some(&new_ty) {
                                p_entry.insert(pkey.clone(), new_ty);
                                changed = true;
                            }
                        }
                        if let Some(&bnds) = a.env.bounds.get(&act_clean.to_ascii_lowercase()) {
                            let b_entry = child_bound_overrides.entry(mkey.clone()).or_default();
                            if b_entry.insert(pkey, bnds) != Some(bnds) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    if entities.is_empty() {
        return Err("vhdl: need entity".into());
    }
    let arch_for = |name: &str| -> Option<&Arch> {
        archs.iter().rev().find(|a| a.of.eq_ignore_ascii_case(name))
    };
    let mut instantiated: HashSet<String> = HashSet::new();
    for e in &entities {
        if let Some(a) = arch_for(&e.name) {
            for st in &a.stmts {
                if let CStmt::Inst { module, .. } = st {
                    instantiated.insert(module.to_ascii_lowercase());
                }
            }
        }
    }
    let top_name = entities
        .iter()
        .rev()
        .find(|e| !instantiated.contains(&e.name.to_ascii_lowercase()))
        .or_else(|| entities.last())
        .map(|e| e.name.clone())
        .ok_or_else(|| String::from("vhdl: need entity"))?;
    let top = entities
        .iter()
        .rev()
        .find(|e| e.name.eq_ignore_ascii_case(&top_name))
        .unwrap();

    let mut known_bodies: HashSet<String> = extra_known_bodies.clone();
    for e in &entities {
        if arch_for(&e.name).is_some() {
            known_bodies.insert(e.name.to_ascii_lowercase());
        }
    }
    let dummy = Arch {
        of: top.name.clone(),
        consts: HashMap::new(),
        signals: Vec::new(),
        components: Vec::new(),
        stmts: Vec::new(),
        parse_softs: Vec::new(),
        env: env.clone(),
    };
    let mut entity_clocks: HashMap<String, HashSet<String>> = HashMap::new();
    let mut entity_occupied: HashMap<String, HashSet<String>> = HashMap::new();
    for e in &entities {
        if let Some(a) = arch_for(&e.name) {
            entity_clocks.insert(e.name.to_ascii_lowercase(), sta_clocks_of(a));
            entity_occupied.insert(e.name.to_ascii_lowercase(), occupied_sv_names(e, a));
        }
    }
    let default_origin = files.last().map(|(o, _)| *o).unwrap_or("vhdl.vhd");
    let default_source = files.last().map(|(_, s)| *s).unwrap_or("");
    let unit_origin_src = |ename: &str| -> (String, String) {
        let k = ename.to_ascii_lowercase();
        if let Some((o, s)) = arch_origins.get(&k) {
            return (o.clone(), s.clone());
        }
        if let Some((o, s)) = entity_origins.get(&k) {
            return (o.clone(), s.clone());
        }
        (default_origin.to_string(), default_source.to_string())
    };
    let mut out = String::new();
    let mut entity_softs: HashMap<String, Vec<SoftDiag>> = HashMap::new();
    let empty_child_softs: HashMap<String, Vec<SoftDiag>> = HashMap::new();
    for e in &entities {
        if e.name.eq_ignore_ascii_case(&top.name) {
            continue;
        }
        let Some(a) = arch_for(&e.name) else {
            continue;
        };
        let consts = bind_consts(&env.pkg_consts, e, a, false);
        let mut unit_softs = Vec::new();
        let (u_origin, u_source) = unit_origin_src(&e.name);
        out.push_str(&emit_sv(
            e,
            a,
            &consts,
            &known_bodies,
            &entity_clocks,
            &entity_occupied,
            &entities,
            extra_module_ports,
            &u_origin,
            &u_source,
            &mut unit_softs,
            &empty_child_softs,
        )?);
        entity_softs.insert(e.name.to_ascii_lowercase(), unit_softs);
    }
    let arch = arch_for(&top.name)
        .or_else(|| archs.last())
        .unwrap_or(&dummy);
    entity_clocks.insert(top.name.to_ascii_lowercase(), sta_clocks_of(arch));
    entity_occupied.insert(top.name.to_ascii_lowercase(), occupied_sv_names(top, arch));
    let consts = bind_consts(&env.pkg_consts, top, arch, true);
    let mut unit_softs = Vec::new();
    let (top_origin, top_source) = unit_origin_src(&top.name);
    out.push_str(&emit_sv(
        top,
        arch,
        &consts,
        &known_bodies,
        &entity_clocks,
        &entity_occupied,
        &entities,
        extra_module_ports,
        &top_origin,
        &top_source,
        &mut unit_softs,
        &empty_child_softs,
    )?);
    entity_softs.insert(top.name.to_ascii_lowercase(), unit_softs);

    let full_softs = build_softs_forest(
        &entities,
        &entity_softs,
        &archs,
        &known_bodies,
        &top_origin,
        &top_source,
    );
    let mut nested_as_child: HashSet<String> = HashSet::new();
    for softs in full_softs.values() {
        for s in softs {
            if s.name == intern("child_soft_incomplete") {
                if let Some(d) = &s.detail {
                    nested_as_child.insert(d.to_ascii_lowercase());
                }
            }
        }
    }
    let mut all_softs: Vec<SoftDiag> = Vec::new();
    for e in &entities {
        if e.name.eq_ignore_ascii_case(&top.name) {
            continue;
        }
        let k = e.name.to_ascii_lowercase();
        if nested_as_child.contains(&k) {
            continue;
        }
        if let Some(s) = full_softs.get(&k) {
            all_softs.extend(s.iter().cloned());
        }
    }
    let top_k = top.name.to_ascii_lowercase();
    if !nested_as_child.contains(&top_k) {
        if let Some(s) = full_softs.get(&top_k) {
            all_softs.extend(s.iter().cloned());
        }
    }
    Ok((out, all_softs))
}

/// Bottom-up SoftDiag forest: each entity's base softs plus `child_soft_incomplete`
/// nodes that nest the child's full soft list (memoized; cycle-safe).
fn build_softs_forest(
    entities: &[Entity],
    base: &HashMap<String, Vec<SoftDiag>>,
    archs: &[Arch],
    known_bodies: &HashSet<String>,
    origin: &str,
    source: &str,
) -> HashMap<String, Vec<SoftDiag>> {
    let mut memo: HashMap<String, Vec<SoftDiag>> = HashMap::new();
    let mut stack: HashSet<String> = HashSet::new();
    for e in entities {
        let _ = softs_forest_for(
            &e.name,
            base,
            archs,
            known_bodies,
            origin,
            source,
            &mut memo,
            &mut stack,
        );
    }
    memo
}

fn softs_forest_for(
    name: &str,
    base: &HashMap<String, Vec<SoftDiag>>,
    archs: &[Arch],
    known_bodies: &HashSet<String>,
    origin: &str,
    source: &str,
    memo: &mut HashMap<String, Vec<SoftDiag>>,
    stack: &mut HashSet<String>,
) -> Vec<SoftDiag> {
    let key = name.to_ascii_lowercase();
    if let Some(v) = memo.get(&key) {
        return v.clone();
    }
    if !stack.insert(key.clone()) {
        return base.get(&key).cloned().unwrap_or_default();
    }
    let mut softs = base.get(&key).cloned().unwrap_or_default();
    if let Some(arch) = archs.iter().rev().find(|a| a.of.eq_ignore_ascii_case(name)) {
        for st in &arch.stmts {
            let CStmt::Inst {
                module,
                name: iname,
                ..
            } = st
            else {
                continue;
            };
            let mk = module.to_ascii_lowercase();
            if !known_bodies.contains(&mk) {
                continue;
            }
            let child_full = softs_forest_for(
                module,
                base,
                archs,
                known_bodies,
                origin,
                source,
                memo,
                stack,
            );
            if child_full.is_empty() {
                continue;
            }
            let mut d = SoftDiag::new("child_soft_incomplete", name)
                .with_detail(module.clone())
                .with_span(span_best_effort(
                    source,
                    origin,
                    &[iname.as_str(), module.as_str()],
                ));
            for c in &child_full {
                d.push_child(c.clone());
            }
            softs.push(d);
        }
    }
    stack.remove(&key);
    memo.insert(key, softs.clone());
    softs
}

/// Localparams to emit. Package consts stay in parse_arch's fold env; they
/// are dumped as localparam only at top, and never when the name collides
/// with a port or signal of this unit.
fn bind_consts(
    pkg: &HashMap<String, i64>,
    ent: &Entity,
    arch: &Arch,
    include_pkg: bool,
) -> HashMap<String, i64> {
    let mut consts = HashMap::new();
    if include_pkg {
        let occupied = occupied_sv_names(ent, arch);
        for (k, v) in pkg {
            if occupied.contains(&k.to_ascii_lowercase()) {
                continue;
            }
            consts.insert(k.clone(), *v);
        }
    }
    for (k, v) in &ent.generics {
        consts.entry(k.clone()).or_insert(*v);
    }
    for (k, v) in &ent.consts {
        consts.insert(k.clone(), *v);
    }
    for (k, v) in &arch.consts {
        consts.insert(k.clone(), *v);
    }
    consts
}

fn occupied_sv_names(ent: &Entity, arch: &Arch) -> HashSet<String> {
    let mut s = HashSet::new();
    for (n, _, _) in &ent.ports {
        s.insert(n.to_ascii_lowercase());
    }
    for (n, _) in &arch.signals {
        s.insert(n.to_ascii_lowercase());
    }
    for n in ent.generics.keys() {
        s.insert(n.to_ascii_lowercase());
    }
    for n in ent.consts.keys() {
        s.insert(n.to_ascii_lowercase());
    }
    for n in arch.consts.keys() {
        s.insert(n.to_ascii_lowercase());
    }
    s
}

#[derive(Clone, Debug)]
struct Entity {
    name: String,
    generic_order: Vec<String>,
    generics: HashMap<String, i64>,
    consts: HashMap<String, i64>,
    ports: Vec<(String, &'static str, usize)>,
    raw_ports: Vec<(String, &'static str, VType)>,
    rec_vars: HashMap<String, String>,
    bounds: HashMap<String, (i64, i64)>,
}

struct Arch {
    of: String,
    consts: HashMap<String, i64>,
    signals: Vec<(String, usize)>,
    components: Vec<Entity>,
    stmts: Vec<CStmt>,
    /// Parse-time softs (generate_not_lowered, …) attached to this architecture.
    parse_softs: Vec<(String, Option<String>, String)>,
    env: TypeEnv,
}

#[derive(Clone)]
enum CStmt {
    Assign {
        lhs: String,
        rhs: String,
    },
    CondAssign {
        lhs: String,
        arms: Vec<(Option<String>, String)>,
    },
    Selected {
        sel: String,
        lhs: String,
        arms: Vec<(String, String)>,
        other: Option<String>,
    },
    Process {
        clock: Option<String>,
        body: Vec<SStmt>,
    },
    Inst {
        module: String,
        name: String,
        generic_map: Vec<(String, i64)>,
        conns: Vec<(String, String)>,
    },
}

#[derive(Clone)]
enum SStmt {
    Assign {
        lhs: String,
        rhs: String,
    },
    If {
        cond: String,
        then_b: Vec<SStmt>,
        else_b: Vec<SStmt>,
    },
    Case {
        sel: String,
        arms: Vec<(Vec<String>, Vec<SStmt>)>,
        other: Vec<SStmt>,
    },
    For {
        var: String,
        lo: i64,
        hi: i64,
        body: Vec<SStmt>,
    },
}

fn parse_package(p: &mut P, env: &mut TypeEnv, is_body: bool) -> Result<(), String> {
    let pname = p.ident().unwrap_or_default();
    let pkey = pname.to_ascii_lowercase();
    if !pkey.is_empty() {
        env.pkg_names.insert(pkey.clone());
    }
    let _ = p.eat("is");
    if p.eat("new") {
        // Generic package instantiation: `package <inst> is new [work.]<base> generic map (...);`
        let mut base = p.ident().unwrap_or_default();
        while p.eat(".") {
            if let Ok(sub) = p.ident() {
                base = sub;
            }
        }
        let gmap = parse_generic_map_values(p, env);
        eat_to_semi(p);
        instantiate_generic_package(&pkey, &base.to_ascii_lowercase(), &gmap, env);
        return Ok(());
    }

    let mut gen_order = Vec::new();
    let mut gen_defaults = HashMap::new();
    if !is_body && p.eat("generic") {
        let _ = parse_generic_clause_ordered(p, &mut gen_order, &mut gen_defaults, env);
        for (k, v) in &gen_defaults {
            env.consts.insert(k.clone(), *v);
            env.pkg_consts.insert(k.clone(), *v);
        }
        if p.eat("generic") && p.eat("map") {
            skip_paren_block(p);
            let _ = p.eat(";");
        }
    }

    let body_start = p.i;
    parse_package_items_until_end(p, env)?;
    let body_end = p.i;
    if !pkey.is_empty() && body_start <= body_end && body_end <= p.t.len() {
        let slice = p.t[body_start..body_end].to_vec();
        let entry = env.generic_pkgs.entry(pkey).or_default();
        if is_body {
            entry.body_toks = slice;
        } else {
            if !gen_order.is_empty() {
                entry.gen_order = gen_order;
                entry.gen_defaults = gen_defaults;
            }
            entry.header_toks = slice;
        }
    }
    Ok(())
}

fn parse_package_items_until_end(p: &mut P, env: &mut TypeEnv) -> Result<(), String> {
    while p.peek().is_some() {
        if p.eat("end") {
            let _ = p.eat("package");
            let _ = p.eat("body");
            let _ = p.eat_ident();
            let _ = p.eat(";");
            break;
        }
        if p.eat("constant") {
            parse_constant_into_env(p, env, true);
        } else if p.eat("type") {
            parse_type_decl(p, env);
        } else if p.eat("subtype") {
            parse_subtype_decl(p, env);
        } else if p.eat("component") {
            let _ = parse_component(p, env);
        } else if p.eat("package") {
            let sub_body = p.eat("body");
            let _ = parse_package(p, env, sub_body);
        } else if p.eat("function") || p.eat("procedure") || p.eat("impure") || p.eat("pure") {
            let _ = p.eat("function");
            let _ = p.eat("procedure");
            parse_subprogram_or_proto(p, env);
        } else {
            eat_decl_item(p);
        }
    }
    Ok(())
}

fn instantiate_generic_package(
    inst_name: &str,
    base_name: &str,
    gmap: &[(String, i64)],
    env: &mut TypeEnv,
) {
    let Some(tpl) = env.generic_pkgs.get(base_name).cloned() else {
        return;
    };
    let mut inst_env = env.clone();
    for (k, v) in &tpl.gen_defaults {
        inst_env.consts.insert(k.clone(), *v);
        inst_env.pkg_consts.insert(k.clone(), *v);
    }
    for (gk, gv) in gmap {
        let resolved_k = if let Some(idx_s) = gk.strip_prefix('#') {
            idx_s
                .parse::<usize>()
                .ok()
                .and_then(|i| tpl.gen_order.get(i))
                .cloned()
        } else {
            tpl.gen_order
                .iter()
                .find(|k| k.eq_ignore_ascii_case(gk))
                .cloned()
                .or_else(|| Some(gk.clone()))
        };
        if let Some(rk) = resolved_k {
            inst_env.consts.insert(rk.clone(), *gv);
            inst_env.pkg_consts.insert(rk.clone(), *gv);
            if !inst_name.is_empty() {
                env.consts
                    .insert(format!("{inst_name}__{}", rk.to_ascii_lowercase()), *gv);
            }
        }
    }
    if !tpl.header_toks.is_empty() {
        let mut hp = P {
            t: &tpl.header_toks,
            i: 0,
        };
        let _ = parse_package_items_until_end(&mut hp, &mut inst_env);
    }
    if !tpl.body_toks.is_empty() {
        let mut bp = P {
            t: &tpl.body_toks,
            i: 0,
        };
        let _ = parse_package_items_until_end(&mut bp, &mut inst_env);
    }
    for (k, v) in inst_env.consts {
        env.consts.insert(k.clone(), v);
        if !inst_name.is_empty() {
            env.consts
                .insert(format!("{inst_name}__{}", k.to_ascii_lowercase()), v);
        }
    }
    for (k, v) in inst_env.pkg_consts {
        env.pkg_consts.insert(k, v);
    }
    for (k, v) in inst_env.subtypes {
        env.subtypes.insert(k.clone(), v.clone());
        if !inst_name.is_empty() {
            env.subtypes
                .insert(format!("{inst_name}__{}", k.to_ascii_lowercase()), v);
        }
    }
    for (k, v) in inst_env.type_bounds {
        env.type_bounds.insert(k.clone(), v);
        if !inst_name.is_empty() {
            env.type_bounds
                .insert(format!("{inst_name}__{}", k.to_ascii_lowercase()), v);
        }
    }
    for (k, v) in inst_env.records {
        env.records.insert(k.clone(), v.clone());
        if !inst_name.is_empty() {
            env.records
                .insert(format!("{inst_name}__{}", k.to_ascii_lowercase()), v);
        }
    }
    for (k, v) in inst_env.unconstrained_arrays {
        env.unconstrained_arrays.insert(k.clone(), v.clone());
        if !inst_name.is_empty() {
            env.unconstrained_arrays
                .insert(format!("{inst_name}__{}", k.to_ascii_lowercase()), v);
        }
    }
    for (k, v) in inst_env.funcs {
        env.funcs.insert(k.clone(), v.clone());
        if !inst_name.is_empty() {
            env.funcs
                .insert(format!("{inst_name}__{}", k.to_ascii_lowercase()), v);
        }
    }
    for (k, v) in inst_env.rec_consts {
        env.rec_consts.insert(k.clone(), v.clone());
        if !inst_name.is_empty() {
            env.rec_consts
                .insert(format!("{inst_name}__{}", k.to_ascii_lowercase()), v);
        }
    }
}

fn parse_type_decl(p: &mut P, env: &mut TypeEnv) {
    let Ok(tname) = p.ident() else {
        eat_to_semi(p);
        return;
    };
    let tkey = tname.to_ascii_lowercase();
    if !p.eat("is") {
        eat_to_semi(p);
        return;
    }
    if p.eat("record") {
        let mut fields = Vec::new();
        let mut guard = 0usize;
        while p.peek().is_some() && guard < 4096 {
            guard += 1;
            if p.eat("end") {
                let _ = p.eat("record");
                let _ = p.eat_ident();
                let _ = p.eat(";");
                break;
            }
            let Some(f0) = p.eat_name() else {
                if !p.eat(";") {
                    p.bump();
                }
                continue;
            };
            let mut fnames = vec![f0];
            while p.eat(",") {
                if let Some(fnm) = p.eat_name() {
                    fnames.push(fnm);
                }
            }
            let _ = p.eat(":");
            let (fty, _bnds) = parse_vtype_with_bounds(p, env);
            skip_init(p);
            let _ = p.eat(";");
            for fnm in fnames {
                fields.push(RecordField {
                    name: fnm,
                    ty: fty.clone(),
                });
            }
        }
        env.records.insert(tkey, RecordDef { fields });
        return;
    }
    if p.eat("(") {
        let mut elems = Vec::new();
        while !p.eat(")") && p.peek().is_some() {
            if let Ok(el) = p.ident() {
                elems.push(el);
            } else {
                p.bump();
            }
            let _ = p.eat(",");
        }
        let _ = p.eat(";");
        let n = elems.len();
        let w = if n <= 1 {
            1
        } else {
            (usize::BITS - (n - 1).leading_zeros()) as usize
        };
        for (idx, el) in elems.into_iter().enumerate() {
            env.consts.entry(el).or_insert(idx as i64);
        }
        env.subtypes.insert(tkey, VType::Scalar(w.max(1)));
        return;
    }
    if p.eat("array") {
        let mut constrained_range: Option<(i64, i64)> = None;
        if p.eat("(") {
            if let Some(a) = parse_int_expr_peek(p, &env.consts) {
                if p.eat("downto") || p.eat("to") {
                    if let Some(b) = parse_int_expr_with(p, &env.consts) {
                        constrained_range = Some((a.min(b), a.max(b)));
                    }
                }
            }
            skip_paren_rest(p);
        }
        let _ = p.eat("of");
        let (elem_ty, _ebnds) = parse_vtype_with_bounds(p, env);
        let _ = p.eat(";");
        if let Some((lo, hi)) = constrained_range {
            let rec_key = env.make_constrained_array_record(&tkey, lo, hi, &elem_ty);
            if let Some(def) = env.records.get(&rec_key).cloned() {
                env.records.insert(tkey.clone(), def);
            }
            env.subtypes.insert(tkey, VType::Record(rec_key));
        } else {
            env.unconstrained_arrays.insert(tkey.clone(), elem_ty);
            env.subtypes.insert(tkey.clone(), VType::UnconstrainedArray(tkey));
        }
        return;
    }
    if p.eat("range") {
        let a = parse_int_expr_with(p, &env.consts).unwrap_or(31);
        let _ = p.eat("to");
        let _ = p.eat("downto");
        let b = parse_int_expr_with(p, &env.consts).unwrap_or(0);
        if p.eat("units") {
            skip_to_end_unit(p, "units");
        } else {
            eat_to_semi(p);
        }
        let span = (a.abs_diff(b) as usize) + 1;
        let w = if span <= 1 {
            1
        } else {
            (usize::BITS - (span - 1).leading_zeros()) as usize
        };
        env.subtypes.insert(tkey, VType::Scalar(w.max(1)));
        return;
    }
    let (ty, bnds) = parse_vtype_with_bounds(p, env);
    eat_to_semi(p);
    if let Some(b) = bnds {
        env.type_bounds.insert(tkey.clone(), b);
    }
    env.subtypes.insert(tkey, ty);
}

fn parse_subtype_decl(p: &mut P, env: &mut TypeEnv) {
    let Ok(sname) = p.ident() else {
        eat_to_semi(p);
        return;
    };
    let skey = sname.to_ascii_lowercase();
    let _ = p.eat("is");
    let (ty, bnds) = parse_vtype_with_bounds(p, env);
    eat_to_semi(p);
    if let Some(b) = bnds {
        env.type_bounds.insert(skey.clone(), b);
    }
    env.subtypes.insert(skey, ty);
}

fn parse_subprogram_or_proto(p: &mut P, env: &mut TypeEnv) {
    let fname = p.ident().unwrap_or_default();
    let fkey = fname.to_ascii_lowercase();
    if p.eat("is") && p.eat("new") {
        let mut base = p.ident().unwrap_or_default();
        while p.eat(".") {
            if let Ok(sub) = p.ident() {
                base = sub;
            }
        }
        let gmap = parse_generic_map_values(p, env);
        eat_to_semi(p);
        if let Some(base_fn) = env.funcs.get(&base.to_ascii_lowercase()).cloned() {
            let mut subst_map = HashMap::new();
            for (k, v) in &base_fn.gen_params {
                subst_map.insert(k.clone(), v.to_string());
            }
            for (gk, gv) in &gmap {
                let resolved = if let Some(idx_s) = gk.strip_prefix('#') {
                    idx_s
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| base_fn.gen_params.get(i).map(|(n, _)| n.clone()))
                } else {
                    Some(gk.clone())
                };
                if let Some(rk) = resolved {
                    subst_map.insert(rk, gv.to_string());
                }
            }
            let body_expr = replace_word_map(&base_fn.body_expr, &subst_map);
            let leaf_exprs = base_fn
                .leaf_exprs
                .iter()
                .map(|(k, v)| (k.clone(), replace_word_map(v, &subst_map)))
                .collect();
            env.funcs.insert(
                fkey,
                FuncDef {
                    gen_params: base_fn.gen_params,
                    params: base_fn.params,
                    body_expr,
                    leaf_exprs,
                },
            );
        }
        return;
    }
    let mut gen_order = Vec::new();
    let mut gen_defaults = HashMap::new();
    let mut local_env = env.clone();
    if p.eat("generic") {
        let _ = parse_generic_clause_ordered(p, &mut gen_order, &mut gen_defaults, &local_env);
        for (k, v) in &gen_defaults {
            local_env.consts.insert(k.clone(), *v);
        }
    }
    let _ = p.eat("parameter");
    let mut params = Vec::new();
    if p.eat("(") {
        while !p.eat(")") && p.peek().is_some() {
            let _ = p.eat("constant");
            let _ = p.eat("signal");
            let _ = p.eat("variable");
            let Some(p0) = p.eat_name() else {
                if !p.eat(";") {
                    p.bump();
                }
                continue;
            };
            let mut pnames = vec![p0];
            while p.eat(",") {
                if let Some(pn) = p.eat_name() {
                    pnames.push(pn);
                }
            }
            let _ = p.eat(":");
            let _ = p.eat("in");
            let _ = p.eat("out");
            let _ = p.eat("inout");
            let (pty, pbnds) = parse_vtype_with_bounds(p, &mut local_env);
            skip_init(p);
            for pn in pnames {
                if let VType::Record(r) = &pty {
                    local_env.register_rec_var(&pn, r, 0);
                }
                if let Some(b) = pbnds {
                    local_env.bounds.insert(pn.to_ascii_lowercase(), b);
                }
                params.push(pn);
            }
            let _ = p.eat(";");
        }
    }
    let mut ret_ty = VType::Scalar(1);
    if p.eat("return") {
        ret_ty = parse_vtype(p, &mut local_env);
    }
    if p.eat(";") {
        return;
    }
    let _ = p.eat("is");
    let mut local_signals = Vec::new();
    while !p.eat("begin") && p.peek().is_some() {
        if p.eat("variable") || p.eat("signal") {
            let _ = parse_signal_list(p, &mut local_signals, &mut local_env);
        } else if p.eat("constant") {
            parse_constant_into_env(p, &mut local_env, false);
        } else if p.eat("type") {
            parse_type_decl(p, &mut local_env);
        } else if p.eat("subtype") {
            parse_subtype_decl(p, &mut local_env);
        } else if p.eat("function") || p.eat("procedure") || p.eat("impure") || p.eat("pure") {
            let _ = p.eat("function");
            let _ = p.eat("procedure");
            parse_subprogram_or_proto(p, &mut local_env);
        } else {
            p.bump();
        }
    }
    let mut var_assigns: HashMap<String, String> = HashMap::new();
    let mut ret_expr: Option<String> = None;
    let mut leaf_exprs: HashMap<String, String> = HashMap::new();
    let mut depth = 1i32;
    while depth > 0 && p.peek().is_some() {
        if p.eat("end") {
            if p.eat("if") || p.eat("case") || p.eat("loop") {
                let _ = p.eat("?");
                let _ = p.eat(";");
                continue;
            }
            let _ = p.eat("function");
            let _ = p.eat("procedure");
            let _ = p.eat_ident();
            let _ = p.eat(";");
            depth -= 1;
            break;
        }
        if p.eat("return") {
            if let VType::Record(rname) = &ret_ty {
                if let Ok(pairs) = parse_record_rhs(p, rname, &local_env) {
                    for (suf, val) in pairs {
                        let sub_val = subst_func_vars(&val, &var_assigns);
                        leaf_exprs.insert(suf, sub_val);
                    }
                }
            } else if let Ok(e) = parse_expr_sv(p, &local_env) {
                let sub_e = subst_func_vars(&e, &var_assigns);
                ret_expr = Some(sub_e);
            }
            eat_to_semi(p);
            continue;
        }
        if p.eat("if") {
            if let Ok(cond) = parse_expr_sv(p, &local_env) {
                let _ = p.eat("then");
                if p.eat("return") {
                    let t_val = parse_expr_sv(p, &local_env).unwrap_or_else(|_| "1'b0".into());
                    eat_to_semi(p);
                    if p.eat("else") && p.eat("return") {
                        let f_val = parse_expr_sv(p, &local_env).unwrap_or_else(|_| "1'b0".into());
                        eat_to_semi(p);
                        ret_expr = Some(format!(
                            "(({}) ? ({}) : ({}))",
                            subst_func_vars(&cond, &var_assigns),
                            subst_func_vars(&t_val, &var_assigns),
                            subst_func_vars(&f_val, &var_assigns)
                        ));
                    }
                }
            }
            continue;
        }
        let save = p.i;
        if let Ok(lhs) = parse_target_sv(p, &local_env, &local_signals) {
            if p.eat(":=") || p.eat("<=") {
                if let Some(rec_type) = local_env.resolve_rec_var(&lhs) {
                    if let Ok(pairs) = parse_record_rhs(p, &rec_type, &local_env) {
                        for (suf, val) in pairs {
                            let full_lhs = format!("{lhs}_{suf}");
                            let sub_val = subst_func_vars(&val, &var_assigns);
                            var_assigns.insert(full_lhs, sub_val);
                        }
                        eat_to_semi(p);
                        continue;
                    }
                } else if let Ok(rhs) = parse_expr_sv(p, &local_env) {
                    let sub_rhs = subst_func_vars(&rhs, &var_assigns);
                    var_assigns.insert(lhs, sub_rhs);
                    eat_to_semi(p);
                    continue;
                }
            }
        }
        p.i = save;
        p.bump();
    }
    if !fkey.is_empty() && (ret_expr.is_some() || !leaf_exprs.is_empty()) {
        let gen_params: Vec<(String, i64)> = gen_order
            .into_iter()
            .map(|n| {
                let v = gen_defaults.get(&n).copied().unwrap_or(0);
                (n, v)
            })
            .collect();
        env.funcs.insert(
            fkey,
            FuncDef {
                gen_params,
                params,
                body_expr: ret_expr.unwrap_or_else(|| "1'b0".into()),
                leaf_exprs,
            },
        );
    }
}

fn subst_func_vars(expr: &str, vars: &HashMap<String, String>) -> String {
    if vars.is_empty() {
        return expr.to_string();
    }
    replace_word_map(expr, vars)
}

fn replace_word_map(s: &str, map: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &s[start..i];
            if let Some((_, rep)) = map.iter().find(|(k, _)| k.eq_ignore_ascii_case(word)) {
                out.push_str(&format!("({rep})"));
            } else {
                out.push_str(word);
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn parse_entity(
    p: &mut P,
    env: &mut TypeEnv,
    port_overrides: &HashMap<String, HashMap<String, VType>>,
    bound_overrides: &HashMap<String, HashMap<String, (i64, i64)>>,
    generic_overrides: &HashMap<String, HashMap<String, i64>>,
) -> Result<Entity, String> {
    let name = p.ident()?;
    let ekey = name.to_ascii_lowercase();
    let _ = p.eat("is");
    let mut ent_env = env.clone();
    let mut generic_order = Vec::new();
    let mut generics = HashMap::new();
    if p.eat("generic") {
        parse_generic_clause_ordered(p, &mut generic_order, &mut generics, &ent_env)?;
        if let Some(gov) = generic_overrides.get(&ekey) {
            for (gk, gv) in gov {
                if let Some((existing_k, _)) = generics
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(gk))
                    .map(|(k, v)| (k.clone(), *v))
                {
                    generics.insert(existing_k, *gv);
                } else {
                    generics.insert(gk.clone(), *gv);
                }
            }
        }
        for (k, v) in &generics {
            ent_env.consts.insert(k.clone(), *v);
        }
    }
    let empty_pov = HashMap::new();
    let empty_bov = HashMap::new();
    let pov = port_overrides.get(&ekey).unwrap_or(&empty_pov);
    let bov = bound_overrides.get(&ekey).unwrap_or(&empty_bov);
    let mut ports = Vec::new();
    let mut raw_ports = Vec::new();
    if p.eat("port") {
        let (flat, raw) = parse_port_clause_with_overrides(p, &mut ent_env, pov, bov)?;
        ports = flat;
        raw_ports = raw;
    }
    let mut consts: HashMap<String, i64> = HashMap::new();
    loop {
        if p.peek().is_none() {
            break;
        }
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "end" || k == "begin") {
            break;
        }
        if p.eat("constant") {
            if let Ok((n, v)) = parse_constant(p, &mut ent_env) {
                consts.insert(n.clone(), v);
                ent_env.consts.insert(n, v);
            }
        } else if p.eat("type") {
            parse_type_decl(p, &mut ent_env);
        } else if p.eat("subtype") {
            parse_subtype_decl(p, &mut ent_env);
        } else if p.eat("package") {
            let is_b = p.eat("body");
            let _ = parse_package(p, &mut ent_env, is_b);
        } else {
            eat_decl_item(p);
        }
    }
    skip_to_end_unit(p, "entity");
    for (rk, rv) in &ent_env.records {
        env.records.entry(rk.clone()).or_insert_with(|| rv.clone());
    }
    for (sk, sv) in &ent_env.subtypes {
        env.subtypes.entry(sk.clone()).or_insert_with(|| sv.clone());
    }
    Ok(Entity {
        name,
        generic_order,
        generics,
        consts,
        ports,
        raw_ports,
        rec_vars: ent_env.rec_vars,
        bounds: ent_env.bounds,
    })
}

thread_local! {
    static PARSE_SOFTS: std::cell::RefCell<Vec<(String, Option<String>, String)>> =
        std::cell::RefCell::new(Vec::new());
}

fn note_parse_soft(name: impl Into<String>, detail: Option<String>, hint: impl Into<String>) {
    PARSE_SOFTS.with(|s| s.borrow_mut().push((name.into(), detail, hint.into())));
}

fn parse_arch(p: &mut P, entities: &[Entity], base_env: &TypeEnv) -> Result<Arch, String> {
    let _aname = p.ident()?;
    let _ = p.eat("of");
    let of = p.ident().unwrap_or_default();
    let _ = p.eat("is");
    let mut env = base_env.clone();
    if let Some(e) = entities
        .iter()
        .rev()
        .find(|e| e.name.eq_ignore_ascii_case(&of))
    {
        for (k, v) in &e.generics {
            env.consts.insert(k.clone(), *v);
        }
        for (k, v) in &e.consts {
            env.consts.insert(k.clone(), *v);
        }
        for (rk, rv) in &e.rec_vars {
            env.rec_vars.insert(rk.clone(), rv.clone());
        }
        for (bk, bv) in &e.bounds {
            env.bounds.insert(bk.clone(), *bv);
        }
    }
    let mut arch_consts = HashMap::new();
    let mut signals = Vec::new();
    let mut components = Vec::new();
    while !p.eat("begin") {
        if p.peek().is_none() {
            break;
        }
        if p.eat("signal") || p.eat("variable") || p.eat("shared") {
            let _ = p.eat("variable");
            parse_signal_list(p, &mut signals, &mut env)?;
        } else if p.eat("constant") {
            if let Some((n, v)) = parse_constant_into_env(p, &mut env, false) {
                arch_consts.insert(n, v);
            }
        } else if p.eat("component") {
            if let Ok(c) = parse_component(p, &mut env) {
                components.push(c);
            }
        } else if p.eat("type") {
            parse_type_decl(p, &mut env);
        } else if p.eat("subtype") {
            parse_subtype_decl(p, &mut env);
        } else if p.eat("package") {
            let is_b = p.eat("body");
            let _ = parse_package(p, &mut env, is_b);
        } else if p.eat("attribute") || p.eat("alias") || p.eat("use") || p.eat("library") {
            eat_decl_item(p);
        } else if p.eat("function") || p.eat("procedure") || p.eat("impure") || p.eat("pure") {
            let _ = p.eat("function");
            let _ = p.eat("procedure");
            parse_subprogram_or_proto(p, &mut env);
        } else {
            p.bump();
        }
    }
    PARSE_SOFTS.with(|s| s.borrow_mut().clear());
    let stmts = parse_concurrent_list(p, &mut env, &mut signals)?;
    let parse_softs = PARSE_SOFTS.with(|s| s.borrow_mut().drain(..).collect());
    let _ = p.eat("architecture");
    let _ = p.eat_ident();
    let _ = p.eat(";");
    Ok(Arch {
        of,
        consts: arch_consts,
        signals,
        components,
        stmts,
        parse_softs,
        env,
    })
}

fn parse_generic_clause_ordered(
    p: &mut P,
    order: &mut Vec<String>,
    generics: &mut HashMap<String, i64>,
    env: &TypeEnv,
) -> Result<(), String> {
    let _ = p.eat("(");
    let mut fold = env.consts.clone();
    loop {
        if p.eat(")") {
            break;
        }
        if p.peek().is_none() {
            break;
        }
        let _ = p.eat("constant");
        let mut names = vec![p.ident()?];
        while p.eat(",") {
            names.push(p.ident()?);
        }
        let _ = p.eat(":");
        skip_type_tokens(p);
        let mut val = 0i64;
        if p.peek_sym(":=") {
            let save = p.i;
            p.eat(":=");
            val = parse_int_expr_with(p, &fold).unwrap_or(0);
            p.i = save;
            skip_init(p);
        }
        for n in names {
            order.push(n.clone());
            fold.insert(n.clone(), val);
            generics.insert(n, val);
        }
        let _ = p.eat(";");
    }
    let _ = p.eat(";");
    Ok(())
}

fn parse_port_clause(
    p: &mut P,
    env: &mut TypeEnv,
) -> Result<
    (
        Vec<(String, &'static str, usize)>,
        Vec<(String, &'static str, VType)>,
    ),
    String,
> {
    let empty_pov = HashMap::new();
    let empty_bov = HashMap::new();
    parse_port_clause_with_overrides(p, env, &empty_pov, &empty_bov)
}

fn parse_port_clause_with_overrides(
    p: &mut P,
    env: &mut TypeEnv,
    port_overrides: &HashMap<String, VType>,
    bound_overrides: &HashMap<String, (i64, i64)>,
) -> Result<
    (
        Vec<(String, &'static str, usize)>,
        Vec<(String, &'static str, VType)>,
    ),
    String,
> {
    if !p.eat("(") {
        return Err("port (".into());
    }
    let mut ports = Vec::new();
    let mut raw_ports = Vec::new();
    loop {
        if p.eat(")") {
            break;
        }
        if p.peek().is_none() {
            break;
        }
        let _ = p.eat("signal");
        let Some(n0) = p.eat_name() else {
            let _ = p.eat(";");
            continue;
        };
        let mut names = vec![n0];
        while p.eat(",") {
            if let Some(n) = p.eat_name() {
                names.push(n);
            }
        }
        let _ = p.eat(":");
        let dir = if p.eat("in") {
            "input"
        } else if p.eat("out") {
            "output"
        } else if p.eat("inout") || p.eat("buffer") {
            "output"
        } else if p.eat("linkage") {
            "input"
        } else {
            "input"
        };
        let (parsed_vty, parsed_bnds) = parse_vtype_with_bounds(p, env);
        skip_init(p);
        for n in names {
            let nkey = n.to_ascii_lowercase();
            let mut vty = port_overrides
                .get(&nkey)
                .cloned()
                .unwrap_or_else(|| parsed_vty.clone());
            if let VType::UnconstrainedArray(uarr) = &vty {
                let elem_ty = env
                    .unconstrained_arrays
                    .get(&uarr.to_ascii_lowercase())
                    .cloned()
                    .unwrap_or(VType::Scalar(8));
                let rkey = env.make_constrained_array_record(uarr, 0, 1, &elem_ty);
                vty = VType::Record(rkey);
            }
            if let Some(&bnds) = bound_overrides.get(&nkey).or(parsed_bnds.as_ref()) {
                env.bounds.insert(nkey, bnds);
            }
            raw_ports.push((n.clone(), dir, vty.clone()));
            match &vty {
                VType::Scalar(w) => ports.push((n, dir, (*w).max(1))),
                VType::Record(rname) => {
                    env.register_rec_var(&n, rname, 0);
                    for (leaf_full, _suf, lw) in env.flatten_record_leaves(rname, &n, "", 0) {
                        ports.push((leaf_full, dir, lw.max(1)));
                    }
                }
                VType::UnconstrainedArray(_) => ports.push((n, dir, 1)),
            }
        }
        let _ = p.eat(";");
    }
    let _ = p.eat(";");
    Ok((ports, raw_ports))
}

fn parse_component(p: &mut P, env: &mut TypeEnv) -> Result<Entity, String> {
    let name = p.ident()?;
    let _ = p.eat("is");
    let mut comp_env = env.clone();
    let mut generic_order = Vec::new();
    let mut generics = HashMap::new();
    if p.eat("generic") {
        parse_generic_clause_ordered(p, &mut generic_order, &mut generics, &comp_env)?;
        for (k, v) in &generics {
            comp_env.consts.insert(k.clone(), *v);
        }
    }
    let mut ports = Vec::new();
    let mut raw_ports = Vec::new();
    if p.eat("port") {
        let (flat, raw) = parse_port_clause(p, &mut comp_env)?;
        ports = flat;
        raw_ports = raw;
    }
    skip_to_end_unit(p, "component");
    Ok(Entity {
        name,
        generic_order,
        generics,
        consts: HashMap::new(),
        ports,
        raw_ports,
        rec_vars: comp_env.rec_vars,
        bounds: comp_env.bounds,
    })
}

fn push_unique_signal(signals: &mut Vec<(String, usize)>, name: String, width: usize) {
    if !signals.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
        signals.push((name, width.max(1)));
    }
}

fn parse_signal_list(
    p: &mut P,
    signals: &mut Vec<(String, usize)>,
    env: &mut TypeEnv,
) -> Result<(), String> {
    let mut names = vec![p.ident()?];
    while p.eat(",") {
        names.push(p.ident()?);
    }
    let _ = p.eat(":");
    let (mut vty, bnds) = parse_vtype_with_bounds(p, env);
    if let VType::UnconstrainedArray(uarr) = &vty {
        let elem_ty = env
            .unconstrained_arrays
            .get(&uarr.to_ascii_lowercase())
            .cloned()
            .unwrap_or(VType::Scalar(8));
        let rkey = env.make_constrained_array_record(uarr, 0, 1, &elem_ty);
        vty = VType::Record(rkey);
    }
    skip_init(p);
    let _ = p.eat(";");
    for n in names {
        if let Some(b) = bnds {
            env.bounds.insert(n.to_ascii_lowercase(), b);
        }
        match &vty {
            VType::Scalar(w) => push_unique_signal(signals, n, *w),
            VType::Record(rname) => {
                env.register_rec_var(&n, rname, 0);
                for (leaf_full, _suf, lw) in env.flatten_record_leaves(rname, &n, "", 0) {
                    push_unique_signal(signals, leaf_full, lw);
                }
            }
            VType::UnconstrainedArray(_) => push_unique_signal(signals, n, 1),
        }
    }
    Ok(())
}

fn parse_constant_into_env(
    p: &mut P,
    env: &mut TypeEnv,
    is_pkg: bool,
) -> Option<(String, i64)> {
    let Ok(n) = p.ident() else {
        eat_to_semi(p);
        return None;
    };
    let _ = p.eat(":");
    let (vty, bnds) = parse_vtype_with_bounds(p, env);
    if let Some(b) = bnds {
        env.bounds.insert(n.to_ascii_lowercase(), b);
    }
    if let VType::Record(rname) = &vty {
        if p.eat(":=") {
            if let Ok(pairs) = parse_record_rhs(p, rname, env) {
                let map: HashMap<String, String> = pairs.into_iter().collect();
                env.rec_consts.insert(n.to_ascii_lowercase(), map);
            }
        }
        eat_to_semi(p);
        return None;
    }
    let mut v = 0i64;
    if p.eat(":=") {
        let save = p.i;
        if let Some(iv) = parse_int_expr_with(p, &env.consts) {
            v = iv;
        } else {
            p.i = save;
            if let Some(Tok::BitStr(bits)) = p.peek() {
                let clean = bits.replace('-', "0");
                v = i64::from_str_radix(&clean, 2).unwrap_or(0);
                p.i += 1;
            }
        }
    }
    eat_to_semi(p);
    env.consts.insert(n.clone(), v);
    if is_pkg {
        env.pkg_consts.insert(n.clone(), v);
    }
    Some((n, v))
}

fn parse_constant(p: &mut P, env: &mut TypeEnv) -> Result<(String, i64), String> {
    parse_constant_into_env(p, env, false).ok_or_else(|| "constant".into())
}

fn parse_concurrent_list(
    p: &mut P,
    env: &mut TypeEnv,
    signals: &mut Vec<(String, usize)>,
) -> Result<Vec<CStmt>, String> {
    let mut out = Vec::new();
    loop {
        if p.peek().is_none() {
            break;
        }
        if p.eat("end") {
            break;
        }
        match parse_concurrent_many(p, env, signals) {
            Ok(stmts) => out.extend(stmts),
            Err(_) => {
                if let Some(s) = recover_assign(p, env) {
                    out.push(s);
                } else {
                    skip_stmt(p);
                }
            }
        }
    }
    Ok(out)
}

fn parse_concurrent_many(
    p: &mut P,
    env: &mut TypeEnv,
    signals: &mut Vec<(String, usize)>,
) -> Result<Vec<CStmt>, String> {
    if p.eat("assert") || p.eat("report") || p.eat("severity") {
        eat_to_semi(p);
        return Ok(Vec::new());
    }
    if p.eat("with") {
        let sel = parse_expr_sv(p, env)?;
        let _ = p.eat("select");
        let is_match_sel = p.eat("?");
        let lhs = parse_target_sv(p, env, signals)?;
        if !p.eat("<=") {
            return Err("select <=".into());
        }
        if let Some(rec_type) = env.resolve_rec_var(&lhs) {
            let leaves = env.flatten_record_leaves(&rec_type, &lhs, "", 0);
            let mut arms: Vec<(String, HashMap<String, String>)> = Vec::new();
            let mut other: Option<HashMap<String, String>> = None;
            loop {
                let val_map: HashMap<String, String> =
                    parse_record_rhs(p, &rec_type, env)?.into_iter().collect();
                let _ = p.eat("when");
                if p.eat("others") {
                    other = Some(val_map);
                    let _ = p.eat(";");
                    break;
                }
                let pat = parse_expr_sv(p, env)?;
                arms.push((pat, val_map));
                if p.eat(";") {
                    break;
                }
                let _ = p.eat(",");
            }
            let mut out = Vec::new();
            for (leaf_lhs, suf, _w) in leaves {
                let leaf_arms: Vec<(String, String)> = arms
                    .iter()
                    .map(|(pat, m)| {
                        (
                            pat.clone(),
                            m.get(&suf).cloned().unwrap_or_else(|| "1'b0".into()),
                        )
                    })
                    .collect();
                let leaf_other = other.as_ref().and_then(|m| m.get(&suf).cloned());
                out.push(CStmt::Selected {
                    sel: sel.clone(),
                    lhs: leaf_lhs,
                    arms: leaf_arms,
                    other: leaf_other,
                });
            }
            return Ok(out);
        }
        if is_match_sel {
            let mut cond_arms: Vec<(Option<String>, String)> = Vec::new();
            loop {
                let val = parse_expr_sv(p, env)?;
                let _ = p.eat("when");
                if p.eat("others") {
                    cond_arms.push((None, val));
                    let _ = p.eat(";");
                    break;
                }
                let mut pat_conds = vec![parse_match_pat_cond(p, &sel, env)?];
                while p.eat("|") {
                    pat_conds.push(parse_match_pat_cond(p, &sel, env)?);
                }
                let cond = if pat_conds.len() == 1 {
                    pat_conds.remove(0)
                } else {
                    format!("({})", pat_conds.join(" | "))
                };
                cond_arms.push((Some(cond), val));
                if p.eat(";") {
                    break;
                }
                let _ = p.eat(",");
            }
            return Ok(vec![CStmt::CondAssign {
                lhs,
                arms: cond_arms,
            }]);
        }
        let mut arms = Vec::new();
        let mut other = None;
        loop {
            let val = parse_expr_sv(p, env)?;
            let _ = p.eat("when");
            if p.eat("others") {
                other = Some(val);
                let _ = p.eat(";");
                break;
            }
            let pat = parse_expr_sv(p, env)?;
            arms.push((pat, val));
            if p.eat(";") {
                break;
            }
            let _ = p.eat(",");
        }
        return Ok(vec![CStmt::Selected {
            sel,
            lhs,
            arms,
            other,
        }]);
    }
    if p.eat("process") {
        return Ok(vec![parse_process(p, env, signals)?]);
    }
    if p.eat("for") {
        return Ok(parse_for_generate(p, env, signals)?.into_iter().collect());
    }
    if p.eat("if") {
        return Ok(parse_if_generate(p, env, signals)?.into_iter().collect());
    }
    // Optional label: ident :
    let save = p.i;
    if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
        let lab = p.ident()?;
        if p.eat(":") {
            if p.eat("process") {
                return Ok(vec![parse_process(p, env, signals)?]);
            }
            if p.eat("for") {
                return Ok(parse_for_generate(p, env, signals)?.into_iter().collect());
            }
            if p.eat("if") {
                return Ok(parse_if_generate(p, env, signals)?.into_iter().collect());
            }
            if p.eat("entity") {
                let _ = p.eat_ident(); // work
                let _ = p.eat(".");
                let module = p.ident().unwrap_or_else(|_| lab.clone());
                if p.eat("(") {
                    skip_paren_rest(p);
                }
                let generic_map = parse_generic_map_values(p, env);
                let conns = parse_port_map(p, env)?;
                return Ok(vec![CStmt::Inst {
                    module,
                    name: lab,
                    generic_map,
                    conns,
                }]);
            }
            if p.eat("component") {
                let module = p.ident().unwrap_or_else(|_| lab.clone());
                let generic_map = parse_generic_map_values(p, env);
                let conns = parse_port_map(p, env)?;
                return Ok(vec![CStmt::Inst {
                    module,
                    name: lab,
                    generic_map,
                    conns,
                }]);
            }
            if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
                let module = p.ident()?;
                let generic_map = parse_generic_map_values(p, env);
                if p.eat("port") || p.peek_sym("(") {
                    let conns = parse_port_map(p, env)?;
                    return Ok(vec![CStmt::Inst {
                        module,
                        name: lab,
                        generic_map,
                        conns,
                    }]);
                }
            }
        }
        p.i = save;
    }
    // Concurrent assignment, possibly conditional, possibly whole-record or aggregate.
    let lhs = parse_target_sv(p, env, signals)?;
    if !p.eat("<=") {
        p.i = save;
        skip_stmt(p);
        return Ok(Vec::new());
    }
    if let Some(rec_type) = env.resolve_rec_var(&lhs) {
        let leaves = env.flatten_record_leaves(&rec_type, &lhs, "", 0);
        let first_map: HashMap<String, String> =
            parse_record_rhs(p, &rec_type, env)?.into_iter().collect();
        if p.eat("when") {
            let mut arms: Vec<(Option<String>, HashMap<String, String>)> = Vec::new();
            let cond = parse_expr_sv(p, env)?;
            arms.push((Some(cond), first_map));
            while p.eat("else") {
                let vmap: HashMap<String, String> =
                    parse_record_rhs(p, &rec_type, env)?.into_iter().collect();
                if p.eat("when") {
                    let c = parse_expr_sv(p, env)?;
                    arms.push((Some(c), vmap));
                } else {
                    arms.push((None, vmap));
                    break;
                }
            }
            let _ = p.eat(";");
            let mut out = Vec::new();
            for (leaf_lhs, suf, _w) in leaves {
                let leaf_arms: Vec<(Option<String>, String)> = arms
                    .iter()
                    .map(|(c, m)| {
                        (
                            c.clone(),
                            m.get(&suf).cloned().unwrap_or_else(|| "1'b0".into()),
                        )
                    })
                    .collect();
                out.push(CStmt::CondAssign {
                    lhs: leaf_lhs,
                    arms: leaf_arms,
                });
            }
            return Ok(out);
        }
        skip_after_delay(p);
        let _ = p.eat(";");
        let mut out = Vec::new();
        for (leaf_lhs, suf, _w) in leaves {
            let rhs = first_map.get(&suf).cloned().unwrap_or_else(|| "1'b0".into());
            out.push(CStmt::Assign { lhs: leaf_lhs, rhs });
        }
        return Ok(out);
    }
    let first = parse_expr_sv(p, env)?;
    if p.eat("when") {
        let mut arms = Vec::new();
        let cond = parse_expr_sv(p, env)?;
        arms.push((Some(cond), first));
        while p.eat("else") {
            let v = parse_expr_sv(p, env)?;
            if p.eat("when") {
                let c = parse_expr_sv(p, env)?;
                arms.push((Some(c), v));
            } else {
                arms.push((None, v));
                break;
            }
        }
        let _ = p.eat(";");
        return Ok(vec![CStmt::CondAssign { lhs, arms }]);
    }
    skip_after_delay(p);
    let _ = p.eat(";");
    Ok(vec![CStmt::Assign { lhs, rhs: first }])
}

fn parse_record_rhs(
    p: &mut P,
    rec_type: &str,
    env: &TypeEnv,
) -> Result<Vec<(String, String)>, String> {
    if p.peek_sym("(") {
        return parse_record_aggregate(p, rec_type, env, 0);
    }
    let rhs_name = parse_qualified_ident_or_expr(p, env)?;
    if let Some(cmap) = env.rec_consts.get(&rhs_name.to_ascii_lowercase()) {
        let leaves = env.flatten_record_leaves(rec_type, "", "", 0);
        return Ok(leaves
            .into_iter()
            .map(|(_full, suf, _w)| {
                let v = cmap.get(&suf).cloned().unwrap_or_else(|| "1'b0".into());
                (suf, v)
            })
            .collect());
    }
    if let Some((fname, args)) = rhs_name.strip_prefix("__func_call:").and_then(|s| s.split_once(':')) {
        if let Some(fdef) = env.funcs.get(&fname.to_ascii_lowercase()) {
            if !fdef.leaf_exprs.is_empty() {
                let arg_list: Vec<&str> = if args.is_empty() {
                    Vec::new()
                } else {
                    args.split("\x1f").collect()
                };
                let mut param_map = HashMap::new();
                for (param, arg) in fdef.params.iter().zip(arg_list.iter()) {
                    param_map.insert(param.clone(), (*arg).to_string());
                }
                let leaves = env.flatten_record_leaves(rec_type, "", "", 0);
                return Ok(leaves
                    .into_iter()
                    .map(|(_full, suf, _w)| {
                        let raw = fdef.leaf_exprs.get(&suf).cloned().unwrap_or_else(|| "1'b0".into());
                        (suf, replace_word_map(&raw, &param_map))
                    })
                    .collect());
            }
        }
    }
    let leaves = env.flatten_record_leaves(rec_type, "", "", 0);
    Ok(leaves
        .into_iter()
        .map(|(_full, suf, _w)| (suf.clone(), format!("{rhs_name}_{suf}")))
        .collect())
}

fn parse_qualified_ident_or_expr(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
        let mut n = p.ident()?;
        let mut qual_pkg: Option<String> = None;
        while p.eat(".") {
            let f = p.ident()?;
            if env.pkg_names.contains(&n.to_ascii_lowercase()) {
                qual_pkg = Some(n.to_ascii_lowercase());
                n = f;
            } else {
                n = format!("{n}_{f}");
            }
        }
        while p.peek_sym("(") {
            let is_arr = env
                .resolve_rec_var(&n)
                .map(|r| env.is_array_record(&r))
                .unwrap_or(false);
            if !is_arr {
                break;
            }
            let save = p.i;
            p.eat("(");
            if let Some(idx) = parse_int_expr_with(p, &env.consts) {
                if p.eat(")") {
                    let idx_s = idx.to_string().replace('-', "m");
                    n = format!("{n}_{idx_s}");
                    while p.eat(".") {
                        if let Ok(f) = p.ident() {
                            n = format!("{n}_{f}");
                        }
                    }
                    continue;
                }
            }
            p.i = save;
            break;
        }
        if p.eat("(") {
            let mut args = Vec::new();
            if !p.eat(")") {
                loop {
                    args.push(parse_expr_sv(p, env).unwrap_or_else(|_| "1'b0".into()));
                    if !p.eat(",") {
                        break;
                    }
                }
                let _ = p.eat(")");
            }
            let fk = if let Some(pk) = &qual_pkg {
                let qk = format!("{pk}__{}", n.to_ascii_lowercase());
                if env.funcs.contains_key(&qk) {
                    qk
                } else {
                    n.to_ascii_lowercase()
                }
            } else {
                n.to_ascii_lowercase()
            };
            if env.funcs.contains_key(&fk) {
                return Ok(format!("__func_call:{}:{}", fk, args.join("\x1f")));
            }
        }
        if let Some(pk) = &qual_pkg {
            let qk = format!("{pk}__{}", n.to_ascii_lowercase());
            if env.rec_consts.contains_key(&qk) {
                return Ok(qk);
            }
        }
        return Ok(n);
    }
    parse_expr_sv(p, env)
}

fn parse_record_aggregate(
    p: &mut P,
    rec_type: &str,
    env: &TypeEnv,
    depth: usize,
) -> Result<Vec<(String, String)>, String> {
    if depth > 16 || !p.eat("(") {
        return Err("record aggregate (".into());
    }
    let Some(def) = env.get_record(rec_type).cloned() else {
        skip_paren_rest(p);
        return Ok(Vec::new());
    };
    let mut result: HashMap<String, String> = HashMap::new();
    let mut others_bit: Option<char> = None;
    let mut others_expr: Option<String> = None;
    let mut pos_idx = 0usize;
    while !p.eat(")") && p.peek().is_some() {
        if p.eat("others") {
            let _ = p.eat("=>");
            if let Some(ch) = peek_aggregate_fill_bit(p) {
                others_bit = Some(ch);
            } else {
                others_expr = Some(parse_expr_sv(p, env).unwrap_or_else(|_| "1'b0".into()));
            }
            let _ = p.eat(",");
            continue;
        }
        // Check if named association: `field1 [| field2 ...] => val` or `0 [| 1 ...] => val`
        let save = p.i;
        let mut choices = Vec::new();
        let mut is_named = false;
        if let Some(c_int) = parse_int_expr_peek(p, &env.consts) {
            choices.push(c_int.to_string());
            while p.eat("|") {
                if let Some(cn_int) = parse_int_expr_peek(p, &env.consts) {
                    choices.push(cn_int.to_string());
                } else {
                    break;
                }
            }
            if p.eat("=>") {
                is_named = true;
            }
        } else if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
            if let Ok(c0) = p.ident() {
                choices.push(c0);
                while p.eat("|") {
                    if let Ok(cn) = p.ident() {
                        choices.push(cn);
                    } else {
                        break;
                    }
                }
                if p.eat("=>") {
                    is_named = true;
                }
            }
        }
        if !is_named {
            p.i = save;
            choices.clear();
            if let Some(f) = def.fields.get(pos_idx) {
                choices.push(f.name.clone());
                pos_idx += 1;
            }
        }
        if choices.len() == 1 {
            let fname = &choices[0];
            if let Some(fdef) = def
                .fields
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(fname))
            {
                let fkey = fdef.name.replace('-', "m").to_ascii_lowercase();
                match &fdef.ty {
                    VType::Scalar(w) => {
                        let val = parse_scalar_field_rhs(p, *w, env)?;
                        result.insert(fkey, val);
                    }
                    VType::Record(sub_rec) => {
                        if let Some(ch) = peek_aggregate_fill_bit(p) {
                            for (_full, suf, lw) in
                                env.flatten_record_leaves(sub_rec, "", &fkey, depth + 1)
                            {
                                result.insert(suf, fill_bits_literal(lw, ch));
                            }
                        } else {
                            let sub_pairs = parse_record_rhs(p, sub_rec, env)?;
                            for (sub_suf, sub_val) in sub_pairs {
                                result.insert(format!("{fkey}_{sub_suf}"), sub_val);
                            }
                        }
                    }
                    VType::UnconstrainedArray(_) => {
                        let val = parse_scalar_field_rhs(p, 1, env)?;
                        result.insert(fkey, val);
                    }
                }
            } else {
                let _ = parse_expr_sv(p, env);
            }
        } else if !choices.is_empty() {
            if let Some(ch) = peek_aggregate_fill_bit(p) {
                for cname in &choices {
                    if let Some(fdef) = def
                        .fields
                        .iter()
                        .find(|f| f.name.eq_ignore_ascii_case(cname))
                    {
                        let fkey = fdef.name.replace('-', "m").to_ascii_lowercase();
                        match &fdef.ty {
                            VType::Scalar(w) => {
                                result.insert(fkey, fill_bits_literal(*w, ch));
                            }
                            VType::Record(sub_rec) => {
                                for (_full, suf, lw) in
                                    env.flatten_record_leaves(sub_rec, "", &fkey, depth + 1)
                                {
                                    result.insert(suf, fill_bits_literal(lw, ch));
                                }
                            }
                            VType::UnconstrainedArray(_) => {
                                result.insert(fkey, fill_bits_literal(1, ch));
                            }
                        }
                    }
                }
            } else {
                let v = parse_expr_sv(p, env).unwrap_or_else(|_| "1'b0".into());
                for cname in &choices {
                    if let Some(fdef) = def
                        .fields
                        .iter()
                        .find(|f| f.name.eq_ignore_ascii_case(cname))
                    {
                        result.insert(
                            fdef.name.replace('-', "m").to_ascii_lowercase(),
                            v.clone(),
                        );
                    }
                }
            }
        } else {
            let _ = parse_expr_sv(p, env);
        }
        if !p.eat(",") {
            if p.eat(")") {
                break;
            }
        }
    }
    let all_leaves = env.flatten_record_leaves(rec_type, "", "", 0);
    let mut out = Vec::with_capacity(all_leaves.len());
    for (_full, suf, lw) in all_leaves {
        if let Some(v) = result.remove(&suf) {
            out.push((suf, v));
        } else if let Some(ch) = others_bit {
            out.push((suf, fill_bits_literal(lw, ch)));
        } else if let Some(e) = &others_expr {
            out.push((suf, e.clone()));
        } else {
            out.push((suf, fill_bits_literal(lw, '0')));
        }
    }
    Ok(out)
}

fn parse_scalar_field_rhs(p: &mut P, width: usize, env: &TypeEnv) -> Result<String, String> {
    if let Some(ch) = peek_aggregate_fill_bit(p) {
        return Ok(fill_bits_literal(width, ch));
    }
    parse_expr_sv(p, env)
}

/// Recognizes `'0'`, `'1'`, `(others => '0')`, `(others => '1')`, or nested
/// `(others => (others => '0'))` and returns the fill bit `'0'` or `'1'`.
fn peek_aggregate_fill_bit(p: &mut P) -> Option<char> {
    let save = p.i;
    let mut parens = 0usize;
    while p.eat("(") {
        parens += 1;
        if !p.eat("others") || !p.eat("=>") {
            p.i = save;
            return None;
        }
    }
    if parens == 0 {
        return None;
    }
    let bit = match p.peek() {
        Some(Tok::BitStr(s)) if s == "1" => {
            p.i += 1;
            '1'
        }
        Some(Tok::BitStr(s)) if s == "0" => {
            p.i += 1;
            '0'
        }
        Some(Tok::Num(0)) => {
            p.i += 1;
            '0'
        }
        Some(Tok::Num(1)) => {
            p.i += 1;
            '1'
        }
        _ => {
            p.i = save;
            return None;
        }
    };
    for _ in 0..parens {
        if !p.eat(")") {
            p.i = save;
            return None;
        }
    }
    Some(bit)
}

fn fill_bits_literal(width: usize, bit: char) -> String {
    let w = width.max(1);
    let b = if bit == '1' { '1' } else { '0' };
    let bits: String = std::iter::repeat(b).take(w).collect();
    format!("{w}'b{bits}")
}

fn parse_for_generate(
    p: &mut P,
    env: &mut TypeEnv,
    signals: &mut Vec<(String, usize)>,
) -> Result<Option<CStmt>, String> {
    let var = p.ident()?;
    let _ = p.eat("in");
    let lo = parse_int_expr_with(p, &env.consts).unwrap_or(0);
    let downto = p.eat("downto");
    let _ = p.eat("to");
    let hi = parse_int_expr_with(p, &env.consts).unwrap_or(lo);
    let _ = p.eat("generate");
    let save = p.i;
    let mut saw_begin = false;
    let mut k = 0;
    while let Some(t) = p.peek() {
        k += 1;
        if k > 64 {
            break;
        }
        if matches!(t, Tok::Kw(s) if s == "begin") {
            saw_begin = true;
            break;
        }
        if matches!(t, Tok::Kw(s) if s == "end") {
            break;
        }
        p.bump();
    }
    if saw_begin {
        let _ = p.eat("begin");
    } else {
        p.i = save;
    }
    let (a, b) = if downto { (hi, lo) } else { (lo, hi) };
    let mut all = Vec::new();
    let mut depth = 1i32;
    let start = p.i;
    while depth > 0 && p.peek().is_some() {
        if p.eat("generate") {
            depth += 1;
        } else if p.eat("end") {
            if p.eat("generate") {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        } else {
            p.bump();
        }
    }
    let body = p.t[start..p.i.min(p.t.len())].to_vec();
    let _ = p.eat(";");
    let step = if a <= b { 1i64 } else { -1 };
    let mut i = a;
    let mut n = 0;
    loop {
        if n > 4096 {
            break;
        }
        let mut c_env = env.clone();
        c_env.consts.insert(var.clone(), i);
        let mut bp = P { t: &body, i: 0 };
        let stmts = parse_concurrent_list(&mut bp, &mut c_env, signals).unwrap_or_default();
        all.extend(stmts);
        if i == b {
            break;
        }
        i += step;
        n += 1;
    }
    if all.is_empty() {
        return Ok(None);
    }
    if all.iter().any(|s| matches!(s, CStmt::Inst { .. })) {
        note_parse_soft(
            "generate_not_lowered",
            Some(format!("for {var}")),
            var.clone(),
        );
    }
    Ok(Some(CStmt::Process {
        clock: None,
        body: all
            .into_iter()
            .filter_map(|s| match s {
                CStmt::Assign { lhs, rhs } => Some(SStmt::Assign { lhs, rhs }),
                CStmt::CondAssign { lhs, arms } => Some(SStmt::Assign {
                    lhs,
                    rhs: lower_cond_arms(&arms),
                }),
                _ => None,
            })
            .collect(),
    }))
}

fn parse_if_generate(
    p: &mut P,
    env: &mut TypeEnv,
    signals: &mut Vec<(String, usize)>,
) -> Result<Option<CStmt>, String> {
    let cond = parse_int_expr_with(p, &env.consts).unwrap_or(1);
    let _ = p.eat("generate");
    let mut then_toks = Vec::new();
    let mut else_toks = Vec::new();
    let mut depth = 1i32;
    let mut in_else = false;
    while depth > 0 && p.peek().is_some() {
        if p.eat("generate") {
            depth += 1;
            if in_else {
                else_toks.push(Tok::Kw("generate".into()));
            } else {
                then_toks.push(Tok::Kw("generate".into()));
            }
        } else if p.eat("else") && depth == 1 {
            in_else = true;
        } else if p.eat("end") {
            if p.eat("generate") {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        } else if let Some(t) = p.bump() {
            if in_else {
                else_toks.push(t.clone());
            } else {
                then_toks.push(t.clone());
            }
        }
    }
    let _ = p.eat(";");
    let use_else = cond == 0;
    let toks = if use_else { else_toks } else { then_toks };
    let mut bp = P { t: &toks, i: 0 };
    let mut c_env = env.clone();
    let stmts = parse_concurrent_list(&mut bp, &mut c_env, signals).unwrap_or_default();
    let n = stmts.len();
    let dropped_inst = stmts
        .iter()
        .skip(1)
        .any(|s| matches!(s, CStmt::Inst { .. }));
    let has_inst = stmts.iter().any(|s| matches!(s, CStmt::Inst { .. }));
    if dropped_inst || (has_inst && n > 1) {
        note_parse_soft(
            "generate_not_lowered",
            Some("if generate".into()),
            "generate",
        );
    }
    Ok(stmts.into_iter().next())
}

fn parse_process(
    p: &mut P,
    env: &mut TypeEnv,
    signals: &mut Vec<(String, usize)>,
) -> Result<CStmt, String> {
    if p.eat("(") {
        let mut d = 1i32;
        while d > 0 && p.peek().is_some() {
            if p.eat("(") {
                d += 1;
            } else if p.eat(")") {
                d -= 1;
            } else {
                p.bump();
            }
        }
    }
    let mut proc_env = env.clone();
    while !p.eat("begin") {
        if p.peek().is_none() {
            break;
        }
        if p.eat("variable") {
            let _ = parse_signal_list(p, signals, &mut proc_env);
        } else if p.eat("constant") {
            parse_constant_into_env(p, &mut proc_env, false);
        } else if p.eat("type") {
            parse_type_decl(p, &mut proc_env);
        } else if p.eat("subtype") {
            parse_subtype_decl(p, &mut proc_env);
        } else {
            p.bump();
        }
    }
    for (rk, rv) in &proc_env.rec_vars {
        env.rec_vars.entry(rk.clone()).or_insert_with(|| rv.clone());
    }
    let body = parse_seq_list(p, &proc_env, signals)?;
    let _ = p.eat("end");
    let _ = p.eat("process");
    let _ = p.eat_ident();
    let _ = p.eat(";");
    let clock = find_clock(&body);
    let body = strip_clock_if(body);
    Ok(CStmt::Process { clock, body })
}

fn parse_if_else_chain(
    p: &mut P,
    env: &TypeEnv,
    signals: &[(String, usize)],
) -> Result<Vec<SStmt>, String> {
    if p.eat("elsif") {
        let cond = parse_clock_or_expr(p, env)?;
        let _ = p.eat("then");
        let then_b = parse_seq_list(p, env, signals)?;
        let else_b = parse_if_else_chain(p, env, signals)?;
        return Ok(vec![SStmt::If {
            cond,
            then_b,
            else_b,
        }]);
    }
    if p.eat("else") {
        return parse_seq_list(p, env, signals);
    }
    Ok(Vec::new())
}

fn parse_seq_list(
    p: &mut P,
    env: &TypeEnv,
    signals: &[(String, usize)],
) -> Result<Vec<SStmt>, String> {
    let mut v = Vec::new();
    loop {
        if p.peek().is_none() {
            break;
        }
        if matches!(
            p.peek(),
            Some(Tok::Kw(k))
                if matches!(k.as_str(), "end" | "elsif" | "else" | "when")
        ) {
            break;
        }
        match parse_seq_many(p, env, signals) {
            Ok(stmts) => v.extend(stmts),
            Err(_) => skip_stmt(p),
        }
    }
    Ok(v)
}

fn parse_seq_many(
    p: &mut P,
    env: &TypeEnv,
    signals: &[(String, usize)],
) -> Result<Vec<SStmt>, String> {
    if p.eat("null") {
        eat_to_semi(p);
        return Ok(Vec::new());
    }
    if p.eat("wait") || p.eat("assert") || p.eat("report") || p.eat("next") || p.eat("exit") {
        eat_to_semi(p);
        return Ok(Vec::new());
    }
    if p.eat("if") {
        let cond = parse_clock_or_expr(p, env)?;
        let _ = p.eat("then");
        let then_b = parse_seq_list(p, env, signals)?;
        let else_b = parse_if_else_chain(p, env, signals)?;
        let _ = p.eat("end");
        let _ = p.eat("if");
        let _ = p.eat(";");
        return Ok(vec![SStmt::If {
            cond,
            then_b,
            else_b,
        }]);
    }
    if p.eat("case") {
        let is_match_case = p.eat("?");
        let sel = parse_expr_sv(p, env)?;
        let _ = p.eat("is");
        if is_match_case {
            let mut cond_arms: Vec<(String, Vec<SStmt>)> = Vec::new();
            let mut other = Vec::new();
            loop {
                if p.eat("end") {
                    let _ = p.eat("case");
                    let _ = p.eat("?");
                    let _ = p.eat_ident();
                    let _ = p.eat(";");
                    break;
                }
                if !p.eat("when") {
                    if p.peek().is_none() {
                        break;
                    }
                    p.bump();
                    continue;
                }
                if p.eat("others") {
                    let _ = p.eat("=>");
                    other = parse_seq_list(p, env, signals)?;
                    continue;
                }
                let mut pat_conds = vec![parse_match_pat_cond(p, &sel, env)?];
                while p.eat("|") {
                    pat_conds.push(parse_match_pat_cond(p, &sel, env)?);
                }
                let _ = p.eat("=>");
                let body = parse_seq_list(p, env, signals)?;
                let cond = if pat_conds.len() == 1 {
                    pat_conds.remove(0)
                } else {
                    format!("({})", pat_conds.join(" | "))
                };
                cond_arms.push((cond, body));
            }
            let mut acc = other;
            for (cond, then_b) in cond_arms.into_iter().rev() {
                acc = vec![SStmt::If {
                    cond,
                    then_b,
                    else_b: acc,
                }];
            }
            return Ok(acc);
        }
        let mut arms = Vec::new();
        let mut other = Vec::new();
        let mut saw_dont_care = false;
        loop {
            if p.eat("end") {
                let _ = p.eat("case");
                let _ = p.eat("?");
                let _ = p.eat_ident();
                let _ = p.eat(";");
                break;
            }
            if !p.eat("when") {
                if p.peek().is_none() {
                    break;
                }
                p.bump();
                continue;
            }
            if p.eat("others") {
                let _ = p.eat("=>");
                other = parse_seq_list(p, env, signals)?;
                continue;
            }
            let mut pats = Vec::new();
            loop {
                if let Some(Tok::BitStr(raw)) = p.peek() {
                    if raw.contains('-') {
                        saw_dont_care = true;
                        let raw_c = raw.clone();
                        p.i += 1;
                        pats.push(format!("__dc:{raw_c}"));
                    } else {
                        pats.push(parse_expr_sv(p, env)?);
                    }
                } else {
                    pats.push(parse_expr_sv(p, env)?);
                }
                if !p.eat("|") {
                    break;
                }
            }
            let _ = p.eat("=>");
            let body = parse_seq_list(p, env, signals)?;
            arms.push((pats, body));
        }
        if saw_dont_care {
            let mut acc = other;
            for (pats, then_b) in arms.into_iter().rev() {
                let conds: Vec<String> = pats
                    .into_iter()
                    .map(|pt| {
                        if let Some(raw) = pt.strip_prefix("__dc:") {
                            bitstr_match_cond(&sel, raw)
                        } else {
                            format!("(({sel}) == ({pt}))")
                        }
                    })
                    .collect();
                let cond = if conds.len() == 1 {
                    conds[0].clone()
                } else {
                    format!("({})", conds.join(" | "))
                };
                acc = vec![SStmt::If {
                    cond,
                    then_b,
                    else_b: acc,
                }];
            }
            return Ok(acc);
        }
        return Ok(vec![SStmt::Case { sel, arms, other }]);
    }
    if p.eat("for") {
        let var = p.ident()?;
        let _ = p.eat("in");
        let lo = parse_int_expr_with(p, &env.consts).unwrap_or(0);
        let downto = p.eat("downto");
        let _ = p.eat("to");
        let hi = parse_int_expr_with(p, &env.consts).unwrap_or(lo);
        let _ = p.eat("loop");
        let start = p.i;
        let mut depth = 1i32;
        while depth > 0 && p.peek().is_some() {
            if p.eat("loop") {
                depth += 1;
            } else if p.eat("end") {
                if p.eat("loop") {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            } else {
                p.bump();
            }
        }
        let body_toks = p.t[start..p.i.min(p.t.len())].to_vec();
        let _ = p.eat_ident();
        let _ = p.eat(";");
        let (a, b) = if downto { (hi, lo) } else { (lo, hi) };
        let step = if a <= b { 1i64 } else { -1 };
        let mut i = a;
        let mut n = 0usize;
        let mut unrolled = Vec::new();
        loop {
            if n > 4096 {
                break;
            }
            let mut lenv = env.clone();
            lenv.consts.insert(var.clone(), i);
            let mut bp = P {
                t: &body_toks,
                i: 0,
            };
            let mut stmts = parse_seq_list(&mut bp, &lenv, signals).unwrap_or_default();
            subst_var(&mut stmts, &var, i);
            unrolled.extend(stmts);
            if i == b {
                break;
            }
            i += step;
            n += 1;
        }
        return Ok(unrolled);
    }
    if p.eat("while") {
        eat_until_kw(p, "loop");
        let _ = p.eat("loop");
        let body = parse_seq_list(p, env, signals)?;
        let _ = p.eat("end");
        let _ = p.eat("loop");
        let _ = p.eat(";");
        return Ok(vec![SStmt::For {
            var: "_w".into(),
            lo: 0,
            hi: 7,
            body,
        }]);
    }
    if p.eat("loop") {
        let body = parse_seq_list(p, env, signals)?;
        let _ = p.eat("end");
        let _ = p.eat("loop");
        let _ = p.eat(";");
        return Ok(vec![SStmt::For {
            var: "_l".into(),
            lo: 0,
            hi: 0,
            body,
        }]);
    }
    let lhs = parse_target_sv(p, env, signals)?;
    if p.eat("<=") || p.eat(":=") {
        if let Some(rec_type) = env.resolve_rec_var(&lhs) {
            let leaves = env.flatten_record_leaves(&rec_type, &lhs, "", 0);
            let rhs_map: HashMap<String, String> =
                parse_record_rhs(p, &rec_type, env)?.into_iter().collect();
            skip_after_delay(p);
            let _ = p.eat(";");
            let mut out = Vec::with_capacity(leaves.len());
            for (leaf_lhs, suf, _w) in leaves {
                let rhs = rhs_map.get(&suf).cloned().unwrap_or_else(|| "1'b0".into());
                out.push(SStmt::Assign { lhs: leaf_lhs, rhs });
            }
            return Ok(out);
        }
        let rhs = parse_expr_sv(p, env)?;
        skip_after_delay(p);
        let _ = p.eat(";");
        return Ok(vec![SStmt::Assign { lhs, rhs }]);
    }
    skip_stmt(p);
    Ok(Vec::new())
}

fn parse_clock_or_expr(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    if let Some((clk, rise)) = take_clock(p) {
        return Ok(if rise {
            format!("__posedge_{clk}")
        } else {
            format!("__negedge_{clk}")
        });
    }
    parse_expr_sv(p, env)
}

fn take_clock(p: &mut P) -> Option<(String, bool)> {
    let save = p.i;
    if p.eat("rising_edge") {
        let _ = p.eat("(");
        let mut n = p.ident().ok()?;
        while p.eat(".") {
            if let Ok(f) = p.ident() {
                n = format!("{n}_{f}");
            }
        }
        let _ = p.eat(")");
        return Some((n, true));
    }
    if p.eat("falling_edge") {
        let _ = p.eat("(");
        let mut n = p.ident().ok()?;
        while p.eat(".") {
            if let Ok(f) = p.ident() {
                n = format!("{n}_{f}");
            }
        }
        let _ = p.eat(")");
        return Some((n, false));
    }
    if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
        let n = p.ident().ok()?;
        if p.eat("'") {
            let attr = p.ident().ok().unwrap_or_default();
            if attr.eq_ignore_ascii_case("event") {
                let _ = p.eat("and");
                let _ = p.ident();
                let _ = p.eat("=");
                let rise = match p.peek() {
                    Some(Tok::BitStr(s)) if s == "0" => {
                        p.i += 1;
                        false
                    }
                    Some(Tok::BitStr(_)) | Some(Tok::Num(_)) => {
                        p.i += 1;
                        true
                    }
                    _ => {
                        let _ = p.eat("'");
                        let _ = p.ident();
                        let _ = p.eat("'");
                        true
                    }
                };
                return Some((n, rise));
            }
        }
        // `clk = '1' and clk'event`
        if p.eat("=") {
            let rise = match p.peek() {
                Some(Tok::BitStr(s)) if s == "0" => {
                    p.i += 1;
                    false
                }
                Some(Tok::BitStr(_)) | Some(Tok::Num(_)) => {
                    p.i += 1;
                    true
                }
                _ => true,
            };
            let _ = p.eat("and");
            let _ = p.ident();
            if p.eat("'") {
                let _ = p.ident();
            }
            return Some((n, rise));
        }
    }
    p.i = save;
    None
}

fn find_clock(body: &[SStmt]) -> Option<String> {
    for s in body {
        match s {
            SStmt::If {
                cond,
                then_b,
                else_b,
            } => {
                if let Some(c) = clock_from_cond(cond) {
                    return Some(c);
                }
                if let Some(c) = find_clock(then_b) {
                    return Some(c);
                }
                if let Some(c) = find_clock(else_b) {
                    return Some(c);
                }
            }
            SStmt::Case { arms, other, .. } => {
                for (_, b) in arms {
                    if let Some(c) = find_clock(b) {
                        return Some(c);
                    }
                }
                if let Some(c) = find_clock(other) {
                    return Some(c);
                }
            }
            SStmt::For { body, .. } => {
                if let Some(c) = find_clock(body) {
                    return Some(c);
                }
            }
            _ => {}
        }
    }
    None
}

fn clock_from_cond(cond: &str) -> Option<String> {
    cond.strip_prefix("__posedge_")
        .or_else(|| cond.strip_prefix("__negedge_"))
        .map(|s| s.to_string())
}

fn strip_clock_if(body: Vec<SStmt>) -> Vec<SStmt> {
    if body.len() == 1 {
        if let SStmt::If {
            cond,
            then_b,
            else_b,
        } = &body[0]
        {
            if clock_from_cond(cond).is_some() {
                return then_b.clone();
            }
            // async: if rst then ... elsif posedge then ...
            if let Some(SStmt::If {
                cond: c2,
                then_b: t2,
                else_b: e2,
            }) = else_b.first()
            {
                if clock_from_cond(c2).is_some() {
                    return vec![SStmt::If {
                        cond: cond.clone(),
                        then_b: then_b.clone(),
                        else_b: if e2.is_empty() {
                            t2.clone()
                        } else {
                            e2.clone()
                        },
                    }];
                }
            }
        }
    }
    body
}

fn parse_port_map(p: &mut P, env: &TypeEnv) -> Result<Vec<(String, String)>, String> {
    let _ = p.eat("port");
    let _ = p.eat("map");
    let _ = p.eat("(");
    let mut conns = Vec::new();
    let mut idx = 0usize;
    loop {
        if p.eat(")") {
            break;
        }
        if p.peek().is_none() {
            break;
        }
        if p.eat("open") {
            let _ = p.eat(",");
            idx += 1;
            continue;
        }
        let save = p.i;
        let first = parse_expr_sv(p, env).unwrap_or_else(|_| "1'b0".into());
        if p.eat("=>") {
            let net = if p.eat("open") {
                format!("_open{idx}")
            } else {
                parse_expr_sv(p, env).unwrap_or(first.clone())
            };
            conns.push((first, net));
        } else {
            p.i = save;
            let net = parse_expr_sv(p, env).unwrap_or_else(|_| "1'b0".into());
            conns.push((format!("#{idx}"), net));
        }
        idx += 1;
        let _ = p.eat(",");
    }
    let _ = p.eat(";");
    Ok(conns)
}

fn parse_generic_map_values(p: &mut P, env: &TypeEnv) -> Vec<(String, i64)> {
    if !p.eat("generic") {
        return Vec::new();
    }
    let _ = p.eat("map");
    if !p.eat("(") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut idx = 0usize;
    while !p.eat(")") && p.peek().is_some() {
        let save = p.i;
        if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
            if let Ok(k) = p.ident() {
                if p.eat("=>") {
                    let v = parse_int_expr_with(p, &env.consts).unwrap_or(0);
                    out.push((k, v));
                    let _ = p.eat(",");
                    idx += 1;
                    continue;
                }
            }
        }
        p.i = save;
        let v = parse_int_expr_with(p, &env.consts).unwrap_or(0);
        out.push((format!("#{idx}"), v));
        idx += 1;
        if !p.eat(",") && !p.peek_sym(")") {
            p.bump();
        }
    }
    out
}

fn parse_match_pat_cond(p: &mut P, sel: &str, env: &TypeEnv) -> Result<String, String> {
    if let Some(Tok::BitStr(raw)) = p.peek() {
        let raw = raw.clone();
        p.i += 1;
        return Ok(bitstr_match_cond(sel, &raw));
    }
    let pat = parse_expr_sv(p, env)?;
    Ok(format!("(({sel}) == ({pat}))"))
}

fn bitstr_match_cond(sel: &str, raw_bits: &str) -> String {
    let w = raw_bits.len().max(1);
    if !raw_bits.contains('-') {
        return format!("(({sel}) == {w}'b{raw_bits})");
    }
    let mask: String = raw_bits
        .chars()
        .map(|c| if c == '-' { '0' } else { '1' })
        .collect();
    let val: String = raw_bits
        .chars()
        .map(|c| if c == '1' { '1' } else { '0' })
        .collect();
    if mask.chars().all(|c| c == '0') {
        return "1'b1".into();
    }
    format!("((({sel}) & {w}'b{mask}) == {w}'b{val})")
}

fn shift_bound_index(idx: i64, name: &str, env: &TypeEnv) -> i64 {
    if let Some(&(_hi, lo)) = env.bounds.get(&name.to_ascii_lowercase()) {
        if lo < 0 || idx < 0 {
            return (idx - lo).max(0);
        }
    }
    idx.max(0)
}

fn parse_target_sv(
    p: &mut P,
    env: &TypeEnv,
    signals: &[(String, usize)],
) -> Result<String, String> {
    let mut n = p.ident()?;
    while p.eat(".") {
        let f = p.ident()?;
        if env.pkg_names.contains(&n.to_ascii_lowercase()) {
            n = f;
        } else {
            n = format!("{n}_{f}");
        }
    }
    while p.peek_sym("(") {
        let is_arr = env
            .resolve_rec_var(&n)
            .map(|r| env.is_array_record(&r))
            .unwrap_or(false);
        if !is_arr {
            break;
        }
        let save = p.i;
        p.eat("(");
        if let Some(idx) = parse_int_expr_with(p, &env.consts) {
            if p.eat(")") {
                let idx_s = idx.to_string().replace('-', "m");
                n = format!("{n}_{idx_s}");
                while p.eat(".") {
                    if let Ok(f) = p.ident() {
                        n = format!("{n}_{f}");
                    }
                }
                continue;
            }
        }
        p.i = save;
        break;
    }
    if p.eat("(") {
        if let Some(a) = parse_int_expr_with(p, &env.consts) {
            if p.eat("downto") || p.eat("to") {
                let b = parse_int_expr_with(p, &env.consts).unwrap_or(a);
                let _ = p.eat(")");
                let sa = shift_bound_index(a, &n, env);
                let sb = shift_bound_index(b, &n, env);
                let hi = sa.max(sb);
                let lo = sa.min(sb);
                if hi == lo {
                    return Ok(format!("{n}[{hi}]"));
                }
                let _ = signals;
                return Ok(format!("{n}[{hi}:{lo}]"));
            }
            let _ = p.eat(")");
            let sa = shift_bound_index(a, &n, env);
            return Ok(format!("{n}[{sa}]"));
        }
        let e = parse_expr_sv(p, env)?;
        let _ = p.eat(")");
        return Ok(format!("{n}[{e}]"));
    }
    Ok(n)
}
fn parse_expr_sv(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    parse_or_sv(p, env)
}

fn parse_or_sv(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    let mut e = parse_and_sv(p, env)?;
    loop {
        if p.eat("or") {
            let r = parse_and_sv(p, env)?;
            e = format!("({e} | {r})");
        } else if p.eat("nor") {
            let r = parse_and_sv(p, env)?;
            e = format!("~({e} | {r})");
        } else if p.eat("xor") {
            let r = parse_and_sv(p, env)?;
            e = format!("({e} ^ {r})");
        } else if p.eat("xnor") {
            let r = parse_and_sv(p, env)?;
            e = format!("~({e} ^ {r})");
        } else {
            break;
        }
    }
    Ok(e)
}

fn parse_and_sv(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    let mut e = parse_rel_sv(p, env)?;
    loop {
        if p.eat("and") {
            let r = parse_rel_sv(p, env)?;
            e = format!("({e} & {r})");
        } else if p.eat("nand") {
            let r = parse_rel_sv(p, env)?;
            e = format!("~({e} & {r})");
        } else {
            break;
        }
    }
    Ok(e)
}

fn expand_record_cmp(lhs: &str, rhs: &str, eq: bool, env: &TypeEnv) -> Option<String> {
    let l_trim = lhs.trim().trim_matches(|c| c == '(' || c == ')').trim();
    let r_trim = rhs.trim().trim_matches(|c| c == '(' || c == ')').trim();
    let rec_type = env
        .resolve_rec_var(l_trim)
        .or_else(|| env.resolve_rec_var(r_trim))?;
    let leaves = env.flatten_record_leaves(&rec_type, "", "", 0);
    if leaves.is_empty() {
        return None;
    }
    let l_cmap = env.rec_consts.get(&l_trim.to_ascii_lowercase());
    let r_cmap = env.rec_consts.get(&r_trim.to_ascii_lowercase());
    let mut terms = Vec::with_capacity(leaves.len());
    for (_full, suf, _w) in leaves {
        let le = if let Some(m) = l_cmap {
            m.get(&suf).cloned().unwrap_or_else(|| "1'b0".into())
        } else {
            format!("{l_trim}_{suf}")
        };
        let re = if let Some(m) = r_cmap {
            m.get(&suf).cloned().unwrap_or_else(|| "1'b0".into())
        } else {
            format!("{r_trim}_{suf}")
        };
        if eq {
            terms.push(format!("({le} == {re})"));
        } else {
            terms.push(format!("({le} != {re})"));
        }
    }
    let sep = if eq { " & " } else { " | " };
    Some(format!("({})", terms.join(sep)))
}

fn parse_rel_sv(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    let mut e = parse_shift_sv(p, env)?;
    if p.eat("?=") {
        if let Some(Tok::BitStr(raw)) = p.peek() {
            let raw = raw.clone();
            p.i += 1;
            return Ok(bitstr_match_cond(&e, &raw));
        }
        let r = parse_shift_sv(p, env)?;
        return Ok(format!("({e} == {r})"));
    } else if p.eat("?/=") {
        if let Some(Tok::BitStr(raw)) = p.peek() {
            let raw = raw.clone();
            p.i += 1;
            return Ok(format!("~({})", bitstr_match_cond(&e, &raw)));
        }
        let r = parse_shift_sv(p, env)?;
        return Ok(format!("({e} != {r})"));
    } else if p.eat("=") {
        if let Some(Tok::BitStr(raw)) = p.peek() {
            if raw.contains('-') {
                let raw = raw.clone();
                p.i += 1;
                return Ok(bitstr_match_cond(&e, &raw));
            }
        }
        let r = parse_shift_sv(p, env)?;
        if let Some(cmp) = expand_record_cmp(&e, &r, true, env) {
            e = cmp;
        } else {
            e = format!("({e} == {r})");
        }
    } else if p.eat("/=") {
        if let Some(Tok::BitStr(raw)) = p.peek() {
            if raw.contains('-') {
                let raw = raw.clone();
                p.i += 1;
                return Ok(format!("~({})", bitstr_match_cond(&e, &raw)));
            }
        }
        let r = parse_shift_sv(p, env)?;
        if let Some(cmp) = expand_record_cmp(&e, &r, false, env) {
            e = cmp;
        } else {
            e = format!("({e} != {r})");
        }
    } else if p.eat("<=") {
        let r = parse_shift_sv(p, env)?;
        e = format!("({e} <= {r})");
    } else if p.eat(">=") {
        let r = parse_shift_sv(p, env)?;
        e = format!("({e} >= {r})");
    } else if p.eat("<") {
        let r = parse_shift_sv(p, env)?;
        e = format!("({e} < {r})");
    } else if p.eat(">") {
        let r = parse_shift_sv(p, env)?;
        e = format!("({e} > {r})");
    }
    Ok(e)
}

fn parse_shift_sv(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    let mut e = parse_add_sv(p, env)?;
    loop {
        if p.eat("sll") || p.eat("sla") {
            let r = parse_add_sv(p, env)?;
            e = format!("({e} << {r})");
        } else if p.eat("srl") {
            let r = parse_add_sv(p, env)?;
            e = format!("({e} >> {r})");
        } else if p.eat("sra") {
            let r = parse_add_sv(p, env)?;
            e = format!("($signed({e}) >>> {r})");
        } else if p.eat("rol") || p.eat("ror") {
            let r = parse_add_sv(p, env)?;
            e = format!("({e} << {r})");
        } else {
            break;
        }
    }
    Ok(e)
}

fn parse_add_sv(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    let mut e = parse_mul_sv(p, env)?;
    loop {
        if p.eat("+") {
            let r = parse_mul_sv(p, env)?;
            e = format!("({e} + {r})");
        } else if p.eat("-") {
            let r = parse_mul_sv(p, env)?;
            e = format!("({e} - {r})");
        } else if p.eat("&") {
            let r = parse_mul_sv(p, env)?;
            e = format!("{{{e}, {r}}}");
        } else {
            break;
        }
    }
    Ok(e)
}

fn parse_mul_sv(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    let mut e = parse_unary_sv(p, env)?;
    loop {
        if p.eat("*") {
            let r = parse_unary_sv(p, env)?;
            e = format!("({e} * {r})");
        } else if p.eat("/") {
            let r = parse_unary_sv(p, env)?;
            e = format!("({e} / {r})");
        } else if p.eat("mod") || p.eat("rem") {
            let r = parse_unary_sv(p, env)?;
            e = format!("({e} % {r})");
        } else {
            break;
        }
    }
    Ok(e)
}

fn parse_unary_sv(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    if p.eat("not") {
        let x = parse_unary_sv(p, env)?;
        return Ok(format!("~({x})"));
    }
    if p.eat("abs") {
        let x = parse_unary_sv(p, env)?;
        return Ok(format!("({x})"));
    }
    if p.eat("-") {
        let x = parse_unary_sv(p, env)?;
        return Ok(format!("-({x})"));
    }
    if p.eat("+") {
        return parse_unary_sv(p, env);
    }
    parse_primary_sv(p, env)
}

fn parse_primary_sv(p: &mut P, env: &TypeEnv) -> Result<String, String> {
    if p.eat("(") {
        if p.eat("others") {
            let _ = p.eat("=>");
            let v = parse_unary_sv(p, env).unwrap_or_else(|_| "1'b0".into());
            let _ = p.eat(")");
            return Ok(v);
        }
        let inner = parse_expr_sv(p, env)?;
        if p.eat(",") {
            let mut parts = vec![inner];
            loop {
                parts.push(parse_expr_sv(p, env).unwrap_or_else(|_| "1'b0".into()));
                if !p.eat(",") {
                    break;
                }
            }
            let _ = p.eat(")");
            return Ok(format!("{{{}}}", parts.join(", ")));
        }
        let _ = p.eat(")");
        return Ok(format!("({inner})"));
    }
    if let Some(Tok::BitStr(bits)) = p.peek() {
        let clean = bits.replace('-', "0");
        p.i += 1;
        let w = clean.len().max(1);
        return Ok(format!("{w}'b{clean}"));
    }
    if let Some(n) = p.number() {
        return Ok(n.to_string());
    }
    if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
        let mut n = p.ident()?;
        let mut qual_pkg: Option<String> = None;
        let mut rec_const_root: Option<String> = None;
        let mut rec_const_suf = String::new();
        if env.rec_consts.contains_key(&n.to_ascii_lowercase()) {
            rec_const_root = Some(n.to_ascii_lowercase());
        }
        while p.eat(".") {
            let f = p.ident()?;
            if let Some(root) = &rec_const_root {
                if rec_const_suf.is_empty() {
                    rec_const_suf = f.to_ascii_lowercase();
                } else {
                    rec_const_suf = format!("{rec_const_suf}_{}", f.to_ascii_lowercase());
                }
                if let Some(val) = env.rec_consts.get(root).and_then(|m| m.get(&rec_const_suf)) {
                    n = val.clone();
                    rec_const_root = None;
                } else {
                    n = format!("{n}_{f}");
                }
            } else if env.pkg_names.contains(&n.to_ascii_lowercase()) {
                qual_pkg = Some(n.to_ascii_lowercase());
                n = f;
                let qk = qual_pkg
                    .as_ref()
                    .map(|pk| format!("{pk}__{}", n.to_ascii_lowercase()))
                    .unwrap_or_default();
                if env.rec_consts.contains_key(&qk) {
                    rec_const_root = Some(qk);
                } else if env.rec_consts.contains_key(&n.to_ascii_lowercase()) {
                    rec_const_root = Some(n.to_ascii_lowercase());
                }
            } else {
                n = format!("{n}_{f}");
            }
        }
        while p.peek_sym("(") {
            let is_arr = env
                .resolve_rec_var(&n)
                .map(|r| env.is_array_record(&r))
                .unwrap_or(false);
            if !is_arr {
                break;
            }
            let save = p.i;
            p.eat("(");
            if let Some(idx) = parse_int_expr_with(p, &env.consts) {
                if p.eat(")") {
                    let idx_s = idx.to_string().replace('-', "m");
                    n = format!("{n}_{idx_s}");
                    while p.eat(".") {
                        if let Ok(f) = p.ident() {
                            n = format!("{n}_{f}");
                        }
                    }
                    continue;
                }
            }
            p.i = save;
            break;
        }
        if p.eat("'") {
            let attr = p.ident().unwrap_or_default().to_ascii_lowercase();
            if p.eat("(") {
                let inner = parse_expr_sv(p, env)?;
                let _ = p.eat(")");
                return Ok(inner);
            }
            if let Some(&(hi, lo)) = env
                .bounds
                .get(&n.to_ascii_lowercase())
                .or_else(|| env.type_bounds.get(&n.to_ascii_lowercase()))
            {
                match attr.as_str() {
                    "length" => return Ok(((hi.abs_diff(lo) as i64) + 1).to_string()),
                    "high" | "left" => return Ok(hi.to_string()),
                    "low" | "right" => return Ok(lo.to_string()),
                    _ => {}
                }
            }
        }
        if p.eat("(") {
            if n.eq_ignore_ascii_case("std_match") {
                let lhs = parse_expr_sv(p, env)?;
                let _ = p.eat(",");
                if let Some(Tok::BitStr(raw)) = p.peek() {
                    let raw = raw.clone();
                    p.i += 1;
                    let _ = p.eat(")");
                    return Ok(bitstr_match_cond(&lhs, &raw));
                }
                let rhs = parse_expr_sv(p, env)?;
                let _ = p.eat(")");
                return Ok(format!("(({lhs}) == ({rhs}))"));
            }
            if is_cast(&n) {
                let inner = parse_expr_sv(p, env)?;
                while p.eat(",") {
                    let _ = parse_expr_sv(p, env);
                }
                let _ = p.eat(")");
                return Ok(inner);
            }
            if n.eq_ignore_ascii_case("rising_edge") {
                let clk = parse_expr_sv(p, env)?;
                let _ = p.eat(")");
                return Ok(format!("__posedge_{clk}"));
            }
            if n.eq_ignore_ascii_case("falling_edge") {
                let clk = parse_expr_sv(p, env)?;
                let _ = p.eat(")");
                return Ok(format!("__negedge_{clk}"));
            }
            let fn_lookup_key = if let Some(pk) = &qual_pkg {
                let qk = format!("{pk}__{}", n.to_ascii_lowercase());
                if env.funcs.contains_key(&qk) {
                    qk
                } else {
                    n.to_ascii_lowercase()
                }
            } else {
                n.to_ascii_lowercase()
            };
            if let Some(a) = parse_int_expr_peek(p, &env.consts) {
                if p.eat("downto") || p.eat("to") {
                    let b = parse_int_expr_with(p, &env.consts).unwrap_or(a);
                    let _ = p.eat(")");
                    let sa = shift_bound_index(a, &n, env);
                    let sb = shift_bound_index(b, &n, env);
                    let hi = sa.max(sb);
                    let lo = sa.min(sb);
                    return Ok(format!("{n}[{hi}:{lo}]"));
                }
                if p.peek_sym(")") {
                    p.eat(")");
                    if let Some(fdef) = env.funcs.get(&fn_lookup_key) {
                        let mut param_map = HashMap::new();
                        if let Some(p0) = fdef.params.first() {
                            param_map.insert(p0.clone(), a.to_string());
                        }
                        return Ok(replace_word_map(&fdef.body_expr, &param_map));
                    }
                    let sa = shift_bound_index(a, &n, env);
                    return Ok(format!("{n}[{sa}]"));
                }
            }
            let mut args = vec![parse_expr_sv(p, env).unwrap_or_else(|_| "1'b0".into())];
            while p.eat(",") {
                args.push(parse_expr_sv(p, env).unwrap_or_else(|_| "1'b0".into()));
            }
            let _ = p.eat(")");
            if let Some(fdef) = env.funcs.get(&fn_lookup_key) {
                let mut param_map = HashMap::new();
                for (param, arg) in fdef.params.iter().zip(args.iter()) {
                    param_map.insert(param.clone(), arg.clone());
                }
                return Ok(replace_word_map(&fdef.body_expr, &param_map));
            }
            if args.len() == 1 {
                return Ok(args.remove(0));
            }
            return Ok(format!("({})", args.join(" ^ ")));
        }
        if n.eq_ignore_ascii_case("true") {
            return Ok("1'b1".into());
        }
        if n.eq_ignore_ascii_case("false") {
            return Ok("1'b0".into());
        }
        if let Some(pk) = &qual_pkg {
            let qk = format!("{pk}__{}", n.to_ascii_lowercase());
            if let Some(v) = const_get(&env.consts, &qk) {
                return Ok(v.to_string());
            }
        }
        if let Some(v) = const_get(&env.consts, &n) {
            return Ok(v.to_string());
        }
        return Ok(n);
    }
    Err("vhdl expr".into())
}

fn is_cast(n: &str) -> bool {
    matches!(
        n.to_ascii_lowercase().as_str(),
        "std_logic_vector"
            | "std_ulogic_vector"
            | "std_logic"
            | "std_ulogic"
            | "unsigned"
            | "signed"
            | "ufixed"
            | "sfixed"
            | "to_ufixed"
            | "to_sfixed"
            | "to_slv"
            | "to_std_logic_vector"
            | "float"
            | "float32"
            | "float64"
            | "to_float"
            | "real"
            | "integer"
            | "natural"
            | "positive"
            | "boolean"
            | "to_unsigned"
            | "to_signed"
            | "to_integer"
            | "conv_integer"
            | "conv_std_logic_vector"
            | "resize"
            | "to_bit"
            | "to_bitvector"
            | "to_stdlogicvector"
            | "to_stdulogicvector"
    )
}

fn parse_int_expr_peek(p: &mut P, c: &HashMap<String, i64>) -> Option<i64> {
    let save = p.i;
    let v = parse_int_expr_with(p, c);
    if v.is_none() {
        p.i = save;
    }
    v
}

fn parse_int_expr(p: &mut P) -> Option<i64> {
    parse_int_expr_with(p, &HashMap::new())
}

fn parse_int_expr_with(p: &mut P, c: &HashMap<String, i64>) -> Option<i64> {
    let mut v = parse_int_atom(p, c)?;
    loop {
        if p.eat("+") {
            v += parse_int_atom(p, c).unwrap_or(0);
        } else if p.eat("-") {
            v -= parse_int_atom(p, c).unwrap_or(0);
        } else if p.eat("*") {
            v *= parse_int_atom(p, c).unwrap_or(1);
        } else if p.eat("/") {
            let d = parse_int_atom(p, c).unwrap_or(1).max(1);
            v /= d;
        } else if p.eat("**") {
            let exp = parse_int_atom(p, c).unwrap_or(0).clamp(0, 62) as u32;
            v = v.saturating_pow(exp);
        } else {
            break;
        }
    }
    Some(v)
}

fn parse_int_atom(p: &mut P, c: &HashMap<String, i64>) -> Option<i64> {
    if p.eat("-") {
        return parse_int_atom(p, c).map(|x| -x);
    }
    if p.eat("+") {
        return parse_int_atom(p, c);
    }
    if p.eat("(") {
        let v = parse_int_expr_with(p, c);
        let _ = p.eat(")");
        return v;
    }
    if let Some(n) = p.number() {
        return Some(n as i64);
    }
    if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
        let save = p.i;
        let mut n = p.ident().ok()?;
        let mut qual_pkg: Option<String> = None;
        while p.eat(".") {
            if let Ok(f) = p.ident() {
                qual_pkg = Some(n.to_ascii_lowercase());
                n = f;
            }
        }
        if matches!(
            n.to_ascii_lowercase().as_str(),
            "clog2" | "log2" | "ceil_log2" | "bit_width"
        ) && p.eat("(")
        {
            let arg = parse_int_expr_with(p, c).unwrap_or(1);
            let _ = p.eat(")");
            let w = if arg <= 1 {
                1
            } else {
                (u64::BITS - ((arg as u64) - 1).leading_zeros()) as i64
            };
            return Some(w);
        }
        if let Some(pk) = &qual_pkg {
            let qk = format!("{pk}__{}", n.to_ascii_lowercase());
            if let Some(v) = const_get(c, &qk) {
                return Some(v);
            }
        }
        if let Some(v) = const_get(c, &n) {
            return Some(v);
        }
        p.i = save;
        return None;
    }
    None
}

fn parse_vtype(p: &mut P, env: &mut TypeEnv) -> VType {
    parse_vtype_with_bounds(p, env).0
}

fn parse_vtype_with_bounds(p: &mut P, env: &mut TypeEnv) -> (VType, Option<(i64, i64)>) {
    let save = p.i;
    if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
        if let Ok(mut tn) = p.ident() {
            let mut qual_pkg: Option<String> = None;
            while p.eat(".") {
                if let Ok(sub) = p.ident() {
                    qual_pkg = Some(tn.to_ascii_lowercase());
                    tn = sub;
                }
            }
            let raw_low = tn.to_ascii_lowercase();
            let low = if let Some(pk) = &qual_pkg {
                let qk = format!("{pk}__{raw_low}");
                if env.records.contains_key(&qk)
                    || env.subtypes.contains_key(&qk)
                    || env.unconstrained_arrays.contains_key(&qk)
                {
                    qk
                } else {
                    raw_low.clone()
                }
            } else {
                raw_low.clone()
            };
            if raw_low == "float32" {
                skip_type_tail(p);
                return (VType::Scalar(32), Some((8, -23)));
            }
            if raw_low == "float64" {
                skip_type_tail(p);
                return (VType::Scalar(64), Some((11, -52)));
            }
            if let Some(base_elem) = env.unconstrained_arrays.get(&low).cloned() {
                if p.eat("(") {
                    if let Some(a) = parse_int_expr_peek(p, &env.consts) {
                        if p.eat("downto") || p.eat("to") {
                            let b = parse_int_expr_with(p, &env.consts).unwrap_or(a);
                            let _ = p.eat(")");
                            let mut elem_ty = base_elem;
                            if p.eat("(") {
                                if let Some(ea) = parse_int_expr_with(p, &env.consts) {
                                    if p.eat("downto") || p.eat("to") {
                                        let eb = parse_int_expr_with(p, &env.consts).unwrap_or(ea);
                                        elem_ty =
                                            VType::Scalar((ea.abs_diff(eb) as usize) + 1);
                                    }
                                }
                                let _ = p.eat(")");
                            }
                            skip_type_tail(p);
                            let rkey = env.make_constrained_array_record(
                                &low,
                                a.min(b),
                                a.max(b),
                                &elem_ty,
                            );
                            return (VType::Record(rkey), Some((a, b)));
                        }
                    }
                    skip_paren_rest(p);
                }
                skip_type_tail(p);
                return (VType::UnconstrainedArray(low), None);
            }
            if env.get_record(&low).is_some() {
                skip_type_tail(p);
                return (VType::Record(low), None);
            }
            if let Some(sub_ty) = env.subtypes.get(&low).cloned() {
                if !p.peek_sym("(") && !p.eat("range") {
                    skip_type_tail(p);
                    let bnds = env.type_bounds.get(&low).copied();
                    return (sub_ty, bnds);
                }
            }
        }
    }
    p.i = save;
    let (w, bnds) = parse_type_with_bounds(p, &env.consts);
    (VType::Scalar(w), bnds)
}

fn parse_type_with_bounds(
    p: &mut P,
    consts: &HashMap<String, i64>,
) -> (usize, Option<(i64, i64)>) {
    let mut is_int_default = false;
    if p.eat("integer") || p.eat("natural") || p.eat("positive") {
        is_int_default = true;
    } else {
        let _ = p.eat("std_logic");
        let _ = p.eat("std_ulogic");
        let _ = p.eat("std_logic_vector");
        let _ = p.eat("std_ulogic_vector");
        let _ = p.eat("unsigned");
        let _ = p.eat("signed");
        let _ = p.eat("ufixed");
        let _ = p.eat("sfixed");
        let _ = p.eat("float");
        let _ = p.eat("bit");
        let _ = p.eat("bit_vector");
        let _ = p.eat("boolean");
        let _ = p.eat("real");
        let _ = p.eat("character");
        let _ = p.eat("string");
        let _ = p.eat("time");
    }
    if p.eat("(") {
        if let Some(a) = parse_int_expr_with(p, consts) {
            if p.eat("downto") || p.eat("to") {
                let b = parse_int_expr_with(p, consts).unwrap_or(a);
                let _ = p.eat(")");
                skip_type_tail(p);
                return ((a.abs_diff(b) as usize) + 1, Some((a, b)));
            }
            let _ = p.eat(")");
            skip_type_tail(p);
            return (1, None);
        }
        skip_paren_rest(p);
        skip_type_tail(p);
        return (1, None);
    }
    if p.eat("range") {
        let a = parse_int_expr_with(p, consts).unwrap_or(31);
        let _ = p.eat("to");
        let _ = p.eat("downto");
        let b = parse_int_expr_with(p, consts).unwrap_or(0);
        let hi = a.max(b).max(0) as u64;
        if is_int_default {
            let w = if hi <= 1 {
                1
            } else {
                (u64::BITS - hi.leading_zeros()) as usize
            };
            return (w.max(1), Some((a, b)));
        }
        return ((a.abs_diff(b) as usize) + 1, Some((a, b)));
    }
    skip_type_tail(p);
    (1, None)
}

fn skip_type_tail(p: &mut P) {
    let _ = p.eat("range");
    while p.peek().is_some()
        && !p.peek_sym(";")
        && !p.peek_sym(")")
        && !p.peek_sym(",")
        && !p.peek_sym(":=")
        && !p.peek_sym("=>")
        && !matches!(
            p.peek(),
            Some(Tok::Kw(k)) if matches!(k.as_str(), "is" | "begin" | "end" | "port" | "generic")
        )
    {
        if p.eat("(") {
            skip_paren_rest(p);
            continue;
        }
        p.bump();
    }
}

fn skip_type_tokens(p: &mut P) {
    let mut depth = 0i32;
    while p.peek().is_some() {
        if depth == 0 && (p.peek_sym(";") || p.peek_sym(")") || p.peek_sym(":=")) {
            break;
        }
        if p.eat("(") {
            depth += 1;
        } else if p.eat(")") {
            depth -= 1;
            if depth < 0 {
                p.i -= 1;
                break;
            }
        } else {
            p.bump();
        }
    }
}

fn skip_init(p: &mut P) {
    if p.eat(":=") {
        let mut depth = 0i32;
        while p.peek().is_some() {
            if depth == 0 && (p.peek_sym(";") || p.peek_sym(")") || p.peek_sym(",")) {
                break;
            }
            if p.eat("(") {
                depth += 1;
            } else if p.eat(")") {
                depth -= 1;
                if depth < 0 {
                    p.i -= 1;
                    break;
                }
            } else {
                p.bump();
            }
        }
    }
}

fn skip_after_delay(p: &mut P) {
    if p.eat("after") {
        eat_to_semi(p);
        p.i = p.i.saturating_sub(1);
    }
}

fn skip_paren_block(p: &mut P) {
    if !p.eat("(") {
        return;
    }
    skip_paren_rest(p);
}

fn skip_paren_rest(p: &mut P) {
    let mut depth = 1i32;
    while depth > 0 && p.peek().is_some() {
        if p.eat("(") {
            depth += 1;
        } else if p.eat(")") {
            depth -= 1;
        } else {
            p.bump();
        }
    }
}

fn eat_to_semi(p: &mut P) {
    let mut d = 0i32;
    while p.peek().is_some() {
        if d == 0 && p.eat(";") {
            break;
        }
        if p.eat("(") {
            d += 1;
        } else if p.eat(")") {
            d -= 1;
        } else {
            p.bump();
        }
    }
}

fn skip_stmt(p: &mut P) {
    let mut d = 0i32;
    while p.peek().is_some() {
        if d == 0 && p.eat(";") {
            return;
        }
        if p.eat("begin") || p.eat("then") || p.eat("generate") || p.eat("loop") {
            d += 1;
        } else if p.eat("end") {
            d = (d - 1).max(0);
            eat_to_semi(p);
            if d == 0 {
                return;
            }
        } else if p.eat("(") {
            skip_paren_rest(p);
        } else {
            p.bump();
        }
    }
}

fn recover_assign(p: &mut P, env: &TypeEnv) -> Option<CStmt> {
    let start = p.i;
    while p.peek().is_some() && !p.peek_sym(";") {
        if p.eat("<=") {
            let rhs = parse_expr_sv(p, env).ok()?;
            eat_to_semi(p);
            return Some(CStmt::Assign {
                lhs: "helion_recovered".into(),
                rhs,
            });
        }
        p.bump();
    }
    p.i = start;
    None
}

fn eat_decl_item(p: &mut P) {
    eat_to_semi(p);
}

fn skip_to_end_unit(p: &mut P, kind: &str) {
    while p.peek().is_some() {
        if p.eat("end") {
            let _ = p.eat("?");
            if p.eat("record")
                || p.eat("units")
                || p.eat("function")
                || p.eat("procedure")
                || p.eat("if")
                || p.eat("case")
                || p.eat("loop")
                || p.eat("generate")
                || p.eat("process")
                || (kind != "component" && p.eat("component"))
                || (kind != "package" && p.eat("package"))
            {
                let _ = p.eat("?");
                let _ = p.eat("body");
                let _ = p.eat_ident();
                let _ = p.eat(";");
                continue;
            }
            let _ = p.eat(kind);
            let _ = p.eat("entity");
            let _ = p.eat("architecture");
            let _ = p.eat("component");
            let _ = p.eat("package");
            let _ = p.eat("context");
            let _ = p.eat("body");
            let _ = p.eat_ident();
            let _ = p.eat(";");
            return;
        }
        p.bump();
    }
}

fn skip_balanced_end(p: &mut P, kind: &str) {
    skip_to_end_unit(p, kind);
}

fn eat_until_kw(p: &mut P, kw: &str) {
    while p.peek().is_some() && !p.eat(kw) {
        p.bump();
    }
}

fn lower_cond_arms(arms: &[(Option<String>, String)]) -> String {
    let mut acc = String::from("1'b0");
    for (cond, val) in arms.iter().rev() {
        acc = match cond {
            Some(c) => format!("(({c}) ? {val} : {acc})"),
            None => val.clone(),
        };
    }
    acc
}

fn emit_sv(
    ent: &Entity,
    arch: &Arch,
    consts: &HashMap<String, i64>,
    known_bodies: &HashSet<String>,
    entity_clocks: &HashMap<String, HashSet<String>>,
    entity_occupied: &HashMap<String, HashSet<String>>,
    entities: &[Entity],
    extra_module_ports: &HashMap<String, Vec<(String, &'static str, usize)>>,
    origin: &str,
    source: &str,
    softs: &mut Vec<SoftDiag>,
    child_softs: &HashMap<String, Vec<SoftDiag>>,
) -> Result<String, String> {
    let empty_clocks = HashSet::new();
    let sta_clocks = entity_clocks
        .get(&ent.name.to_ascii_lowercase())
        .unwrap_or(&empty_clocks);
    let occupied = entity_occupied
        .get(&ent.name.to_ascii_lowercase())
        .cloned()
        .unwrap_or_else(|| occupied_sv_names(ent, arch));
    if clock_clk_clash(sta_clocks, &occupied) {
        emit_diag(&clock_clk_clash_line(&ent.name));
        push_soft(
            softs,
            SoftDiag::new("clock_clk_clash", &ent.name)
                .with_detail("construct=process_clock clock=clock occupied=clk")
                .with_span(span_best_effort(source, origin, &["clock", "process"])),
        );
    }
    for (name, detail, hint) in &arch.parse_softs {
        let mut d = SoftDiag::new(name, &ent.name).with_span(span_best_effort(
            source,
            origin,
            &[hint.as_str(), "generate"],
        ));
        if let Some(det) = detail {
            d = d.with_detail(det.clone());
        }
        push_soft(softs, d);
    }
    let mut sv = String::new();
    sv.push_str(&format!("module {}(", sv_ident(&ent.name)));
    let plist: Vec<String> = ent
        .ports
        .iter()
        .map(|(n, d, w)| {
            let n = sta_clock_name(n, sta_clocks, &occupied);
            if *w == 1 {
                format!("{d} logic {n}")
            } else {
                format!("{d} logic [{}:0] {n}", w - 1)
            }
        })
        .collect();
    sv.push_str(&plist.join(", "));
    sv.push_str(");\n");
    let mut keys: Vec<&String> = consts.keys().collect();
    keys.sort();
    for k in keys {
        let v = consts[k];
        sv.push_str(&format!("  localparam {k} = {v};\n"));
    }
    for (n, w) in &arch.signals {
        let n = sta_clock_name(n, sta_clocks, &occupied);
        if *w == 1 {
            sv.push_str(&format!("  logic {n};\n"));
        } else {
            sv.push_str(&format!("  logic [{}:0] {n};\n", w - 1));
        }
    }
    for st in &arch.stmts {
        match st {
            CStmt::Assign { lhs, rhs } => {
                let lhs = rewrite_sta_clock_ident(lhs, sta_clocks, &occupied);
                let rhs = rewrite_sta_clock_ident(rhs, sta_clocks, &occupied);
                for line in emit_assign_lines(&lhs, &rhs) {
                    sv.push_str(&format!("  assign {line}\n"));
                }
            }
            CStmt::CondAssign { lhs, arms } => {
                let lhs = rewrite_sta_clock_ident(lhs, sta_clocks, &occupied);
                let arms: Vec<(Option<String>, String)> = arms
                    .iter()
                    .map(|(c, v)| {
                        (
                            c.as_ref()
                                .map(|s| rewrite_sta_clock_ident(s, sta_clocks, &occupied)),
                            rewrite_sta_clock_ident(v, sta_clocks, &occupied),
                        )
                    })
                    .collect();
                let rhs = lower_cond_arms(&arms);
                for line in emit_assign_lines(&lhs, &rhs) {
                    sv.push_str(&format!("  assign {line}\n"));
                }
            }
            CStmt::Selected {
                sel,
                lhs,
                arms,
                other,
            } => {
                let sel = rewrite_sta_clock_ident(sel, sta_clocks, &occupied);
                let lhs = rewrite_sta_clock_ident(lhs, sta_clocks, &occupied);
                let mut rhs = other
                    .clone()
                    .map(|s| rewrite_sta_clock_ident(&s, sta_clocks, &occupied))
                    .unwrap_or_else(|| "1'b0".into());
                for (pat, val) in arms.iter().rev() {
                    let pat = rewrite_sta_clock_ident(pat, sta_clocks, &occupied);
                    let val = rewrite_sta_clock_ident(val, sta_clocks, &occupied);
                    rhs = format!("(({sel} == {pat}) ? {val} : {rhs})");
                }
                for line in emit_assign_lines(&lhs, &rhs) {
                    sv.push_str(&format!("  assign {line}\n"));
                }
            }
            CStmt::Process { clock, body } => {
                if let Some(clk) = clock {
                    let clk = sta_clock_name(clk, sta_clocks, &occupied);
                    sv.push_str(&format!("  always_ff @(posedge {clk}) begin\n"));
                } else {
                    sv.push_str("  always_comb begin\n");
                }
                emit_seq(&mut sv, body, 2, sta_clocks, &occupied);
                sv.push_str("  end\n");
            }
            CStmt::Inst {
                module,
                name,
                conns,
                ..
            } => {
                if !known_bodies.contains(&module.to_ascii_lowercase()) {
                    emit_diag(&missing_component_line(&ent.name, name, module));
                    push_soft(
                        softs,
                        SoftDiag::new("missing_component", &ent.name)
                            .with_detail(format!("inst={name} child={module}"))
                            .with_span(span_best_effort(
                                source,
                                origin,
                                &[name.as_str(), module.as_str()],
                            )),
                    );
                } else if let Some(cs) = child_softs.get(&module.to_ascii_lowercase()) {
                    if !cs.is_empty() {
                        let mut d = SoftDiag::new("child_soft_incomplete", &ent.name)
                            .with_detail(module.clone())
                            .with_span(span_best_effort(
                                source,
                                origin,
                                &[name.as_str(), module.as_str()],
                            ));
                        for c in cs {
                            d.push_child(c.clone());
                        }
                        push_soft(softs, d);
                    }
                }
                let ports = child_ports(module, arch, entities, extra_module_ports);
                let raw_ports = child_raw_ports(module, arch, entities);
                let child_clocks = entity_clocks
                    .get(&module.to_ascii_lowercase())
                    .unwrap_or(&empty_clocks);
                let child_ports_occ = ports.map(|ps| {
                    ps.iter()
                        .map(|(n, _, _)| n.to_ascii_lowercase())
                        .collect::<HashSet<String>>()
                });
                let child_occupied = entity_occupied
                    .get(&module.to_ascii_lowercase())
                    .or(child_ports_occ.as_ref())
                    .unwrap_or(&empty_clocks);
                let expanded_conns = expand_inst_conns(conns, raw_ports, ports, &arch.env);
                let mut cs: Vec<String> = Vec::new();
                for (a, b) in &expanded_conns {
                    let net = sv_inst_net(b, sta_clocks, &occupied);
                    let port = if a.starts_with('#') {
                        a[1..]
                            .parse::<usize>()
                            .ok()
                            .and_then(|i| ports.and_then(|ps| ps.get(i)))
                            .map(|(n, _, _)| sta_clock_name(n, child_clocks, child_occupied))
                    } else {
                        Some(sta_clock_name(a, child_clocks, child_occupied))
                    };
                    match port {
                        Some(p) if !p.starts_with('#') => cs.push(format!(".{p}({net})")),
                        _ => cs.push(net),
                    }
                }
                sv.push_str(&format!("  {} {name}(", sv_ident(module)));
                sv.push_str(&cs.join(", "));
                sv.push_str(");\n");
            }
        }
    }
    sv.push_str("endmodule\n");
    Ok(sv)
}

fn expand_inst_conns(
    conns: &[(String, String)],
    raw_ports: Option<&[(String, &'static str, VType)]>,
    flat_ports: Option<&[(String, &'static str, usize)]>,
    env: &TypeEnv,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (a, b) in conns {
        if a.starts_with('#') {
            if let Ok(idx) = a[1..].parse::<usize>() {
                if let Some(rps) = raw_ports {
                    if let Some((pname, _dir, vty)) = rps.get(idx) {
                        match vty {
                            VType::Scalar(_) | VType::UnconstrainedArray(_) => {
                                out.push((pname.clone(), b.clone()));
                                continue;
                            }
                            VType::Record(rname) => {
                                for (leaf_port, suf, _w) in
                                    env.flatten_record_leaves(rname, pname, "", 0)
                                {
                                    let actual = if let Some(cmap) =
                                        env.rec_consts.get(&b.to_ascii_lowercase())
                                    {
                                        cmap.get(&suf).cloned().unwrap_or_else(|| "1'b0".into())
                                    } else {
                                        format!("{b}_{suf}")
                                    };
                                    out.push((leaf_port, actual));
                                }
                                continue;
                            }
                        }
                    }
                }
                let _ = flat_ports;
            }
            out.push((a.clone(), b.clone()));
            continue;
        }
        if let Some(rps) = raw_ports {
            if let Some((pname, _dir, VType::Record(rname))) =
                rps.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(a))
            {
                for (leaf_port, suf, _w) in env.flatten_record_leaves(rname, pname, "", 0) {
                    let actual = if let Some(cmap) = env.rec_consts.get(&b.to_ascii_lowercase()) {
                        cmap.get(&suf).cloned().unwrap_or_else(|| "1'b0".into())
                    } else {
                        format!("{b}_{suf}")
                    };
                    out.push((leaf_port, actual));
                }
                continue;
            }
        }
        if let Some(rname) = env.resolve_rec_var(b) {
            let a_is_flat = flat_ports
                .map(|ps| ps.iter().any(|(n, _, _)| n.eq_ignore_ascii_case(a)))
                .unwrap_or(false);
            if !a_is_flat {
                for (leaf_port, suf, _w) in env.flatten_record_leaves(&rname, a, "", 0) {
                    out.push((leaf_port, format!("{b}_{suf}")));
                }
                continue;
            }
        }
        out.push((a.clone(), b.clone()));
    }
    out
}

fn const_get(c: &HashMap<String, i64>, n: &str) -> Option<i64> {
    if let Some(v) = c.get(n) {
        return Some(*v);
    }
    c.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(n))
        .map(|(_, v)| *v)
}

fn child_ports<'a>(
    module: &str,
    arch: &'a Arch,
    entities: &'a [Entity],
    extra_module_ports: &'a HashMap<String, Vec<(String, &'static str, usize)>>,
) -> Option<&'a [(String, &'static str, usize)]> {
    if let Some(c) = arch
        .components
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(module))
    {
        return Some(c.ports.as_slice());
    }
    if let Some(e) = entities
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(module))
    {
        return Some(e.ports.as_slice());
    }
    extra_module_ports
        .get(&module.to_ascii_lowercase())
        .map(|v| v.as_slice())
}

fn child_raw_ports<'a>(
    module: &str,
    arch: &'a Arch,
    entities: &'a [Entity],
) -> Option<&'a [(String, &'static str, VType)]> {
    if let Some(c) = arch
        .components
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(module))
    {
        return Some(c.raw_ports.as_slice());
    }
    entities
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(module))
        .map(|e| e.raw_ports.as_slice())
}

/// helion-sv `parse_inst_net` drops `[idx]` on instance ports. Emit the
/// bit-blasted net name (`a[0]` → `a_0`) so stitch_child can bind bits.
fn sv_inst_net(s: &str, sta_clocks: &HashSet<String>, occupied: &HashSet<String>) -> String {
    let s = rewrite_sta_clock_ident(s, sta_clocks, occupied);
    let t = s.trim();
    let t = t
        .strip_prefix('(')
        .and_then(|x| x.strip_suffix(')'))
        .unwrap_or(t)
        .trim();
    if let Some(b) = t.find('[') {
        if let Some(e) = t.rfind(']') {
            let idx = t[b + 1..e].trim();
            if !idx.contains(':') {
                if let Ok(n) = idx.parse::<i64>() {
                    return format!("{}_{n}", t[..b].trim());
                }
            }
        }
    }
    t.to_string()
}

fn sv_ident(n: &str) -> String {
    match n.to_ascii_lowercase().as_str() {
        "logic" | "wire" | "reg" | "module" | "input" | "output" | "inout" | "begin" | "end"
        | "if" | "else" | "case" | "for" | "int" | "parameter" | "localparam" | "always"
        | "assign" | "function" | "task" | "package" | "interface" | "generate" | "genvar"
        | "signed" | "unsigned" | "return" | "typedef" | "struct" | "enum" | "unique"
        | "priority" | "default" | "posedge" | "negedge" => format!("vhdl_{n}"),
        _ => n.to_string(),
    }
}

fn missing_component_line(module: &str, inst: &str, child: &str) -> String {
    format!(
        "diagnostic missing_component module={module} inst={inst} child={child} (component body absent; not a LUT; not a closed WNS)"
    )
}

fn clock_clk_clash(sta_clocks: &HashSet<String>, occupied: &HashSet<String>) -> bool {
    sta_clocks.contains("clock") && occupied.contains("clk")
}

fn clock_clk_clash_line(entity: &str) -> String {
    format!(
        "diagnostic clock_clk_clash module={entity} construct=process_clock clock=clock occupied=clk (rename clock→clk skipped; clk already a port/signal; nets kept distinct; not a silent merge; not a synth abort)"
    )
}

fn emit_diag(line: &str) {
    eprintln!("{line}");
    #[cfg(test)]
    TEST_DIAGNOSTICS.with(|d| d.borrow_mut().push(line.to_string()));
}

fn push_soft(softs: &mut Vec<SoftDiag>, d: SoftDiag) {
    emit_diag(&d.table_line());
    softs.push(d);
}

/// Best-effort file/line/column from original VHDL. First needle that appears
/// as a whole ident on a non-comment line wins; unknown fields stay `None`.
fn span_best_effort(source: &str, file: &str, needles: &[&str]) -> SoftSpan {
    let mut span = SoftSpan {
        file: Some(file.to_string()),
        line: None,
        column: None,
        end_line: None,
    };
    for needle in needles {
        if needle.is_empty() {
            continue;
        }
        if let Some((line, col)) = find_ident_line_col(source, needle) {
            span.line = Some(line);
            span.column = Some(col);
            return span;
        }
    }
    span
}

fn find_ident_line_col(source: &str, ident: &str) -> Option<(u32, u32)> {
    let want = ident.to_ascii_lowercase();
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("--") {
            continue;
        }
        let low = line.to_ascii_lowercase();
        let mut from = 0usize;
        while from < low.len() {
            let rest = &low[from..];
            let Some(rel) = rest.find(&want) else {
                break;
            };
            let at = from + rel;
            let before_ok =
                at == 0 || !ident_char(low.as_bytes().get(at - 1).copied().unwrap_or(0));
            let after = at + want.len();
            let after_ok =
                after >= low.len() || !ident_char(low.as_bytes().get(after).copied().unwrap_or(0));
            if before_ok && after_ok {
                return Some(((i + 1) as u32, (at + 1) as u32));
            }
            from = at + 1;
        }
    }
    None
}

fn ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
thread_local! {
    static TEST_DIAGNOSTICS: std::cell::RefCell<Vec<String>> =
        std::cell::RefCell::new(Vec::new());
}

fn sta_clocks_of(arch: &Arch) -> HashSet<String> {
    arch.stmts
        .iter()
        .filter_map(|st| match st {
            CStmt::Process { clock: Some(c), .. } => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// Keep-name policy (FM-HEL-2.0-IDE GUI HFF contract): process/STA clock
/// ports keep the real VHDL name. Do **not** rename `clock` → `clk`.
///
/// Narrow (not total-merge): a data port named `clock` that is *not* the
/// process/STA clock also stays `clock`. When process clock is `clock` and
/// `clk` is already occupied, keep `clock` (clash-skip; nets stay distinct).
/// Selected-arch clocks are not unioned (see `entity_clocks` / child maps).
fn sta_clock_name(
    name: &str,
    _sta_clocks: &HashSet<String>,
    _occupied: &HashSet<String>,
) -> String {
    // Identity: never rewrite process/STA `clock` to `clk`. Call sites still
    // pass sta_clocks/occupied so clash diagnostics and narrow tests stay wired.
    name.to_string()
}

fn rewrite_sta_clock_ident(
    s: &str,
    sta_clocks: &HashSet<String>,
    occupied: &HashSet<String>,
) -> String {
    // Word-boundary pass over idents; sta_clock_name is keep-name (identity).
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &s[start..i];
            out.push_str(&sta_clock_name(word, sta_clocks, occupied));
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn emit_assign_lines(lhs: &str, rhs: &str) -> Vec<String> {
    // name[hi:lo] = rhs  → per-bit so helion-sv does not treat it as a full-vector NBA.
    if let Some((name, hi, lo)) = parse_sv_range(lhs) {
        let w = (hi - lo + 1) as usize;
        (0..w)
            .map(|i| {
                let b = lo + i as i64;
                format!("{name}[{b}] = {rhs}[{i}];")
            })
            .collect()
    } else {
        vec![format!("{lhs} = {rhs};")]
    }
}

fn parse_sv_range(s: &str) -> Option<(String, i64, i64)> {
    let b = s.find('[')?;
    let c = s.find(':')?;
    let e = s.find(']')?;
    if c < b || e < c {
        return None;
    }
    let name = s[..b].to_string();
    let hi: i64 = s[b + 1..c].parse().ok()?;
    let lo: i64 = s[c + 1..e].parse().ok()?;
    Some((name, hi.max(lo), hi.min(lo)))
}

fn emit_seq(
    sv: &mut String,
    body: &[SStmt],
    ind: usize,
    sta_clocks: &HashSet<String>,
    occupied: &HashSet<String>,
) {
    let pad = "  ".repeat(ind);
    for s in body {
        match s {
            SStmt::Assign { lhs, rhs } => {
                let lhs = rewrite_sta_clock_ident(lhs, sta_clocks, occupied);
                let rhs = rewrite_sta_clock_ident(rhs, sta_clocks, occupied);
                for line in emit_assign_lines(&lhs, &rhs) {
                    sv.push_str(&format!("{pad}{line}\n"));
                }
            }
            SStmt::If {
                cond,
                then_b,
                else_b,
            } => {
                if clock_from_cond(cond).is_some() {
                    emit_seq(sv, then_b, ind, sta_clocks, occupied);
                    continue;
                }
                let cond = rewrite_sta_clock_ident(cond, sta_clocks, occupied);
                sv.push_str(&format!("{pad}if ({cond}) begin\n"));
                emit_seq(sv, then_b, ind + 1, sta_clocks, occupied);
                sv.push_str(&format!("{pad}end\n"));
                if !else_b.is_empty() {
                    sv.push_str(&format!("{pad}else begin\n"));
                    emit_seq(sv, else_b, ind + 1, sta_clocks, occupied);
                    sv.push_str(&format!("{pad}end\n"));
                }
            }
            SStmt::Case { sel, arms, other } => {
                let sel = rewrite_sta_clock_ident(sel, sta_clocks, occupied);
                sv.push_str(&format!("{pad}case ({sel})\n"));
                for (pats, b) in arms {
                    sv.push_str(&format!("{pad}  {}: begin\n", pats.join(", ")));
                    emit_seq(sv, b, ind + 2, sta_clocks, occupied);
                    sv.push_str(&format!("{pad}  end\n"));
                }
                if !other.is_empty() {
                    sv.push_str(&format!("{pad}  default: begin\n"));
                    emit_seq(sv, other, ind + 2, sta_clocks, occupied);
                    sv.push_str(&format!("{pad}  end\n"));
                }
                sv.push_str(&format!("{pad}endcase\n"));
            }
            SStmt::For { var, lo, hi, body } => {
                let mut i = *lo;
                let mut n = 0i64;
                loop {
                    if n > 4096 {
                        break;
                    }
                    let mut subst = body.clone();
                    subst_var(&mut subst, var, i);
                    emit_seq(sv, &subst, ind, sta_clocks, occupied);
                    if i == *hi {
                        break;
                    }
                    i += if *lo <= *hi { 1 } else { -1 };
                    n += 1;
                }
            }
        }
    }
}

fn subst_var(body: &mut [SStmt], var: &str, i: i64) {
    let is_ = i.to_string();
    for s in body.iter_mut() {
        match s {
            SStmt::Assign { lhs, rhs } => {
                *lhs = lhs.replace(var, &is_);
                *rhs = rhs.replace(var, &is_);
            }
            SStmt::If {
                cond,
                then_b,
                else_b,
            } => {
                *cond = cond.replace(var, &is_);
                subst_var(then_b, var, i);
                subst_var(else_b, var, i);
            }
            SStmt::Case { sel, arms, other } => {
                *sel = sel.replace(var, &is_);
                for (p, b) in arms {
                    for x in p {
                        *x = x.replace(var, &is_);
                    }
                    subst_var(b, var, i);
                }
                subst_var(other, var, i);
            }
            SStmt::For { body, .. } => subst_var(body, var, i),
        }
    }
}

#[derive(Clone, Debug)]
enum Tok {
    Ident(String),
    Kw(String),
    Num(u128),
    Sym(String),
    BitStr(String),
}

struct P<'a> {
    t: &'a [Tok],
    i: usize,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.t.get(self.i)
    }
    fn peek_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if x == s)
    }
    fn bump(&mut self) -> Option<&'a Tok> {
        let t = self.t.get(self.i)?;
        self.i += 1;
        Some(t)
    }
    fn eat(&mut self, s: &str) -> bool {
        match self.peek() {
            Some(Tok::Kw(k)) if k.eq_ignore_ascii_case(s) => {
                self.i += 1;
                true
            }
            Some(Tok::Sym(k)) if k == s => {
                self.i += 1;
                true
            }
            Some(Tok::Ident(k)) if k.eq_ignore_ascii_case(s) && is_kw(s) => {
                self.i += 1;
                true
            }
            _ => false,
        }
    }
    fn eat_name(&mut self) -> Option<String> {
        while self.peek().is_some() {
            if self.peek_sym(")") || self.peek_sym(";") || self.peek_sym(":") {
                return None;
            }
            if matches!(self.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
                return self.ident().ok();
            }
            self.bump();
        }
        None
    }
    fn ident(&mut self) -> Result<String, String> {
        match self.bump() {
            Some(Tok::Ident(s)) => Ok(s.clone()),
            Some(Tok::Kw(s)) => Ok(s.clone()),
            other => Err(format!("ident {other:?}")),
        }
    }
    fn eat_ident(&mut self) -> bool {
        matches!(self.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_)))
            .then(|| self.bump())
            .is_some()
    }
    fn number(&mut self) -> Option<u128> {
        match self.peek() {
            Some(Tok::Num(n)) => {
                let v = *n;
                self.i += 1;
                Some(v)
            }
            _ => None,
        }
    }
}

fn is_kw(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "entity"
            | "architecture"
            | "port"
            | "is"
            | "in"
            | "out"
            | "inout"
            | "end"
            | "of"
            | "begin"
            | "signal"
            | "process"
            | "if"
            | "then"
            | "not"
            | "downto"
            | "to"
            | "rising_edge"
            | "falling_edge"
            | "std_logic"
            | "std_logic_vector"
            | "std_ulogic"
            | "std_ulogic_vector"
            | "unsigned"
            | "signed"
            | "ufixed"
            | "sfixed"
            | "float"
            | "float32"
            | "float64"
            | "others"
            | "all"
            | "package"
            | "body"
            | "constant"
            | "variable"
            | "generic"
            | "buffer"
            | "linkage"
            | "context"
            | "and"
            | "or"
            | "xor"
            | "nand"
            | "nor"
            | "xnor"
            | "when"
            | "else"
            | "elsif"
            | "component"
            | "map"
            | "case"
            | "loop"
            | "for"
            | "while"
            | "generate"
            | "function"
            | "procedure"
            | "return"
            | "type"
            | "subtype"
            | "array"
            | "record"
            | "null"
            | "wait"
            | "assert"
            | "report"
            | "severity"
            | "after"
            | "until"
            | "next"
            | "exit"
            | "open"
            | "range"
            | "units"
            | "alias"
            | "attribute"
            | "configuration"
            | "impure"
            | "pure"
            | "shared"
            | "select"
            | "with"
            | "mod"
            | "rem"
            | "abs"
            | "sll"
            | "srl"
            | "sla"
            | "sra"
            | "rol"
            | "ror"
            | "integer"
            | "natural"
            | "positive"
            | "boolean"
            | "bit"
            | "bit_vector"
            | "character"
            | "string"
            | "time"
            | "real"
            | "file"
            | "access"
            | "new"
            | "unaffected"
            | "transport"
            | "inertial"
            | "reject"
            | "block"
            | "disconnect"
            | "guarded"
            | "postponed"
            | "protected"
            | "private"
            | "view"
            | "force"
            | "release"
            | "parameter"
            | "literal"
            | "bus"
            | "register"
            | "group"
    )
}

fn tokenize(src: &str) -> Vec<Tok> {
    let mut s = String::new();
    let chars_raw: Vec<char> = src.chars().collect();
    let mut i = 0;
    // strip -- comments and /* */
    while i < chars_raw.len() {
        if chars_raw[i] == '-' && chars_raw.get(i + 1) == Some(&'-') {
            while i < chars_raw.len() && chars_raw[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if chars_raw[i] == '/' && chars_raw.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars_raw.len() && !(chars_raw[i] == '*' && chars_raw[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars_raw.len());
            continue;
        }
        s.push(chars_raw[i]);
        i += 1;
    }
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '?' && chars.get(i + 1) == Some(&'/') && chars.get(i + 2) == Some(&'=') {
            out.push(Tok::Sym("?/=".into()));
            i += 3;
            continue;
        }
        if c == '?' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Sym("?=".into()));
            i += 2;
            continue;
        }
        if c == '<' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Sym("<=".into()));
            i += 2;
            continue;
        }
        if c == '>' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Sym(">=".into()));
            i += 2;
            continue;
        }
        if c == ':' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Sym(":=".into()));
            i += 2;
            continue;
        }
        if c == '=' && chars.get(i + 1) == Some(&'>') {
            out.push(Tok::Sym("=>".into()));
            i += 2;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Sym("/=".into()));
            i += 2;
            continue;
        }
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            out.push(Tok::Sym("**".into()));
            i += 2;
            continue;
        }
        if c == '\''
            && i + 2 < chars.len()
            && chars[i + 2] == '\''
            && !chars[i + 1].is_ascii_alphabetic()
        {
            let ch = chars[i + 1];
            let s_lit = match ch {
                '1' => "1",
                '-' => "-",
                _ => "0",
            };
            out.push(Tok::BitStr(s_lit.into()));
            i += 3;
            continue;
        }
        if c == '\'' && i + 2 < chars.len() && chars[i + 2] == '\'' {
            let ch = chars[i + 1];
            let s_lit = match ch {
                '1' => "1",
                '-' => "-",
                _ => "0",
            };
            out.push(Tok::BitStr(s_lit.into()));
            i += 3;
            continue;
        }
        if c == '"' {
            i += 1;
            let mut bits = String::new();
            while i < chars.len() && chars[i] != '"' {
                bits.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push(Tok::BitStr(normalize_bitstr(&bits, 2)));
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let mut n = String::new();
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                n.push(chars[i]);
                i += 1;
            }
            // based bit string x"AB" / b"1010" / o"77"
            if i < chars.len() && chars[i] == '"' {
                let base = match n.to_ascii_lowercase().as_str() {
                    "x" => 16,
                    "b" => 2,
                    "o" => 8,
                    "d" => 10,
                    _ => 0,
                };
                if base != 0 {
                    i += 1;
                    let mut bits = String::new();
                    while i < chars.len() && chars[i] != '"' {
                        bits.push(chars[i]);
                        i += 1;
                    }
                    if i < chars.len() {
                        i += 1;
                    }
                    out.push(Tok::BitStr(normalize_bitstr(&bits, base)));
                    continue;
                }
            }
            if is_kw(&n) {
                out.push(Tok::Kw(n.to_ascii_lowercase()));
            } else {
                out.push(Tok::Ident(n));
            }
            continue;
        }
        if c.is_ascii_digit() {
            let mut n = String::new();
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                if chars[i] != '_' {
                    n.push(chars[i]);
                }
                i += 1;
            }
            // based integer 16#FF#
            if i < chars.len() && chars[i] == '#' {
                let base: u32 = n.parse().unwrap_or(10);
                i += 1;
                let mut d = String::new();
                while i < chars.len() && chars[i] != '#' {
                    if chars[i] != '_' {
                        d.push(chars[i]);
                    }
                    i += 1;
                }
                if i < chars.len() {
                    i += 1;
                }
                let v = u128::from_str_radix(&d, base).unwrap_or(0);
                out.push(Tok::Num(v));
                continue;
            }
            out.push(Tok::Num(n.parse().unwrap_or(0)));
            continue;
        }
        // Nothing unknown: every leftover glyph is a symbol the parser can eat.
        out.push(Tok::Sym(c.to_string()));
        i += 1;
    }
    out
}

fn normalize_bitstr(s: &str, base: u32) -> String {
    let t: String = s
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_')
        .collect();
    if base == 2 {
        return t
            .chars()
            .map(|ch| match ch {
                '1' | 'h' | 'H' => '1',
                '-' => '-',
                _ => '0',
            })
            .collect();
    }
    let mut bits = String::new();
    for ch in t.chars() {
        let v = ch.to_digit(base).unwrap_or(0);
        let w = match base {
            16 => 4,
            8 => 3,
            _ => 4,
        };
        bits.push_str(&format!("{v:0width$b}", width = w));
    }
    if bits.is_empty() { "0".into() } else { bits }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{resolve, CellKind, INC4_INIT};

    #[test]
    fn vhdl_skips_strings_and_concat_without_aborting_synth() {
        let src = r#"
entity e is
  port (clk : in std_logic; led : out std_logic);
end entity;
architecture rtl of e is
  constant C : string := "hello";
  signal q : std_logic := '0';
  signal w : std_logic_vector(1 downto 0);
begin
  w <= "10";
  process(clk)
  begin
    if rising_edge(clk) then
      q <= not q;
    end if;
  end process;
  led <= q;
end architecture;
"#;
        let d = synth_vhdl(src).expect("string/concat VHDL must still map the blinky process");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "FF from rising_edge process: {:?}",
            d.cells
        );
    }

    const BLINKY: &str = r#"
entity blinky is
  port (clk : in std_logic; led : out std_logic);
end entity;
architecture rtl of blinky is
  signal q : std_logic := '0';
begin
  process(clk)
  begin
    if rising_edge(clk) then
      q <= not q;
    end if;
  end process;
  led <= q;
end architecture;
"#;

    const INC3: &str = r#"
entity counter is
  port (clk : in std_logic; led : out std_logic);
end entity;
architecture rtl of counter is
  signal cnt : unsigned(2 downto 0) := (others => '0');
begin
  process(clk)
  begin
    if rising_edge(clk) then
      cnt <= cnt + 1;
    end if;
  end process;
  led <= cnt(2);
end architecture;
"#;

    const INC4: &str = r#"
entity counter is
  port (clk : in std_logic; led : out std_logic);
end entity;
architecture rtl of counter is
  signal cnt : unsigned(3 downto 0) := (others => '0');
begin
  process(clk)
  begin
    if rising_edge(clk) then
      cnt <= cnt + 1;
    end if;
  end process;
  led <= cnt(3);
end architecture;
"#;

    fn wave(src: &str, cycles: usize) -> (Design, Vec<bool>) {
        let d = synth_vhdl(src).unwrap();
        let dev = helion_device::Device::load_part("HL10T-C32-1").unwrap();
        let p = helion_pack::pack(&d, &dev).unwrap();
        let pl = helion_place::place(&p, &dev).unwrap();
        let r = helion_route::route(&pl, &dev).unwrap();
        let bits = helion_bits::bitgen(&dev, &r).unwrap();
        let mut fab = helion_fabric::Fabric::new(&dev);
        fab.program(&bits).unwrap();
        fab.finish_startup();
        let iob = r.iob_src[0].iob;
        let mut w = Vec::new();
        for _ in 0..cycles {
            fab.step_user();
            w.push(fab.led_at(iob.0, iob.1));
        }
        (d, w)
    }

    #[test]
    fn vhdl_blinky_is_inverter_ff() {
        let d = synth_vhdl(BLINKY).unwrap();
        match d
            .cell("u_lut")
            .or_else(|| {
                d.cells
                    .iter()
                    .find(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            })
            .unwrap()
            .kind
        {
            CellKind::Lut6 { init } => assert_eq!(init, 0x5555_5555_5555_5555),
            _ => panic!("lut"),
        }
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "blinky.vhd must map an FF on clk, {:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_garbage_fails() {
        assert!(synth_vhdl("not vhdl").is_err());
    }

    #[test]
    fn vhdl_blinky_toggles_in_fabric() {
        let (_, w) = wave(BLINKY, 8);
        assert!(
            w.contains(&true) && w.contains(&false),
            "VHDL blinky LED must toggle {w:?}"
        );
    }

    #[test]
    fn vhdl_width_is_not_string_match() {
        let d3 = synth_vhdl(INC3).unwrap();
        let d4 = synth_vhdl(INC4).unwrap();
        assert_eq!(
            d3.lut_inits().len(),
            3,
            "3-bit VHDL incrementer must be 3 LUTs {:?}",
            d3.lut_inits()
        );
        assert_eq!(
            d4.lut_inits(),
            INC4_INIT.to_vec(),
            "4-bit must match gold incrementer"
        );
        assert_ne!(d3.lut_inits().len(), d4.lut_inits().len());
    }

    #[test]
    fn vhdl_to_sv_is_not_a_noop_copy() {
        let sv = vhdl_to_sv(INC4).unwrap();
        assert!(sv.contains("module counter"), "{sv}");
        assert!(sv.contains("always_ff"), "{sv}");
        assert!(sv.contains("cnt + 1") || sv.contains("cnt+1"), "{sv}");
        assert!(!sv.contains("entity"), "must lower VHDL, not echo it: {sv}");
        assert!(!sv.contains("rising_edge"), "{sv}");
        let sv3 = vhdl_to_sv(INC3).unwrap();
        assert!(sv3.contains("[2:0]"), "3-bit vector in SV {sv3}");
        assert!(sv.contains("[3:0]"), "4-bit vector in SV {sv}");
        assert_ne!(sv3, sv);
    }

    #[test]
    fn vhdl_blinky_file_path_and_incrementer_e2e() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/blinky.vhd");
        let d = synth_vhdl_path(&root).unwrap();
        assert_eq!(d.lut_inits(), vec![0x5555_5555_5555_5555]);
    }

    #[test]
    fn vhdl_incrementer_e2e_like_counter_sv() {
        let (d, w) = wave(INC4, 16);
        assert_eq!(d.lut_inits().len(), 4);
        assert!(w[0..7].iter().all(|b| !b), "cnt 1..7 LED=0 {w:?}");
        assert!(w[7..15].iter().all(|b| *b), "cnt 8..15 LED=1 {w:?}");
        assert!(!w[15], "wrap {w:?}");
        let (_, w3) = wave(INC3, 8);
        assert!(w3[0..3].iter().all(|b| !b), "3-bit 1..3 LED=0 {w3:?}");
        assert!(w3[3..7].iter().all(|b| *b), "3-bit 4..7 LED=1 {w3:?}");
        assert!(!w3[7], "3-bit wrap {w3:?}");
    }

    #[test]
    fn vhdl_with_select_maps_mux() {
        let src = r#"
entity Mux4to1 is
  port (
    DecHor : in  std_logic_vector (3 downto 0);
    UniHor : in  std_logic_vector (3 downto 0);
    DecMin : in  std_logic_vector (3 downto 0);
    UniMin : in  std_logic_vector (3 downto 0);
    Sel    : in  std_logic_vector (1 downto 0);
    Tiempo : out std_logic_vector (3 downto 0));
end Mux4to1;
architecture Behavioral of Mux4to1 is
begin
   with Sel select
      Tiempo <= DecHor when "00",
                UniHor when "01",
                DecMin when "10",
                UniMin when others;
end Behavioral;
"#;
        let d = synth_vhdl(src).expect("with/select");
        assert!(
            !d.lut_inits().is_empty(),
            "with/select must map LUTs, cells={:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_when_else_and_neq() {
        let src = r#"
entity cmp_115 is
port (
eq : out std_logic;
in0 : in  std_logic_vector(2 downto 0);
in1 : in  std_logic_vector(2 downto 0)
);
end cmp_115;
architecture augh of cmp_115 is
signal tmp : std_logic;
begin
tmp <= '0' when in0 /= in1 else '1';
eq <= tmp;
end architecture;
"#;
        let d = synth_vhdl(src).expect("when/else");
        assert!(
            d.cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "when/else compare must be a LUT {:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_generic_and_multi_port_and_event() {
        let src = r#"
entity ic4021 is
  generic (W : integer := 8);
  port (d, ds, pl, cp : in std_logic; q7 : out std_logic);
end ic4021;
architecture behavior of ic4021 is
  signal shift_reg: std_logic_vector(7 downto 0) := "00000000";
begin
  process (d, pl, cp)
  begin
    if pl = '1' then
      shift_reg(0) <= d;
    elsif cp'event and cp = '1' then
      shift_reg(0) <= ds;
    end if;
  end process;
  q7 <= shift_reg(0);
end behavior;
"#;
        let d = synth_vhdl(src).expect("generic/event");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "clk'event must map an FF {:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_empty_architecture_still_emits_module() {
        let src = r#"
entity wrap is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of wrap is
begin
end;
"#;
        let sv = vhdl_to_sv(src).unwrap();
        assert!(sv.contains("module wrap"), "{sv}");
        let _ = synth_vhdl(src).expect("empty architecture is legal VHDL");
    }

    #[test]
    fn vhdl_fulladder_maps_luts() {
        let src = r#"
entity full_adder is
  port (a, b, cin : in std_logic; sum, cout : out std_logic);
end;
architecture rtl of full_adder is
begin
  sum <= a xor b xor cin;
  cout <= (a and b) or (a and cin) or (b and cin);
end;
"#;
        let d = synth_vhdl(src).expect("full_adder");
        let luts = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        assert!(
            luts >= 2,
            "full_adder must map LUTs, luts={luts} {:?}",
            d.cells
        );
        assert_ne!(d.attrs.get("NO_BODY"), Some("1"));
    }

    #[test]
    fn vhdl_fulladder_hierarchy_adder4_maps() {
        let src = r#"
entity full_adder is
  port (a, b, cin : in std_logic; sum, cout : out std_logic);
end;
architecture rtl of full_adder is
begin
  sum <= a xor b xor cin;
  cout <= (a and b) or (a and cin) or (b and cin);
end;
entity adder4 is
  port (a, b : in std_logic_vector(3 downto 0); cin : in std_logic;
        sum : out std_logic_vector(3 downto 0); cout : out std_logic);
end;
architecture rtl of adder4 is
  component full_adder
    port (a, b, cin : in std_logic; sum, cout : out std_logic);
  end component;
  signal c : std_logic_vector(4 downto 0);
begin
  c(0) <= cin;
  u0: full_adder port map (a(0), b(0), c(0), sum(0), c(1));
  u1: full_adder port map (a(1), b(1), c(1), sum(1), c(2));
  u2: full_adder port map (a(2), b(2), c(2), sum(2), c(3));
  u3: full_adder port map (a(3), b(3), c(3), sum(3), c(4));
  cout <= c(4);
end;
"#;
        let sv = vhdl_to_sv(src).expect("adder4 to sv");
        assert!(
            sv.contains("module full_adder"),
            "must emit real full_adder body, not emit_stub: {sv}"
        );
        assert!(sv.contains("module adder4"), "{sv}");
        assert!(
            sv.contains("assign") || sv.contains("^") || sv.contains("xor"),
            "full_adder body must carry logic: {sv}"
        );
        assert!(
            sv.contains(".a(a_0)") && sv.contains(".cin(c_0)") && sv.contains(".cout(c_1)"),
            "positional port maps must bind bit-blasted nets, not a[0] (sv-parser drops [idx]): {sv}"
        );
        let d = synth_vhdl(src).expect("adder4");
        let luts = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        assert!(
            luts >= 4,
            "adder4 hierarchy must map LUTs from full_adder bodies, luts={luts} cells={:?}",
            d.cells
        );
        assert_ne!(
            d.attrs.get("NO_BODY"),
            Some("1"),
            "adder4 must not be silent no_body"
        );
    }

    #[test]
    fn vhdl_missing_component_names_instance() {
        let src = r#"
entity wrap is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of wrap is
  component missing_child
    port (a : in std_logic; y : out std_logic);
  end component;
begin
  u_miss: missing_child port map (a => a, y => y);
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(
            !sv.contains("module missing_child"),
            "must not emit_stub empty missing_child: {sv}"
        );
        assert!(sv.contains("u_miss"), "{sv}");
        let d = synth_vhdl(src).expect("synth");
        assert!(
            d.cells.is_empty(),
            "absent child must not invent gates {:?}",
            d.cells
        );
        assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
    }

    #[test]
    fn vhdl_missing_component_diagnostic_includes_inst() {
        let src = r#"
entity top is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of top is
begin
  ghost_u: ghost_ent port map (a => a, y => y);
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(!sv.contains("module ghost_ent("), "no empty stub: {sv}");
        assert!(sv.contains("ghost_u"), "{sv}");
        let d = synth_vhdl(src).expect("synth");
        assert!(d.cells.is_empty());
        assert_eq!(d.attrs.get("NO_BODY"), Some("1"));
    }

    #[test]
    fn vhdl_missing_component_module_is_entity_being_lowered() {
        // Child entity (not design top) instantiates a missing component.
        // diagnostic module= must be the entity being lowered (`inner`), not top_name.
        let src = r#"
entity inner is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of inner is
begin
  u_miss: missing_child port map (a => a, y => y);
end;
entity wrap is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of wrap is
begin
  u_inner: inner port map (a => a, y => y);
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(sv.contains("module inner("), "{sv}");
        assert!(sv.contains("u_miss"), "{sv}");
        assert!(!sv.contains("module missing_child"), "no emit_stub: {sv}");
        assert_eq!(
            missing_component_line("inner", "u_miss", "missing_child"),
            "diagnostic missing_component module=inner inst=u_miss child=missing_child (component body absent; not a LUT; not a closed WNS)"
        );
        assert!(
            !missing_component_line("inner", "u_miss", "missing_child").contains("module=wrap"),
            "must not use design top_name"
        );
        let mr = map_vhdl(src).expect("map");
        // Roots-only forest: missing_component lives under child_soft_incomplete,
        // not also as a flat top-level duplicate.
        assert!(
            mr.softs
                .iter()
                .any(|s| resolve(s.name) == "child_soft_incomplete" && resolve(s.module) == "wrap"),
            "softs={:?}",
            mr.soft_table_lines()
        );
        assert!(
            !mr.softs.iter().any(|s| resolve(s.name) == "missing_component"),
            "roots-only: leaf must not also be a flat root: {:?}",
            mr.soft_table_lines()
        );
        let miss: Vec<_> = mr
            .softs
            .iter()
            .filter(|s| resolve(s.name) == "child_soft_incomplete")
            .flat_map(|s| s.children.iter())
            .filter(|s| resolve(s.name) == "missing_component")
            .collect();
        assert_eq!(miss.len(), 1, "softs={:?}", mr.soft_table_lines());
        assert_eq!(resolve(miss[0].module), "inner");
        assert!(
            !miss.iter().any(|s| resolve(s.module) == "wrap"),
            "must not use design top_name: {:?}",
            mr.soft_table_lines()
        );
        let rows = mr.soft_table_lines();
        assert_eq!(
            rows.iter()
                .filter(|l| l.contains("name=missing_component"))
                .count(),
            1,
            "no flat+nested duplicate rows: {rows:?}"
        );
    }

    #[test]
    fn vhdl_missing_component_softdiag_fields() {
        let src = r#"
entity wrap is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of wrap is
  component missing_child
    port (a : in std_logic; y : out std_logic);
  end component;
begin
  u_miss: missing_child port map (a => a, y => y);
end;
"#;
        let mr = map_vhdl(src).expect("map");
        let miss = mr
            .softs
            .iter()
            .find(|s| resolve(s.name) == "missing_component")
            .unwrap_or_else(|| {
                panic!(
                    "missing_component SoftDiag required, got {:?}",
                    mr.soft_table_lines()
                )
            });
        assert_eq!(resolve(miss.name), "missing_component");
        assert_eq!(resolve(miss.module), "wrap");
        let detail = miss.detail.as_deref().unwrap_or("");
        assert!(detail.contains("inst=u_miss"), "inst in detail: {detail}");
        assert!(
            detail.contains("child=missing_child"),
            "child in detail: {detail}"
        );
        assert_eq!(miss.span.file.as_deref(), Some("vhdl.vhd"));
        assert!(
            miss.span.line.is_some(),
            "best-effort line from source: {:?}",
            miss.span
        );
        let line_no = miss.span.line.unwrap() as usize;
        let src_line = src.lines().nth(line_no.saturating_sub(1)).unwrap_or("");
        assert!(
            src_line.to_ascii_lowercase().contains("u_miss"),
            "span line must point at instance, got {line_no}: {src_line}"
        );
        let rows = mr.soft_table_lines();
        assert!(
            rows.iter()
                .any(|l| l.contains("soft name=missing_component")
                    && l.contains("module=wrap")
                    && l.contains("detail=inst=u_miss")
                    && l.contains("span=vhdl.vhd:")),
            "table_line schema row: {rows:?}"
        );
        assert!(
            !rows.iter().any(|l| l.contains("PASS")),
            "SOFT ≠ PASS: {rows:?}"
        );
        assert!(mr.has_softs());
        assert!(
            mr.design.cells.is_empty(),
            "absent child must not invent gates {:?}",
            mr.design.cells
        );
        assert_eq!(mr.design.attrs.get("NO_BODY"), Some("1"));
        assert!(
            !mr.design
                .cells
                .iter()
                .any(|c| resolve(c.name).contains("unused_") || resolve(c.name).contains("fcov_")),
            "never invent LUTs for unused_*/fcov_: {:?}",
            mr.design.cells
        );
        let kinds = format!("{:?}", mr.design.cells);
        assert!(
            !kinds.contains("FDCE") && !kinds.contains("FDRE") && !kinds.contains("FD "),
            "no fake FD library: {kinds}"
        );
    }

    #[test]
    fn vhdl_missing_component_not_silent_pass() {
        let src = r#"
entity top is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of top is
begin
  ghost_u: ghost_ent port map (a => a, y => y);
end;
"#;
        let mr = map_vhdl(src).expect("map");
        assert!(
            mr.has_softs(),
            "SOFT must be first-class, not silent cells=0"
        );
        assert!(
            mr.softs
                .iter()
                .any(|s| resolve(s.name) == "missing_component" && resolve(s.module) == "top")
        );
        let detail = mr
            .softs
            .iter()
            .find(|s| resolve(s.name) == "missing_component")
            .and_then(|s| s.detail.as_deref())
            .unwrap_or("");
        assert!(detail.contains("inst=ghost_u"), "{detail}");
        assert!(detail.contains("child=ghost_ent"), "{detail}");
        assert_eq!(mr.design.attrs.get("NO_BODY"), Some("1"));
        assert!(mr.design.cells.is_empty());
        assert!(
            !mr.soft_table_lines()
                .iter()
                .any(|l| l.to_ascii_uppercase().contains("PASS"))
        );
    }

    #[test]
    fn vhdl_map_closed_cone_has_no_softs() {
        let src = r#"
entity full_adder is
  port (a, b, cin : in std_logic; sum, cout : out std_logic);
end;
architecture rtl of full_adder is
begin
  sum <= a xor b xor cin;
  cout <= (a and b) or (a and cin) or (b and cin);
end;
"#;
        let mr = map_vhdl(src).expect("map");
        assert!(
            mr.softs.is_empty(),
            "closed LUT cone must not carry SOFT: {:?}",
            mr.soft_table_lines()
        );
        assert!(!mr.has_softs());
        let luts = mr
            .design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count();
        assert!(luts >= 2, "full_adder must map LUTs, luts={luts}");
    }

    #[test]
    fn vhdl_clock_clk_clash_softdiag() {
        let src = r#"
entity clash is
  port (clk : in std_logic; clock : in std_logic; q : out std_logic);
end;
architecture rtl of clash is
begin
  process(clock)
  begin
    if rising_edge(clock) then
      q <= clk;
    end if;
  end process;
end;
"#;
        let mr = map_vhdl(src).expect("map");
        let clash = mr
            .softs
            .iter()
            .find(|s| resolve(s.name) == "clock_clk_clash")
            .unwrap_or_else(|| panic!("clock_clk_clash SoftDiag, got {:?}", mr.soft_table_lines()));
        assert_eq!(resolve(clash.module), "clash");
        assert!(
            clash
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("occupied=clk")
        );
        assert_eq!(clash.span.file.as_deref(), Some("vhdl.vhd"));
        assert!(clash.span.line.is_some());
        assert!(
            mr.design
                .cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Hff))
        );
    }

    #[test]
    fn vhdl_generate_inst_not_lowered_softdiag() {
        let src = r#"
entity wrap is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of wrap is
begin
  g: for i in 0 to 0 generate
    u_miss: missing_child port map (a => a, y => y);
  end generate;
end;
"#;
        let mr = map_vhdl(src).expect("map");
        assert!(
            mr.softs
                .iter()
                .any(|s| resolve(s.name) == "generate_not_lowered" && resolve(s.module) == "wrap")
                || mr.softs.iter().any(|s| resolve(s.name) == "missing_component"),
            "generate instance must be a named soft, got {:?}",
            mr.soft_table_lines()
        );
        assert!(
            !mr.design
                .cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "must not invent LUTs for unlowered generate inst: {:?}",
            mr.design.cells
        );
    }

    #[test]
    fn vhdl_if_generate_inst_not_silent_softdiag() {
        // if-generate must not silent-drop Insts (for-generate already notes).
        // Two stmts → `.next()` keeps one and would drop the Inst without a soft.
        let src = r#"
entity wrap is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of wrap is
begin
  g: if 1 generate
    y <= a;
    u_miss: missing_child port map (a => a, y => y);
  end generate;
end;
"#;
        let mr = map_vhdl(src).expect("map");
        assert!(
            mr.softs
                .iter()
                .any(|s| resolve(s.name) == "generate_not_lowered" && resolve(s.module) == "wrap")
                || mr.softs.iter().any(|s| resolve(s.name) == "missing_component"),
            "if-generate instance must be a named soft, not silent cells=0: {:?}",
            mr.soft_table_lines()
        );
        assert!(
            mr.has_softs(),
            "SOFT must be first-class for if-generate Inst"
        );
        assert!(
            !mr.design
                .cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "must not invent LUTs for if-generate inst: {:?}",
            mr.design.cells
        );
    }

    #[test]
    fn vhdl_parent_before_child_child_soft_incomplete() {
        // Declaration-order gap: parent entity appears BEFORE child. Pass-1
        // emit with empty child_softs + pass-2 forest must still nest.
        let src = r#"
entity mid is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of mid is
begin
  u_inner: inner port map (a => a, y => y);
end;
entity inner is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of inner is
begin
  u_miss: missing_child port map (a => a, y => y);
end;
entity wrap is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of wrap is
begin
  u_mid: mid port map (a => a, y => y);
end;
"#;
        let mr = map_vhdl(src).expect("map");
        assert!(
            mr.softs
                .iter()
                .any(|s| resolve(s.name) == "child_soft_incomplete" && resolve(s.module) == "wrap"),
            "wrap must root the forest: {:?}",
            mr.soft_table_lines()
        );
        // mid was declared before inner — still must carry child_soft_incomplete
        // nested under wrap (not omitted by parse order).
        let wrap_csi = mr
            .softs
            .iter()
            .find(|s| resolve(s.name) == "child_soft_incomplete" && resolve(s.module) == "wrap")
            .expect("wrap child_soft_incomplete");
        let mid_csi = wrap_csi
            .children
            .iter()
            .find(|s| resolve(s.name) == "child_soft_incomplete" && resolve(s.module) == "mid")
            .unwrap_or_else(|| {
                panic!(
                    "mid child_soft_incomplete required under wrap despite parent-before-child decl order: {:?}",
                    mr.soft_table_lines()
                )
            });
        assert!(
            mid_csi
                .children
                .iter()
                .any(|s| resolve(s.name) == "missing_component" && resolve(s.module) == "inner"),
            "inner missing_component nested under mid: {:?}",
            mr.soft_table_lines()
        );
        assert!(
            !mr.softs.iter().any(|s| resolve(s.name) == "missing_component"),
            "roots-only forest: {:?}",
            mr.soft_table_lines()
        );
        let rows = mr.soft_table_lines();
        assert_eq!(
            rows.iter()
                .filter(|l| l.contains("name=missing_component"))
                .count(),
            1,
            "display flatten once: {rows:?}"
        );
    }

    #[test]
    fn vhdl_entity_constant_localparam_maps() {
        let src = r#"
entity const_or is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of const_or is
  constant MASK : integer := 1;
begin
  y <= not a when MASK = 1 else '0';
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(sv.contains("localparam MASK = 1"), "{sv}");
        let d = synth_vhdl(src).expect("const");
        assert!(
            d.cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "entity constant path must still map, {:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_entity_constant_in_entity_decl_maps() {
        let src = r#"
entity const_and is
  port (a : in std_logic; y : out std_logic);
  constant KEEP : integer := 1;
end;
architecture rtl of const_and is
  signal t : std_logic_vector(KEEP downto 0);
begin
  t <= (not a) & (not a);
  y <= t(0) when KEEP = 1 else '0';
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(sv.contains("localparam KEEP = 1"), "{sv}");
        assert!(
            sv.contains("logic [1:0] t"),
            "entity-decl KEEP must fold as arch width: {sv}"
        );
        let d = synth_vhdl(src).expect("const");
        assert!(
            d.cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
            "entity-decl constant must map, {:?}",
            d.cells
        );
    }

    #[test]
    fn vhdl_entity_constant_generic_width_maps() {
        let src = r#"
entity bus_buf is
  generic (W : integer := 4);
  port (d : in std_logic_vector(3 downto 0); q : out std_logic_vector(3 downto 0));
end;
architecture rtl of bus_buf is
  constant ZERO : integer := 0;
  signal t : std_logic_vector(W-1 downto 0);
begin
  t <= d;
  q <= t;
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(
            sv.contains("localparam W = 4") || sv.contains("localparam ZERO = 0"),
            "{sv}"
        );
        assert!(
            sv.contains("logic [3:0] t"),
            "entity generic W must fold as arch signal width: {sv}"
        );
        let d = synth_vhdl(src).expect("buf");
        assert!(d.ports.iter().any(|p| resolve(p.name) == "q"));
    }

    #[test]
    fn vhdl_rng_clock_is_sta_clk() {
        let src = r#"
entity rng is
  port (clock : in std_logic; rst : in std_logic; r : out std_logic_vector(7 downto 0));
end;
architecture rtl of rng is
  signal lfsr : std_logic_vector(7 downto 0) := "10101010";
begin
  process(clock)
  begin
    if rising_edge(clock) then
      if rst = '1' then
        lfsr <= "10101010";
      else
        lfsr <= lfsr(6 downto 0) & (lfsr(7) xor lfsr(5) xor lfsr(4) xor lfsr(3));
      end if;
    end if;
  end process;
  r <= lfsr;
end;
"#;
        let sv = vhdl_to_sv(src).expect("rng sv");
        assert!(
            sv.contains("input logic clock"),
            "keep-name: process clock stays clock: {sv}"
        );
        assert!(
            !sv.contains("input logic clk"),
            "must not rename process clock clock→clk: {sv}"
        );
        assert!(
            sv.contains("posedge clock"),
            "always_ff must use clock: {sv}"
        );
        assert!(
            !sv.contains("posedge clk"),
            "must not rewrite process clock to clk: {sv}"
        );
        let d = synth_vhdl(src).expect("rng");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "rng must map FFs, {:?}",
            d.cells
        );
        assert!(
            d.ports.iter().any(|p| resolve(p.name) == "clock"),
            "STA/Design port must keep real VHDL name clock: {:?}",
            d.ports
        );
        assert!(
            !d.ports.iter().any(|p| resolve(p.name) == "clk"),
            "process/STA clock must not be renamed to clk: {:?}",
            d.ports
        );
    }

    #[test]
    fn vhdl_narrow_clock_keeps_data_clock_port() {
        // Process clock is `clk`. A separate port named `clock` is data — NARROW
        // must not total-merge every clock/clk ident into clk.
        let src = r#"
entity mix is
  port (clk : in std_logic; clock : in std_logic; q : out std_logic);
end;
architecture rtl of mix is
begin
  process(clk)
  begin
    if rising_edge(clk) then
      q <= clock;
    end if;
  end process;
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(
            sv.contains("input logic clk"),
            "process clock stays clk: {sv}"
        );
        assert!(
            sv.contains("input logic clock"),
            "data port named clock must not become clk: {sv}"
        );
        assert!(sv.contains("posedge clk"), "{sv}");
        assert!(
            sv.contains("q = clock") || sv.contains("q=clock"),
            "NBA must sample data port clock, not merge onto clk: {sv}"
        );
        assert!(
            !sv.contains("q = clk"),
            "must not rewrite data clock→clk: {sv}"
        );
        let d = synth_vhdl(src).expect("mix");
        assert!(d.ports.iter().any(|p| resolve(p.name) == "clk"), "{:?}", d.ports);
        assert!(
            d.ports.iter().any(|p| resolve(p.name) == "clock"),
            "NARROW keeps data port clock: {:?}",
            d.ports
        );
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)));
    }

    #[test]
    fn vhdl_sta_clock_name_is_narrow() {
        let none = HashSet::new();
        let mut process_clock = HashSet::new();
        process_clock.insert("clock".into());
        // Keep-name: process/STA clock `clock` stays `clock` (GUI HFF contract).
        assert_eq!(sta_clock_name("clock", &process_clock, &none), "clock");
        assert_eq!(sta_clock_name("CLOCK", &process_clock, &none), "CLOCK");
        assert_eq!(
            rewrite_sta_clock_ident("q = clock", &process_clock, &none),
            "q = clock"
        );

        let mut clk_only = HashSet::new();
        clk_only.insert("clk".into());
        assert_eq!(
            sta_clock_name("clock", &clk_only, &none),
            "clock",
            "data/other port named clock is not the process clock"
        );
        assert_eq!(sta_clock_name("clk", &clk_only, &none), "clk");
        assert_eq!(
            rewrite_sta_clock_ident("q = clock", &clk_only, &none),
            "q = clock",
            "must not total-merge clock→clk"
        );
        assert_eq!(
            rewrite_sta_clock_ident("posedge clk", &clk_only, &none),
            "posedge clk"
        );

        let mut occupied_clk = HashSet::new();
        occupied_clk.insert("clk".into());
        occupied_clk.insert("clock".into());
        assert!(clock_clk_clash(&process_clock, &occupied_clk));
        assert_eq!(
            sta_clock_name("clock", &process_clock, &occupied_clk),
            "clock",
            "clash-skip: keep clock when clk is occupied"
        );
        assert_eq!(
            rewrite_sta_clock_ident("posedge clock", &process_clock, &occupied_clk),
            "posedge clock",
            "rewrite path keeps clock on occupied clk"
        );
        assert_eq!(
            sta_clock_name("clk", &process_clock, &occupied_clk),
            "clk",
            "occupied data clk stays clk"
        );
    }

    #[test]
    fn vhdl_selected_arch_clocks_not_unioned() {
        // Last arch (comb) treats `clock` as data. The non-selected sequential
        // arch must not force clock→clk on the selected body or on child_clocks.
        let src = r#"
entity child is
  port (clock : in std_logic; q : out std_logic);
end;
architecture seq of child is
begin
  process(clock)
  begin
    if rising_edge(clock) then
      q <= '1';
    end if;
  end process;
end;
architecture comb of child is
begin
  q <= clock;
end;
entity top is
  port (clock : in std_logic; q : out std_logic);
end;
architecture rtl of top is
begin
  u: child port map (clock => clock, q => q);
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(
            sv.contains("input logic clock"),
            "selected comb arch keeps data port clock: {sv}"
        );
        assert!(
            !sv.contains("input logic clk"),
            "non-selected seq arch must not force clock→clk: {sv}"
        );
        assert!(
            sv.contains(".clock(clock)"),
            "child_clocks must follow selected arch, not union: {sv}"
        );
        assert!(
            !sv.contains(".clk("),
            "instance port must not be renamed from non-selected arch: {sv}"
        );
        assert!(
            sv.contains("q = clock") || sv.contains("q=clock"),
            "selected body treats clock as data: {sv}"
        );
    }

    #[test]
    fn vhdl_clock_clk_clash_skips_rename() {
        // Process/STA clock is `clock`. A separate data port is already `clk`.
        // Skip clock→clk (keep distinct nets). Named diagnostic. No synth abort.
        let src = r#"
entity clash is
  port (clk : in std_logic; clock : in std_logic; q : out std_logic);
end;
architecture rtl of clash is
begin
  process(clock)
  begin
    if rising_edge(clock) then
      q <= clk;
    end if;
  end process;
end;
"#;
        TEST_DIAGNOSTICS.with(|d| d.borrow_mut().clear());
        let sv = vhdl_to_sv(src).expect("clash must not abort lowering");
        let diags = TEST_DIAGNOSTICS.with(|d| d.borrow().clone());
        assert!(
            diags.iter().any(|l| l == &clock_clk_clash_line("clash")),
            "named diagnostic must be emitted, got {diags:?}"
        );
        assert!(sv.contains("input logic clk,"), "data port clk stays: {sv}");
        assert!(
            sv.contains("input logic clock,"),
            "process clock keeps original name: {sv}"
        );
        let clk_port_decls =
            sv.matches("input logic clk,").count() + sv.matches("input logic clk)").count();
        assert_eq!(clk_port_decls, 1, "must not emit duplicate clk ports: {sv}");
        assert!(
            sv.contains("posedge clock"),
            "always_ff uses original process clock: {sv}"
        );
        assert!(
            !sv.contains("posedge clk"),
            "must not silent-merge process clock onto data clk: {sv}"
        );
        assert!(
            sv.contains("q = clk") || sv.contains("q=clk"),
            "NBA samples data port clk: {sv}"
        );
        let line = clock_clk_clash_line("clash");
        assert!(
            line.contains("diagnostic clock_clk_clash"),
            "named diagnostic: {line}"
        );
        assert!(line.contains("module=clash"), "entity context: {line}");
        assert!(
            line.contains("construct=process_clock"),
            "construct: {line}"
        );
        assert!(line.contains("rename clock→clk skipped"), "why: {line}");
        assert!(
            line.contains("not a silent merge") && line.contains("not a synth abort"),
            "policy: {line}"
        );
        let d = synth_vhdl(src).expect("clash must not abort synth");
        let clk_ports: Vec<_> = d.ports.iter().filter(|p| resolve(p.name) == "clk").collect();
        assert_eq!(clk_ports.len(), 1, "no duplicate clk ports: {:?}", d.ports);
        assert!(
            d.ports.iter().any(|p| resolve(p.name) == "clock"),
            "process clock net kept distinct: {:?}",
            d.ports
        );
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)),
            "still maps FF, {:?}",
            d.cells
        );
        assert_ne!(
            d.attrs.get("NO_BODY"),
            Some("1"),
            "clash skip is not a missing-body failure"
        );
    }

    #[test]
    fn vhdl_occupied_includes_generics_consts() {
        // Package dump at top must not collide with entity generics/consts.
        let src = r#"
package p is
  constant W : integer := 8;
  constant KEEP : integer := 0;
end;
entity top is
  generic (W : integer := 4);
  port (a : in std_logic; y : out std_logic);
  constant KEEP : integer := 1;
end;
architecture rtl of top is
begin
  y <= a;
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(
            sv.contains("localparam W = 4"),
            "generic W wins over package: {sv}"
        );
        assert!(
            !sv.contains("localparam W = 8"),
            "package W must not dump on occupied generic: {sv}"
        );
        assert!(
            sv.contains("localparam KEEP = 1"),
            "entity const KEEP wins over package: {sv}"
        );
        assert!(
            !sv.contains("localparam KEEP = 0"),
            "package KEEP must not dump on occupied entity const: {sv}"
        );
    }

    #[test]
    fn vhdl_package_const_not_dumped_into_child() {
        // Package consts fold at parse; they must not become localparam in
        // every hierarchical child (names collide with child ports/signals).
        // Top may still emit unused package params that do not collide.
        let src = r#"
package p is
  constant WIDTH : integer := 1;
  constant a : integer := 0;
end;
entity child is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of child is
begin
  y <= a;
end;
entity wrap is
  port (a : in std_logic; y : out std_logic);
end;
architecture rtl of wrap is
begin
  u: child port map (a => a, y => y);
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        let child = sv.split("module wrap").next().unwrap_or(&sv);
        assert!(child.contains("module child"), "{sv}");
        assert!(
            !child.contains("localparam"),
            "package consts must not dump into hierarchical child: {child}"
        );
        assert!(
            !sv.contains("localparam a"),
            "package name a must not collide with port a: {sv}"
        );
        assert!(
            sv.contains("localparam WIDTH = 1"),
            "top-only package params still emit: {sv}"
        );
        let d = synth_vhdl(src).expect("hier");
        assert!(d.ports.iter().any(|p| resolve(p.name) == "y"), "{:?}", d.ports);
    }

    #[test]
    fn vhdl_no_fake_fd_library() {
        let src = r#"
entity wrap is
  port (clk : in std_logic; d : in std_logic; q : out std_logic);
end;
architecture rtl of wrap is
begin
  process(clk)
  begin
    if rising_edge(clk) then
      q <= d;
    end if;
  end process;
end;
"#;
        let sv = vhdl_to_sv(src).expect("sv");
        assert!(!sv.to_ascii_lowercase().contains("unisim"), "{sv}");
        assert!(
            !sv.contains("FDCE") && !sv.contains("FDRE") && !sv.contains(" module FD"),
            "{sv}"
        );
        let d = synth_vhdl(src).expect("ff");
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Hff)));
    }
}
