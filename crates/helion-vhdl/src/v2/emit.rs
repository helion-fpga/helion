//! Write the elaborated design as flat, explicitly sized SystemVerilog.
//!
//! Every graph node becomes a `wire` of its exact width whose operands have
//! that width too, so SystemVerilog's context-width and signedness rules
//! never change the meaning. Registers are `always_ff` blocks with one
//! non-blocking assignment.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use super::ast::Mode;
use super::dag::*;
use super::elab::*;

/// SystemVerilog keywords that are legal VHDL identifiers.
const SV_KEYWORDS: &[&str] = &[
    "accept_on",
    "alias",
    "always",
    "always_comb",
    "always_ff",
    "always_latch",
    "and",
    "assert",
    "assign",
    "assume",
    "automatic",
    "before",
    "begin",
    "bind",
    "bins",
    "binsof",
    "bit",
    "break",
    "buf",
    "bufif0",
    "bufif1",
    "byte",
    "case",
    "casex",
    "casez",
    "cell",
    "chandle",
    "checker",
    "class",
    "clocking",
    "cmos",
    "config",
    "const",
    "constraint",
    "context",
    "continue",
    "cover",
    "covergroup",
    "coverpoint",
    "cross",
    "deassign",
    "default",
    "defparam",
    "design",
    "disable",
    "dist",
    "do",
    "edge",
    "else",
    "end",
    "endcase",
    "endchecker",
    "endclass",
    "endclocking",
    "endconfig",
    "endfunction",
    "endgenerate",
    "endgroup",
    "endinterface",
    "endmodule",
    "endpackage",
    "endprimitive",
    "endprogram",
    "endproperty",
    "endsequence",
    "endspecify",
    "endtable",
    "endtask",
    "enum",
    "event",
    "eventually",
    "expect",
    "export",
    "extends",
    "extern",
    "final",
    "first_match",
    "for",
    "force",
    "foreach",
    "forever",
    "fork",
    "forkjoin",
    "function",
    "generate",
    "genvar",
    "global",
    "highz0",
    "highz1",
    "if",
    "iff",
    "ifnone",
    "ignore_bins",
    "illegal_bins",
    "implements",
    "implies",
    "import",
    "incdir",
    "include",
    "initial",
    "inout",
    "input",
    "inside",
    "instance",
    "int",
    "integer",
    "interconnect",
    "interface",
    "intersect",
    "join",
    "join_any",
    "join_none",
    "large",
    "let",
    "liblist",
    "library",
    "local",
    "localparam",
    "logic",
    "longint",
    "macromodule",
    "matches",
    "medium",
    "modport",
    "module",
    "nand",
    "negedge",
    "nettype",
    "new",
    "nexttime",
    "nmos",
    "nor",
    "noshowcancelled",
    "not",
    "notif0",
    "notif1",
    "null",
    "or",
    "output",
    "package",
    "packed",
    "parameter",
    "pmos",
    "posedge",
    "primitive",
    "priority",
    "program",
    "property",
    "protected",
    "pull0",
    "pull1",
    "pulldown",
    "pullup",
    "pulsestyle_ondetect",
    "pulsestyle_onevent",
    "pure",
    "rand",
    "randc",
    "randcase",
    "randsequence",
    "rcmos",
    "real",
    "realtime",
    "ref",
    "reg",
    "reject_on",
    "release",
    "repeat",
    "restrict",
    "return",
    "rnmos",
    "rpmos",
    "rtran",
    "rtranif0",
    "rtranif1",
    "s_always",
    "s_eventually",
    "s_nexttime",
    "s_until",
    "s_until_with",
    "scalared",
    "sequence",
    "shortint",
    "shortreal",
    "showcancelled",
    "signed",
    "small",
    "soft",
    "solve",
    "specify",
    "specparam",
    "static",
    "string",
    "strong",
    "strong0",
    "strong1",
    "struct",
    "super",
    "supply0",
    "supply1",
    "sync_accept_on",
    "sync_reject_on",
    "table",
    "tagged",
    "task",
    "this",
    "throughout",
    "time",
    "timeprecision",
    "timeunit",
    "tran",
    "tranif0",
    "tranif1",
    "tri",
    "tri0",
    "tri1",
    "triand",
    "trior",
    "trireg",
    "type",
    "typedef",
    "union",
    "unique",
    "unique0",
    "unsigned",
    "until",
    "until_with",
    "untyped",
    "use",
    "uwire",
    "var",
    "vectored",
    "virtual",
    "void",
    "wait",
    "wait_order",
    "wand",
    "weak",
    "weak0",
    "weak1",
    "while",
    "wildcard",
    "wire",
    "with",
    "within",
    "wor",
    "xnor",
    "xor",
];

/// SV spelling of a top-level name: keywords get a prefix that the caller
/// maps back after synthesis.
pub fn sv_safe(name: &str) -> String {
    if SV_KEYWORDS
        .binary_search(&name.to_ascii_lowercase().as_str())
        .is_ok()
    {
        format!("hv_kw_{name}")
    } else {
        name.to_string()
    }
}

fn lit(bits: &[bool]) -> String {
    let w = bits.len();
    if w == 0 {
        return "1'b0".into();
    }
    if w % 4 == 0 && w >= 8 {
        let mut s = format!("{w}'h");
        for k in (0..w / 4).rev() {
            let mut d = 0u8;
            for j in 0..4 {
                if bits[k * 4 + j] {
                    d |= 1 << j;
                }
            }
            s.push(char::from_digit(d as u32, 16).unwrap());
        }
        return s;
    }
    let mut s = format!("{w}'b");
    for &b in bits.iter().rev() {
        s.push(if b { '1' } else { '0' });
    }
    s
}

fn range(w: u32) -> String {
    if w == 1 {
        String::new()
    } else {
        format!("[{}:0] ", w - 1)
    }
}

impl Elab {
    fn check_drivers(&self) -> R<()> {
        for net in &self.nets {
            let mut d: Vec<&Drv> = net.drivers.iter().collect();
            d.sort_by_key(|x| x.lo);
            for p in d.windows(2) {
                if p[0].lo + p[0].w > p[1].lo {
                    return err(
                        p[1].line,
                        format!(
                            "signal `{}` has more than one driver (also driven at line {}); resolved multiple drivers are not synthesizable",
                            net.name, p[0].line
                        ),
                    );
                }
            }
        }
        for p in &self.top_ports {
            if p.dir == Mode::In && !self.nets[p.net as usize].drivers.is_empty() {
                return err(
                    self.nets[p.net as usize].drivers[0].line,
                    format!("input port `{}` is assigned", p.name),
                );
            }
        }
        Ok(())
    }

    fn is_top_input(&self, net: u32) -> bool {
        self.top_ports
            .iter()
            .any(|p| p.net == net && p.dir == Mode::In)
    }

    /// Replace reads of pure alias / constant nets by their source.
    fn forward_aliases(&mut self) -> R<()> {
        let mut rep: HashMap<u32, N> = HashMap::new();
        for (i, net) in self.nets.iter().enumerate() {
            let i = i as u32;
            if self.is_top_input(i) {
                continue;
            }
            if net.drivers.is_empty() {
                rep.insert(i, u32::MAX); // constant: filled below
                continue;
            }
            if net.drivers.len() == 1 && net.drivers[0].lo == 0 && net.drivers[0].w == net.w {
                if let DrvSrc::Comb(n) = net.drivers[0].src {
                    let alias = match self.dag.op(n) {
                        Op::Read(_) | Op::Const(_) => true,
                        Op::Slice(x, _) => matches!(self.dag.op(*x), Op::Read(_)),
                        _ => false,
                    };
                    if alias {
                        rep.insert(i, n);
                    }
                }
            }
        }
        let keys: Vec<u32> = rep.keys().copied().collect();
        for k in &keys {
            if rep[k] == u32::MAX {
                let init = self.nets[*k as usize].init.clone();
                let c = self.dag.konst(init);
                rep.insert(*k, c);
            }
        }
        // Resolve alias chains to a fixpoint.
        let mut pass = 0;
        loop {
            let mut changed = false;
            for k in &keys {
                let n = rep[k];
                let mut memo = HashMap::new();
                let r2 = rep.clone();
                let m = self.dag.subst(
                    n,
                    &mut |d: &mut Dag, x| match d.op(x) {
                        Op::Read(net) => r2.get(net).copied(),
                        _ => None,
                    },
                    &mut memo,
                );
                if m != n {
                    rep.insert(*k, m);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
            pass += 1;
            if pass > keys.len() + 2 {
                let names: Vec<String> = keys
                    .iter()
                    .take(4)
                    .map(|k| self.nets[*k as usize].name.clone())
                    .collect();
                return Err(format!(
                    "combinational loop through signals {}",
                    names.join(", ")
                ));
            }
        }
        let mut memo = HashMap::new();
        let mut f = |d: &mut Dag, x: N| match d.op(x) {
            Op::Read(net) => rep.get(net).copied(),
            _ => None,
        };
        for ni in 0..self.nets.len() {
            for di in 0..self.nets[ni].drivers.len() {
                if let DrvSrc::Comb(n) = self.nets[ni].drivers[di].src {
                    let m = self.dag.subst(n, &mut f, &mut memo);
                    self.nets[ni].drivers[di].src = DrvSrc::Comb(m);
                }
            }
        }
        for ri in 0..self.regs.len() {
            let r = self.regs[ri].clone();
            let d = self.dag.subst(r.d, &mut f, &mut memo);
            let clk = self.dag.subst(r.clk, &mut f, &mut memo);
            let arst = match r.arst {
                Some(a) => Some(Arst {
                    sig: self.dag.subst(a.sig, &mut f, &mut memo),
                    active_high: a.active_high,
                    val: self.dag.subst(a.val, &mut f, &mut memo),
                }),
                None => None,
            };
            self.regs[ri].d = d;
            self.regs[ri].clk = clk;
            self.regs[ri].arst = arst;
        }
        Ok(())
    }

    pub fn emit_sv(&mut self) -> R<String> {
        self.check_drivers()?;
        self.forward_aliases()?;

        // Reachability from the top-level outputs.
        let mut nets_r: HashSet<u32> = HashSet::new();
        let mut nodes_r: HashSet<N> = HashSet::new();
        let mut regs_r: HashSet<u32> = HashSet::new();
        let mut net_stack: Vec<u32> = self
            .top_ports
            .iter()
            .filter(|p| p.dir != Mode::In)
            .map(|p| p.net)
            .collect();
        let mut node_stack: Vec<N> = Vec::new();
        loop {
            if let Some(n) = node_stack.pop() {
                if !nodes_r.insert(n) {
                    continue;
                }
                match self.dag.op(n) {
                    Op::Read(net) => net_stack.push(*net),
                    Op::Hold(net) => {
                        return Err(format!(
                            "internal: hold of `{}` reached the netlist",
                            self.nets[*net as usize].name
                        ));
                    }
                    Op::VHold(_) => {
                        return Err("internal: unassigned variable reached the netlist".into());
                    }
                    _ => node_stack.extend(self.dag.kids(n)),
                }
                continue;
            }
            if let Some(net) = net_stack.pop() {
                if !nets_r.insert(net) {
                    continue;
                }
                for d in &self.nets[net as usize].drivers {
                    match d.src {
                        DrvSrc::Comb(n) => node_stack.push(n),
                        DrvSrc::Reg(r) => {
                            if regs_r.insert(r) {
                                let g = &self.regs[r as usize];
                                node_stack.push(g.d);
                                node_stack.push(g.clk);
                                if let Some(a) = &g.arst {
                                    node_stack.push(a.sig);
                                    node_stack.push(a.val);
                                }
                            }
                        }
                    }
                }
                continue;
            }
            break;
        }

        // SV names.
        let mut net_name: HashMap<u32, String> = HashMap::new();
        let mut port_alias: Vec<(String, u32, Mode)> = Vec::new();
        let mut header = Vec::new();
        let mut renames: Vec<(String, String)> = Vec::new();
        for p in &self.top_ports {
            let svn = sv_safe(&p.name);
            if svn != p.name {
                renames.push((p.name.clone(), svn.clone()));
            }
        }
        let port_sv = |n: &str| sv_safe(n);
        for p in &self.top_ports {
            let w = self.nets[p.net as usize].w;
            let decl = match p.range {
                None => String::new(),
                Some((l, r)) => format!("[{l}:{r}] "),
            };
            let dir = if p.dir == Mode::In { "input" } else { "output" };
            header.push(format!("{dir} wire {decl}{}", port_sv(&p.name)));
            let canonical = match p.range {
                None => w == 1,
                Some((l, r)) => r == 0 && l == w as i64 - 1,
            };
            if canonical {
                net_name.insert(p.net, port_sv(&p.name));
            } else {
                net_name.insert(p.net, format!("hv_n{}", p.net));
                port_alias.push((port_sv(&p.name), p.net, p.dir));
            }
        }
        // A net driven entirely by one register is that register.
        let mut reg_is_net: HashSet<u32> = HashSet::new();
        for &ni in &nets_r {
            let net = &self.nets[ni as usize];
            if net_name.contains_key(&ni) {
                continue;
            }
            if net.drivers.len() == 1 && net.drivers[0].lo == 0 && net.drivers[0].w == net.w {
                if let DrvSrc::Reg(r) = net.drivers[0].src {
                    net_name.insert(ni, format!("hv_r{r}"));
                    reg_is_net.insert(ni);
                    continue;
                }
            }
            net_name.insert(ni, format!("hv_n{ni}"));
        }

        let opnd = |dag: &Dag, n: N| -> String {
            match dag.op(n) {
                Op::Const(b) => lit(b),
                Op::Read(net) => net_name[net].clone(),
                _ => format!("hv_t{n}"),
            }
        };

        let mut v = String::new();
        writeln!(v, "// Generated by Helion's VHDL front end. Do not edit.").unwrap();
        let modname = sv_safe(&self.top_name);
        if modname != self.top_name {
            renames.push((self.top_name.clone(), modname.clone()));
        }
        for (o, m) in &renames {
            writeln!(v, "// port-rename {o} {m}").unwrap();
        }
        self.renames = renames;
        writeln!(v, "module {modname}({});", header.join(", ")).unwrap();
        let mut nets_sorted: Vec<u32> = nets_r.iter().copied().collect();
        nets_sorted.sort();
        for &ni in &nets_sorted {
            let name = &net_name[&ni];
            if self
                .top_ports
                .iter()
                .any(|p| p.net == ni && net_name[&ni] == port_sv(&p.name))
                || reg_is_net.contains(&ni)
            {
                continue;
            }
            let w = self.nets[ni as usize].w;
            if w == 0 {
                continue;
            }
            writeln!(
                v,
                "  wire {}{name}; // {}",
                range(w),
                self.nets[ni as usize].name
            )
            .unwrap();
        }
        let mut regs_sorted: Vec<u32> = regs_r.iter().copied().collect();
        regs_sorted.sort();
        for &ri in &regs_sorted {
            let g = &self.regs[ri as usize];
            writeln!(
                v,
                "  reg {}hv_r{ri} = {}; // {}",
                range(g.w),
                lit(&g.init),
                g.name
            )
            .unwrap();
        }
        for (pname, net, dir) in &port_alias {
            if *dir == Mode::In {
                writeln!(v, "  assign {} = {pname};", net_name[net]).unwrap();
            }
        }

        // Nodes in dependency order.
        let mut order: Vec<N> = Vec::new();
        let mut done: HashSet<N> = HashSet::new();
        let mut roots: Vec<N> = nodes_r.iter().copied().collect();
        roots.sort();
        for r in roots {
            let mut stack = vec![(r, false)];
            while let Some((x, exp)) = stack.pop() {
                if done.contains(&x) {
                    continue;
                }
                if exp {
                    done.insert(x);
                    order.push(x);
                    continue;
                }
                stack.push((x, true));
                if !matches!(self.dag.op(x), Op::Read(_) | Op::Const(_)) {
                    for k in self.dag.kids(x) {
                        if !done.contains(&k) {
                            stack.push((k, false));
                        }
                    }
                }
            }
        }
        // Register D expressions used nowhere else are written inside the
        // always_ff (`q <= q + 1`), which lets helion-sv recognise counters
        // and enables.
        let mut uses: HashMap<N, u32> = HashMap::new();
        for &n in &order {
            if !matches!(self.dag.op(n), Op::Read(_) | Op::Const(_)) {
                for k in self.dag.kids(n) {
                    *uses.entry(k).or_default() += 1;
                }
            }
        }
        for &ni in &nets_sorted {
            for d in &self.nets[ni as usize].drivers {
                if let DrvSrc::Comb(n) = d.src {
                    *uses.entry(n).or_default() += 1;
                }
            }
        }
        for &ri in &regs_sorted {
            let g = &self.regs[ri as usize];
            for n in [
                Some(g.d),
                Some(g.clk),
                g.arst.as_ref().map(|a| a.sig),
                g.arst.as_ref().map(|a| a.val),
            ]
            .into_iter()
            .flatten()
            {
                *uses.entry(n).or_default() += 1;
            }
        }
        let inline: HashSet<N> = regs_sorted
            .iter()
            .map(|&ri| self.regs[ri as usize].d)
            .filter(|d| {
                uses.get(d) == Some(&1)
                    && !matches!(self.dag.op(*d), Op::Read(_) | Op::Const(_) | Op::Slice(..))
            })
            .collect();
        let render = |dag: &Dag, n: N| -> Result<String, String> {
            let o = |k: N| opnd(dag, k);
            let w = dag.w(n);
            Ok(match dag.op(n) {
                Op::Const(_) | Op::Read(_) => o(n),
                Op::Hold(_) | Op::VHold(_) => unreachable!(),
                Op::Slice(a, lo) => {
                    let base = o(*a);
                    if matches!(dag.op(*a), Op::Const(_)) {
                        return Err("internal: slice of a constant".into());
                    }
                    if w == 1 {
                        format!("{base}[{lo}]")
                    } else {
                        format!("{base}[{}:{lo}]", lo + w - 1)
                    }
                }
                Op::Concat(p) => format!(
                    "{{{}}}",
                    p.iter().map(|k| o(*k)).collect::<Vec<_>>().join(", ")
                ),
                Op::Not(a) => format!("~{}", o(*a)),
                Op::And(a, b) => format!("{} & {}", o(*a), o(*b)),
                Op::Or(a, b) => format!("{} | {}", o(*a), o(*b)),
                Op::Xor(a, b) => format!("{} ^ {}", o(*a), o(*b)),
                Op::Add(a, b) => format!("{} + {}", o(*a), o(*b)),
                Op::Sub(a, b) => format!("{} - {}", o(*a), o(*b)),
                Op::Mul(a, b) => format!("{} * {}", o(*a), o(*b)),
                Op::Udiv(..) | Op::Urem(..) | Op::Shl(..) | Op::Lshr(..) => {
                    return Err("internal: unlowered division or shift".into());
                }
                Op::Eq(a, b) => format!("{} == {}", o(*a), o(*b)),
                Op::Ult(a, b) => format!("{} < {}", o(*a), o(*b)),
                Op::Mux(s, a, b) => format!("{} ? {} : {}", o(*s), o(*a), o(*b)),
                Op::RedAnd(a) => format!("&{}", o(*a)),
                Op::RedOr(a) => format!("|{}", o(*a)),
                Op::RedXor(a) => format!("^{}", o(*a)),
            })
        };
        for n in order {
            let w = self.dag.w(n);
            if w == 0 || inline.contains(&n) {
                continue;
            }
            if matches!(self.dag.op(n), Op::Const(_) | Op::Read(_)) {
                continue;
            }
            let rhs = render(&self.dag, n)?;
            writeln!(v, "  wire {}hv_t{n} = {rhs};", range(w)).unwrap();
        }

        // Net values.
        for &ni in &nets_sorted {
            if reg_is_net.contains(&ni) || self.is_top_input(ni) {
                continue;
            }
            let net = &self.nets[ni as usize];
            if net.w == 0 {
                continue;
            }
            let mut segs: Vec<(u32, u32, String)> = Vec::new();
            for d in &net.drivers {
                let s = match d.src {
                    DrvSrc::Comb(n) => opnd(&self.dag, n),
                    DrvSrc::Reg(r) => format!("hv_r{r}"),
                };
                segs.push((d.lo, d.w, s));
            }
            segs.sort_by_key(|s| s.0);
            let mut parts: Vec<String> = Vec::new();
            let mut pos = 0u32;
            for (lo, w, s) in segs {
                if lo > pos {
                    parts.push(lit(&net.init[pos as usize..lo as usize]));
                }
                parts.push(s);
                pos = lo + w;
            }
            if pos < net.w {
                parts.push(lit(&net.init[pos as usize..]));
            }
            parts.reverse();
            let rhs = if parts.len() == 1 {
                parts.pop().unwrap()
            } else {
                format!("{{{}}}", parts.join(", "))
            };
            writeln!(v, "  assign {} = {rhs};", net_name[&ni]).unwrap();
        }
        for (pname, net, dir) in &port_alias {
            if *dir != Mode::In {
                writeln!(v, "  assign {pname} = {};", net_name[net]).unwrap();
            }
        }

        // Registers.
        for &ri in &regs_sorted {
            let g = &self.regs[ri as usize];
            let clk = opnd(&self.dag, g.clk);
            let d = if inline.contains(&g.d) {
                render(&self.dag, g.d)?
            } else {
                opnd(&self.dag, g.d)
            };
            match &g.arst {
                None => writeln!(v, "  always_ff @(posedge {clk}) hv_r{ri} <= {d};").unwrap(),
                Some(a) => {
                    let rst = opnd(&self.dag, a.sig);
                    let rv = opnd(&self.dag, a.val);
                    let (edge, test) = if a.active_high {
                        ("posedge", rst.clone())
                    } else {
                        ("negedge", format!("!{rst}"))
                    };
                    writeln!(v, "  always_ff @(posedge {clk} or {edge} {rst}) if ({test}) hv_r{ri} <= {rv}; else hv_r{ri} <= {d};").unwrap();
                }
            }
        }
        writeln!(v, "endmodule").unwrap();
        Ok(v)
    }
}
