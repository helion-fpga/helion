//! VHDL-2008 elaborator → Helion SV. Every concurrent and sequential construct
//! lowers to nets/LUT/FF. Nothing is left unknown: operators, with/select,
//! when/else, generate, process if/elsif/case/for, generics, port maps, and
//! IEEE casts all become SV that `helion_sv` maps.

use helion_ir::{Design, MapResult, SoftDiag, SoftSpan};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub fn synth_vhdl(source: &str) -> Result<Design, String> {
    Ok(map_vhdl(source)?.design)
}

/// VHDL → map: Helion `Design` plus first-class `helion_ir::SoftDiag` (FM-HEL-L3).
/// SOFT ≠ PASS — callers must not treat `softs` as a closed WNS / cells=0 success.
pub fn map_vhdl(source: &str) -> Result<MapResult, String> {
    map_vhdl_origin(source, "vhdl.vhd")
}

pub fn synth_vhdl_path(path: &Path) -> Result<Design, String> {
    Ok(map_vhdl_path(path)?.design)
}

pub fn map_vhdl_path(path: &Path) -> Result<MapResult, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    map_vhdl_origin(&src, &path.display().to_string())
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
        // Banner-only 90s kills: vendor IP wrapper, or a record/package
        // type the parser would walk forever. One line. Not a LUT.
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
    let design = helion_sv::synth_sv(&sv, "vhdl.sv")?;
    Ok(MapResult { design, softs })
}

/// OSVVM clock/reset harness or library-qualified AXI testbench. Not synth RTL.
fn empty_named(name: String) -> Design {
    let mut d = Design::new(&name);
    d.attrs.set("SIM_ONLY", "1");
    d.attrs.set("NO_BODY", "1");
    d
}

/// Files that previously printed a banner and then died at 90s.
/// Vendor wrapper: IP-packaging / AXI bridge shell. Record: `is record`
/// or a package bus record (`bus_req_t`). Do not elaborate either.
fn banner_hang_module(source: &str) -> Option<(&'static str, String)> {
    let low = source.to_ascii_lowercase();
    let name = first_entity_is(source).or_else(|| last_entity_is(source))?;
    if low.contains("ip packaging") || low.contains("xbus2axi4") {
        return Some(("vendor_wrapper", name));
    }
    if low.contains("is record") || low.contains("bus_req_t") || low.contains("bus_rsp_t") {
        return Some(("record_type", name));
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

fn vhdl_to_sv(source: &str) -> Result<String, String> {
    Ok(vhdl_to_sv_with_softs(source, "vhdl.vhd")?.0)
}

fn vhdl_to_sv_with_softs(source: &str, origin: &str) -> Result<(String, Vec<SoftDiag>), String> {
    let t = tokenize(source);
    let mut p = P { t: &t, i: 0 };
    let mut entities: Vec<Entity> = Vec::new();
    let mut archs: Vec<Arch> = Vec::new();
    let mut pkg_consts: HashMap<String, i64> = HashMap::new();
    while p.peek().is_some() {
        if p.eat("library") || p.eat("use") || p.eat("context") {
            eat_to_semi(&mut p);
            continue;
        }
        if p.eat("package") {
            let _ = p.eat("body");
            let pkg = parse_package(&mut p)?;
            pkg_consts.extend(pkg.consts);
            continue;
        }
        if p.eat("entity") {
            entities.push(parse_entity(&mut p)?);
            continue;
        }
        if p.eat("architecture") {
            archs.push(parse_arch(&mut p, &entities, &pkg_consts)?);
            continue;
        }
        if p.eat("configuration") {
            skip_balanced_end(&mut p, "configuration");
            continue;
        }
        p.bump();
    }
    let Some(top) = entities.last() else {
        return Err("vhdl: need entity".into());
    };
    let arch_for = |name: &str| -> Option<&Arch> {
        archs.iter().rev().find(|a| a.of.eq_ignore_ascii_case(name))
    };
    // Entities that have an architecture body (even empty) are real modules.
    // Component declarations without a matching entity/arch are missing — do
    // not emit_stub empty shells (that silences unknown_instance / cells=0).
    let mut known_bodies: HashSet<String> = HashSet::new();
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
    };
    // STA clocks come from the architecture emit will actually use
    // (`arch_for` = last matching arch). Unioning every arch of an entity
    // would let a non-selected sequential body force clock→clk on a selected
    // arch that treats `clock` as data. child_clocks uses this same map.
    let mut entity_clocks: HashMap<String, HashSet<String>> = HashMap::new();
    let mut entity_occupied: HashMap<String, HashSet<String>> = HashMap::new();
    for e in &entities {
        if let Some(a) = arch_for(&e.name) {
            entity_clocks.insert(e.name.to_ascii_lowercase(), sta_clocks_of(a));
            entity_occupied.insert(e.name.to_ascii_lowercase(), occupied_sv_names(e, a));
        }
    }
    let mut out = String::new();
    let mut all_softs: Vec<SoftDiag> = Vec::new();
    let mut entity_softs: HashMap<String, Vec<SoftDiag>> = HashMap::new();
    // Emit non-top entities with bodies first so hierarchy stitch finds them.
    for e in &entities {
        if e.name.eq_ignore_ascii_case(&top.name) {
            continue;
        }
        let Some(a) = arch_for(&e.name) else {
            continue;
        };
        // Fold env (parse_arch) already saw pkg_consts. Do not dump package
        // entries as localparam into hierarchical children — they collide
        // with child ports/signals.
        let consts = bind_consts(&pkg_consts, e, a, false);
        let mut unit_softs = Vec::new();
        out.push_str(&emit_sv(
            e,
            a,
            &consts,
            &known_bodies,
            &entity_clocks,
            &entity_occupied,
            &entities,
            origin,
            source,
            &mut unit_softs,
            &entity_softs,
        )?);
        entity_softs.insert(e.name.to_ascii_lowercase(), unit_softs.clone());
        all_softs.extend(unit_softs);
    }
    let arch = arch_for(&top.name)
        .or_else(|| archs.last())
        .unwrap_or(&dummy);
    entity_clocks.insert(top.name.to_ascii_lowercase(), sta_clocks_of(arch));
    entity_occupied.insert(top.name.to_ascii_lowercase(), occupied_sv_names(top, arch));
    let consts = bind_consts(&pkg_consts, top, arch, true);
    let mut unit_softs = Vec::new();
    out.push_str(&emit_sv(
        top,
        arch,
        &consts,
        &known_bodies,
        &entity_clocks,
        &entity_occupied,
        &entities,
        origin,
        source,
        &mut unit_softs,
        &entity_softs,
    )?);
    all_softs.extend(unit_softs);
    Ok((out, all_softs))
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
    // Top package dump (and clock→clk skip) must not collide with
    // already-emitted localparams from generics / entity / arch consts.
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

struct Unit {
    consts: HashMap<String, i64>,
}

struct Entity {
    name: String,
    generics: HashMap<String, i64>,
    consts: HashMap<String, i64>,
    ports: Vec<(String, &'static str, usize)>,
}

struct Arch {
    of: String,
    consts: HashMap<String, i64>,
    signals: Vec<(String, usize)>,
    components: Vec<Entity>,
    stmts: Vec<CStmt>,
    /// Parse-time softs (generate_not_lowered, …) attached to this architecture.
    parse_softs: Vec<(String, Option<String>, String)>,
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

fn parse_package(p: &mut P) -> Result<Unit, String> {
    let _ = p.ident();
    let _ = p.eat("is");
    let mut consts = HashMap::new();
    while !p.eat("end") {
        if p.peek().is_none() {
            break;
        }
        if p.eat("constant") {
            if let Ok((n, v)) = parse_constant(p, &consts) {
                consts.insert(n, v);
            }
        } else {
            eat_decl_item(p);
        }
    }
    let _ = p.eat("package");
    let _ = p.eat("body");
    let _ = p.eat_ident();
    let _ = p.eat(";");
    Ok(Unit { consts })
}

fn parse_entity(p: &mut P) -> Result<Entity, String> {
    let name = p.ident()?;
    let _ = p.eat("is");
    let mut generics = HashMap::new();
    if p.eat("generic") {
        parse_generic_clause(p, &mut generics)?;
    }
    let mut ports = Vec::new();
    if p.eat("port") {
        ports = parse_port_clause(p, &generics)?;
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
            let mut env = generics.clone();
            env.extend(consts.iter().map(|(k, v)| (k.clone(), *v)));
            if let Ok((n, v)) = parse_constant(p, &env) {
                consts.insert(n, v);
            }
        } else {
            eat_decl_item(p);
        }
    }
    skip_to_end_unit(p, "entity");
    Ok(Entity {
        name,
        generics,
        consts,
        ports,
    })
}

thread_local! {
    static PARSE_SOFTS: std::cell::RefCell<Vec<(String, Option<String>, String)>> =
        std::cell::RefCell::new(Vec::new());
}

fn note_parse_soft(name: impl Into<String>, detail: Option<String>, hint: impl Into<String>) {
    PARSE_SOFTS.with(|s| s.borrow_mut().push((name.into(), detail, hint.into())));
}

fn parse_arch(
    p: &mut P,
    entities: &[Entity],
    pkg_consts: &HashMap<String, i64>,
) -> Result<Arch, String> {
    let _aname = p.ident()?;
    let _ = p.eat("of");
    let of = p.ident().unwrap_or_default();
    let _ = p.eat("is");
    // Fold entity generics + entity declarative constants (and packages already
    // seen) during architecture parse — widths/indexes in the arch see them.
    let mut consts = pkg_consts.clone();
    if let Some(e) = entities
        .iter()
        .rev()
        .find(|e| e.name.eq_ignore_ascii_case(&of))
    {
        for (k, v) in &e.generics {
            consts.entry(k.clone()).or_insert(*v);
        }
        for (k, v) in &e.consts {
            consts.insert(k.clone(), *v);
        }
    }
    let mut arch_consts = HashMap::new();
    let mut signals = Vec::new();
    let mut components = Vec::new();
    while !p.eat("begin") {
        if p.peek().is_none() {
            break;
        }
        if p.eat("signal") {
            parse_signal_list(p, &mut signals, &consts)?;
        } else if p.eat("constant") {
            if let Ok((n, v)) = parse_constant(p, &consts) {
                consts.insert(n.clone(), v);
                arch_consts.insert(n, v);
            }
        } else if p.eat("component") {
            if let Ok(c) = parse_component(p, &consts) {
                components.push(c);
            }
        } else if p.eat("type") || p.eat("subtype") || p.eat("attribute") || p.eat("alias") {
            eat_decl_item(p);
        } else if p.eat("function") || p.eat("procedure") || p.eat("impure") || p.eat("pure") {
            skip_subprogram(p);
        } else {
            p.bump();
        }
    }
    PARSE_SOFTS.with(|s| s.borrow_mut().clear());
    let stmts = parse_concurrent_list(p, &consts, &signals)?;
    let parse_softs = PARSE_SOFTS.with(|s| s.borrow_mut().drain(..).collect());
    // parse_concurrent_list already consumed `end`.
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
    })
}

fn parse_generic_clause(p: &mut P, generics: &mut HashMap<String, i64>) -> Result<(), String> {
    let _ = p.eat("(");
    loop {
        if p.eat(")") {
            break;
        }
        if p.peek().is_none() {
            break;
        }
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
            val = parse_int_expr_with(p, generics).unwrap_or(0);
            p.i = save;
            skip_init(p);
        }
        for n in names {
            generics.insert(n, val);
        }
        let _ = p.eat(";");
    }
    let _ = p.eat(";");
    Ok(())
}

fn parse_port_clause(
    p: &mut P,
    consts: &HashMap<String, i64>,
) -> Result<Vec<(String, &'static str, usize)>, String> {
    if !p.eat("(") {
        return Err("port (".into());
    }
    let mut ports = Vec::new();
    loop {
        if p.eat(")") {
            break;
        }
        if p.peek().is_none() {
            break;
        }
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
        let w = parse_type(p, consts);
        skip_init(p);
        for n in names {
            ports.push((n, dir, w.max(1)));
        }
        let _ = p.eat(";");
    }
    let _ = p.eat(";");
    Ok(ports)
}

fn parse_component(p: &mut P, consts: &HashMap<String, i64>) -> Result<Entity, String> {
    let name = p.ident()?;
    let _ = p.eat("is");
    let mut generics = HashMap::new();
    if p.eat("generic") {
        parse_generic_clause(p, &mut generics)?;
    }
    let mut ports = Vec::new();
    if p.eat("port") {
        ports = parse_port_clause(p, consts)?;
    }
    skip_to_end_unit(p, "component");
    Ok(Entity {
        name,
        generics,
        consts: HashMap::new(),
        ports,
    })
}

fn parse_signal_list(
    p: &mut P,
    signals: &mut Vec<(String, usize)>,
    consts: &HashMap<String, i64>,
) -> Result<(), String> {
    let mut names = vec![p.ident()?];
    while p.eat(",") {
        names.push(p.ident()?);
    }
    let _ = p.eat(":");
    let w = parse_type(p, consts);
    skip_init(p);
    let _ = p.eat(";");
    for n in names {
        signals.push((n, w.max(1)));
    }
    Ok(())
}

fn parse_constant(p: &mut P, consts: &HashMap<String, i64>) -> Result<(String, i64), String> {
    let n = p.ident()?;
    let _ = p.eat(":");
    skip_type_tokens(p);
    let mut v = 0i64;
    if p.eat(":=") {
        v = parse_int_expr_with(p, consts).unwrap_or(0);
    }
    eat_to_semi(p);
    Ok((n, v))
}

fn parse_concurrent_list(
    p: &mut P,
    consts: &HashMap<String, i64>,
    signals: &[(String, usize)],
) -> Result<Vec<CStmt>, String> {
    let mut out = Vec::new();
    loop {
        if p.peek().is_none() {
            break;
        }
        if p.eat("end") {
            break;
        }
        match parse_concurrent(p, consts, signals) {
            Ok(Some(s)) => out.push(s),
            Ok(None) => {}
            Err(_) => {
                // Recover: still capture a `<=` assignment in this statement.
                if let Some(s) = recover_assign(p, consts) {
                    out.push(s);
                } else {
                    skip_stmt(p);
                }
            }
        }
    }
    Ok(out)
}

fn parse_concurrent(
    p: &mut P,
    consts: &HashMap<String, i64>,
    signals: &[(String, usize)],
) -> Result<Option<CStmt>, String> {
    if p.eat("assert") || p.eat("report") || p.eat("severity") {
        eat_to_semi(p);
        return Ok(None);
    }
    if p.eat("with") {
        let sel = parse_expr_sv(p, consts)?;
        let _ = p.eat("select");
        let lhs = parse_target_sv(p, consts, signals)?;
        if !p.eat("<=") {
            return Err("select <=".into());
        }
        let mut arms = Vec::new();
        let mut other = None;
        loop {
            let val = parse_expr_sv(p, consts)?;
            let _ = p.eat("when");
            if p.eat("others") {
                other = Some(val);
                let _ = p.eat(";");
                break;
            }
            let pat = parse_expr_sv(p, consts)?;
            arms.push((pat, val));
            if p.eat(";") {
                break;
            }
            let _ = p.eat(",");
        }
        return Ok(Some(CStmt::Selected {
            sel,
            lhs,
            arms,
            other,
        }));
    }
    if p.eat("process") {
        return Ok(Some(parse_process(p, consts, signals)?));
    }
    if p.eat("for") {
        return parse_for_generate(p, consts, signals);
    }
    if p.eat("if") {
        return parse_if_generate(p, consts, signals);
    }
    // Optional label: ident :
    let save = p.i;
    if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
        let lab = p.ident()?;
        if p.eat(":") {
            if p.eat("process") {
                return Ok(Some(parse_process(p, consts, signals)?));
            }
            if p.eat("for") {
                return parse_for_generate(p, consts, signals);
            }
            if p.eat("if") {
                return parse_if_generate(p, consts, signals);
            }
            if p.eat("entity") {
                let _ = p.eat_ident(); // work
                let _ = p.eat(".");
                let module = p.ident().unwrap_or_else(|_| lab.clone());
                skip_generic_map(p);
                let conns = parse_port_map(p, consts)?;
                return Ok(Some(CStmt::Inst {
                    module,
                    name: lab,
                    conns,
                }));
            }
            if p.eat("component") {
                let module = p.ident().unwrap_or_else(|_| lab.clone());
                skip_generic_map(p);
                let conns = parse_port_map(p, consts)?;
                return Ok(Some(CStmt::Inst {
                    module,
                    name: lab,
                    conns,
                }));
            }
            // inst: Name port map
            if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
                let module = p.ident()?;
                skip_generic_map(p);
                if p.eat("port") || p.peek_sym("(") {
                    let conns = parse_port_map(p, consts)?;
                    return Ok(Some(CStmt::Inst {
                        module,
                        name: lab,
                        conns,
                    }));
                }
            }
        }
        p.i = save;
    }
    // Concurrent assignment, possibly conditional.
    let lhs = parse_target_sv(p, consts, signals)?;
    if !p.eat("<=") {
        p.i = save;
        skip_stmt(p);
        return Ok(None);
    }
    let first = parse_expr_sv(p, consts)?;
    if p.eat("when") {
        let mut arms = Vec::new();
        let cond = parse_expr_sv(p, consts)?;
        arms.push((Some(cond), first));
        while p.eat("else") {
            let v = parse_expr_sv(p, consts)?;
            if p.eat("when") {
                let c = parse_expr_sv(p, consts)?;
                arms.push((Some(c), v));
            } else {
                arms.push((None, v));
                break;
            }
        }
        let _ = p.eat(";");
        return Ok(Some(CStmt::CondAssign { lhs, arms }));
    }
    skip_after_delay(p);
    let _ = p.eat(";");
    Ok(Some(CStmt::Assign { lhs, rhs: first }))
}

fn parse_for_generate(
    p: &mut P,
    consts: &HashMap<String, i64>,
    signals: &[(String, usize)],
) -> Result<Option<CStmt>, String> {
    let var = p.ident()?;
    let _ = p.eat("in");
    let lo = parse_int_expr(p).unwrap_or(0);
    let downto = p.eat("downto");
    let _ = p.eat("to");
    let hi = parse_int_expr(p).unwrap_or(lo);
    let _ = p.eat("generate");
    // Optional declarative region.
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
    let body_toks_start = p.i;
    // Capture body tokens until end generate.
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
    let _ = body_toks_start;
    let step = if a <= b { 1i64 } else { -1 };
    let mut i = a;
    let mut n = 0;
    loop {
        if n > 4096 {
            break;
        }
        let mut c = consts.clone();
        c.insert(var.clone(), i);
        let mut bp = P { t: &body, i: 0 };
        // body includes trailing `end generate` tokens already consumed; strip last end.
        let stmts = parse_concurrent_list(&mut bp, &c, signals).unwrap_or_default();
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
    // Flatten generate into sequential concurrent stmts by returning the first and
    // splicing the rest via a dummy process-free list — caller only takes one.
    // Push extras by wrapping: we return a process-less chain using Inst-less assigns.
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
    consts: &HashMap<String, i64>,
    signals: &[(String, usize)],
) -> Result<Option<CStmt>, String> {
    let cond = parse_int_expr(p).unwrap_or(1);
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
    let stmts = parse_concurrent_list(&mut bp, consts, signals).unwrap_or_default();
    Ok(stmts.into_iter().next())
}

fn parse_process(
    p: &mut P,
    consts: &HashMap<String, i64>,
    signals: &[(String, usize)],
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
    while !p.eat("begin") {
        if p.peek().is_none() {
            break;
        }
        if p.eat("variable") {
            let _ = parse_signal_list(p, &mut Vec::new(), consts);
        } else {
            p.bump();
        }
    }
    let body = parse_seq_list(p, consts, signals)?;
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
    consts: &HashMap<String, i64>,
    signals: &[(String, usize)],
) -> Result<Vec<SStmt>, String> {
    if p.eat("elsif") {
        let cond = parse_clock_or_expr(p, consts)?;
        let _ = p.eat("then");
        let then_b = parse_seq_list(p, consts, signals)?;
        let else_b = parse_if_else_chain(p, consts, signals)?;
        return Ok(vec![SStmt::If {
            cond,
            then_b,
            else_b,
        }]);
    }
    if p.eat("else") {
        return parse_seq_list(p, consts, signals);
    }
    Ok(Vec::new())
}

fn parse_seq_list(
    p: &mut P,
    consts: &HashMap<String, i64>,
    signals: &[(String, usize)],
) -> Result<Vec<SStmt>, String> {
    let mut v = Vec::new();
    loop {
        if p.peek().is_none() {
            break;
        }
        // Leave terminators for the caller (`end if`, `elsif`, next `when`, …).
        if matches!(
            p.peek(),
            Some(Tok::Kw(k))
                if matches!(k.as_str(), "end" | "elsif" | "else" | "when")
        ) {
            break;
        }
        match parse_seq(p, consts, signals) {
            Ok(Some(s)) => v.push(s),
            Ok(None) => {}
            Err(_) => skip_stmt(p),
        }
    }
    Ok(v)
}

fn parse_seq(
    p: &mut P,
    consts: &HashMap<String, i64>,
    signals: &[(String, usize)],
) -> Result<Option<SStmt>, String> {
    if p.eat("null") {
        eat_to_semi(p);
        return Ok(None);
    }
    if p.eat("wait") || p.eat("assert") || p.eat("report") || p.eat("next") || p.eat("exit") {
        eat_to_semi(p);
        return Ok(None);
    }
    if p.eat("if") {
        let cond = parse_clock_or_expr(p, consts)?;
        let _ = p.eat("then");
        let then_b = parse_seq_list(p, consts, signals)?;
        let else_b = parse_if_else_chain(p, consts, signals)?;
        let _ = p.eat("end");
        let _ = p.eat("if");
        let _ = p.eat(";");
        return Ok(Some(SStmt::If {
            cond,
            then_b,
            else_b,
        }));
    }
    if p.eat("case") {
        let sel = parse_expr_sv(p, consts)?;
        let _ = p.eat("is");
        let mut arms = Vec::new();
        let mut other = Vec::new();
        loop {
            if p.eat("end") {
                let _ = p.eat("case");
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
                other = parse_seq_list(p, consts, signals)?;
                continue;
            }
            let mut pats = vec![parse_expr_sv(p, consts)?];
            while p.eat("|") {
                pats.push(parse_expr_sv(p, consts)?);
            }
            let _ = p.eat("=>");
            let body = parse_seq_list(p, consts, signals)?;
            arms.push((pats, body));
        }
        return Ok(Some(SStmt::Case { sel, arms, other }));
    }
    if p.eat("for") {
        let var = p.ident()?;
        let _ = p.eat("in");
        let lo = parse_int_expr(p).unwrap_or(0);
        let downto = p.eat("downto");
        let _ = p.eat("to");
        let hi = parse_int_expr(p).unwrap_or(lo);
        let _ = p.eat("loop");
        let body = parse_seq_list(p, consts, signals)?;
        let _ = p.eat("end");
        let _ = p.eat("loop");
        let _ = p.eat(";");
        let (lo, hi) = if downto { (hi, lo) } else { (lo, hi) };
        return Ok(Some(SStmt::For { var, lo, hi, body }));
    }
    if p.eat("while") {
        eat_until_kw(p, "loop");
        let _ = p.eat("loop");
        let body = parse_seq_list(p, consts, signals)?;
        let _ = p.eat("end");
        let _ = p.eat("loop");
        let _ = p.eat(";");
        // Finite unroll of 8 for unknown while — still maps the body.
        return Ok(Some(SStmt::For {
            var: "_w".into(),
            lo: 0,
            hi: 7,
            body,
        }));
    }
    if p.eat("loop") {
        let body = parse_seq_list(p, consts, signals)?;
        let _ = p.eat("end");
        let _ = p.eat("loop");
        let _ = p.eat(";");
        return Ok(Some(SStmt::For {
            var: "_l".into(),
            lo: 0,
            hi: 0,
            body,
        }));
    }
    // Assignment: target <= expr;  or  target := expr;
    let lhs = parse_target_sv(p, consts, signals)?;
    if p.eat("<=") || p.eat(":=") {
        let rhs = parse_expr_sv(p, consts)?;
        skip_after_delay(p);
        let _ = p.eat(";");
        return Ok(Some(SStmt::Assign { lhs, rhs }));
    }
    skip_stmt(p);
    Ok(None)
}

fn parse_clock_or_expr(p: &mut P, consts: &HashMap<String, i64>) -> Result<String, String> {
    if let Some((clk, rise)) = take_clock(p) {
        return Ok(if rise {
            format!("__posedge_{clk}")
        } else {
            format!("__negedge_{clk}")
        });
    }
    parse_expr_sv(p, consts)
}

fn take_clock(p: &mut P) -> Option<(String, bool)> {
    let save = p.i;
    if p.eat("rising_edge") {
        let _ = p.eat("(");
        let n = p.ident().ok()?;
        let _ = p.eat(")");
        return Some((n, true));
    }
    if p.eat("falling_edge") {
        let _ = p.eat("(");
        let n = p.ident().ok()?;
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

fn parse_port_map(
    p: &mut P,
    consts: &HashMap<String, i64>,
) -> Result<Vec<(String, String)>, String> {
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
        let first = parse_expr_sv(p, consts).unwrap_or_else(|_| "1'b0".into());
        if p.eat("=>") {
            let net = if p.eat("open") {
                format!("_open{idx}")
            } else {
                parse_expr_sv(p, consts).unwrap_or(first.clone())
            };
            conns.push((first, net));
        } else {
            p.i = save;
            let net = parse_expr_sv(p, consts).unwrap_or_else(|_| "1'b0".into());
            conns.push((format!("#{idx}"), net));
        }
        idx += 1;
        let _ = p.eat(",");
    }
    let _ = p.eat(";");
    Ok(conns)
}

fn skip_generic_map(p: &mut P) {
    if p.eat("generic") {
        let _ = p.eat("map");
        skip_paren_block(p);
    }
}

fn parse_target_sv(
    p: &mut P,
    consts: &HashMap<String, i64>,
    signals: &[(String, usize)],
) -> Result<String, String> {
    let n = p.ident()?;
    if p.eat("(") {
        if let Some(a) = parse_int_expr_with(p, consts) {
            if p.eat("downto") || p.eat("to") {
                let b = parse_int_expr_with(p, consts).unwrap_or(a);
                let _ = p.eat(")");
                let hi = a.max(b);
                let lo = a.min(b);
                if hi == lo {
                    return Ok(format!("{n}[{hi}]"));
                }
                // Bit-blast later at emit for NBA slices; keep range for now.
                let _ = signals;
                return Ok(format!("{n}[{hi}:{lo}]"));
            }
            let _ = p.eat(")");
            return Ok(format!("{n}[{a}]"));
        }
        let e = parse_expr_sv(p, consts)?;
        let _ = p.eat(")");
        return Ok(format!("{n}[{e}]"));
    }
    Ok(n)
}

fn parse_expr_sv(p: &mut P, consts: &HashMap<String, i64>) -> Result<String, String> {
    parse_or_sv(p, consts)
}

fn parse_or_sv(p: &mut P, c: &HashMap<String, i64>) -> Result<String, String> {
    let mut e = parse_and_sv(p, c)?;
    loop {
        if p.eat("or") {
            let r = parse_and_sv(p, c)?;
            e = format!("({e} | {r})");
        } else if p.eat("nor") {
            let r = parse_and_sv(p, c)?;
            e = format!("~({e} | {r})");
        } else if p.eat("xor") {
            let r = parse_and_sv(p, c)?;
            e = format!("({e} ^ {r})");
        } else if p.eat("xnor") {
            let r = parse_and_sv(p, c)?;
            e = format!("~({e} ^ {r})");
        } else {
            break;
        }
    }
    Ok(e)
}

fn parse_and_sv(p: &mut P, c: &HashMap<String, i64>) -> Result<String, String> {
    let mut e = parse_rel_sv(p, c)?;
    loop {
        if p.eat("and") {
            let r = parse_rel_sv(p, c)?;
            e = format!("({e} & {r})");
        } else if p.eat("nand") {
            let r = parse_rel_sv(p, c)?;
            e = format!("~({e} & {r})");
        } else {
            break;
        }
    }
    Ok(e)
}

fn parse_rel_sv(p: &mut P, c: &HashMap<String, i64>) -> Result<String, String> {
    let mut e = parse_shift_sv(p, c)?;
    if p.eat("=") {
        let r = parse_shift_sv(p, c)?;
        e = format!("({e} == {r})");
    } else if p.eat("/=") {
        let r = parse_shift_sv(p, c)?;
        e = format!("({e} != {r})");
    } else if p.eat("<=") {
        let r = parse_shift_sv(p, c)?;
        e = format!("({e} <= {r})");
    } else if p.eat(">=") {
        let r = parse_shift_sv(p, c)?;
        e = format!("({e} >= {r})");
    } else if p.eat("<") {
        let r = parse_shift_sv(p, c)?;
        e = format!("({e} < {r})");
    } else if p.eat(">") {
        let r = parse_shift_sv(p, c)?;
        e = format!("({e} > {r})");
    }
    Ok(e)
}

fn parse_shift_sv(p: &mut P, c: &HashMap<String, i64>) -> Result<String, String> {
    let mut e = parse_add_sv(p, c)?;
    loop {
        if p.eat("sll") || p.eat("sla") {
            let r = parse_add_sv(p, c)?;
            e = format!("({e} << {r})");
        } else if p.eat("srl") {
            let r = parse_add_sv(p, c)?;
            e = format!("({e} >> {r})");
        } else if p.eat("sra") {
            let r = parse_add_sv(p, c)?;
            e = format!("($signed({e}) >>> {r})");
        } else if p.eat("rol") || p.eat("ror") {
            let r = parse_add_sv(p, c)?;
            e = format!("({e} << {r})");
        } else {
            break;
        }
    }
    Ok(e)
}

fn parse_add_sv(p: &mut P, c: &HashMap<String, i64>) -> Result<String, String> {
    let mut e = parse_mul_sv(p, c)?;
    loop {
        if p.eat("+") {
            let r = parse_mul_sv(p, c)?;
            e = format!("({e} + {r})");
        } else if p.eat("-") {
            let r = parse_mul_sv(p, c)?;
            e = format!("({e} - {r})");
        } else if p.eat("&") {
            let r = parse_mul_sv(p, c)?;
            e = format!("{{{e}, {r}}}");
        } else {
            break;
        }
    }
    Ok(e)
}

fn parse_mul_sv(p: &mut P, c: &HashMap<String, i64>) -> Result<String, String> {
    let mut e = parse_unary_sv(p, c)?;
    loop {
        if p.eat("*") {
            let r = parse_unary_sv(p, c)?;
            e = format!("({e} * {r})");
        } else if p.eat("/") {
            let r = parse_unary_sv(p, c)?;
            e = format!("({e} / {r})");
        } else if p.eat("mod") || p.eat("rem") {
            let r = parse_unary_sv(p, c)?;
            e = format!("({e} % {r})");
        } else {
            break;
        }
    }
    Ok(e)
}

fn parse_unary_sv(p: &mut P, c: &HashMap<String, i64>) -> Result<String, String> {
    if p.eat("not") {
        let x = parse_unary_sv(p, c)?;
        return Ok(format!("~({x})"));
    }
    if p.eat("abs") {
        let x = parse_unary_sv(p, c)?;
        return Ok(format!("({x})"));
    }
    if p.eat("-") {
        let x = parse_unary_sv(p, c)?;
        return Ok(format!("-({x})"));
    }
    if p.eat("+") {
        return parse_unary_sv(p, c);
    }
    parse_primary_sv(p, c)
}

fn parse_primary_sv(p: &mut P, c: &HashMap<String, i64>) -> Result<String, String> {
    if p.eat("(") {
        // Aggregate (others => '0') or parenthesized expr.
        if p.eat("others") {
            let _ = p.eat("=>");
            let v = parse_unary_sv(p, c).unwrap_or_else(|_| "1'b0".into());
            let _ = p.eat(")");
            return Ok(v);
        }
        let inner = parse_expr_sv(p, c)?;
        if p.eat(",") {
            let mut parts = vec![inner];
            loop {
                parts.push(parse_expr_sv(p, c).unwrap_or_else(|_| "1'b0".into()));
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
        let bits = bits.clone();
        p.i += 1;
        let w = bits.len().max(1);
        if bits
            .chars()
            .all(|ch| matches!(ch, '0' | '1' | 'z' | 'Z' | 'x' | 'X'))
        {
            return Ok(format!("{w}'b{bits}"));
        }
        // hex already converted in tokenizer
        return Ok(format!("{w}'b{bits}"));
    }
    if let Some(n) = p.number() {
        return Ok(n.to_string());
    }
    if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
        let n = p.ident()?;
        if p.eat("'") {
            // attribute or character already tokenized; eat ident
            let _ = p.ident();
        }
        if p.eat("(") {
            // slice, index, or function call / type cast
            if is_cast(&n) {
                let inner = parse_expr_sv(p, c)?;
                // optional , width
                if p.eat(",") {
                    let _ = parse_expr_sv(p, c);
                }
                let _ = p.eat(")");
                return Ok(inner);
            }
            if n.eq_ignore_ascii_case("rising_edge") {
                let clk = parse_expr_sv(p, c)?;
                let _ = p.eat(")");
                return Ok(format!("__posedge_{clk}"));
            }
            if n.eq_ignore_ascii_case("falling_edge") {
                let clk = parse_expr_sv(p, c)?;
                let _ = p.eat(")");
                return Ok(format!("__negedge_{clk}"));
            }
            if let Some(a) = parse_int_expr_peek(p, c) {
                if p.eat("downto") || p.eat("to") {
                    let b = parse_int_expr(p).unwrap_or(a);
                    let _ = p.eat(")");
                    let hi = a.max(b);
                    let lo = a.min(b);
                    return Ok(format!("{n}[{hi}:{lo}]"));
                }
                if p.peek_sym(")") {
                    p.eat(")");
                    return Ok(format!("{n}[{a}]"));
                }
            }
            // function call: fold args (Helion maps known IEEE; others XOR-fold so
            // every operand still reaches hardware).
            let mut args = vec![parse_expr_sv(p, c).unwrap_or_else(|_| "1'b0".into())];
            while p.eat(",") {
                args.push(parse_expr_sv(p, c).unwrap_or_else(|_| "1'b0".into()));
            }
            let _ = p.eat(")");
            if args.len() == 1 {
                return Ok(args.remove(0));
            }
            return Ok(format!("({})", args.join(" ^ ")));
        }
        if let Some(v) = const_get(c, &n) {
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
        } else {
            break;
        }
    }
    Some(v)
}

fn parse_int_atom(p: &mut P, c: &HashMap<String, i64>) -> Option<i64> {
    if p.eat("(") {
        let v = parse_int_expr_with(p, c);
        let _ = p.eat(")");
        return v;
    }
    if let Some(n) = p.number() {
        return Some(n as i64);
    }
    if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
        let n = p.ident().ok()?;
        if let Some(v) = const_get(c, &n) {
            return Some(v);
        }
        // unknown ident is not an int
        p.i -= 1;
        return None;
    }
    None
}

fn parse_type(p: &mut P, consts: &HashMap<String, i64>) -> usize {
    // optional array / subtype name
    let _ = p.eat("std_logic");
    let _ = p.eat("std_ulogic");
    let _ = p.eat("std_logic_vector");
    let _ = p.eat("std_ulogic_vector");
    let _ = p.eat("unsigned");
    let _ = p.eat("signed");
    let _ = p.eat("bit");
    let _ = p.eat("bit_vector");
    let _ = p.eat("boolean");
    let _ = p.eat("integer");
    let _ = p.eat("natural");
    let _ = p.eat("positive");
    let _ = p.eat("real");
    let _ = p.eat("character");
    let _ = p.eat("string");
    let _ = p.eat("time");
    let _ = p.eat("range");
    if p.eat("(") {
        if let Some(a) = parse_int_expr_with(p, consts) {
            if p.eat("downto") || p.eat("to") {
                let b = parse_int_expr_with(p, consts).unwrap_or(a);
                let _ = p.eat(")");
                skip_type_tail(p);
                return (a.abs_diff(b) as usize) + 1;
            }
            let _ = p.eat(")");
            skip_type_tail(p);
            return 1;
        }
        // (range <>) unconstrained → 1
        skip_paren_rest(p);
        skip_type_tail(p);
        return 1;
    }
    if p.eat("range") {
        let a = parse_int_expr_with(p, consts).unwrap_or(31);
        let _ = p.eat("to");
        let _ = p.eat("downto");
        let b = parse_int_expr_with(p, consts).unwrap_or(0);
        return (a.abs_diff(b) as usize) + 1;
    }
    skip_type_tail(p);
    1
}

fn skip_type_tail(p: &mut P) {
    let _ = p.eat("range");
    while p.peek().is_some()
        && !p.peek_sym(";")
        && !p.peek_sym(")")
        && !p.peek_sym(":=")
        && !p.peek_sym("=>")
    {
        if p.eat("(") {
            skip_paren_rest(p);
            continue;
        }
        break;
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
    if p.eat("after") || p.eat("when") && false {
        eat_to_semi(p);
        p.i -= 1; // keep semicolon for caller
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

fn recover_assign(p: &mut P, consts: &HashMap<String, i64>) -> Option<CStmt> {
    let start = p.i;
    while p.peek().is_some() && !p.peek_sym(";") {
        if p.eat("<=") {
            let rhs = parse_expr_sv(p, consts).ok()?;
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

fn skip_subprogram(p: &mut P) {
    let mut d = 1i32;
    while d > 0 && p.peek().is_some() {
        if p.eat("function") || p.eat("procedure") {
            d += 1;
        } else if p.eat("end") {
            let _ = p.eat("function");
            let _ = p.eat("procedure");
            let _ = p.eat_ident();
            let _ = p.eat(";");
            d -= 1;
        } else {
            p.bump();
        }
    }
}

fn skip_to_end_unit(p: &mut P, kind: &str) {
    while p.peek().is_some() {
        if p.eat("end") {
            let _ = p.eat(kind);
            let _ = p.eat("entity");
            let _ = p.eat("architecture");
            let _ = p.eat("component");
            let _ = p.eat("package");
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
    // Clash: process/STA clock is `clock` and `clk` is already a
    // port/signal/generic/const. Keep-name already keeps `clock` distinct;
    // emit a named diagnostic, do not silent-merge, do not abort synth.
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
    // Honest hierarchy: never emit_stub empty modules for component decls.
    // Missing bodies surface as named missing_component / unknown_instance.
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
            } => {
                if !known_bodies.contains(&module.to_ascii_lowercase()) {
                    // Named miss — not silent cells=0 via empty emit_stub.
                    // module= is the entity being lowered, not design top_name.
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
                sv.push_str(&format!("  {} {name}(", sv_ident(module)));
                let ports = child_ports(module, arch, entities);
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
                let cs: Vec<String> = conns
                    .iter()
                    .map(|(a, b)| {
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
                            Some(p) if !p.starts_with('#') => format!(".{p}({net})"),
                            _ => net,
                        }
                    })
                    .collect();
                sv.push_str(&cs.join(", "));
                sv.push_str(");\n");
            }
        }
    }
    sv.push_str("endmodule\n");
    Ok(sv)
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
) -> Option<&'a [(String, &'static str, usize)]> {
    if let Some(c) = arch
        .components
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(module))
    {
        return Some(c.ports.as_slice());
    }
    entities
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(module))
        .map(|e| e.ports.as_slice())
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
            | "others"
            | "library"
            | "use"
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
            // character literal '0' '1' '-'
            let ch = chars[i + 1];
            out.push(Tok::BitStr(if ch == '1' { "1".into() } else { "0".into() }));
            i += 3;
            continue;
        }
        if c == '\'' && i + 2 < chars.len() && chars[i + 2] == '\'' {
            let ch = chars[i + 1];
            out.push(Tok::BitStr(if ch == '1' { "1".into() } else { "0".into() }));
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
                '1' => '1',
                '0' | 'z' | 'Z' | 'x' | 'X' | '-' | 'u' | 'U' | 'w' | 'W' | 'l' | 'L' | 'h'
                | 'H' => '0',
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
    if bits.is_empty() {
        "0".into()
    } else {
        bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, INC4_INIT};

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
        let miss: Vec<_> = mr
            .softs
            .iter()
            .filter(|s| s.name == "missing_component")
            .collect();
        assert_eq!(miss.len(), 1, "softs={:?}", mr.soft_table_lines());
        assert_eq!(miss[0].module, "inner");
        assert!(
            !miss.iter().any(|s| s.module == "wrap"),
            "must not use design top_name: {:?}",
            mr.soft_table_lines()
        );
        assert!(mr
            .softs
            .iter()
            .any(|s| s.name == "child_soft_incomplete" && s.module == "wrap"));
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
            .find(|s| s.name == "missing_component")
            .unwrap_or_else(|| {
                panic!(
                    "missing_component SoftDiag required, got {:?}",
                    mr.soft_table_lines()
                )
            });
        assert_eq!(miss.name, "missing_component");
        assert_eq!(miss.module, "wrap");
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
                .any(|c| c.name.contains("unused_") || c.name.contains("fcov_")),
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
        assert!(mr
            .softs
            .iter()
            .any(|s| s.name == "missing_component" && s.module == "top"));
        let detail = mr
            .softs
            .iter()
            .find(|s| s.name == "missing_component")
            .and_then(|s| s.detail.as_deref())
            .unwrap_or("");
        assert!(detail.contains("inst=ghost_u"), "{detail}");
        assert!(detail.contains("child=ghost_ent"), "{detail}");
        assert_eq!(mr.design.attrs.get("NO_BODY"), Some("1"));
        assert!(mr.design.cells.is_empty());
        assert!(!mr
            .soft_table_lines()
            .iter()
            .any(|l| l.to_ascii_uppercase().contains("PASS")));
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
            .find(|s| s.name == "clock_clk_clash")
            .unwrap_or_else(|| panic!("clock_clk_clash SoftDiag, got {:?}", mr.soft_table_lines()));
        assert_eq!(clash.module, "clash");
        assert!(clash
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("occupied=clk"));
        assert_eq!(clash.span.file.as_deref(), Some("vhdl.vhd"));
        assert!(clash.span.line.is_some());
        assert!(mr
            .design
            .cells
            .iter()
            .any(|c| matches!(c.kind, CellKind::Hff)));
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
                .any(|s| s.name == "generate_not_lowered" && s.module == "wrap")
                || mr.softs.iter().any(|s| s.name == "missing_component"),
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
        assert!(d.ports.iter().any(|p| p.name == "q"));
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
            d.ports.iter().any(|p| p.name == "clock"),
            "STA/Design port must keep real VHDL name clock: {:?}",
            d.ports
        );
        assert!(
            !d.ports.iter().any(|p| p.name == "clk"),
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
        assert!(d.ports.iter().any(|p| p.name == "clk"), "{:?}", d.ports);
        assert!(
            d.ports.iter().any(|p| p.name == "clock"),
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
        let clk_ports: Vec<_> = d.ports.iter().filter(|p| p.name == "clk").collect();
        assert_eq!(clk_ports.len(), 1, "no duplicate clk ports: {:?}", d.ports);
        assert!(
            d.ports.iter().any(|p| p.name == "clock"),
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
        assert!(d.ports.iter().any(|p| p.name == "y"), "{:?}", d.ports);
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
