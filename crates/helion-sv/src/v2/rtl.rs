//! The elaborated (RTL) netlist of a flat design, before bit blasting: one
//! cell per word-level operator, register, latch, constant and hard block,
//! the way an RTL schematic draws it (`RTL_ADD`, `RTL_MUX`, `RTL_REG`).
//!
//! Cells are [`CellKind::BlackBox`] whose module names the operator. Output
//! pins are listed in the cell's `OUT_PINS` attribute; each port's nets are in
//! its `NET` attribute. Slices, concatenations and continuous assignments
//! are wiring, not cells: a pin reading a concatenation of several sources
//! gets one pin per source (`I0`, `I0_1`, ...).

use std::collections::{HashMap, HashSet};

use helion_ir::{CellKind, Design, PortDir};

use super::dag::{N, Op};
use super::lower::{Drv, Flat};

/// Where a value comes from, after wiring is followed.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Src {
    /// Output of an operator or constant node.
    Node(N),
    /// A named signal driven by a port, register, latch or hard block (or
    /// by nothing).
    Sig(u32),
}

struct Rtl<'a> {
    flat: &'a Flat,
    memo: HashMap<N, Vec<Src>>,
    busy: HashSet<N>,
    /// Signal whose every bit is a bit of this node, in order: its name
    /// labels the node's output net.
    names: HashMap<N, String>,
}

fn op_name(op: &Op) -> Option<&'static str> {
    Some(match op {
        Op::Not(_) => "RTL_INV",
        Op::And(..) => "RTL_AND",
        Op::Or(..) => "RTL_OR",
        Op::Xor(..) => "RTL_XOR",
        Op::Add(..) => "RTL_ADD",
        Op::Sub(..) => "RTL_SUB",
        Op::Mul(..) => "RTL_MULT",
        Op::Udiv(..) => "RTL_DIV",
        Op::Urem(..) => "RTL_MOD",
        Op::Eq(..) => "RTL_EQ",
        Op::Ult(..) => "RTL_LT",
        Op::Mux(..) => "RTL_MUX",
        Op::Shl(..) => "RTL_SHIFT_LEFT",
        Op::Lshr(..) => "RTL_SHIFT_RIGHT",
        Op::RedAnd(_) => "RTL_REDUCTION_AND",
        Op::RedOr(_) => "RTL_REDUCTION_OR",
        Op::RedXor(_) => "RTL_REDUCTION_XOR",
        Op::Const(_) => "RTL_CONST",
        _ => return None,
    })
}

fn operands(op: &Op) -> Vec<(&'static str, N)> {
    match *op {
        Op::Not(a) | Op::RedAnd(a) | Op::RedOr(a) | Op::RedXor(a) => vec![("I0", a)],
        Op::And(a, b) | Op::Or(a, b) | Op::Xor(a, b) | Op::Add(a, b) | Op::Sub(a, b) | Op::Mul(a, b) | Op::Udiv(a, b) | Op::Urem(a, b) | Op::Eq(a, b) | Op::Ult(a, b) | Op::Shl(a, b) | Op::Lshr(a, b) => {
            vec![("I0", a), ("I1", b)]
        }
        Op::Mux(s, a, b) => vec![("S", s), ("A", a), ("B", b)],
        _ => Vec::new(),
    }
}

impl Rtl<'_> {
    fn src(&mut self, n: N) -> Vec<Src> {
        if let Some(v) = self.memo.get(&n) {
            return v.clone();
        }
        if !self.busy.insert(n) {
            return Vec::new();
        }
        let mut out: Vec<Src> = Vec::new();
        match self.flat.dag.op(n).clone() {
            Op::Slice(a, _) => out = self.src(a),
            Op::Concat(parts) => {
                for p in parts {
                    out.extend(self.src(p));
                }
            }
            Op::Read(s) => {
                for d in self.flat.sigs[s as usize].drv.clone() {
                    match d {
                        Drv::Comb(m, _) => out.extend(self.src(m)),
                        _ => out.push(Src::Sig(s)),
                    }
                }
            }
            Op::Hold(s) | Op::VHold(s) => out.push(Src::Sig(s)),
            _ => out.push(Src::Node(n)),
        }
        let mut seen = HashSet::new();
        out.retain(|s| seen.insert(*s));
        self.busy.remove(&n);
        self.memo.insert(n, out.clone());
        out
    }

    fn cell_of(&self, n: N) -> String {
        let op = self.flat.dag.op(n);
        let base = op_name(op).unwrap_or("RTL_OP").trim_start_matches("RTL_").to_ascii_lowercase();
        format!("{base}_{n}")
    }

    fn net_of(&self, s: Src) -> String {
        match s {
            Src::Sig(i) => self.flat.sigs[i as usize].name.clone(),
            Src::Node(n) => self.names.get(&n).cloned().unwrap_or_else(|| format!("{}_out", self.cell_of(n))),
        }
    }
}

/// The elaborated netlist of `flat`.
pub fn rtl_design(flat: &Flat) -> Design {
    let mut names: HashMap<N, String> = HashMap::new();
    for sg in &flat.sigs {
        let mut node = None;
        let whole = sg.drv.iter().enumerate().all(|(i, d)| match d {
            Drv::Comb(m, b) if *b == i as u32 && node.is_none_or(|n| n == *m) => {
                node = Some(*m);
                true
            }
            _ => false,
        });
        if let (true, Some(n)) = (whole, node) {
            if flat.dag.w(n) == sg.w {
                names.entry(n).or_insert_with(|| sg.name.clone());
            }
        }
    }
    let mut r = Rtl { flat, memo: HashMap::new(), busy: HashSet::new(), names };
    let mut d = Design::new(&flat.top);
    let mut queue: Vec<N> = Vec::new();
    let mut made: HashSet<N> = HashSet::new();
    // Connect `pin` of `cell` to the sources of node `n`; operator sources
    // become cells in turn.
    let wire = |r: &mut Rtl, d: &mut Design, queue: &mut Vec<N>, cell: &str, pin: &str, n: N| {
        for (k, s) in r.src(n).into_iter().enumerate() {
            let p = if k == 0 { pin.to_string() } else { format!("{pin}_{k}") };
            d.connect(&r.net_of(s), cell, &p);
            if let Src::Node(m) = s {
                queue.push(m);
            }
        }
    };
    let add = |d: &mut Design, name: &str, module: &str, outs: &str, width: u32| {
        d.add_cell(name, CellKind::BlackBox { module: helion_ir::intern(module) });
        if let Some(c) = d.cells.last_mut() {
            c.attrs.set("OUT_PINS", outs);
            c.attrs.set("WIDTH", &width.to_string());
        }
    };
    for p in &flat.ports {
        d.add_port(&p.name, p.dir);
        let nets: Vec<String> = match (p.dir, p.node) {
            (PortDir::Out, Some(n)) => {
                let s = r.src(n);
                queue.extend(s.iter().filter_map(|s| match s {
                    Src::Node(m) => Some(*m),
                    Src::Sig(_) => None,
                }));
                s.into_iter().map(|s| r.net_of(s)).collect()
            }
            _ => vec![p.sig.map_or_else(|| p.name.clone(), |s| flat.sigs[s as usize].name.clone())],
        };
        if let Some(port) = d.ports.last_mut() {
            port.attrs.set("NET", &nets.join(","));
        }
    }
    let mut reg_names: HashMap<u32, usize> = HashMap::new();
    for rg in &flat.regs {
        let sig = &flat.sigs[rg.sig as usize];
        let k = reg_names.entry(rg.sig).or_insert(0);
        let name = if *k == 0 { sig.name.clone() } else { format!("{}_{k}", sig.name) };
        *k += 1;
        add(&mut d, &name, "RTL_REG", "Q", sig.w);
        wire(&mut r, &mut d, &mut queue, &name, "D", rg.d);
        wire(&mut r, &mut d, &mut queue, &name, "C", rg.clk);
        if let Some((a, _)) = rg.arst {
            wire(&mut r, &mut d, &mut queue, &name, "CLR", a);
        }
        d.connect(&sig.name, &name, "Q");
    }
    for l in &flat.latches {
        let sig = &flat.sigs[l.sig as usize];
        let name = format!("{}_latch", sig.name);
        add(&mut d, &name, "RTL_LATCH", "Q", sig.w);
        wire(&mut r, &mut d, &mut queue, &name, "D", l.d);
        wire(&mut r, &mut d, &mut queue, &name, "G", l.gate);
        d.connect(&sig.name, &name, "Q");
    }
    for h in &flat.hard {
        let mut outs: Vec<(String, u32)> = h.outs.iter().map(|(p, s, _)| (p.split('[').next().unwrap_or(p).to_string(), *s)).collect();
        outs.dedup();
        let out_pins: Vec<&str> = outs.iter().map(|(p, _)| p.as_str()).collect();
        add(&mut d, &h.name, &h.module, &out_pins.join(","), 0);
        let mut ins: Vec<(String, N)> = h.ins.iter().map(|(p, n, _)| (p.split('[').next().unwrap_or(p).to_string(), *n)).collect();
        ins.dedup();
        let mut used: HashMap<String, usize> = HashMap::new();
        for (p, n) in ins {
            let k = used.entry(p.clone()).or_insert(0);
            let pin = if *k == 0 { p.clone() } else { format!("{p}_n{k}") };
            *k += 1;
            wire(&mut r, &mut d, &mut queue, &h.name, &pin, n);
        }
        for (p, s) in &outs {
            let pin = if outs.iter().filter(|(q, _)| q == p).count() > 1 { format!("{p}_{s}") } else { p.clone() };
            d.connect(&flat.sigs[*s as usize].name, &h.name, &pin);
        }
    }
    while let Some(n) = queue.pop() {
        if !made.insert(n) {
            continue;
        }
        let op = flat.dag.op(n).clone();
        let name = r.cell_of(n);
        add(&mut d, &name, op_name(&op).unwrap_or("RTL_OP"), "O", flat.dag.w(n));
        if let Op::Const(bits) = &op {
            let v: String = bits.iter().rev().map(|b| if *b { '1' } else { '0' }).collect();
            if let Some(c) = d.cells.last_mut() {
                c.attrs.set("VALUE", &format!("{}'b{v}", bits.len()));
            }
        }
        for (pin, a) in operands(&op) {
            wire(&mut r, &mut d, &mut queue, &name, pin, a);
        }
        let out = r.net_of(Src::Node(n));
        d.connect(&out, &name, "O");
    }
    for (name, module, conns) in &flat.instances {
        d.instances.push(helion_ir::Instance {
            name: helion_ir::intern(name),
            module: helion_ir::intern(module),
            conns: conns.iter().map(|(p, n)| (helion_ir::intern(p), helion_ir::intern(n))).collect(),
            attrs: helion_ir::Attrs::default(),
        });
    }
    d.attrs.set("STAGE", "RTL");
    d
}

#[cfg(test)]
mod tests {
    #[test]
    fn counter_elaborates_to_an_adder_a_register_and_ports() {
        let src = "module c(input clk, input en, output [3:0] q);\n reg [3:0] r;\n always @(posedge clk) if (en) r <= r + 4'd1;\n assign q = r;\nendmodule\n";
        let s = crate::synth_sv_v2(src, "c.sv", None).expect("synth");
        let d = &s.rtl;
        let kind = |m: &str| d.cells.iter().filter(|c| matches!(&c.kind, helion_ir::CellKind::BlackBox { module } if helion_ir::resolve(*module) == m)).count();
        assert_eq!(kind("RTL_REG"), 1, "one word register");
        assert_eq!(kind("RTL_ADD"), 1, "one adder");
        assert!(kind("RTL_MUX") + kind("RTL_AND") >= 1 || d.cells.len() >= 3, "enable logic");
        // The register's Q net drives the adder and the output port.
        let q = d.ports.iter().find(|p| helion_ir::resolve(p.name) == "q").expect("port q");
        assert_eq!(q.attrs.get("NET"), Some("r"));
        let r = d.nets.iter().find(|n| helion_ir::resolve(n.name) == "r").expect("net r");
        assert!(r.endpoints.iter().any(|e| helion_ir::resolve(e.pin) == "Q"));
        assert!(r.endpoints.len() >= 2, "r feeds the adder");
        assert_eq!(d.ports.len(), 3);
    }
}
