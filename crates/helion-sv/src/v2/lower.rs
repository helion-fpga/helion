//! Word-level graph to netlist: bit-blast into an and-inverter graph, cover
//! it with 6-input LUTs, and write LUT, flop and pad cells.
//!
//! The cover is depth-optimal over priority cuts (a bounded set of cuts per
//! node, best depth first, then area flow). Every LUT's truth table is
//! computed from the graph, so the netlist is the design by construction;
//! `tools/svcheck` compares it with the RTL anyway.

use super::dag::{Dag, Op, N};
use helion_ir::{CellKind, Design, PortDir};
use std::collections::HashMap;

/// A literal: node index * 2 + inverted. Node 0 is constant false.
pub type Lit = u32;
pub const FALSE: Lit = 0;
pub const TRUE: Lit = 1;

#[derive(Default)]
pub struct Aig {
    /// Fan-ins of AND nodes; `(u32::MAX, _)` marks an input or the constant.
    nodes: Vec<(Lit, Lit)>,
    strash: HashMap<(Lit, Lit), u32>,
}

impl Aig {
    pub fn new() -> Self {
        Aig { nodes: vec![(u32::MAX, 0)], strash: HashMap::new() }
    }
    pub fn input(&mut self) -> Lit {
        self.nodes.push((u32::MAX, 0));
        (self.nodes.len() as u32 - 1) * 2
    }
    fn is_and(&self, node: u32) -> bool {
        self.nodes[node as usize].0 != u32::MAX
    }
    pub fn and(&mut self, a: Lit, b: Lit) -> Lit {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        if a == FALSE || a == (b ^ 1) {
            return FALSE;
        }
        if a == TRUE || a == b {
            return b;
        }
        if let Some(&n) = self.strash.get(&(a, b)) {
            return n * 2;
        }
        self.nodes.push((a, b));
        let n = self.nodes.len() as u32 - 1;
        self.strash.insert((a, b), n);
        n * 2
    }
    pub fn or(&mut self, a: Lit, b: Lit) -> Lit {
        self.and(a ^ 1, b ^ 1) ^ 1
    }
    pub fn xor(&mut self, a: Lit, b: Lit) -> Lit {
        let p = self.and(a, b ^ 1);
        let q = self.and(a ^ 1, b);
        self.or(p, q)
    }
    pub fn mux(&mut self, s: Lit, a: Lit, b: Lit) -> Lit {
        if a == b {
            return a;
        }
        let p = self.and(s, a);
        let q = self.and(s ^ 1, b);
        self.or(p, q)
    }
    fn tree(&mut self, mut v: Vec<Lit>, f: fn(&mut Aig, Lit, Lit) -> Lit, empty: Lit) -> Lit {
        if v.is_empty() {
            return empty;
        }
        while v.len() > 1 {
            let mut next = Vec::with_capacity(v.len().div_ceil(2));
            for c in v.chunks(2) {
                next.push(if c.len() == 2 { f(self, c[0], c[1]) } else { c[0] });
            }
            v = next;
        }
        v[0]
    }
}

/// What drives one bit of a signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drv {
    /// Nothing: the bit reads 0 and is reported.
    None,
    /// Bit `.1` of graph node `.0`.
    Comb(N, u32),
    /// Flop of register `.0`.
    Reg(u32),
    /// Top-level input.
    Input,
}

pub struct SigInfo {
    pub name: String,
    pub w: u32,
    pub drv: Vec<Drv>,
    /// Power-up value of register bits.
    pub init: Vec<bool>,
}

/// The flops of one clocked process for one signal: bit `i` is a flop when
/// `mask[i]`, with data input bit `i` of `d`.
pub struct Reg {
    pub sig: u32,
    pub d: N,
    pub clk: N,
    pub negedge: bool,
    pub mask: Vec<bool>,
}

pub struct Port {
    pub name: String,
    pub dir: PortDir,
    pub w: u32,
    /// Output: the value.
    pub node: Option<N>,
    pub sig: Option<u32>,
}

/// The elaborated, flat design.
pub struct Flat {
    pub top: String,
    pub dag: Dag,
    pub sigs: Vec<SigInfo>,
    pub ports: Vec<Port>,
    pub regs: Vec<Reg>,
}

pub fn bit_name(sig: &str, w: u32, bit: u32) -> String {
    if w == 1 {
        sig.to_string()
    } else {
        format!("{sig}_{bit}")
    }
}

/// Bit-blasts on demand: a bit of a node is built only when something
/// needs it, and a signal is followed to its driver bit by bit. A design
/// whose words depend on themselves but whose bits do not (a carry chain
/// written as `assign c[i+1] = f(c[i])`) is therefore fine, and logic that
/// feeds nothing is never built.
struct Blaster<'a> {
    flat: &'a Flat,
    aig: Aig,
    memo: HashMap<(N, u32), Lit>,
    /// Carry into bit `i` of an adder node (index 0 is the carry in).
    carries: HashMap<N, Vec<Lit>>,
    whole: HashMap<N, Vec<Lit>>,
    /// (signal, bit) -> literal of that bit.
    sig_bit: HashMap<(u32, u32), Lit>,
    visiting: std::collections::HashSet<(u32, u32)>,
    /// Inputs of the logic: (literal, signal, bit).
    pi_name: Vec<(Lit, u32, u32)>,
    /// Register bits whose output is used, in first-use order.
    live_regs: Vec<(u32, u32)>,
    hints: Vec<(Lit, u32, u32)>,
    undriven: Vec<String>,
}

impl<'a> Blaster<'a> {
    fn read_bit(&mut self, sig: u32, bit: u32) -> Result<Lit, String> {
        if let Some(&l) = self.sig_bit.get(&(sig, bit)) {
            return Ok(l);
        }
        let s = &self.flat.sigs[sig as usize];
        let l = match s.drv[bit as usize] {
            Drv::Input => {
                let l = self.aig.input();
                self.pi_name.push((l, sig, bit));
                l
            }
            Drv::Reg(_) => {
                let l = self.aig.input();
                self.pi_name.push((l, sig, bit));
                self.live_regs.push((sig, bit));
                // A flop that powers up at 1 stores the inverted value.
                if s.init[bit as usize] { l ^ 1 } else { l }
            }
            Drv::None => {
                self.undriven.push(bit_name(&s.name, s.w, bit));
                FALSE
            }
            Drv::Comb(n, b) => {
                if !self.visiting.insert((sig, bit)) {
                    return Err(format!("combinational loop through '{}'", bit_name(&s.name, s.w, bit)));
                }
                let l = self.bit(n, b)?;
                self.visiting.remove(&(sig, bit));
                self.hints.push((l, sig, bit));
                l
            }
        };
        self.sig_bit.insert((sig, bit), l);
        Ok(l)
    }

    fn all(&mut self, n: N) -> Result<Vec<Lit>, String> {
        (0..self.flat.dag.w(n)).map(|i| self.bit(n, i)).collect()
    }

    /// Carry into bit `i` of adder node `n` (`a + b + cin`).
    fn carry(&mut self, n: N, a: N, b: N, invert_b: bool, i: u32) -> Result<Lit, String> {
        let have = self.carries.get(&n).map(|c| c.len()).unwrap_or(0);
        if have == 0 {
            self.carries.insert(n, vec![if invert_b { TRUE } else { FALSE }]);
        }
        let mut k = self.carries[&n].len() as u32 - 1;
        while k < i {
            let x = self.bit(a, k)?;
            let y = self.bit(b, k)? ^ (invert_b as u32);
            let c = self.carries[&n][k as usize];
            let p = self.aig.xor(x, y);
            let g = self.aig.and(x, y);
            let t = self.aig.and(p, c);
            let nc = self.aig.or(g, t);
            self.carries.get_mut(&n).unwrap().push(nc);
            k += 1;
        }
        Ok(self.carries[&n][i as usize])
    }

    fn bit(&mut self, n: N, i: u32) -> Result<Lit, String> {
        if let Some(&l) = self.memo.get(&(n, i)) {
            return Ok(l);
        }
        let dag = &self.flat.dag;
        let w = dag.w(n);
        debug_assert!(i < w);
        let l = match dag.op(n).clone() {
            Op::Const(b) => {
                if b[i as usize] { TRUE } else { FALSE }
            }
            Op::Read(sig) => self.read_bit(sig, i)?,
            Op::Hold(_) | Op::VHold(_) => return Err("internal: a hold value reached the netlist".into()),
            Op::Slice(a, lo) => self.bit(a, lo + i)?,
            Op::Concat(parts) => {
                let mut off = 0;
                let mut found = None;
                for p in parts.iter().rev() {
                    let pw = dag.w(*p);
                    if i < off + pw {
                        found = Some((*p, i - off));
                        break;
                    }
                    off += pw;
                }
                let (p, k) = found.ok_or("internal: concat bit out of range")?;
                self.bit(p, k)?
            }
            Op::Not(a) => self.bit(a, i)? ^ 1,
            Op::And(a, b) => {
                let (x, y) = (self.bit(a, i)?, self.bit(b, i)?);
                self.aig.and(x, y)
            }
            Op::Or(a, b) => {
                let (x, y) = (self.bit(a, i)?, self.bit(b, i)?);
                self.aig.or(x, y)
            }
            Op::Xor(a, b) => {
                let (x, y) = (self.bit(a, i)?, self.bit(b, i)?);
                self.aig.xor(x, y)
            }
            Op::Add(a, b) | Op::Sub(a, b) => {
                let inv = matches!(dag.op(n), Op::Sub(..));
                let c = self.carry(n, a, b, inv, i)?;
                let x = self.bit(a, i)?;
                let y = self.bit(b, i)? ^ (inv as u32);
                let p = self.aig.xor(x, y);
                self.aig.xor(p, c)
            }
            Op::Mul(a, b) => {
                if !self.whole.contains_key(&n) {
                    let (x, y) = (self.all(a)?, self.all(b)?);
                    let w = w as usize;
                    let mut acc = vec![FALSE; w];
                    for (k, &yk) in y.iter().enumerate() {
                        if yk == FALSE {
                            continue;
                        }
                        let mut c = FALSE;
                        for j in k..w {
                            let t = self.aig.and(x[j - k], yk);
                            let p = self.aig.xor(acc[j], t);
                            let s = self.aig.xor(p, c);
                            let g = self.aig.and(acc[j], t);
                            let h = self.aig.and(p, c);
                            c = self.aig.or(g, h);
                            acc[j] = s;
                        }
                    }
                    self.whole.insert(n, acc);
                }
                self.whole[&n][i as usize]
            }
            Op::Udiv(..) | Op::Urem(..) | Op::Shl(..) | Op::Lshr(..) => {
                return Err("internal: division or variable shift was not expanded".into())
            }
            Op::Eq(a, b) => {
                let (x, y) = (self.all(a)?, self.all(b)?);
                let eqs: Vec<Lit> = x.iter().zip(&y).map(|(p, q)| self.aig.xor(*p, *q) ^ 1).collect();
                self.aig.tree(eqs, Aig::and, TRUE)
            }
            Op::Ult(a, b) => {
                // a < b  <=>  no carry out of a + ~b + 1
                let aw = dag.w(a);
                self.carry(n, a, b, true, aw)? ^ 1
            }
            Op::Mux(s, a, b) => {
                let sel = self.bit(s, 0)?;
                if sel == TRUE {
                    self.bit(a, i)?
                } else if sel == FALSE {
                    self.bit(b, i)?
                } else {
                    let (x, y) = (self.bit(a, i)?, self.bit(b, i)?);
                    self.aig.mux(sel, x, y)
                }
            }
            Op::RedAnd(a) => {
                let v = self.all(a)?;
                self.aig.tree(v, Aig::and, TRUE)
            }
            Op::RedOr(a) => {
                let v = self.all(a)?;
                self.aig.tree(v, Aig::or, FALSE)
            }
            Op::RedXor(a) => {
                let v = self.all(a)?;
                self.aig.tree(v, Aig::xor, FALSE)
            }
        };
        self.memo.insert((n, i), l);
        Ok(l)
    }
}

const K: usize = 6;
const CUTS: usize = 8;
const VAR: [u64; 6] = [
    0xAAAA_AAAA_AAAA_AAAA,
    0xCCCC_CCCC_CCCC_CCCC,
    0xF0F0_F0F0_F0F0_F0F0,
    0xFF00_FF00_FF00_FF00,
    0xFFFF_0000_FFFF_0000,
    0xFFFF_FFFF_0000_0000,
];

#[derive(Clone)]
struct Cut {
    leaves: Vec<u32>,
    tt: u64,
    depth: u32,
    flow: f32,
}

/// Truth table `tt` over `from` rewritten over the superset `to`.
fn expand(tt: u64, from: &[u32], to: &[u32]) -> u64 {
    if from.len() == to.len() {
        return tt;
    }
    let pos: Vec<usize> = from.iter().map(|l| to.iter().position(|x| x == l).unwrap()).collect();
    let mut out = 0u64;
    for m in 0..(1u64 << to.len()) {
        let mut idx = 0u64;
        for (i, &p) in pos.iter().enumerate() {
            idx |= ((m >> p) & 1) << i;
        }
        out |= ((tt >> idx) & 1) << m;
    }
    // Replicate so unused high variables do not matter.
    let mut span = 1u32 << to.len();
    while span < 64 {
        out |= out << span;
        span *= 2;
    }
    out
}

fn merge_leaves(a: &[u32], b: &[u32]) -> Option<Vec<u32>> {
    let mut out = Vec::with_capacity(K);
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let x = if j >= b.len() || (i < a.len() && a[i] < b[j]) {
            i += 1;
            a[i - 1]
        } else if i >= a.len() || b[j] < a[i] {
            j += 1;
            b[j - 1]
        } else {
            i += 1;
            j += 1;
            a[i - 1]
        };
        if out.len() == K {
            return None;
        }
        out.push(x);
    }
    Some(out)
}

struct Mapper {
    cuts: Vec<Vec<Cut>>,
    depth: Vec<u32>,
    flow: Vec<f32>,
}

impl Mapper {
    fn run(aig: &Aig) -> Self {
        let n = aig.nodes.len();
        let mut refs = vec![0u32; n];
        for &(a, b) in &aig.nodes {
            if a != u32::MAX {
                refs[(a / 2) as usize] += 1;
                refs[(b / 2) as usize] += 1;
            }
        }
        let mut m = Mapper { cuts: vec![Vec::new(); n], depth: vec![0; n], flow: vec![0.0; n] };
        for i in 0..n {
            let (a, b) = aig.nodes[i];
            if a == u32::MAX {
                continue;
            }
            let (na, nb) = ((a / 2) as usize, (b / 2) as usize);
            let trivial = |k: usize, m: &Mapper| Cut { leaves: vec![k as u32], tt: VAR[0], depth: m.depth[k], flow: m.flow[k] };
            let ca: Vec<Cut> = std::iter::once(trivial(na, &m)).chain(m.cuts[na].iter().cloned()).collect();
            let cb: Vec<Cut> = std::iter::once(trivial(nb, &m)).chain(m.cuts[nb].iter().cloned()).collect();
            let mut cand: Vec<(Vec<u32>, usize, usize, u32, f32)> = Vec::new();
            for (ia, x) in ca.iter().enumerate() {
                for (ib, y) in cb.iter().enumerate() {
                    let Some(leaves) = merge_leaves(&x.leaves, &y.leaves) else { continue };
                    if cand.iter().any(|c| c.0 == leaves) {
                        continue;
                    }
                    let depth = 1 + leaves.iter().map(|&l| m.depth[l as usize]).max().unwrap_or(0);
                    let flow = 1.0 + leaves.iter().map(|&l| m.flow[l as usize]).sum::<f32>();
                    cand.push((leaves, ia, ib, depth, flow));
                }
            }
            cand.sort_by(|p, q| p.3.cmp(&q.3).then(p.4.partial_cmp(&q.4).unwrap()).then(p.0.len().cmp(&q.0.len())));
            cand.truncate(CUTS);
            let mut out = Vec::with_capacity(cand.len());
            for (leaves, ia, ib, depth, flow) in cand {
                let mut ta = expand(ca[ia].tt, &ca[ia].leaves, &leaves);
                let mut tb = expand(cb[ib].tt, &cb[ib].leaves, &leaves);
                if a & 1 == 1 {
                    ta = !ta;
                }
                if b & 1 == 1 {
                    tb = !tb;
                }
                out.push(Cut { leaves, tt: ta & tb, depth, flow });
            }
            m.depth[i] = out[0].depth;
            m.flow[i] = out[0].flow / refs[i].max(1) as f32;
            m.cuts[i] = out;
        }
        m
    }
}

pub struct Stats {
    pub luts: usize,
    pub ffs: usize,
    pub depth: u32,
    pub ands: usize,
}

/// Build the netlist of `flat`. Returns it with statistics and the names
/// of signal bits that nothing drives (they read 0).
pub fn lower(flat: &Flat) -> Result<(Design, Stats, Vec<String>), String> {
    let mut bl = Blaster {
        flat,
        aig: Aig::new(),
        memo: HashMap::new(),
        carries: HashMap::new(),
        whole: HashMap::new(),
        sig_bit: HashMap::new(),
        visiting: Default::default(),
        pi_name: Vec::new(),
        live_regs: Vec::new(),
        hints: Vec::new(),
        undriven: Vec::new(),
    };
    // Roots: output bits, then the data and clock of every flop whose
    // output something uses.
    let mut out_bits: Vec<(usize, u32, Lit)> = Vec::new();
    for (pi, p) in flat.ports.iter().enumerate() {
        if p.dir == PortDir::Out {
            let n = p.node.ok_or_else(|| format!("output {} has no value", p.name))?;
            for i in 0..p.w {
                let l = bl.bit(n, i)?;
                out_bits.push((pi, i, l));
            }
        }
    }
    struct RegBit {
        sig: u32,
        bit: u32,
        d: Lit,
        clk: Lit,
        negedge: bool,
    }
    let mut reg_bits: Vec<RegBit> = Vec::new();
    let mut done = 0usize;
    while done < bl.live_regs.len() {
        let (sig, bit) = bl.live_regs[done];
        done += 1;
        let Drv::Reg(ri) = flat.sigs[sig as usize].drv[bit as usize] else { unreachable!() };
        let r = &flat.regs[ri as usize];
        let mut d = bl.bit(r.d, bit)?;
        if flat.sigs[sig as usize].init[bit as usize] {
            d ^= 1;
        }
        let clk = bl.bit(r.clk, 0)?;
        reg_bits.push(RegBit { sig, bit, d, clk, negedge: r.negedge });
    }
    let Blaster { aig, pi_name, hints, undriven, .. } = bl;
    let map = Mapper::run(&aig);

    let mut d = Design::new(&flat.top);
    for p in &flat.ports {
        d.add_port(&p.name, p.dir);
    }
    // Net name of every literal that exists as a net.
    let mut net_of: HashMap<Lit, String> = HashMap::new();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    for &(l, sig, bit) in &pi_name {
        let s = &flat.sigs[sig as usize];
        let n = bit_name(&s.name, s.w, bit);
        taken.insert(n.clone());
        net_of.insert(l, n);
    }
    for p in &flat.ports {
        if p.dir != PortDir::Out {
            for i in 0..p.w {
                taken.insert(bit_name(&p.name, p.w, i));
            }
        }
        if p.w > 1 {
            // The pads of a bus all sit on the net named for the port.
            taken.insert(p.name.clone());
        }
    }
    let mut hint: HashMap<Lit, String> = HashMap::new();
    // Output port bits name the net of the literal they show, when free;
    // other signal bits name theirs after that.
    for &(pi, bit, l) in &out_bits {
        let p = &flat.ports[pi];
        if l > TRUE && !net_of.contains_key(&l) {
            hint.entry(l).or_insert(bit_name(&p.name, p.w, bit));
        }
    }
    for (l, sig, bit) in hints {
        let s = &flat.sigs[sig as usize];
        let name = bit_name(&s.name, s.w, bit);
        if l > TRUE && !taken.contains(&name) {
            hint.entry(l).or_insert(name);
        }
    }

    let mut n_lut = 0usize;
    let mut depth = 0u32;
    // Emit the LUT for literal `l` (and, recursively, its leaves).
    fn emit(
        l: Lit,
        aig: &Aig,
        map: &Mapper,
        d: &mut Design,
        net_of: &mut HashMap<Lit, String>,
        hint: &HashMap<Lit, String>,
        taken: &mut std::collections::HashSet<String>,
        n_lut: &mut usize,
    ) -> String {
        // Iterative: a stack of literals whose LUT is still to be written.
        let mut stack = vec![l];
        while let Some(&top) = stack.last() {
            if net_of.contains_key(&top) {
                stack.pop();
                continue;
            }
            let node = top / 2;
            if top <= TRUE {
                let name = if top == TRUE { "const1" } else { "const0" }.to_string();
                if !taken.contains(&name) {
                    taken.insert(name.clone());
                    let cell = format!("u_{name}");
                    d.add_cell(&cell, CellKind::Lut6 { init: if top == TRUE { u64::MAX } else { 0 } });
                    d.connect(&name, &cell, "O");
                    *n_lut += 1;
                }
                net_of.insert(top, name);
                stack.pop();
                continue;
            }
            if !aig.is_and(node) {
                // An inverted input: one inverter.
                let src = net_of[&(top ^ 1)].clone();
                let name = fresh(hint.get(&top), taken, n_lut);
                let cell = format!("{name}_lut");
                d.add_cell(&cell, CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
                d.connect(&src, &cell, "I0");
                d.connect(&name, &cell, "O");
                *n_lut += 1;
                net_of.insert(top, name);
                stack.pop();
                continue;
            }
            let cut = &map.cuts[node as usize][0];
            let mut ready = true;
            for &leaf in &cut.leaves {
                if !net_of.contains_key(&(leaf * 2)) {
                    stack.push(leaf * 2);
                    ready = false;
                }
            }
            if !ready {
                continue;
            }
            let name = fresh(hint.get(&top), taken, n_lut);
            let cell = format!("{name}_lut");
            let tt = if top & 1 == 1 { !cut.tt } else { cut.tt };
            d.add_cell(&cell, CellKind::Lut6 { init: tt });
            for (i, &leaf) in cut.leaves.iter().enumerate() {
                d.connect(&net_of[&(leaf * 2)], &cell, &format!("I{i}"));
            }
            d.connect(&name, &cell, "O");
            *n_lut += 1;
            net_of.insert(top, name);
            stack.pop();
        }
        net_of[&l].clone()
    }
    fn fresh(hint: Option<&String>, taken: &mut std::collections::HashSet<String>, n_lut: &usize) -> String {
        if let Some(h) = hint {
            if taken.insert(h.clone()) {
                return h.clone();
            }
        }
        let mut k = *n_lut;
        loop {
            let name = format!("n{k}");
            if taken.insert(name.clone()) {
                return name;
            }
            k += 1;
        }
    }

    // Registers.
    for rb in &reg_bits {
        let s = &flat.sigs[rb.sig as usize];
        let q = bit_name(&s.name, s.w, rb.bit);
        let cell = format!("{q}_reg");
        depth = depth.max(map.depth[(rb.d / 2) as usize]);
        let dnet = emit(rb.d, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
        let cnet = emit(rb.clk, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
        d.add_cell(&cell, CellKind::Hff);
        if rb.negedge {
            if let Some(c) = d.cells.last_mut() {
                c.attrs.set("CLK_INV", "1");
            }
        }
        d.connect(&dnet, &cell, "D");
        d.connect(&q, &cell, "Q");
        d.connect(&cnet, &cell, "CLK");
    }
    // Output pads: one per bit, in bit order.
    let mut n_iob = 0usize;
    for &(pi, bit, l) in &out_bits {
        let p = &flat.ports[pi];
        depth = depth.max(map.depth[(l / 2) as usize]);
        let src = emit(l, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
        let cell = if n_iob == 0 { "u_iob".to_string() } else { format!("u_iob{n_iob}") };
        n_iob += 1;
        d.add_cell(&cell, CellKind::IobOut);
        if let Some(c) = d.cells.last_mut() {
            c.attrs.set("PORT", &p.name);
            c.attrs.set("BIT", &bit.to_string());
        }
        d.connect(&src, &cell, "I");
        d.connect(&p.name, &cell, "PAD");
    }
    let stats = Stats { luts: n_lut, ffs: reg_bits.len(), depth, ands: aig.nodes.iter().filter(|n| n.0 != u32::MAX).count() };
    Ok((d, stats, undriven))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_lut(init: u64, ins: &[bool]) -> bool {
        let mut a = 0u64;
        for (i, &b) in ins.iter().enumerate() {
            a |= (b as u64) << i;
        }
        (init >> a) & 1 == 1
    }

    #[test]
    fn expand_keeps_the_function() {
        // f(a, b) = a & !b over leaves [3, 7], rewritten over [1, 3, 7].
        let tt = VAR[0] & !VAR[1];
        let e = expand(tt, &[3, 7], &[1, 3, 7]);
        for m in 0..8u32 {
            let (x1, x3, x7) = (m & 1 == 1, m & 2 == 2, m & 4 == 4);
            assert_eq!(eval_lut(e, &[x1, x3, x7]), x3 && !x7);
        }
    }

    #[test]
    fn adder_maps_and_matches() {
        let mut dag = Dag::default();
        let a = dag.read(0, 3);
        let b = dag.read(1, 3);
        let s = dag.add(a, b);
        let flat = Flat {
            top: "t".into(),
            dag,
            sigs: vec![
                SigInfo { name: "a".into(), w: 3, drv: vec![Drv::Input; 3], init: vec![false; 3] },
                SigInfo { name: "b".into(), w: 3, drv: vec![Drv::Input; 3], init: vec![false; 3] },
            ],
            ports: vec![
                Port { name: "a".into(), dir: PortDir::In, w: 3, node: None, sig: Some(0) },
                Port { name: "b".into(), dir: PortDir::In, w: 3, node: None, sig: Some(1) },
                Port { name: "s".into(), dir: PortDir::Out, w: 3, node: Some(s), sig: None },
            ],
            regs: vec![],
        };
        let (d, st, _) = lower(&flat).unwrap();
        assert!(st.luts <= 3, "3-bit add is 3 LUTs of at most 6 inputs, got {}", st.luts);
        let pins = d.pin_index();
        for x in 0..8u32 {
            for y in 0..8u32 {
                let mut val: HashMap<String, bool> = HashMap::new();
                for i in 0..3 {
                    val.insert(format!("a_{i}"), (x >> i) & 1 == 1);
                    val.insert(format!("b_{i}"), (y >> i) & 1 == 1);
                }
                for _ in 0..4 {
                    for c in &d.cells {
                        if let CellKind::Lut6 { init } = c.kind {
                            let name = helion_ir::resolve(c.name);
                            let ins: Vec<bool> = (0..6)
                                .map(|i| pins.net_on(name, &format!("I{i}")).map(|n| val.get(helion_ir::resolve(n)).copied().unwrap_or(false)).unwrap_or(false))
                                .collect();
                            let o = helion_ir::resolve(pins.net_on(name, "O").unwrap()).to_string();
                            val.insert(o, eval_lut(init, &ins));
                        }
                    }
                }
                for i in 0..3 {
                    assert_eq!(val[&format!("s_{i}")], ((x + y) >> i) & 1 == 1, "{x}+{y} bit {i}");
                }
            }
        }
    }
}
