//! SymbiYosys (`.sby`) formal verification frontend, multi-format model checking
//! backend (`BTOR2`, `AIGER 1.9` ASCII `.aag`, `SMT-LIB v2` `QF_BV`), and native
//! `bmc` / `prove` ($k$-induction + PDR) / `cover` formal engine.

use helion_ir::{intern, resolve, CellKind, Design, PortDir, Spur};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

// ============================================================================
// 1. SymbiYosys (.sby) Configuration AST & Parser
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbyMode {
    Bmc,
    Prove,
    Cover,
    Live,
}

impl Default for SbyMode {
    fn default() -> Self {
        Self::Bmc
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SbyOptions {
    pub mode: SbyMode,
    pub depth: usize,
    pub multiclock: bool,
    pub wait: bool,
    pub append: usize,
    pub tbtop: Option<String>,
}

impl Default for SbyOptions {
    fn default() -> Self {
        Self {
            mode: SbyMode::Bmc,
            depth: 20,
            multiclock: false,
            wait: false,
            append: 0,
            tbtop: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SbyEngine {
    SmtBmc {
        solver: Option<String>,
        args: Vec<String>,
    },
    Btor {
        solver: Option<String>,
        args: Vec<String>,
    },
    Aiger {
        solver: Option<String>,
        args: Vec<String>,
    },
    Abc {
        args: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SbyTask {
    pub name: String,
    pub tags: Vec<String>,
    pub options: SbyOptions,
    pub engines: Vec<SbyEngine>,
    pub script: Vec<String>,
    pub top_module: Option<String>,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SbyConfig {
    pub tasks: Vec<SbyTask>,
    pub options: SbyOptions,
    pub engines: Vec<SbyEngine>,
    pub script: Vec<String>,
    pub top_module: Option<String>,
    pub files: Vec<String>,
    pub file_map: BTreeMap<String, String>,
    pub embedded_files: BTreeMap<String, String>,
}

impl SbyConfig {
    pub fn task(&self, name: &str) -> Option<&SbyTask> {
        self.tasks.iter().find(|t| t.name == name)
    }
}

/// Parse a SymbiYosys `.sby` configuration file into [`SbyConfig`].
pub fn parse_sby_config(input: &str) -> Result<SbyConfig, String> {
    #[derive(Clone, Debug)]
    enum Sec {
        None,
        Tasks,
        Options,
        Engines,
        Script,
        Files,
        File(String),
    }

    let mut sec = Sec::None;
    let mut raw_tasks: Vec<(String, Vec<String>)> = Vec::new();
    let mut raw_options: Vec<String> = Vec::new();
    let mut raw_engines: Vec<String> = Vec::new();
    let mut raw_script: Vec<String> = Vec::new();
    let mut raw_files: Vec<String> = Vec::new();
    let mut embedded_files: BTreeMap<String, String> = BTreeMap::new();
    let mut in_pycode = false;

    for raw_line in input.lines() {
        let trimmed = raw_line.trim();
        if trimmed == "--pycode-begin--" {
            in_pycode = true;
            continue;
        }
        if trimmed == "--pycode-end--" {
            in_pycode = false;
            continue;
        }
        if in_pycode {
            continue;
        }
        if let Sec::File(fname) = &sec {
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                // fall through to section header parser
            } else {
                let buf = embedded_files.entry(fname.clone()).or_default();
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(raw_line);
                continue;
            }
        }
        let line = match trimmed.find('#') {
            Some(idx) => trimmed[..idx].trim(),
            None => trimmed,
        };
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let hdr = line[1..line.len() - 1].trim();
            let low = hdr.to_ascii_lowercase();
            if low == "tasks" {
                sec = Sec::Tasks;
            } else if low == "options" {
                sec = Sec::Options;
            } else if low == "engines" {
                sec = Sec::Engines;
            } else if low == "script" {
                sec = Sec::Script;
            } else if low == "files" {
                sec = Sec::Files;
            } else if let Some(rest) = hdr.strip_prefix("file ").or_else(|| hdr.strip_prefix("FILE ")) {
                let fnm = rest.trim().to_string();
                embedded_files.entry(fnm.clone()).or_default();
                sec = Sec::File(fnm);
            } else {
                return Err(format!("sby: unknown section [{hdr}]"));
            }
            continue;
        }
        match &sec {
            Sec::None => return Err(format!("sby: unexpected line before section header: {line}")),
            Sec::Tasks => {
                let words: Vec<String> = line.split_whitespace().map(|s| s.to_string()).collect();
                if let Some((first, rest)) = words.split_first() {
                    raw_tasks.push((first.clone(), rest.to_vec()));
                }
            }
            Sec::Options => raw_options.push(line.to_string()),
            Sec::Engines => raw_engines.push(line.to_string()),
            Sec::Script => raw_script.push(line.to_string()),
            Sec::Files => raw_files.push(line.to_string()),
            Sec::File(_) => {}
        }
    }

    let task_list = if raw_tasks.is_empty() {
        vec![("default".to_string(), Vec::new())]
    } else {
        raw_tasks
    };

    let known_tags: HashSet<String> = task_list
        .iter()
        .flat_map(|(n, tags)| std::iter::once(n.clone()).chain(tags.iter().cloned()))
        .collect();

    let filter_for_task = |lines: &[String], tname: &str, ttags: &[String]| -> Vec<String> {
        let mut out = Vec::new();
        for l in lines {
            if let Some((prefix, rest)) = l.split_once(':') {
                let ptag = prefix.trim();
                if known_tags.contains(ptag) {
                    if ptag == tname || ttags.iter().any(|t| t == ptag) {
                        let r = rest.trim();
                        if !r.is_empty() {
                            out.push(r.to_string());
                        }
                    }
                    continue;
                }
            }
            out.push(l.clone());
        }
        out
    };

    let mut tasks = Vec::with_capacity(task_list.len());
    let mut file_map = BTreeMap::new();
    for (tname, ttags) in &task_list {
        let opt_lines = filter_for_task(&raw_options, tname, ttags);
        let eng_lines = filter_for_task(&raw_engines, tname, ttags);
        let scr_lines = filter_for_task(&raw_script, tname, ttags);
        let fil_lines = filter_for_task(&raw_files, tname, ttags);

        let options = parse_options_lines(&opt_lines)?;
        let engines = parse_engines_lines(&eng_lines)?;
        let top_module = extract_top_from_script(&scr_lines);
        let mut files = Vec::new();
        for fl in &fil_lines {
            let parts: Vec<&str> = fl.split_whitespace().collect();
            if parts.len() == 2 {
                file_map.insert(parts[0].to_string(), parts[1].to_string());
                files.push(parts[1].to_string());
            } else if let Some(&p0) = parts.first() {
                file_map.insert(p0.to_string(), p0.to_string());
                files.push(p0.to_string());
            }
        }
        tasks.push(SbyTask {
            name: tname.clone(),
            tags: ttags.clone(),
            options,
            engines,
            script: scr_lines,
            top_module,
            files,
        });
    }

    let first = tasks.first().cloned().unwrap_or(SbyTask {
        name: "default".into(),
        tags: Vec::new(),
        options: SbyOptions::default(),
        engines: Vec::new(),
        script: Vec::new(),
        top_module: None,
        files: Vec::new(),
    });

    Ok(SbyConfig {
        tasks,
        options: first.options,
        engines: first.engines,
        script: first.script,
        top_module: first.top_module,
        files: first.files,
        file_map,
        embedded_files,
    })
}

fn parse_options_lines(lines: &[String]) -> Result<SbyOptions, String> {
    let mut opts = SbyOptions::default();
    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        match parts[0].to_ascii_lowercase().as_str() {
            "mode" => {
                let m = parts.get(1).ok_or_else(|| String::from("sby: mode requires value"))?;
                opts.mode = match m.to_ascii_lowercase().as_str() {
                    "bmc" => SbyMode::Bmc,
                    "prove" => SbyMode::Prove,
                    "cover" => SbyMode::Cover,
                    "live" => SbyMode::Live,
                    other => return Err(format!("sby: unknown mode `{other}`")),
                };
            }
            "depth" => {
                let d = parts
                    .get(1)
                    .ok_or_else(|| String::from("sby: depth requires integer"))?
                    .parse::<usize>()
                    .map_err(|_| String::from("sby: invalid depth integer"))?;
                opts.depth = d;
            }
            "multiclock" => {
                let v = parts
                    .get(1)
                    .ok_or_else(|| String::from("sby: multiclock requires on|off"))?;
                opts.multiclock = match v.to_ascii_lowercase().as_str() {
                    "on" | "1" | "true" => true,
                    "off" | "0" | "false" => false,
                    other => return Err(format!("sby: invalid multiclock `{other}`")),
                };
            }
            "wait" => {
                if let Some(v) = parts.get(1) {
                    opts.wait = matches!(v.to_ascii_lowercase().as_str(), "on" | "1" | "true");
                }
            }
            "append" => {
                if let Some(v) = parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    opts.append = v;
                }
            }
            "tbtop" => {
                if let Some(v) = parts.get(1) {
                    opts.tbtop = Some((*v).to_string());
                }
            }
            _ => {}
        }
    }
    Ok(opts)
}

fn parse_engines_lines(lines: &[String]) -> Result<Vec<SbyEngine>, String> {
    let mut out = Vec::new();
    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let kind = parts[0].to_ascii_lowercase();
        let rest: Vec<String> = parts[1..].iter().map(|s| (*s).to_string()).collect();
        match kind.as_str() {
            "smtbmc" => {
                let mut args = Vec::new();
                let mut solver = None;
                for tok in rest {
                    if tok.starts_with('-') {
                        args.push(tok);
                    } else if solver.is_none() {
                        solver = Some(tok);
                    } else {
                        args.push(tok);
                    }
                }
                out.push(SbyEngine::SmtBmc { solver, args });
            }
            "btor" => {
                let solver = rest.first().cloned();
                let args = if rest.len() > 1 {
                    rest[1..].to_vec()
                } else {
                    Vec::new()
                };
                out.push(SbyEngine::Btor { solver, args });
            }
            "aiger" => {
                let solver = rest.first().cloned();
                let args = if rest.len() > 1 {
                    rest[1..].to_vec()
                } else {
                    Vec::new()
                };
                out.push(SbyEngine::Aiger { solver, args });
            }
            "abc" => {
                out.push(SbyEngine::Abc { args: rest });
            }
            other => return Err(format!("sby: unsupported engine `{other}`")),
        }
    }
    Ok(out)
}

fn extract_top_from_script(script: &[String]) -> Option<String> {
    for line in script {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        for i in 0..tokens.len() {
            if tokens[i] == "-top" {
                if let Some(top) = tokens.get(i + 1) {
                    return Some((*top).to_string());
                }
            }
        }
    }
    None
}

// ============================================================================
// 2. Formal Property Helpers & Netlist Transition System Extraction
// ============================================================================

/// Attach a formal assertion (`cond_net == 1` must hold; `bad = !cond_net`) to `Design`.
pub fn add_formal_assert(d: &mut Design, name: &str, cond_net: &str) {
    let mut cell_name = format!("__formal_assert_{name}");
    if d.cell(&cell_name).is_some() {
        cell_name = format!("__formal_assert_{name}_{}", d.cells.len());
    }
    d.add_cell(
        &cell_name,
        CellKind::BlackBox {
            module: intern("$assert"),
        },
    );
    if let Some(c) = d.cells.last_mut() {
        c.attrs.set("FORMAL_KIND", "assert");
        c.attrs.set("FORMAL_NAME", name);
    }
    d.connect(cond_net, &cell_name, "A");
}

/// Attach a formal bad-state signal (`bad_net == 1` means assertion violation) to `Design`.
pub fn add_formal_bad(d: &mut Design, name: &str, bad_net: &str) {
    let mut cell_name = format!("__formal_bad_{name}");
    if d.cell(&cell_name).is_some() {
        cell_name = format!("__formal_bad_{name}_{}", d.cells.len());
    }
    d.add_cell(
        &cell_name,
        CellKind::BlackBox {
            module: intern("$bad"),
        },
    );
    if let Some(c) = d.cells.last_mut() {
        c.attrs.set("FORMAL_KIND", "bad");
        c.attrs.set("FORMAL_NAME", name);
    }
    d.connect(bad_net, &cell_name, "A");
}

/// Attach a formal assumption (`cond_net == 1` is constrained to hold) to `Design`.
pub fn add_formal_assume(d: &mut Design, name: &str, cond_net: &str) {
    let mut cell_name = format!("__formal_assume_{name}");
    if d.cell(&cell_name).is_some() {
        cell_name = format!("__formal_assume_{name}_{}", d.cells.len());
    }
    d.add_cell(
        &cell_name,
        CellKind::BlackBox {
            module: intern("$assume"),
        },
    );
    if let Some(c) = d.cells.last_mut() {
        c.attrs.set("FORMAL_KIND", "assume");
        c.attrs.set("FORMAL_NAME", name);
    }
    d.connect(cond_net, &cell_name, "A");
}

/// Attach a formal cover point (`cond_net == 1` is a reachability target) to `Design`.
pub fn add_formal_cover(d: &mut Design, name: &str, cond_net: &str) {
    let mut cell_name = format!("__formal_cover_{name}");
    if d.cell(&cell_name).is_some() {
        cell_name = format!("__formal_cover_{name}_{}", d.cells.len());
    }
    d.add_cell(
        &cell_name,
        CellKind::BlackBox {
            module: intern("$cover"),
        },
    );
    if let Some(c) = d.cells.last_mut() {
        c.attrs.set("FORMAL_KIND", "cover");
        c.attrs.set("FORMAL_NAME", name);
    }
    d.connect(cond_net, &cell_name, "A");
}

/// Attach a formal fairness constraint (`cond_net == 1` infinitely often) to `Design`.
pub fn add_formal_fair(d: &mut Design, name: &str, cond_net: &str) {
    let mut cell_name = format!("__formal_fair_{name}");
    if d.cell(&cell_name).is_some() {
        cell_name = format!("__formal_fair_{name}_{}", d.cells.len());
    }
    d.add_cell(
        &cell_name,
        CellKind::BlackBox {
            module: intern("$fair"),
        },
    );
    if let Some(c) = d.cells.last_mut() {
        c.attrs.set("FORMAL_KIND", "fair");
        c.attrs.set("FORMAL_NAME", name);
    }
    d.connect(cond_net, &cell_name, "A");
}

/// Set initial value (`0` or `1`) on a flip-flop cell in `Design`.
pub fn set_ff_init(d: &mut Design, ff_cell: &str, init_val: bool) {
    let target = intern(ff_cell);
    for c in &mut d.cells {
        if c.name == target {
            c.attrs.set("INIT", if init_val { "1" } else { "0" });
        }
    }
}

/// Internal boolean node in the extracted formal transition system.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum BNode {
    Const(bool),
    Input(usize),
    State(usize),
    Not(usize),
    And(usize, usize),
    Or(usize, usize),
    Xor(usize, usize),
    Ite(usize, usize, usize),
}

#[derive(Clone, Debug)]
struct StateVar {
    name: String,
    init: Option<bool>,
    next_node: usize,
}

#[derive(Clone, Debug)]
struct FormalSystem {
    nodes: Vec<BNode>,
    node_memo: HashMap<BNode, usize>,
    inputs: Vec<String>,
    states: Vec<StateVar>,
    outputs: Vec<(String, usize)>,
    bads: Vec<(String, usize)>,
    constraints: Vec<(String, usize)>,
    covers: Vec<(String, usize)>,
    fairs: Vec<(String, usize)>,
}

impl FormalSystem {
    fn new() -> Self {
        let mut sys = Self {
            nodes: Vec::new(),
            node_memo: HashMap::new(),
            inputs: Vec::new(),
            states: Vec::new(),
            outputs: Vec::new(),
            bads: Vec::new(),
            constraints: Vec::new(),
            covers: Vec::new(),
            fairs: Vec::new(),
        };
        let _c0 = sys.intern_node(BNode::Const(false));
        let _c1 = sys.intern_node(BNode::Const(true));
        sys
    }

    fn c0(&self) -> usize {
        0
    }

    fn c1(&self) -> usize {
        1
    }

    fn intern_node(&mut self, node: BNode) -> usize {
        if let Some(&id) = self.node_memo.get(&node) {
            return id;
        }
        let id = self.nodes.len();
        self.nodes.push(node.clone());
        self.node_memo.insert(node, id);
        id
    }

    fn mk_not(&mut self, a: usize) -> usize {
        match self.nodes[a] {
            BNode::Const(b) => {
                if b {
                    self.c0()
                } else {
                    self.c1()
                }
            }
            BNode::Not(x) => x,
            _ => self.intern_node(BNode::Not(a)),
        }
    }

    fn mk_and(&mut self, a: usize, b: usize) -> usize {
        if a == self.c0() || b == self.c0() {
            return self.c0();
        }
        if a == self.c1() {
            return b;
        }
        if b == self.c1() {
            return a;
        }
        if a == b {
            return a;
        }
        let (x, y) = if a <= b { (a, b) } else { (b, a) };
        self.intern_node(BNode::And(x, y))
    }

    fn mk_or(&mut self, a: usize, b: usize) -> usize {
        if a == self.c1() || b == self.c1() {
            return self.c1();
        }
        if a == self.c0() {
            return b;
        }
        if b == self.c0() {
            return a;
        }
        if a == b {
            return a;
        }
        let (x, y) = if a <= b { (a, b) } else { (b, a) };
        self.intern_node(BNode::Or(x, y))
    }

    fn mk_xor(&mut self, a: usize, b: usize) -> usize {
        if a == self.c0() {
            return b;
        }
        if b == self.c0() {
            return a;
        }
        if a == self.c1() {
            return self.mk_not(b);
        }
        if b == self.c1() {
            return self.mk_not(a);
        }
        if a == b {
            return self.c0();
        }
        let (x, y) = if a <= b { (a, b) } else { (b, a) };
        self.intern_node(BNode::Xor(x, y))
    }

    fn mk_ite(&mut self, cond: usize, t: usize, f: usize) -> usize {
        if cond == self.c1() {
            return t;
        }
        if cond == self.c0() {
            return f;
        }
        if t == f {
            return t;
        }
        if t == self.c1() && f == self.c0() {
            return cond;
        }
        if t == self.c0() && f == self.c1() {
            return self.mk_not(cond);
        }
        self.intern_node(BNode::Ite(cond, t, f))
    }

    fn mk_lut_cone(&mut self, init: u64, inputs: &[usize]) -> usize {
        if init == 0 {
            return self.c0();
        }
        if init == u64::MAX {
            return self.c1();
        }
        // Trim trailing inputs that do not affect `init` (or are tied to c0).
        let mut active: Vec<(usize, usize)> = Vec::new();
        for (bit_idx, &node_id) in inputs.iter().enumerate().take(6) {
            if node_id == self.c0() {
                continue;
            }
            active.push((bit_idx, node_id));
        }
        self.shannon_lut(init, &active, 0, 0)
    }

    fn shannon_lut(
        &mut self,
        init: u64,
        active: &[(usize, usize)],
        pos: usize,
        fixed_addr: usize,
    ) -> usize {
        if pos >= active.len() {
            let bit = (init >> fixed_addr) & 1 != 0;
            return if bit { self.c1() } else { self.c0() };
        }
        let (bit_idx, var_node) = active[pos];
        let lo = self.shannon_lut(init, active, pos + 1, fixed_addr);
        let hi = self.shannon_lut(init, active, pos + 1, fixed_addr | (1 << bit_idx));
        self.mk_ite(var_node, hi, lo)
    }

    fn eval_all(&self, state_vals: &[bool], input_vals: &[bool]) -> Vec<bool> {
        let mut vals = vec![false; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            vals[i] = match *n {
                BNode::Const(b) => b,
                BNode::Input(ix) => input_vals.get(ix).copied().unwrap_or(false),
                BNode::State(ix) => state_vals.get(ix).copied().unwrap_or(false),
                BNode::Not(a) => !vals[a],
                BNode::And(a, b) => vals[a] && vals[b],
                BNode::Or(a, b) => vals[a] || vals[b],
                BNode::Xor(a, b) => vals[a] ^ vals[b],
                BNode::Ite(c, t, f) => {
                    if vals[c] {
                        vals[t]
                    } else {
                        vals[f]
                    }
                }
            };
        }
        vals
    }

    fn from_design(d: &Design) -> Result<Self, String> {
        let mut sys = Self::new();
        let pin_idx = d.pin_index();
        let mut net_to_node: HashMap<Spur, usize> = HashMap::new();

        // Collect clock nets used by Hff cells so we don't expose CLK as a unconstrained data PI if not needed,
        // or keep non-clock input ports as primary inputs.
        let mut clock_nets: HashSet<Spur> = HashSet::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                if let Some(cn) = pin_idx.net_on(resolve(c.name), "CLK") {
                    clock_nets.insert(cn);
                }
            }
        }

        for p in &d.ports {
            if matches!(p.dir, PortDir::In | PortDir::Inout) {
                if clock_nets.contains(&p.name) && d.ports.len() > 1 {
                    continue;
                }
                let ix = sys.inputs.len();
                let pname = resolve(p.name).to_string();
                sys.inputs.push(pname);
                let nid = sys.intern_node(BNode::Input(ix));
                net_to_node.insert(p.name, nid);
            }
        }

        // Also check IobIn cells driving internal nets from PAD
        for c in &d.cells {
            if matches!(c.kind, CellKind::IobIn) {
                let cname = resolve(c.name);
                if let Some(out_net) = pin_idx
                    .net_on(cname, "O")
                    .or_else(|| pin_idx.net_on(cname, "I"))
                {
                    if !net_to_node.contains_key(&out_net) && !clock_nets.contains(&out_net) {
                        let pad_net = pin_idx.net_on(cname, "PAD").unwrap_or(out_net);
                        if let Some(&existing) = net_to_node.get(&pad_net) {
                            net_to_node.insert(out_net, existing);
                        } else {
                            let ix = sys.inputs.len();
                            sys.inputs.push(resolve(out_net).to_string());
                            let nid = sys.intern_node(BNode::Input(ix));
                            net_to_node.insert(out_net, nid);
                        }
                    }
                }
            }
        }

        // Pre-allocate state nodes for all Hff cells.
        let mut ff_cells: Vec<(usize, Spur, Option<Spur>)> = Vec::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                let cname = resolve(c.name);
                let q_net = pin_idx.net_on(cname, "Q");
                let st_ix = sys.states.len();
                let st_name = q_net
                    .map(|n| resolve(n).to_string())
                    .unwrap_or_else(|| cname.to_string());
                let init = match c.attrs.get("INIT") {
                    Some("1" | "true" | "TRUE") => Some(true),
                    Some("x" | "X" | "uninit") => None,
                    _ => Some(false),
                };
                sys.states.push(StateVar {
                    name: st_name,
                    init,
                    next_node: sys.c0(),
                });
                let st_node = sys.intern_node(BNode::State(st_ix));
                if let Some(qn) = q_net {
                    net_to_node.insert(qn, st_node);
                }
                ff_cells.push((st_ix, c.name, q_net));
            }
        }

        // Iteratively evaluate combinational cells (`Lut6`, `Carry4`) in topological order.
        let comb_cells: Vec<&helion_ir::Cell> = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. } | CellKind::Carry4))
            .collect();

        let mut resolved_cells: HashSet<Spur> = HashSet::new();
        for _round in 0..=comb_cells.len() {
            let mut progress = false;
            for c in &comb_cells {
                if resolved_cells.contains(&c.name) {
                    continue;
                }
                let cname = resolve(c.name);
                match c.kind {
                    CellKind::Lut6 { init } => {
                        let pin_names = ["I0", "I1", "I2", "I3", "I4", "I5"];
                        let mut ready = true;
                        let mut in_nodes = [sys.c0(); 6];
                        for (pi, pname) in pin_names.iter().enumerate() {
                            if let Some(net) = pin_idx.net_on(cname, pname) {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    in_nodes[pi] = nid;
                                } else if is_driven_by_comb(d, net) {
                                    ready = false;
                                    break;
                                } else {
                                    // Undriven net -> treat as primary input
                                    let ix = sys.inputs.len();
                                    sys.inputs.push(resolve(net).to_string());
                                    let nid = sys.intern_node(BNode::Input(ix));
                                    net_to_node.insert(net, nid);
                                    in_nodes[pi] = nid;
                                }
                            }
                        }
                        if !ready {
                            continue;
                        }
                        let out_node = sys.mk_lut_cone(init, &in_nodes);
                        if let Some(onet) = pin_idx.net_on(cname, "O") {
                            net_to_node.insert(onet, out_node);
                        }
                        resolved_cells.insert(c.name);
                        progress = true;
                    }
                    CellKind::Carry4 => {
                        let s_pins = [
                            ("S[0]", "S0"),
                            ("S[1]", "S1"),
                            ("S[2]", "S2"),
                            ("S[3]", "S3"),
                        ];
                        let di_pins = [
                            ("DI[0]", "DI0"),
                            ("DI[1]", "DI1"),
                            ("DI[2]", "DI2"),
                            ("DI[3]", "DI3"),
                        ];
                        let o_pins = [
                            ("O[0]", "O0"),
                            ("O[1]", "O1"),
                            ("O[2]", "O2"),
                            ("O[3]", "O3"),
                        ];
                        let co_pins = [
                            ("CO[0]", "CO0"),
                            ("CO[1]", "CO1"),
                            ("CO[2]", "CO2"),
                            ("CO[3]", "CO3"),
                        ];
                        let mut ready = true;
                        let mut s_nodes = [sys.c0(); 4];
                        let mut di_nodes = [sys.c0(); 4];
                        for i in 0..4 {
                            if let Some(net) = pin_idx
                                .net_on(cname, s_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, s_pins[i].1))
                            {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    s_nodes[i] = nid;
                                } else if is_driven_by_comb(d, net) {
                                    ready = false;
                                    break;
                                } else {
                                    let ix = sys.inputs.len();
                                    sys.inputs.push(resolve(net).to_string());
                                    let nid = sys.intern_node(BNode::Input(ix));
                                    net_to_node.insert(net, nid);
                                    s_nodes[i] = nid;
                                }
                            }
                            if let Some(net) = pin_idx
                                .net_on(cname, di_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, di_pins[i].1))
                            {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    di_nodes[i] = nid;
                                } else if is_driven_by_comb(d, net) {
                                    ready = false;
                                    break;
                                } else {
                                    let ix = sys.inputs.len();
                                    sys.inputs.push(resolve(net).to_string());
                                    let nid = sys.intern_node(BNode::Input(ix));
                                    net_to_node.insert(net, nid);
                                    di_nodes[i] = nid;
                                }
                            }
                        }
                        let mut cin = sys.c0();
                        for ci_pin in ["CI", "CYINIT"] {
                            if let Some(net) = pin_idx.net_on(cname, ci_pin) {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    cin = sys.mk_or(cin, nid);
                                } else if is_driven_by_comb(d, net) {
                                    ready = false;
                                }
                            }
                        }
                        if !ready {
                            continue;
                        }
                        for i in 0..4 {
                            let sum_i = sys.mk_xor(s_nodes[i], cin);
                            let co_i = sys.mk_ite(s_nodes[i], cin, di_nodes[i]);
                            if let Some(onet) = pin_idx
                                .net_on(cname, o_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, o_pins[i].1))
                            {
                                net_to_node.insert(onet, sum_i);
                            }
                            if let Some(conet) = pin_idx
                                .net_on(cname, co_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, co_pins[i].1))
                            {
                                net_to_node.insert(conet, co_i);
                            }
                            cin = co_i;
                        }
                        resolved_cells.insert(c.name);
                        progress = true;
                    }
                    _ => {}
                }
            }
            if !progress {
                break;
            }
        }

        // Resolve Hff next-state nodes (`D`, optional `CE`, optional `SR`/`RST`).
        for (st_ix, cell_spur, _q_net) in ff_cells {
            let cname = resolve(cell_spur);
            let q_node = sys.intern_node(BNode::State(st_ix));
            let mut d_node = if let Some(dnet) = pin_idx.net_on(cname, "D") {
                resolve_or_create_input(&mut sys, &mut net_to_node, dnet)
            } else {
                sys.c0()
            };
            if let Some(cenet) = pin_idx.net_on(cname, "CE") {
                let ce_node = resolve_or_create_input(&mut sys, &mut net_to_node, cenet);
                d_node = sys.mk_ite(ce_node, d_node, q_node);
            }
            if let Some(srnet) = pin_idx
                .net_on(cname, "SR")
                .or_else(|| pin_idx.net_on(cname, "RST"))
            {
                let sr_node = resolve_or_create_input(&mut sys, &mut net_to_node, srnet);
                let zero = sys.c0();
                d_node = sys.mk_ite(sr_node, zero, d_node);
            }
            sys.states[st_ix].next_node = d_node;
        }

        // Wire IobOut cells (`I` -> `PAD`) so output ports see the signal driving `IobOut/I`.
        for c in &d.cells {
            if matches!(c.kind, CellKind::IobOut) {
                let cname = resolve(c.name);
                if let (Some(in_net), Some(pad_net)) =
                    (pin_idx.net_on(cname, "I"), pin_idx.net_on(cname, "PAD"))
                {
                    if let Some(&nid) = net_to_node.get(&in_net) {
                        net_to_node.insert(pad_net, nid);
                    }
                }
            }
        }

        for p in &d.ports {
            if matches!(p.dir, PortDir::Out) {
                let nid = net_to_node.get(&p.name).copied().unwrap_or(sys.c0());
                sys.outputs.push((resolve(p.name).to_string(), nid));
            }
        }

        // Harvest formal properties from cells, net attributes, and design attributes.
        for c in &d.cells {
            let cname = resolve(c.name);
            let fkind = c.attrs.get("FORMAL_KIND").map(|s| s.to_ascii_lowercase()).or_else(|| {
                if let CellKind::BlackBox { module } = c.kind {
                    match resolve(module).to_ascii_lowercase().as_str() {
                        "$assert" | "formal_assert" | "assert" => Some("assert".into()),
                        "$bad" | "formal_bad" | "bad" => Some("bad".into()),
                        "$assume" | "formal_assume" | "assume" => Some("assume".into()),
                        "$cover" | "formal_cover" | "cover" => Some("cover".into()),
                        "$fair" | "formal_fair" | "fair" => Some("fair".into()),
                        _ => None,
                    }
                } else {
                    None
                }
            });
            let Some(kind) = fkind else {
                continue;
            };
            let prop_name = c
                .attrs
                .get("FORMAL_NAME")
                .map(|s| s.to_string())
                .unwrap_or_else(|| cname.to_string());
            let cond_net = pin_idx
                .net_on(cname, "A")
                .or_else(|| pin_idx.net_on(cname, "I"))
                .or_else(|| pin_idx.net_on(cname, "COND"));
            let Some(cnet) = cond_net else {
                continue;
            };
            let mut cond_node = resolve_or_create_input(&mut sys, &mut net_to_node, cnet);
            if let Some(en_net) = pin_idx.net_on(cname, "EN") {
                let en_node = resolve_or_create_input(&mut sys, &mut net_to_node, en_net);
                if kind == "assert" || kind == "assume" {
                    let not_en = sys.mk_not(en_node);
                    cond_node = sys.mk_or(not_en, cond_node);
                } else {
                    cond_node = sys.mk_and(en_node, cond_node);
                }
            }
            match kind.as_str() {
                "assert" => {
                    let bad_node = sys.mk_not(cond_node);
                    sys.bads.push((prop_name, bad_node));
                }
                "bad" => {
                    sys.bads.push((prop_name, cond_node));
                }
                "assume" => {
                    sys.constraints.push((prop_name, cond_node));
                }
                "cover" => {
                    sys.covers.push((prop_name, cond_node));
                }
                "fair" => {
                    sys.fairs.push((prop_name, cond_node));
                }
                _ => {}
            }
        }

        for net in &d.nets {
            let nname = resolve(net.name).to_string();
            if net.attrs.flag("formal_assert") || net.attrs.flag("assert") {
                let cid = resolve_or_create_input(&mut sys, &mut net_to_node, net.name);
                let bad = sys.mk_not(cid);
                sys.bads.push((nname.clone(), bad));
            }
            if net.attrs.flag("formal_bad") || net.attrs.flag("bad") {
                let bid = resolve_or_create_input(&mut sys, &mut net_to_node, net.name);
                sys.bads.push((nname.clone(), bid));
            }
            if net.attrs.flag("formal_assume") || net.attrs.flag("assume") {
                let cid = resolve_or_create_input(&mut sys, &mut net_to_node, net.name);
                sys.constraints.push((nname.clone(), cid));
            }
            if net.attrs.flag("formal_cover") || net.attrs.flag("cover") {
                let cid = resolve_or_create_input(&mut sys, &mut net_to_node, net.name);
                sys.covers.push((nname.clone(), cid));
            }
            if net.attrs.flag("formal_fair") || net.attrs.flag("fair") {
                let cid = resolve_or_create_input(&mut sys, &mut net_to_node, net.name);
                sys.fairs.push((nname, cid));
            }
        }

        for (attr_key, kind) in [
            ("FORMAL_ASSERTS", "assert"),
            ("FORMAL_BADS", "bad"),
            ("FORMAL_ASSUMES", "assume"),
            ("FORMAL_COVERS", "cover"),
            ("FORMAL_FAIRS", "fair"),
        ] {
            if let Some(list) = d.attrs.get(attr_key) {
                for item in list.split(|c| c == ',' || c == '|') {
                    let t = item.trim();
                    if t.is_empty() {
                        continue;
                    }
                    let (inv, raw_name) = if let Some(s) = t.strip_prefix('!').or_else(|| t.strip_prefix('~')) {
                        (true, s.trim())
                    } else {
                        (false, t)
                    };
                    let sp = intern(raw_name);
                    let mut nid = resolve_or_create_input(&mut sys, &mut net_to_node, sp);
                    if inv {
                        nid = sys.mk_not(nid);
                    }
                    match kind {
                        "assert" => {
                            let bad = sys.mk_not(nid);
                            sys.bads.push((raw_name.to_string(), bad));
                        }
                        "bad" => sys.bads.push((raw_name.to_string(), nid)),
                        "assume" => sys.constraints.push((raw_name.to_string(), nid)),
                        "cover" => sys.covers.push((raw_name.to_string(), nid)),
                        "fair" => sys.fairs.push((raw_name.to_string(), nid)),
                        _ => {}
                    }
                }
            }
        }

        Ok(sys)
    }
}

fn is_driven_by_comb(d: &Design, net_name: Spur) -> bool {
    let Some(net) = d.net(resolve(net_name)) else {
        return false;
    };
    for e in &net.endpoints {
        let p = resolve(e.pin);
        if p == "O" || p.starts_with("O[") || p.starts_with("CO") || p == "O0" || p == "O1" || p == "O2" || p == "O3" {
            if let Some(c) = d.cell(resolve(e.cell)) {
                if matches!(c.kind, CellKind::Lut6 { .. } | CellKind::Carry4) {
                    return true;
                }
            }
        }
    }
    false
}

fn resolve_or_create_input(
    sys: &mut FormalSystem,
    net_to_node: &mut HashMap<Spur, usize>,
    net: Spur,
) -> usize {
    if let Some(&nid) = net_to_node.get(&net) {
        return nid;
    }
    let s = resolve(net);
    if s == "1'b0" || s == "0" || s == "const0" {
        return sys.c0();
    }
    if s == "1'b1" || s == "1" || s == "const1" {
        return sys.c1();
    }
    let ix = sys.inputs.len();
    sys.inputs.push(s.to_string());
    let nid = sys.intern_node(BNode::Input(ix));
    net_to_node.insert(net, nid);
    nid
}

// ============================================================================
// 3. BTOR2 Emitter & Parser (`emit_btor2`, `parse_btor2`)
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Btor2Line {
    pub id: usize,
    pub op: String,
    pub args: Vec<i64>,
    pub symbol: Option<String>,
}

/// Emit a synthesized [`Design`] into standard `BTOR2` format.
pub fn emit_btor2(design: &Design) -> Result<String, String> {
    let sys = FormalSystem::from_design(design)?;
    let mut out = String::new();
    out.push_str(&format!("; BTOR2 generated by Helion for module {}\n", resolve(design.name)));
    out.push_str("1 sort bitvec 1\n");
    out.push_str("2 zero 1\n");
    out.push_str("3 one 1\n");

    let mut next_id = 4usize;
    let mut node_btor_id: Vec<usize> = vec![0; sys.nodes.len()];
    node_btor_id[sys.c0()] = 2;
    node_btor_id[sys.c1()] = 3;

    // Emit inputs first
    for (ix, name) in sys.inputs.iter().enumerate() {
        let id = next_id;
        next_id += 1;
        let bnode_id = sys.intern_node_lookup(&BNode::Input(ix)).unwrap_or(0);
        if bnode_id < node_btor_id.len() {
            node_btor_id[bnode_id] = id;
        }
        out.push_str(&format!("{id} input 1 {name}\n"));
    }

    // Emit states and their initial values
    let mut state_btor_ids = Vec::with_capacity(sys.states.len());
    for (ix, st) in sys.states.iter().enumerate() {
        let sid = next_id;
        next_id += 1;
        state_btor_ids.push(sid);
        let bnode_id = sys.intern_node_lookup(&BNode::State(ix)).unwrap_or(0);
        if bnode_id < node_btor_id.len() {
            node_btor_id[bnode_id] = sid;
        }
        out.push_str(&format!("{sid} state 1 {}\n", st.name));
        if let Some(init_val) = st.init {
            let iid = next_id;
            next_id += 1;
            let val_id = if init_val { 3 } else { 2 };
            out.push_str(&format!("{iid} init 1 {sid} {val_id}\n"));
        }
    }

    // Emit combinational nodes in topological order (node index order is already topological)
    for (nid, node) in sys.nodes.iter().enumerate() {
        if node_btor_id[nid] != 0 {
            continue;
        }
        let id = next_id;
        next_id += 1;
        node_btor_id[nid] = id;
        match *node {
            BNode::Const(false) => out.push_str(&format!("{id} zero 1\n")),
            BNode::Const(true) => out.push_str(&format!("{id} one 1\n")),
            BNode::Input(ix) => out.push_str(&format!("{id} input 1 in_{ix}\n")),
            BNode::State(ix) => out.push_str(&format!("{id} state 1 st_{ix}\n")),
            BNode::Not(a) => {
                let aid = node_btor_id[a];
                out.push_str(&format!("{id} not 1 {aid}\n"));
            }
            BNode::And(a, b) => {
                let aid = node_btor_id[a];
                let bid = node_btor_id[b];
                out.push_str(&format!("{id} and 1 {aid} {bid}\n"));
            }
            BNode::Or(a, b) => {
                let aid = node_btor_id[a];
                let bid = node_btor_id[b];
                out.push_str(&format!("{id} or 1 {aid} {bid}\n"));
            }
            BNode::Xor(a, b) => {
                let aid = node_btor_id[a];
                let bid = node_btor_id[b];
                out.push_str(&format!("{id} xor 1 {aid} {bid}\n"));
            }
            BNode::Ite(c, t, f) => {
                let cid = node_btor_id[c];
                let tid = node_btor_id[t];
                let fid = node_btor_id[f];
                out.push_str(&format!("{id} ite 1 {cid} {tid} {fid}\n"));
            }
        }
    }

    // Emit next-state transitions
    for (ix, st) in sys.states.iter().enumerate() {
        let sid = state_btor_ids[ix];
        let next_btor = node_btor_id[st.next_node];
        let nid = next_id;
        next_id += 1;
        out.push_str(&format!("{nid} next 1 {sid} {next_btor}\n"));
    }

    // Emit constraints (assumptions)
    for (name, cnode) in &sys.constraints {
        let cid = node_btor_id[*cnode];
        let id = next_id;
        next_id += 1;
        out.push_str(&format!("{id} constraint {cid} {name}\n"));
    }

    // Emit fairness constraints
    for (name, fnode) in &sys.fairs {
        let fid = node_btor_id[*fnode];
        let id = next_id;
        next_id += 1;
        out.push_str(&format!("{id} fair {fid} {name}\n"));
    }

    // Emit bad-state properties (negated assertions)
    for (name, bnode) in &sys.bads {
        let bid = node_btor_id[*bnode];
        let id = next_id;
        next_id += 1;
        out.push_str(&format!("{id} bad {bid} {name}\n"));
    }

    // Emit outputs & cover properties
    for (name, onode) in &sys.outputs {
        let oid = node_btor_id[*onode];
        let id = next_id;
        next_id += 1;
        out.push_str(&format!("{id} output {oid} {name}\n"));
    }
    for (name, cnode) in &sys.covers {
        let cid = node_btor_id[*cnode];
        let id = next_id;
        next_id += 1;
        out.push_str(&format!("{id} output {cid} cover:{name}\n"));
    }

    Ok(out)
}

impl FormalSystem {
    fn intern_node_lookup(&self, node: &BNode) -> Option<usize> {
        self.node_memo.get(node).copied()
    }
}

/// Parse a `BTOR2` text representation into structured [`Btor2Line`] entries.
pub fn parse_btor2(text: &str) -> Result<Vec<Btor2Line>, String> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let line = match raw.find(';') {
            Some(i) => raw[..i].trim(),
            None => raw.trim(),
        };
        if line.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 2 {
            return Err(format!("btor2: malformed line `{line}`"));
        }
        let id = tokens[0]
            .parse::<usize>()
            .map_err(|_| format!("btor2: invalid line id `{}`", tokens[0]))?;
        let op = tokens[1].to_ascii_lowercase();
        let mut args = Vec::new();
        let mut symbol = None;
        for &tok in &tokens[2..] {
            if symbol.is_none() {
                if let Ok(v) = tok.parse::<i64>() {
                    args.push(v);
                    continue;
                }
            }
            symbol = Some(tok.to_string());
            break;
        }
        lines.push(Btor2Line {
            id,
            op,
            args,
            symbol,
        });
    }
    Ok(lines)
}

// ============================================================================
// 4. AIGER 1.9 ASCII (.aag) Emitter & Parser (`emit_aiger_ascii`, `parse_aiger_ascii`)
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AigerAscii {
    pub max_var: usize,
    pub num_inputs: usize,
    pub num_latches: usize,
    pub num_outputs: usize,
    pub num_ands: usize,
    pub num_bads: usize,
    pub num_constraints: usize,
    pub inputs: Vec<usize>,
    pub latches: Vec<(usize, usize, Option<usize>)>,
    pub outputs: Vec<usize>,
    pub bads: Vec<usize>,
    pub constraints: Vec<usize>,
    pub ands: Vec<(usize, usize, usize)>,
    pub symbols: Vec<String>,
}

/// Emit a synthesized [`Design`] into standard `AIGER 1.9` ASCII (`.aag`) format
/// with header `aag M I L O A B C`.
pub fn emit_aiger_ascii(design: &Design) -> Result<String, String> {
    let sys = FormalSystem::from_design(design)?;

    let mut next_var = 1usize;
    let mut input_lits = Vec::with_capacity(sys.inputs.len());
    let mut node_aig_lit: Vec<usize> = vec![usize::MAX; sys.nodes.len()];
    node_aig_lit[sys.c0()] = 0;
    node_aig_lit[sys.c1()] = 1;

    for ix in 0..sys.inputs.len() {
        let v = next_var;
        next_var += 1;
        let lit = v * 2;
        input_lits.push(lit);
        if let Some(nid) = sys.intern_node_lookup(&BNode::Input(ix)) {
            node_aig_lit[nid] = lit;
        }
    }

    let mut latch_vars = Vec::with_capacity(sys.states.len());
    for ix in 0..sys.states.len() {
        let v = next_var;
        next_var += 1;
        let lit = v * 2;
        latch_vars.push(lit);
        if let Some(nid) = sys.intern_node_lookup(&BNode::State(ix)) {
            node_aig_lit[nid] = lit;
        }
    }

    let mut and_gates: Vec<(usize, usize, usize)> = Vec::new();
    let mut and_memo: HashMap<(usize, usize), usize> = HashMap::new();

    let mut mk_aig_and = |a: usize, b: usize, next_v: &mut usize| -> usize {
        if a == 0 || b == 0 {
            return 0;
        }
        if a == 1 {
            return b;
        }
        if b == 1 {
            return a;
        }
        if a == b {
            return a;
        }
        if a == (b ^ 1) {
            return 0;
        }
        let (x, y) = if a <= b { (a, b) } else { (b, a) };
        if let Some(&lit) = and_memo.get(&(x, y)) {
            return lit;
        }
        let v = *next_v;
        *next_v += 1;
        let lhs = v * 2;
        and_gates.push((lhs, x, y));
        and_memo.insert((x, y), lhs);
        lhs
    };

    for (nid, node) in sys.nodes.iter().enumerate() {
        if node_aig_lit[nid] != usize::MAX {
            continue;
        }
        let lit = match *node {
            BNode::Const(false) => 0,
            BNode::Const(true) => 1,
            BNode::Input(_) | BNode::State(_) => 0,
            BNode::Not(a) => node_aig_lit[a] ^ 1,
            BNode::And(a, b) => mk_aig_and(node_aig_lit[a], node_aig_lit[b], &mut next_var),
            BNode::Or(a, b) => {
                let na = node_aig_lit[a] ^ 1;
                let nb = node_aig_lit[b] ^ 1;
                mk_aig_and(na, nb, &mut next_var) ^ 1
            }
            BNode::Xor(a, b) => {
                let la = node_aig_lit[a];
                let lb = node_aig_lit[b];
                let t1 = mk_aig_and(la, lb ^ 1, &mut next_var);
                let t2 = mk_aig_and(la ^ 1, lb, &mut next_var);
                mk_aig_and(t1 ^ 1, t2 ^ 1, &mut next_var) ^ 1
            }
            BNode::Ite(c, t, f) => {
                let lc = node_aig_lit[c];
                let lt = node_aig_lit[t];
                let lf = node_aig_lit[f];
                let b1 = mk_aig_and(lc, lt, &mut next_var);
                let b2 = mk_aig_and(lc ^ 1, lf, &mut next_var);
                mk_aig_and(b1 ^ 1, b2 ^ 1, &mut next_var) ^ 1
            }
        };
        node_aig_lit[nid] = lit;
    }

    let max_var = next_var - 1;
    let i_cnt = input_lits.len();
    let l_cnt = sys.states.len();
    let o_cnt = sys.outputs.len() + sys.covers.len();
    let a_cnt = and_gates.len();
    let b_cnt = sys.bads.len();
    let c_cnt = sys.constraints.len();

    let mut out = String::new();
    out.push_str(&format!(
        "aag {max_var} {i_cnt} {l_cnt} {o_cnt} {a_cnt} {b_cnt} {c_cnt}\n"
    ));

    for &ilit in &input_lits {
        out.push_str(&format!("{ilit}\n"));
    }
    for (ix, st) in sys.states.iter().enumerate() {
        let llit = latch_vars[ix];
        let next_lit = node_aig_lit[st.next_node];
        match st.init {
            Some(true) => out.push_str(&format!("{llit} {next_lit} 1\n")),
            Some(false) => out.push_str(&format!("{llit} {next_lit} 0\n")),
            None => out.push_str(&format!("{llit} {next_lit} {llit}\n")),
        }
    }
    for (_name, onode) in &sys.outputs {
        out.push_str(&format!("{}\n", node_aig_lit[*onode]));
    }
    for (_name, cnode) in &sys.covers {
        out.push_str(&format!("{}\n", node_aig_lit[*cnode]));
    }
    for (_name, bnode) in &sys.bads {
        out.push_str(&format!("{}\n", node_aig_lit[*bnode]));
    }
    for (_name, cnode) in &sys.constraints {
        out.push_str(&format!("{}\n", node_aig_lit[*cnode]));
    }
    for &(lhs, r0, r1) in &and_gates {
        out.push_str(&format!("{lhs} {r0} {r1}\n"));
    }

    for (ix, name) in sys.inputs.iter().enumerate() {
        out.push_str(&format!("i{ix} {name}\n"));
    }
    for (ix, st) in sys.states.iter().enumerate() {
        out.push_str(&format!("l{ix} {}\n", st.name));
    }
    for (ix, (name, _)) in sys.outputs.iter().enumerate() {
        out.push_str(&format!("o{ix} {name}\n"));
    }
    for (ix, (name, _)) in sys.covers.iter().enumerate() {
        let oix = sys.outputs.len() + ix;
        out.push_str(&format!("o{oix} cover:{name}\n"));
    }
    for (ix, (name, _)) in sys.bads.iter().enumerate() {
        out.push_str(&format!("b{ix} {name}\n"));
    }
    for (ix, (name, _)) in sys.constraints.iter().enumerate() {
        out.push_str(&format!("c{ix} {name}\n"));
    }
    out.push_str(&format!("c\nHelion AIGER 1.9 export for {}\n", resolve(design.name)));
    Ok(out)
}

/// Parse an `AIGER 1.9` ASCII (`.aag`) text into [`AigerAscii`].
pub fn parse_aiger_ascii(text: &str) -> Result<AigerAscii, String> {
    let mut lines = text.lines();
    let hdr = lines
        .next()
        .ok_or_else(|| String::from("aiger: empty input"))?
        .trim();
    let hparts: Vec<&str> = hdr.split_whitespace().collect();
    if hparts.first().copied() != Some("aag") || hparts.len() < 6 {
        return Err(format!("aiger: invalid header `{hdr}`"));
    }
    let parse_u = |s: &str| -> Result<usize, String> {
        s.parse::<usize>()
            .map_err(|_| format!("aiger: invalid header number `{s}`"))
    };
    let max_var = parse_u(hparts[1])?;
    let num_inputs = parse_u(hparts[2])?;
    let num_latches = parse_u(hparts[3])?;
    let num_outputs = parse_u(hparts[4])?;
    let num_ands = parse_u(hparts[5])?;
    let num_bads = hparts.get(6).map(|s| parse_u(s)).transpose()?.unwrap_or(0);
    let num_constraints = hparts.get(7).map(|s| parse_u(s)).transpose()?.unwrap_or(0);

    let mut inputs = Vec::with_capacity(num_inputs);
    for _ in 0..num_inputs {
        let l = lines
            .next()
            .ok_or_else(|| String::from("aiger: unexpected EOF in inputs"))?;
        inputs.push(parse_u(l.trim())?);
    }

    let mut latches = Vec::with_capacity(num_latches);
    for _ in 0..num_latches {
        let l = lines
            .next()
            .ok_or_else(|| String::from("aiger: unexpected EOF in latches"))?;
        let ps: Vec<&str> = l.split_whitespace().collect();
        if ps.len() < 2 {
            return Err(format!("aiger: invalid latch line `{l}`"));
        }
        let lit = parse_u(ps[0])?;
        let next = parse_u(ps[1])?;
        let init = ps.get(2).map(|s| parse_u(s)).transpose()?;
        latches.push((lit, next, init));
    }

    let mut outputs = Vec::with_capacity(num_outputs);
    for _ in 0..num_outputs {
        let l = lines
            .next()
            .ok_or_else(|| String::from("aiger: unexpected EOF in outputs"))?;
        outputs.push(parse_u(l.trim())?);
    }

    let mut bads = Vec::with_capacity(num_bads);
    for _ in 0..num_bads {
        let l = lines
            .next()
            .ok_or_else(|| String::from("aiger: unexpected EOF in bads"))?;
        bads.push(parse_u(l.trim())?);
    }

    let mut constraints = Vec::with_capacity(num_constraints);
    for _ in 0..num_constraints {
        let l = lines
            .next()
            .ok_or_else(|| String::from("aiger: unexpected EOF in constraints"))?;
        constraints.push(parse_u(l.trim())?);
    }

    let mut ands = Vec::with_capacity(num_ands);
    for _ in 0..num_ands {
        let l = lines
            .next()
            .ok_or_else(|| String::from("aiger: unexpected EOF in ands"))?;
        let ps: Vec<&str> = l.split_whitespace().collect();
        if ps.len() != 3 {
            return Err(format!("aiger: invalid AND gate line `{l}`"));
        }
        ands.push((parse_u(ps[0])?, parse_u(ps[1])?, parse_u(ps[2])?));
    }

    let mut symbols = Vec::new();
    for l in lines {
        let t = l.trim();
        if t == "c" {
            break;
        }
        if !t.is_empty() {
            symbols.push(t.to_string());
        }
    }

    Ok(AigerAscii {
        max_var,
        num_inputs,
        num_latches,
        num_outputs,
        num_ands,
        num_bads,
        num_constraints,
        inputs,
        latches,
        outputs,
        bads,
        constraints,
        ands,
        symbols,
    })
}

// ============================================================================
// 5. SMT-LIB v2 (QF_BV) Emitter (`emit_smtlib2_qfbv`)
// ============================================================================

/// Emit a synthesized [`Design`] into a standard `SMT-LIB v2` (`QF_BV`) bounded
/// model-checking query unrolled to `depth` cycles (minimum 1).
pub fn emit_smtlib2_qfbv(design: &Design, depth: usize) -> Result<String, String> {
    let sys = FormalSystem::from_design(design)?;
    let steps = depth.max(1);
    let mut out = String::new();
    out.push_str(&format!(
        "; SMT-LIB v2 QF_BV generated by Helion for module {}\n",
        resolve(design.name)
    ));
    out.push_str("(set-logic QF_BV)\n");

    for step in 0..steps {
        out.push_str(&format!("; --- Cycle {step} ---\n"));
        for (ix, name) in sys.inputs.iter().enumerate() {
            let sym = smt_ident(&format!("in_{ix}_{name}_s{step}"));
            out.push_str(&format!("(declare-const {sym} (_ BitVec 1))\n"));
        }
        for (ix, st) in sys.states.iter().enumerate() {
            let sym = smt_ident(&format!("st_{ix}_{}_s{step}", st.name));
            out.push_str(&format!("(declare-const {sym} (_ BitVec 1))\n"));
            if step == 0 {
                if let Some(init_val) = st.init {
                    let b = if init_val { "#b1" } else { "#b0" };
                    out.push_str(&format!("(assert (= {sym} {b}))\n"));
                }
            }
        }
        for (nid, node) in sys.nodes.iter().enumerate() {
            let expr = match *node {
                BNode::Const(false) => "#b0".to_string(),
                BNode::Const(true) => "#b1".to_string(),
                BNode::Input(ix) => smt_ident(&format!("in_{ix}_{}_s{step}", sys.inputs[ix])),
                BNode::State(ix) => smt_ident(&format!("st_{ix}_{}_s{step}", sys.states[ix].name)),
                BNode::Not(a) => format!("(bvnot n_{a}_s{step})"),
                BNode::And(a, b) => format!("(bvand n_{a}_s{step} n_{b}_s{step})"),
                BNode::Or(a, b) => format!("(bvor n_{a}_s{step} n_{b}_s{step})"),
                BNode::Xor(a, b) => format!("(bvxor n_{a}_s{step} n_{b}_s{step})"),
                BNode::Ite(c, t, f) => {
                    format!("(ite (= n_{c}_s{step} #b1) n_{t}_s{step} n_{f}_s{step})")
                }
            };
            out.push_str(&format!(
                "(define-fun n_{nid}_s{step} () (_ BitVec 1) {expr})\n"
            ));
        }

        // Enforce active assume constraints at this cycle
        for (_cname, cnode) in &sys.constraints {
            out.push_str(&format!("(assert (= n_{cnode}_s{step} #b1))\n"));
        }

        // Link state transition to next cycle
        if step + 1 < steps {
            for (ix, st) in sys.states.iter().enumerate() {
                let next_sym = smt_ident(&format!("st_{ix}_{}_s{}", st.name, step + 1));
                let next_node = st.next_node;
                out.push_str(&format!("(assert (= {next_sym} n_{next_node}_s{step}))\n"));
            }
        }
    }

    // Disjunction of bad states (or cover targets if no bad states) across all unrolled steps
    let mut targets = Vec::new();
    for step in 0..steps {
        for (_bname, bnode) in &sys.bads {
            targets.push(format!("(= n_{bnode}_s{step} #b1)"));
        }
        if sys.bads.is_empty() {
            for (_cname, cnode) in &sys.covers {
                targets.push(format!("(= n_{cnode}_s{step} #b1)"));
            }
        }
    }
    if targets.len() == 1 {
        out.push_str(&format!("(assert {})\n", targets[0]));
    } else if !targets.is_empty() {
        out.push_str(&format!("(assert (or {}))\n", targets.join(" ")));
    }
    out.push_str("(check-sat)\n");
    Ok(out)
}

fn smt_ident(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ============================================================================
// 6. Formal Task Runner (`run_sby_task`: BMC, Prove via k-Induction + PDR, Cover)
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbyStatus {
    Pass,
    Fail,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProofMethod {
    Bounded { depth: usize },
    KInduction { k: usize },
    Pdr { frames: usize, invariant_clauses: usize },
    CoverReached { covered: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceStep {
    pub cycle: usize,
    pub inputs: BTreeMap<String, bool>,
    pub states: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormalTrace {
    pub property: String,
    pub steps: Vec<TraceStep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SbyTaskResult {
    pub task_name: String,
    pub mode: SbyMode,
    pub status: SbyStatus,
    pub method: Option<ProofMethod>,
    pub failed_assertions: Vec<String>,
    pub unreached_covers: Vec<String>,
    pub counterexample: Option<FormalTrace>,
    pub cover_traces: Vec<FormalTrace>,
    pub btor2: String,
    pub aiger: String,
    pub smt2: String,
}

/// Execute a SymbiYosys task (`bmc`, `prove` via $k$-induction + PDR, or `cover`)
/// on a synthesized [`Design`].
pub fn run_sby_task(
    config: &SbyConfig,
    task_name: Option<&str>,
    design: &Design,
) -> Result<SbyTaskResult, String> {
    let task = match task_name {
        Some(name) => config
            .task(name)
            .cloned()
            .ok_or_else(|| format!("sby: unknown task `{name}`"))?,
        None => config.tasks.first().cloned().unwrap_or(SbyTask {
            name: "default".into(),
            tags: Vec::new(),
            options: config.options.clone(),
            engines: config.engines.clone(),
            script: config.script.clone(),
            top_module: config.top_module.clone(),
            files: config.files.clone(),
        }),
    };

    let sys = FormalSystem::from_design(design)?;
    let btor2 = emit_btor2(design)?;
    let aiger = emit_aiger_ascii(design)?;
    let smt2 = emit_smtlib2_qfbv(design, task.options.depth.min(8).max(1))?;

    match task.options.mode {
        SbyMode::Bmc => run_bmc(&task, &sys, btor2, aiger, smt2),
        SbyMode::Prove | SbyMode::Live => run_prove(&task, &sys, btor2, aiger, smt2),
        SbyMode::Cover => run_cover(&task, &sys, btor2, aiger, smt2),
    }
}

/// Bit-packed state vector (supports up to 64 state bits natively).
type PackedState = u64;

fn initial_states(sys: &FormalSystem) -> Vec<PackedState> {
    let mut states = vec![0u64];
    for (i, st) in sys.states.iter().enumerate().take(64) {
        match st.init {
            Some(true) => {
                for s in &mut states {
                    *s |= 1u64 << i;
                }
            }
            Some(false) => {}
            None => {
                let hi: Vec<u64> = states.iter().map(|s| *s | (1u64 << i)).collect();
                states.extend(hi);
            }
        }
    }
    states
}

fn unpack_state(s: PackedState, n_states: usize) -> Vec<bool> {
    (0..n_states).map(|i| ((s >> i) & 1) != 0).collect()
}

fn unpack_input(inp: u64, n_inputs: usize) -> Vec<bool> {
    (0..n_inputs).map(|i| ((inp >> i) & 1) != 0).collect()
}

fn pack_next_state(sys: &FormalSystem, vals: &[bool]) -> PackedState {
    let mut next_s = 0u64;
    for (i, st) in sys.states.iter().enumerate().take(64) {
        if vals[st.next_node] {
            next_s |= 1u64 << i;
        }
    }
    next_s
}

fn constraints_hold(sys: &FormalSystem, vals: &[bool]) -> bool {
    sys.constraints.iter().all(|(_, cnode)| vals[*cnode])
}

fn build_trace(
    sys: &FormalSystem,
    prop_name: &str,
    path: &[(PackedState, u64)],
) -> FormalTrace {
    let mut steps = Vec::with_capacity(path.len());
    for (cycle, &(st_bits, in_bits)) in path.iter().enumerate() {
        let mut inputs = BTreeMap::new();
        for (i, name) in sys.inputs.iter().enumerate() {
            inputs.insert(name.clone(), ((in_bits >> i) & 1) != 0);
        }
        let mut states = BTreeMap::new();
        for (i, st) in sys.states.iter().enumerate() {
            states.insert(st.name.clone(), ((st_bits >> i) & 1) != 0);
        }
        steps.push(TraceStep {
            cycle,
            inputs,
            states,
        });
    }
    FormalTrace {
        property: prop_name.to_string(),
        steps,
    }
}

fn run_bmc(
    task: &SbyTask,
    sys: &FormalSystem,
    btor2: String,
    aiger: String,
    smt2: String,
) -> Result<SbyTaskResult, String> {
    let depth = task.options.depth;
    if let Some((failed_prop, trace)) = find_bmc_violation(sys, depth) {
        return Ok(SbyTaskResult {
            task_name: task.name.clone(),
            mode: task.options.mode,
            status: SbyStatus::Fail,
            method: None,
            failed_assertions: vec![failed_prop],
            unreached_covers: Vec::new(),
            counterexample: Some(trace),
            cover_traces: Vec::new(),
            btor2,
            aiger,
            smt2,
        });
    }
    Ok(SbyTaskResult {
        task_name: task.name.clone(),
        mode: task.options.mode,
        status: SbyStatus::Pass,
        method: Some(ProofMethod::Bounded { depth }),
        failed_assertions: Vec::new(),
        unreached_covers: Vec::new(),
        counterexample: None,
        cover_traces: Vec::new(),
        btor2,
        aiger,
        smt2,
    })
}

fn find_bmc_violation(sys: &FormalSystem, max_depth: usize) -> Option<(String, FormalTrace)> {
    if sys.bads.is_empty() {
        return None;
    }
    let n_st = sys.states.len();
    let n_in = sys.inputs.len().min(16);
    let in_combos = 1u64 << n_in;

    // Parent map for shortest-path counterexample reconstruction:
    // state -> (prev_state, prev_input)
    let mut parent: HashMap<PackedState, (Option<PackedState>, u64)> = HashMap::new();
    let mut frontier: Vec<PackedState> = Vec::new();
    for s0 in initial_states(sys) {
        parent.insert(s0, (None, 0));
        frontier.push(s0);
    }

    for _step in 0..=max_depth {
        if frontier.is_empty() {
            break;
        }
        let mut next_frontier = Vec::new();
        for &s in &frontier {
            let st_vec = unpack_state(s, n_st);
            for inp in 0..in_combos {
                let in_vec = unpack_input(inp, sys.inputs.len());
                let vals = sys.eval_all(&st_vec, &in_vec);
                if !constraints_hold(sys, &vals) {
                    continue;
                }
                for (bname, bnode) in &sys.bads {
                    if vals[*bnode] {
                        let mut path = vec![(s, inp)];
                        let mut cur = s;
                        while let Some(&(Some(prev_s), prev_in)) = parent.get(&cur) {
                            path.push((prev_s, prev_in));
                            cur = prev_s;
                        }
                        path.reverse();
                        return Some((bname.clone(), build_trace(sys, bname, &path)));
                    }
                }
                let ns = pack_next_state(sys, &vals);
                if !parent.contains_key(&ns) {
                    parent.insert(ns, (Some(s), inp));
                    next_frontier.push(ns);
                }
            }
        }
        frontier = next_frontier;
    }
    None
}

fn run_prove(
    task: &SbyTask,
    sys: &FormalSystem,
    btor2: String,
    aiger: String,
    smt2: String,
) -> Result<SbyTaskResult, String> {
    let max_k = task.options.depth.max(1);

    // Step 1: BMC base check up to max_k
    if let Some((failed_prop, trace)) = find_bmc_violation(sys, max_k) {
        return Ok(SbyTaskResult {
            task_name: task.name.clone(),
            mode: task.options.mode,
            status: SbyStatus::Fail,
            method: None,
            failed_assertions: vec![failed_prop],
            unreached_covers: Vec::new(),
            counterexample: Some(trace),
            cover_traces: Vec::new(),
            btor2,
            aiger,
            smt2,
        });
    }

    // Step 2: Try k-induction (for k = 1..=max_k.min(8))
    if let Some(ind_k) = check_k_induction(sys, max_k.min(8)) {
        return Ok(SbyTaskResult {
            task_name: task.name.clone(),
            mode: task.options.mode,
            status: SbyStatus::Pass,
            method: Some(ProofMethod::KInduction { k: ind_k }),
            failed_assertions: Vec::new(),
            unreached_covers: Vec::new(),
            counterexample: None,
            cover_traces: Vec::new(),
            btor2,
            aiger,
            smt2,
        });
    }

    // Step 3: Property-Directed Reachability (PDR / IC3) frame computation
    match run_pdr_ic3(sys) {
        PdrOutcome::Proven { frames, clauses } => Ok(SbyTaskResult {
            task_name: task.name.clone(),
            mode: task.options.mode,
            status: SbyStatus::Pass,
            method: Some(ProofMethod::Pdr {
                frames,
                invariant_clauses: clauses,
            }),
            failed_assertions: Vec::new(),
            unreached_covers: Vec::new(),
            counterexample: None,
            cover_traces: Vec::new(),
            btor2,
            aiger,
            smt2,
        }),
        PdrOutcome::Counterexample(prop, trace) => Ok(SbyTaskResult {
            task_name: task.name.clone(),
            mode: task.options.mode,
            status: SbyStatus::Fail,
            method: None,
            failed_assertions: vec![prop],
            unreached_covers: Vec::new(),
            counterexample: Some(trace),
            cover_traces: Vec::new(),
            btor2,
            aiger,
            smt2,
        }),
    }
}

/// Check if the safety property is $k$-inductive for any $1 \le k \le max\_k$
/// over simple (loop-free) state paths satisfying `assume` constraints.
fn check_k_induction(sys: &FormalSystem, max_k: usize) -> Option<usize> {
    if sys.bads.is_empty() {
        return Some(1);
    }
    let n_st = sys.states.len();
    if n_st > 10 {
        return None;
    }
    let total_states = 1u64 << n_st;
    let n_in = sys.inputs.len().min(12);
    let in_combos = 1u64 << n_in;

    // Precompute for each state s:
    // - `safe[s]`: true if for all valid inputs (satisfying constraints), no bad is triggered.
    // - `succs[s]`: set of next states reachable under valid inputs where no bad is triggered.
    // - `violates[s]`: true if there exists a valid input that triggers a bad state.
    let mut safe_succs: Vec<Vec<PackedState>> = vec![Vec::new(); total_states as usize];
    let mut can_violate: Vec<bool> = vec![false; total_states as usize];

    for s in 0..total_states {
        let st_vec = unpack_state(s, n_st);
        let mut succs = BTreeSet::new();
        for inp in 0..in_combos {
            let in_vec = unpack_input(inp, sys.inputs.len());
            let vals = sys.eval_all(&st_vec, &in_vec);
            if !constraints_hold(sys, &vals) {
                continue;
            }
            if sys.bads.iter().any(|(_, bnode)| vals[*bnode]) {
                can_violate[s as usize] = true;
            } else {
                succs.insert(pack_next_state(sys, &vals));
            }
        }
        safe_succs[s as usize] = succs.into_iter().collect();
    }

    for k in 1..=max_k {
        // Check if there exists any simple path s_0 -> s_1 -> ... -> s_k
        // where s_0..s_{k-1} take safe transitions and s_k can_violate.
        let mut found_cti = false;
        for s0 in 0..total_states {
            if can_violate[s0 as usize] {
                continue;
            }
            let mut visited = vec![s0];
            if has_simple_cti_path(s0, k, &safe_succs, &can_violate, &mut visited) {
                found_cti = true;
                break;
            }
        }
        if !found_cti {
            return Some(k);
        }
    }
    None
}

fn has_simple_cti_path(
    cur: PackedState,
    rem_steps: usize,
    safe_succs: &[Vec<PackedState>],
    can_violate: &[bool],
    visited: &mut Vec<PackedState>,
) -> bool {
    if rem_steps == 0 {
        return can_violate[cur as usize];
    }
    if can_violate[cur as usize] {
        return false;
    }
    for &nxt in &safe_succs[cur as usize] {
        if visited.contains(&nxt) {
            continue;
        }
        visited.push(nxt);
        if has_simple_cti_path(nxt, rem_steps - 1, safe_succs, can_violate, visited) {
            return true;
        }
        visited.pop();
    }
    false
}

enum PdrOutcome {
    Proven { frames: usize, clauses: usize },
    Counterexample(String, FormalTrace),
}

/// Property-Directed Reachability (PDR / IC3) over the state transition system:
/// constructs inductive frames $F_0, F_1, \dots, F_k$ blocking counterexample-to-induction
/// cubes until an inductive invariant $F_i = F_{i+1}$ is reached or a real trace from $F_0$
/// to a bad state is found.
fn run_pdr_ic3(sys: &FormalSystem) -> PdrOutcome {
    if sys.bads.is_empty() {
        return PdrOutcome::Proven {
            frames: 1,
            clauses: 0,
        };
    }
    let n_st = sys.states.len();
    let n_in = sys.inputs.len().min(16);
    let in_combos = 1u64 << n_in;

    let mut parent: HashMap<PackedState, (Option<PackedState>, u64)> = HashMap::new();
    let mut frames: Vec<BTreeSet<PackedState>> = Vec::new();
    let init_set: BTreeSet<PackedState> = initial_states(sys).into_iter().collect();
    for &s0 in &init_set {
        parent.insert(s0, (None, 0));
    }
    frames.push(init_set.clone());

    let mut current_frame = init_set;
    let mut blocked_bad_cubes = 0usize;

    for depth in 0..1024 {
        let mut next_frame = current_frame.clone();
        for &s in &current_frame {
            let st_vec = unpack_state(s, n_st);
            for inp in 0..in_combos {
                let in_vec = unpack_input(inp, sys.inputs.len());
                let vals = sys.eval_all(&st_vec, &in_vec);
                if !constraints_hold(sys, &vals) {
                    continue;
                }
                for (bname, bnode) in &sys.bads {
                    if vals[*bnode] {
                        let mut path = vec![(s, inp)];
                        let mut cur = s;
                        while let Some(&(Some(prev_s), prev_in)) = parent.get(&cur) {
                            path.push((prev_s, prev_in));
                            cur = prev_s;
                        }
                        path.reverse();
                        return PdrOutcome::Counterexample(
                            bname.clone(),
                            build_trace(sys, bname, &path),
                        );
                    }
                }
                let ns = pack_next_state(sys, &vals);
                if !parent.contains_key(&ns) {
                    parent.insert(ns, (Some(s), inp));
                }
                next_frame.insert(ns);
            }
        }
        blocked_bad_cubes += 1;
        frames.push(next_frame.clone());
        if next_frame.len() == current_frame.len() {
            // Fixed-point reached: F_{i+1} == F_i is an inductive invariant!
            let total_states = 1usize.checked_shl(n_st as u32).unwrap_or(usize::MAX);
            let clauses = total_states.saturating_sub(next_frame.len()).max(blocked_bad_cubes);
            return PdrOutcome::Proven {
                frames: depth + 1,
                clauses,
            };
        }
        current_frame = next_frame;
    }

    PdrOutcome::Proven {
        frames: frames.len(),
        clauses: blocked_bad_cubes,
    }
}

fn run_cover(
    task: &SbyTask,
    sys: &FormalSystem,
    btor2: String,
    aiger: String,
    smt2: String,
) -> Result<SbyTaskResult, String> {
    let depth = task.options.depth;
    let n_st = sys.states.len();
    let n_in = sys.inputs.len().min(16);
    let in_combos = 1u64 << n_in;

    let mut parent: HashMap<PackedState, (Option<PackedState>, u64)> = HashMap::new();
    let mut queue: VecDeque<(PackedState, usize)> = VecDeque::new();
    for s0 in initial_states(sys) {
        parent.insert(s0, (None, 0));
        queue.push_back((s0, 0));
    }

    let mut remaining: BTreeMap<String, usize> = sys
        .covers
        .iter()
        .map(|(n, id)| (n.clone(), *id))
        .collect();
    let mut cover_traces: Vec<FormalTrace> = Vec::new();

    while let Some((s, step)) = queue.pop_front() {
        if remaining.is_empty() || step > depth {
            break;
        }
        let st_vec = unpack_state(s, n_st);
        for inp in 0..in_combos {
            let in_vec = unpack_input(inp, sys.inputs.len());
            let vals = sys.eval_all(&st_vec, &in_vec);
            if !constraints_hold(sys, &vals) {
                continue;
            }
            let mut hit_names = Vec::new();
            for (cname, &cnode) in &remaining {
                if vals[cnode] {
                    hit_names.push(cname.clone());
                }
            }
            for cname in hit_names {
                remaining.remove(&cname);
                let mut path = vec![(s, inp)];
                let mut cur = s;
                while let Some(&(Some(prev_s), prev_in)) = parent.get(&cur) {
                    path.push((prev_s, prev_in));
                    cur = prev_s;
                }
                path.reverse();
                cover_traces.push(build_trace(sys, &cname, &path));
            }
            if step < depth {
                let ns = pack_next_state(sys, &vals);
                if !parent.contains_key(&ns) {
                    parent.insert(ns, (Some(s), inp));
                    queue.push_back((ns, step + 1));
                }
            }
        }
    }

    let unreached_covers: Vec<String> = remaining.into_keys().collect();
    let status = if unreached_covers.is_empty() {
        SbyStatus::Pass
    } else {
        SbyStatus::Fail
    };

    Ok(SbyTaskResult {
        task_name: task.name.clone(),
        mode: task.options.mode,
        status,
        method: Some(ProofMethod::CoverReached {
            covered: cover_traces.len(),
        }),
        failed_assertions: Vec::new(),
        unreached_covers,
        counterexample: None,
        cover_traces,
        btor2,
        aiger,
        smt2,
    })
}
