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
    /// Two-level AND rules (the Xilinx flow's deeper optimization).
    two_level: bool,
}

impl Aig {
    pub fn new() -> Self {
        Aig { nodes: vec![(u32::MAX, 0)], strash: HashMap::new(), two_level: super::dag::carry_arith() }
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
        // Two-level rules: an operand that is itself an AND of the other.
        for (x, y) in if self.two_level { [(a, b), (b, a)] } else { [(0, 0); 2] } {
            if y > TRUE && self.is_and(y / 2) {
                let (p, q) = self.nodes[(y / 2) as usize];
                if y & 1 == 0 {
                    if p == x ^ 1 || q == x ^ 1 {
                        return FALSE; // x & (!x & q)
                    }
                    if p == x || q == x {
                        return y; // x & (x & q)
                    }
                } else {
                    if p == x ^ 1 || q == x ^ 1 {
                        return x; // x & !(!x & q)
                    }
                    if p == x {
                        return self.and(x, q ^ 1); // x & !(x & q)
                    }
                    if q == x {
                        return self.and(x, p ^ 1);
                    }
                }
            }
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
    /// The logic of `roots` rebuilt with every AND tree balanced by arrival
    /// level (case and if chains come out of elaboration as linear AND/OR
    /// chains, which a cut cover cannot shorten). Returns the new graph and
    /// old node -> new literal for every node built. An input listed in
    /// `before` (a carry chain sum) is built after the nodes it is computed
    /// from and arrives one level after them.
    fn balance(&self, roots: &[Lit], before: &HashMap<u32, Vec<u32>>) -> (Aig, HashMap<u32, Lit>) {
        let mut refs = vec![0u32; self.nodes.len()];
        for &(a, b) in &self.nodes {
            if a != u32::MAX {
                refs[(a / 2) as usize] += 1;
                refs[(b / 2) as usize] += 1;
            }
        }
        for &r in roots {
            refs[(r / 2) as usize] += 1;
        }
        let mut st = Bal { new: Aig::new(), lvl: vec![0], memo: HashMap::new() };
        st.memo.insert(0, FALSE);
        for &r in roots {
            self.bal(r / 2, &refs, before, &mut st);
        }
        (st.new, st.memo)
    }
    fn bal(&self, node: u32, refs: &[u32], before: &HashMap<u32, Vec<u32>>, st: &mut Bal) -> Lit {
        if let Some(&l) = st.memo.get(&node) {
            return l;
        }
        let l = if !self.is_and(node) {
            let mut lv = 0;
            for &k in before.get(&node).map(|v| v.as_slice()).unwrap_or(&[]) {
                let s = self.bal(k, refs, before, st);
                lv = lv.max(st.lvl[(s / 2) as usize] + 1);
            }
            let l = st.new.input();
            st.lvl.push(lv);
            l
        } else {
            // The supergate: AND inputs through uninverted single-fanout ANDs.
            let (a, b) = self.nodes[node as usize];
            let (mut leaves, mut stack) = (Vec::new(), vec![a, b]);
            while let Some(f) = stack.pop() {
                let m = (f / 2) as usize;
                if f & 1 == 0 && self.is_and(m as u32) && refs[m] == 1 {
                    stack.extend([self.nodes[m].0, self.nodes[m].1]);
                } else {
                    leaves.push(f);
                }
            }
            let mut items: Vec<Lit> = leaves.iter().map(|&f| self.bal(f / 2, refs, before, st) ^ (f & 1)).collect();
            // Pair the two earliest arrivals until one is left.
            while items.len() > 1 {
                items.sort_by_key(|&x| std::cmp::Reverse(st.lvl[(x / 2) as usize]));
                let (x, y) = (items.pop().unwrap(), items.pop().unwrap());
                let z = st.new.and(x, y);
                if (z / 2) as usize >= st.lvl.len() {
                    let lv = 1 + st.lvl[(x / 2) as usize].max(st.lvl[(y / 2) as usize]);
                    st.lvl.push(lv);
                }
                items.push(z);
            }
            items[0]
        };
        st.memo.insert(node, l);
        l
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

struct Bal {
    new: Aig,
    /// Level of each node of `new`.
    lvl: Vec<u32>,
    memo: HashMap<u32, Lit>,
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
    /// Output of latch `.0`.
    Latch(u32),
    /// An output pin of hard cell `.0` (block RAM or primitive instance).
    Hard(u32),
}

/// A cell built as is: an inferred block RAM or an instance of a device
/// primitive. Its outputs drive signal bits marked [`Drv::Hard`].
pub struct Hard {
    pub name: String,
    /// `RAMB18E1`, `RAMB36E1`, `DSP48E1`, ...
    pub module: String,
    /// Parameters, written the way the back end reads them.
    pub attrs: Vec<(String, String)>,
    /// Input pin bits: (pin, node, bit of node).
    pub ins: Vec<(String, N, u32)>,
    /// Output pin bits: (pin, signal, bit of signal).
    pub outs: Vec<(String, u32, u32)>,
}

/// A level-sensitive latch: `sig` follows `d` while `gate` is 1. Only a
/// latch used as a clock gate is implemented (see [`clock_shape`]).
pub struct Latch {
    pub sig: u32,
    pub d: N,
    pub gate: N,
}

pub struct SigInfo {
    pub name: String,
    /// Dotted hierarchical name in the source.
    pub hier: String,
    /// Source index of each bit when the signal is a plain vector.
    pub index: Option<Vec<i64>>,
    pub w: u32,
    pub drv: Vec<Drv>,
    /// Power-up value of register bits.
    pub init: Vec<bool>,
    /// Keep a net of this signal's name (`mark_debug`).
    pub debug: bool,
}

/// The flops of one clocked process for one signal: bit `i` is a flop when
/// `mask[i]`, with data input bit `i` of `d`.
/// The part of a concatenation (MSB first) holding bit `i`, and the bit in it.
/// Bit `i` of `n` when it is a constant.
fn const_bit(dag: &Dag, n: N, i: u32) -> Result<Option<bool>, String> {
    Ok(match dag.op(n) {
        Op::Const(b) => Some(b[i as usize]),
        Op::Slice(a, lo) => const_bit(dag, *a, lo + i)?,
        Op::Concat(parts) => {
            let (p, k) = concat_part(dag, parts, i)?;
            const_bit(dag, p, k)?
        }
        _ => None,
    })
}

fn concat_part(dag: &Dag, parts: &[N], i: u32) -> Result<(N, u32), String> {
    let mut off = 0;
    for p in parts.iter().rev() {
        let pw = dag.w(*p);
        if i < off + pw {
            return Ok((*p, i - off));
        }
        off += pw;
    }
    Err("internal: concat bit out of range".into())
}

pub struct Reg {
    pub sig: u32,
    pub d: N,
    pub clk: N,
    pub negedge: bool,
    pub mask: Vec<bool>,
    /// Asynchronous control: (one-bit node, active level). While it is at
    /// that level every flop with `arst_mask` holds its power-up value.
    pub arst: Option<(N, bool)>,
    pub arst_mask: Vec<bool>,
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
    pub latches: Vec<Latch>,
    pub instances: Vec<(String, String, Vec<(String, String)>)>,
    pub hard: Vec<Hard>,
}

pub fn bit_name(sig: &str, w: u32, bit: u32) -> String {
    if w == 1 {
        sig.to_string()
    } else {
        format!("{sig}_{bit}")
    }
}

/// Follow wires: slices, concatenations and combinationally driven signal
/// bits, down to the node that computes or stores the bit.
pub(super) fn wire(flat: &Flat, mut n: N, mut bit: u32) -> (N, u32) {
    loop {
        match flat.dag.op(n) {
            Op::Slice(a, lo) => {
                bit += lo;
                n = *a;
            }
            Op::Concat(parts) => {
                let mut off = 0;
                let mut next = None;
                for p in parts.iter().rev() {
                    let pw = flat.dag.w(*p);
                    if bit < off + pw {
                        next = Some((*p, bit - off));
                        break;
                    }
                    off += pw;
                }
                match next {
                    Some((p, b)) => {
                        n = p;
                        bit = b;
                    }
                    None => return (n, bit),
                }
            }
            Op::Read(sig) => match flat.sigs[*sig as usize].drv[bit as usize] {
                Drv::Comb(m, b) => {
                    n = m;
                    bit = b;
                }
                _ => return (n, bit),
            },
            _ => return (n, bit),
        }
    }
}

/// The clock of a flop as (root clock bit, enables). A clock of the form
/// `latch & clk`, where the latch is transparent while `clk` is low, is a
/// gated clock: the flop is clocked by `clk` and loads only when the
/// latch's data input is 1, which is what the gate does.
pub(super) fn clock_shape(flat: &Flat, clk: N) -> ((N, u32), Vec<(N, u32)>) {
    let (n, b) = wire(flat, clk, 0);
    if let Op::And(x, y) = flat.dag.op(n) {
        if flat.dag.w(n) == 1 {
            for (l, c) in [(*x, *y), (*y, *x)] {
                let (ln, lb) = wire(flat, l, 0);
                let Op::Read(sig) = flat.dag.op(ln) else { continue };
                let Drv::Latch(k) = flat.sigs[*sig as usize].drv[lb as usize] else { continue };
                let latch = &flat.latches[k as usize];
                let (gn, _) = wire(flat, latch.gate, 0);
                let Op::Not(g) = flat.dag.op(gn) else { continue };
                if wire(flat, *g, 0) != wire(flat, c, 0) {
                    continue;
                }
                let (root, mut ens) = clock_shape(flat, c);
                ens.push((latch.d, lb));
                return (root, ens);
            }
        }
    }
    ((n, b), Vec::new())
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
    /// Adders kept for a dedicated carry chain (see `dag::carry_arith`).
    chains: Vec<Chain>,
}

/// An adder on a CARRY4 chain: bits `lo..` of `x + y + cin`, each sum bit
/// an AIG input (the chain's O pin) so the cover treats it as a leaf.
struct Chain {
    lo: usize,
    xs: Vec<Lit>,
    ys: Vec<Lit>,
    cin: bool,
    sums: Vec<Lit>,
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
            Drv::Hard(_) => {
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
            Drv::Latch(_) => {
                return Err(format!(
                    "'{}' is a latch; a latch is only supported as a clock gate (enabled while the clock is low and ANDed with that clock)",
                    bit_name(&s.name, s.w, bit)
                ))
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

    /// `xs + ys + cin` on a carry chain. Constant low bits (the `1 + 1` of
    /// `a + b + 1`) fold into the carry in; constant high bits (operands
    /// widened to 32 bits) take only the chain's carry out. A short chain,
    /// or one with a constant side, is plain logic the AIG simplifies (the
    /// `31 - i` of an array index is `!i`).
    fn chain(&mut self, xs: Vec<Lit>, ys: Vec<Lit>, cin: bool) -> Vec<Lit> {
        let w = xs.len();
        let (mut c, mut sums) = (cin, Vec::with_capacity(w));
        while sums.len() < w && xs[sums.len()] <= TRUE && ys[sums.len()] <= TRUE {
            let (x, y) = (xs[sums.len()] == TRUE, ys[sums.len()] == TRUE);
            sums.push(if x ^ y ^ c { TRUE } else { FALSE });
            c = (x && y) || (c && (x ^ y));
        }
        let lo = sums.len();
        let konst = |l: Lit| l <= TRUE;
        let hi = (lo..w).rev().find(|&k| !konst(xs[k]) || !konst(ys[k])).map_or(lo, |k| k + 1);
        let one_side = (lo..hi).all(|k| konst(xs[k])) || (lo..hi).all(|k| konst(ys[k]));
        let mut cl = if c { TRUE } else { FALSE };
        let ripple = |bl: &mut Self, sums: &mut Vec<Lit>, cl: &mut Lit, k: usize| {
            let p = bl.aig.xor(xs[k], ys[k]);
            sums.push(bl.aig.xor(p, *cl));
            let (g, t) = (bl.aig.and(xs[k], ys[k]), bl.aig.and(p, *cl));
            *cl = bl.aig.or(g, t);
        };
        if hi - lo <= super::dag::PREFIX_MIN_W as usize || (one_side && hi - lo <= 8) {
            for k in lo..w {
                ripple(self, &mut sums, &mut cl, k);
            }
            return sums;
        }
        // The chain runs one bit past the varying part, so that bit's sum
        // gives the carry out (both its operands are constants).
        let top = (hi + 1).min(w);
        while sums.len() < top {
            let l = self.aig.input();
            sums.push(l);
        }
        self.chains.push(Chain { lo, xs: xs[..top].to_vec(), ys: ys[..top].to_vec(), cin: c, sums: sums.clone() });
        if top < w {
            cl = sums[hi] ^ ((xs[hi] ^ ys[hi]) & 1);
            let (g, t) = (self.aig.and(xs[hi], ys[hi]), self.aig.and(xs[hi] ^ ys[hi], cl));
            cl = self.aig.or(g, t);
            for k in top..w {
                ripple(self, &mut sums, &mut cl, k);
            }
        }
        sums
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

    /// Bit `i` of `n` as the next value of bit `rbit` of register `sig`:
    /// `(enable, data)`, the flop loading `data` when `enable` is 1 and
    /// keeping its value otherwise (`data` is `None` where it always keeps
    /// it). Holds are found in the select tree only.
    fn enable(&mut self, n: N, i: u32, sig: u32, rbit: u32, memo: &mut HashMap<(N, u32), (Lit, Option<Lit>)>) -> Result<(Lit, Option<Lit>), String> {
        if let Some(&r) = memo.get(&(n, i)) {
            return Ok(r);
        }
        let dag = &self.flat.dag;
        let r = match dag.op(n).clone() {
            Op::Read(s) if s == sig && i == rbit => (FALSE, None),
            Op::Slice(a, lo) => self.enable(a, lo + i, sig, rbit, memo)?,
            Op::Concat(parts) => {
                let (p, k) = concat_part(dag, &parts, i)?;
                self.enable(p, k, sig, rbit, memo)?
            }
            Op::Mux(s, a, b) => {
                let (ea, da) = self.enable(a, i, sig, rbit, memo)?;
                let (eb, db) = self.enable(b, i, sig, rbit, memo)?;
                if ea == TRUE && eb == TRUE {
                    (TRUE, Some(self.bit(n, i)?))
                } else {
                    let sel = self.bit(s, 0)?;
                    let en = self.aig.mux(sel, ea, eb);
                    // Where one side keeps the value, its data is never loaded.
                    let data = match (da, db) {
                        (Some(x), Some(y)) => Some(self.aig.mux(sel, x, y)),
                        (x, None) | (None, x) => x,
                    };
                    (en, data)
                }
            }
            _ => (TRUE, Some(self.bit(n, i)?)),
        };
        memo.insert((n, i), r);
        Ok(r)
    }

    /// Bit `i` of `n` as a flop's next value with a synchronous reset on
    /// top: `(select literal, value, rest)` when the outermost select is a
    /// mux with a constant arm (`if (rst) q <= K; else ...`), the flop
    /// loading `value` while the select is 1 and `rest` (a node and bit)
    /// otherwise. Only the outermost mux counts: a reset under an enable
    /// must not override it, and the SR pin does.
    fn sync_reset(&mut self, n: N, i: u32) -> Result<Option<(Lit, bool, N, u32)>, String> {
        let dag = &self.flat.dag;
        match dag.op(n).clone() {
            Op::Slice(a, lo) => self.sync_reset(a, lo + i),
            Op::Concat(parts) => {
                let (p, k) = concat_part(dag, &parts, i)?;
                self.sync_reset(p, k)
            }
            Op::Mux(s, a, b) => {
                let ca = const_bit(dag, a, i)?;
                let cb = const_bit(dag, b, i)?;
                let (inv, v, rest) = match (ca, cb) {
                    (Some(_), Some(_)) => return Ok(None),
                    (Some(v), None) => (0, v, b),
                    (None, Some(v)) => (1, v, a),
                    (None, None) => return Ok(None),
                };
                let sel = self.bit(s, 0)?;
                if sel == TRUE || sel == FALSE {
                    return Ok(None);
                }
                Ok(Some((sel ^ inv, v, rest, i)))
            }
            _ => Ok(None),
        }
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
                let (p, k) = concat_part(dag, &parts, i)?;
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
            Op::Add(a, b) | Op::Sub(a, b) if w > super::dag::PREFIX_MIN_W && super::dag::carry_arith() => {
                if !self.whole.contains_key(&n) {
                    let inv = matches!(dag.op(n), Op::Sub(..));
                    let xs = self.all(a)?;
                    let ys: Vec<Lit> = self.all(b)?.into_iter().map(|y| y ^ inv as u32).collect();
                    let sums = self.chain(xs, ys, inv);
                    self.whole.insert(n, sums);
                }
                self.whole[&n][i as usize]
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
                if !self.whole.contains_key(&n) && w > super::dag::PREFIX_MIN_W && super::dag::carry_arith() {
                    // Wallace tree: full adders cut every column to two rows
                    // in log depth, then one carry chain adds the rows.
                    let (mut x, mut y) = (self.all(a)?, self.all(b)?);
                    let w = w as usize;
                    // Top bits of y that repeat one literal (a sign extension) from k0 up.
                    let run = |v: &[Lit]| w - 1 - v.iter().rev().skip(1).take_while(|&&l| l == v[w - 1]).count();
                    if run(&x) < run(&y) {
                        std::mem::swap(&mut x, &mut y);
                    }
                    let k0 = run(&y);
                    let mut cols: Vec<Vec<Lit>> = vec![Vec::new(); w];
                    for (k, &yk) in y.iter().enumerate().take(k0) {
                        for j in k..w {
                            let t = self.aig.and(x[j - k], yk);
                            if t != FALSE {
                                cols[j].push(t);
                            }
                        }
                    }
                    // Rows k0.. are ys & x << k, which sum to -(ys & x) << k0
                    // modulo 2^w: ys & !x << k0, plus ys at bit k0.
                    let ys = y[w - 1];
                    if k0 < w && ys != FALSE {
                        for j in k0..w {
                            let t = self.aig.and(x[j - k0] ^ 1, ys);
                            if t != FALSE {
                                cols[j].push(t);
                            }
                        }
                        cols[k0].push(ys);
                    }
                    while cols.iter().any(|c| c.len() > 2) {
                        let mut next: Vec<Vec<Lit>> = vec![Vec::new(); w];
                        for j in 0..w {
                            let mut it = cols[j].chunks_exact(3);
                            for t in &mut it {
                                let p = self.aig.xor(t[0], t[1]);
                                next[j].push(self.aig.xor(p, t[2]));
                                if j + 1 < w {
                                    let g = self.aig.and(t[0], t[1]);
                                    let h = self.aig.and(p, t[2]);
                                    let co = self.aig.or(g, h);
                                    next[j + 1].push(co);
                                }
                            }
                            next[j].extend_from_slice(it.remainder());
                        }
                        cols = next;
                    }
                    let row = |r: usize| cols.iter().map(|c| c.get(r).copied().unwrap_or(FALSE)).collect::<Vec<Lit>>();
                    let (r0, r1) = (row(0), row(1));
                    let sums = self.chain(r0, r1, false);
                    self.whole.insert(n, sums);
                }
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
/// Priority cuts kept per node: more for the Xilinx flow (fewer levels).
fn cuts() -> usize {
    if super::dag::carry_arith() { 24 } else { 8 }
}
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
    /// `arrive` gives an input node the nodes it is computed from (a carry
    /// chain's sum bit); its depth is one more than theirs.
    fn run(aig: &Aig, arrive: &HashMap<u32, Vec<u32>>) -> Self {
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
                if let Some(src) = arrive.get(&(i as u32)) {
                    m.depth[i] = 1 + src.iter().map(|&k| m.depth[k as usize]).max().unwrap_or(0);
                }
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
            cand.truncate(cuts());
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

/// One flop of the netlist and the source register bit it holds.
pub struct RegMap {
    /// Net on the flop's Q pin.
    pub net: String,
    pub hier: String,
    /// Bit index in the source; `None` for a scalar or a bit of a struct or array.
    pub index: Option<i64>,
    pub scalar: bool,
    /// The flop stores the complement (the register powers up at 1).
    pub inverted: bool,
}

pub struct Stats {
    pub regmap: Vec<RegMap>,
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
        chains: Vec::new(),
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
    // Signals marked for debugging are roots too: each bit keeps a net.
    let mut debug_bits: Vec<(u32, u32, Lit)> = Vec::new();
    for (si, sg) in flat.sigs.iter().enumerate() {
        if sg.debug {
            for b in 0..sg.w {
                if !matches!(sg.drv[b as usize], Drv::None | Drv::Latch(_)) {
                    let l = bl.read_bit(si as u32, b)?;
                    debug_bits.push((si as u32, b, l));
                }
            }
        }
    }
    struct RegBit {
        sig: u32,
        bit: u32,
        d: Lit,
        clk: Lit,
        negedge: bool,
        arst: Option<(Lit, bool)>,
        /// Clock enable (Xilinx flow), else the hold is a mux in front of D.
        ce: Option<Lit>,
        /// Synchronous reset (Xilinx flow): the SR pin, which wins over CE,
        /// and the stored value it loads (SRVAL).
        sr: Option<(Lit, bool)>,
    }
    let mut reg_bits: Vec<RegBit> = Vec::new();
    // Every hard cell is built; its inputs are roots.
    let mut hard_ins: Vec<Vec<Lit>> = Vec::with_capacity(flat.hard.len());
    for h in &flat.hard {
        hard_ins.push(h.ins.iter().map(|(_, n, b)| bl.bit(*n, *b)).collect::<Result<_, _>>()?);
    }
    let mut done = 0usize;
    while done < bl.live_regs.len() {
        let (sig, bit) = bl.live_regs[done];
        done += 1;
        let Drv::Reg(ri) = flat.sigs[sig as usize].drv[bit as usize] else { unreachable!() };
        let r = &flat.regs[ri as usize];
        let ((cn, cb), enables) = clock_shape(flat, r.clk);
        // Xilinx flow: holds in the next value's select tree and clock
        // gates drive the flop's CE pin instead of a mux in front of D.
        let xil = super::dag::carry_arith();
        let init = flat.sigs[sig as usize].init[bit as usize];
        // A synchronous reset goes on the SR pin (G59): not with an
        // asynchronous one (one SR pin) or a gated clock (the reset would
        // act while the gate is shut).
        let has_arst = r.arst.is_some() && r.arst_mask[bit as usize];
        let srst = match xil && !has_arst && enables.is_empty() {
            true => bl.sync_reset(r.d, bit)?,
            false => None,
        };
        let (dn, di) = srst.map_or((r.d, bit), |(_, _, n, i)| (n, i));
        let sr = srst.map(|(l, v, _, _)| (l, v ^ init));
        let (mut d, mut ce) = match xil {
            true => match bl.enable(dn, di, sig, bit, &mut HashMap::new())? {
                (en, Some(data)) if en != FALSE => (data, en),
                (_, None) if sr.is_some() => (bl.sig_bit[&(sig, bit)], TRUE),
                _ => (bl.bit(dn, di)?, TRUE),
            },
            false => (bl.bit(dn, di)?, TRUE),
        };
        if init {
            d ^= 1;
        }
        if !enables.is_empty() {
            // Gated clock: keep the stored value unless every gate is open.
            let q = bl.sig_bit[&(sig, bit)] ^ (flat.sigs[sig as usize].init[bit as usize] as u32);
            let mut en = TRUE;
            for (en_n, en_b) in enables {
                let e = bl.bit(en_n, en_b)?;
                en = bl.aig.and(en, e);
            }
            if xil {
                ce = bl.aig.and(ce, en);
            } else {
                d = bl.aig.mux(en, d, q);
            }
        }
        let ce = (ce != TRUE).then_some(ce);
        let clk = bl.bit(cn, cb)?;
        let arst = match r.arst {
            Some((n, level)) if r.arst_mask[bit as usize] => {
                let l = bl.bit(n, 0)?;
                // An inverted literal is the same signal with the other level.
                Some((l & !1, level ^ (l & 1 == 1)))
            }
            _ => None,
        };
        reg_bits.push(RegBit { sig, bit, d, clk, negedge: r.negedge, arst, ce, sr });
    }
    // Each enable or reset is a slice control set: one shared by only a few
    // flops costs slice space for nothing, so those keep the mux in front
    // of D. A reset folded back also opens the enable (it wins over CE).
    const CE_MIN_FLOPS: usize = 4;
    let mut sr_uses: HashMap<Lit, usize> = HashMap::new();
    for (l, _) in reg_bits.iter().filter_map(|rb| rb.sr) {
        *sr_uses.entry(l).or_default() += 1;
    }
    for rb in &mut reg_bits {
        if let Some((l, v)) = rb.sr.filter(|(l, _)| sr_uses[l] < CE_MIN_FLOPS) {
            rb.d = bl.aig.mux(l, if v { TRUE } else { FALSE }, rb.d);
            rb.ce = rb.ce.map(|c| bl.aig.or(c, l));
            rb.sr = None;
        }
    }
    let mut ce_uses: HashMap<Lit, usize> = HashMap::new();
    for c in reg_bits.iter().filter_map(|rb| rb.ce) {
        *ce_uses.entry(c).or_default() += 1;
    }
    for rb in &mut reg_bits {
        if let Some(c) = rb.ce.filter(|c| *c == FALSE || ce_uses[c] < CE_MIN_FLOPS) {
            let q = bl.sig_bit[&(rb.sig, rb.bit)] ^ (flat.sigs[rb.sig as usize].init[rb.bit as usize] as u32);
            rb.d = bl.aig.mux(c, rb.d, q);
            rb.ce = None;
        }
    }
    let Blaster { aig, pi_name, hints, undriven, mut chains, mut sig_bit, .. } = bl;
    let (aig, pi_name, hints) = if !super::dag::carry_arith() {
        (aig, pi_name, hints)
    } else {
        let mut roots: Vec<Lit> = out_bits.iter().map(|o| o.2).chain(debug_bits.iter().map(|o| o.2)).collect();
        for rb in &reg_bits {
            roots.extend([rb.d, rb.clk]);
            roots.extend(rb.arst.map(|a| a.0));
            roots.extend(rb.ce);
            roots.extend(rb.sr.map(|s| s.0));
        }
        // Block RAM / DSP inputs are roots too (their outputs are inputs).
        roots.extend(hard_ins.iter().flatten());
        let mut before: HashMap<u32, Vec<u32>> = HashMap::new();
        for ch in &chains {
            roots.extend(ch.xs.iter().chain(&ch.ys).chain(&ch.sums[ch.lo..]));
            for k in ch.lo..ch.sums.len() {
                before.insert(ch.sums[k] / 2, ch.xs[ch.lo..=k].iter().chain(&ch.ys[ch.lo..=k]).map(|l| l / 2).collect());
            }
        }
        let (new, memo) = aig.balance(&roots, &before);
        let m = |l: Lit| memo.get(&(l / 2)).map(|&n| n ^ (l & 1));
        for o in &mut out_bits {
            o.2 = m(o.2).unwrap();
        }
        for o in &mut debug_bits {
            o.2 = m(o.2).unwrap();
        }
        for rb in &mut reg_bits {
            rb.d = m(rb.d).unwrap();
            rb.clk = m(rb.clk).unwrap();
            if let Some(a) = &mut rb.arst {
                a.0 = m(a.0).unwrap();
            }
            rb.ce = rb.ce.map(|c| m(c).unwrap());
            rb.sr = rb.sr.map(|(l, v)| (m(l).unwrap(), v));
        }
        for ch in &mut chains {
            for l in ch.xs.iter_mut().chain(ch.ys.iter_mut()).chain(ch.sums.iter_mut()) {
                *l = m(*l).unwrap_or(*l);
            }
        }
        for ins in &mut hard_ins {
            for l in ins.iter_mut() {
                *l = m(*l).unwrap();
            }
        }
        // Hard-cell outputs are found by signal bit; an unread one is dropped
        // here and stays open below, as before.
        sig_bit = sig_bit.into_iter().filter_map(|(k, l)| m(l).map(|n| (k, n))).collect();
        let pi: Vec<(Lit, u32, u32)> = pi_name.into_iter().filter_map(|(l, s, b)| m(l).map(|n| (n, s, b))).collect();
        let hints: Vec<(Lit, u32, u32)> = hints.into_iter().filter_map(|(l, s, b)| m(l).map(|n| (n, s, b))).collect();
        (new, pi, hints)
    };
    let mut arrive: HashMap<u32, Vec<u32>> = HashMap::new();
    for ch in &chains {
        let mut src = Vec::new();
        for k in ch.lo..ch.sums.len() {
            src.extend([ch.xs[k] / 2, ch.ys[k] / 2]);
            arrive.insert(ch.sums[k] / 2, src.clone());
        }
    }
    let map = Mapper::run(&aig, &arrive);

    let mut d = Design::new(&flat.top);
    for p in &flat.ports {
        d.add_port(&p.name, p.dir);
    }
    // Net name of every literal that exists as a net.
    let mut net_of: HashMap<Lit, String> = HashMap::new();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    for &(l, sig, bit) in &pi_name {
        let s = &flat.sigs[sig as usize];
        let mut n = bit_name(&s.name, s.w, bit);
        if matches!(s.drv[bit as usize], Drv::Reg(_)) && s.init[bit as usize] {
            // This flop stores the complement; do not call that net by the signal's name.
            n.push_str("_n");
        }
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
    // A debug signal names its net before anything else may.
    for &(sig, bit, l) in &debug_bits {
        let sg = &flat.sigs[sig as usize];
        if l > TRUE && !net_of.contains_key(&l) {
            hint.entry(l).or_insert(bit_name(&sg.name, sg.w, bit));
        }
    }
    // Output port bits name the net of the literal they show, when free;
    // other signal bits name theirs after that.
    for &(pi, bit, l) in &out_bits {
        let p = &flat.ports[pi];
        if l > TRUE && !net_of.contains_key(&l) {
            hint.entry(l).or_insert(bit_name(&p.name, p.w, bit));
        }
    }
    // The logic in front of a flop is named for the register it feeds.
    for rb in &reg_bits {
        let s = &flat.sigs[rb.sig as usize];
        if rb.d > TRUE && !net_of.contains_key(&rb.d) {
            hint.entry(rb.d).or_insert(format!("{}_d", bit_name(&s.name, s.w, rb.bit)));
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

    // Carry chains: name every sum bit first (chains and logic may feed
    // each other), then write the CARRY4 cells in the IR convention pack
    // reads: S from a LUT of its own, DI the generate, O the sum.
    for ch in &chains {
        for &l in &ch.sums[ch.lo..] {
            let name = fresh(hint.get(&l), &mut taken, &n_lut);
            net_of.insert(l, name);
        }
    }
    for (ci, ch) in chains.iter().enumerate() {
        let bits = ch.sums.len() - ch.lo;
        for g in 0..bits.div_ceil(4) {
            let cell = format!("u_cc{ci}_c{g}");
            d.add_cell(&cell, CellKind::Carry4);
            let _ = d.set_cell_attr(&cell, "ADDER", "1");
            if g == 0 {
                let _ = d.set_cell_attr(&cell, "CYINIT", if ch.cin { "1" } else { "0" });
            } else {
                d.connect(&format!("cc{ci}_co{}", g - 1), &cell, "CI");
            }
            if (g + 1) * 4 < bits {
                taken.insert(format!("cc{ci}_co{g}"));
                d.connect(&format!("cc{ci}_co{g}"), &cell, "CO");
            }
            for j in 0..4.min(bits - g * 4) {
                let k = ch.lo + g * 4 + j;
                let (x, y) = (ch.xs[k], ch.ys[k]);
                // Propagate x ^ y over the true nets, the polarity in the table.
                let mut ins: Vec<Lit> = Vec::new();
                for v in [x & !1, y & !1] {
                    if v > TRUE && !ins.contains(&v) {
                        ins.push(v);
                    }
                }
                let mut tt = 0u64;
                for a in 0..64u32 {
                    let val = |l: Lit| -> bool {
                        let base = match ins.iter().position(|&v| v == l & !1) {
                            Some(p) => (a >> p) & 1 == 1,
                            None => false,
                        };
                        base ^ (l & 1 == 1)
                    };
                    if val(x) ^ val(y) {
                        tt |= 1 << a;
                    }
                }
                let s_net = fresh(Some(&format!("cc{ci}_s{k}")), &mut taken, &n_lut);
                let s_cell = format!("{s_net}_lut");
                let in_nets: Vec<String> = ins.iter().map(|&v| emit(v, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut)).collect();
                d.add_cell(&s_cell, CellKind::Lut6 { init: tt });
                for (p, net) in in_nets.iter().enumerate() {
                    d.connect(net, &s_cell, &format!("I{p}"));
                }
                d.connect(&s_net, &s_cell, "O");
                n_lut += 1;
                d.connect(&s_net, &cell, &format!("S{j}"));
                // With the propagate 0 the operands are equal: either is the generate.
                let generate = if x & 1 == 0 || y & 1 == 1 { x } else { y };
                let di = emit(generate, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
                d.connect(&di, &cell, &format!("DI{j}"));
                d.connect(&net_of[&ch.sums[k]], &cell, &format!("O{j}"));
            }
        }
    }

    // Registers.
    let mut regmap = Vec::with_capacity(reg_bits.len());
    for rb in &reg_bits {
        let s = &flat.sigs[rb.sig as usize];
        let inverted = s.init[rb.bit as usize];
        let q = format!("{}{}", bit_name(&s.name, s.w, rb.bit), if inverted { "_n" } else { "" });
        let cell = format!("{}_reg", bit_name(&s.name, s.w, rb.bit));
        regmap.push(RegMap {
            net: q.clone(),
            hier: s.hier.clone(),
            index: s.index.as_ref().map(|ix| ix[rb.bit as usize]),
            scalar: s.w == 1,
            inverted,
        });
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
        if let Some(c) = rb.ce {
            depth = depth.max(map.depth[(c / 2) as usize]);
            let enet = emit(c, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
            d.connect(&enet, &cell, "CE");
        }
        if let Some((l, v)) = rb.sr {
            depth = depth.max(map.depth[(l / 2) as usize]);
            let snet = emit(l, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
            d.connect(&snet, &cell, "SR");
            if let Some(c) = d.cells.iter_mut().rev().find(|c| helion_ir::resolve(c.name) == cell) {
                c.attrs.set("SRVAL", if v { "1" } else { "0" });
            }
        }
        if let Some((l, level)) = rb.arst {
            // The stored bit is the register XOR its power-up value, and
            // the reset value is that power-up value: the flop resets to 0.
            let rnet = emit(l, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
            d.connect(&rnet, &cell, "ARST");
            if let Some(c) = d.cells.iter_mut().rev().find(|c| helion_ir::resolve(c.name) == cell) {
                c.attrs.set("ARST_LEVEL", if level { "1" } else { "0" });
                c.attrs.set("ARST_VAL", "0");
            }
        }
    }
    // Hard cells: inputs from the logic, outputs onto the nets of the
    // signal bits they drive (an output nothing reads stays open).
    for (h, ins) in flat.hard.iter().zip(&hard_ins) {
        d.add_cell(&h.name, CellKind::BlackBox { module: helion_ir::intern(&h.module) });
        if let Some(c) = d.cells.last_mut() {
            for (k, v) in &h.attrs {
                c.attrs.set(k, v);
            }
        }
        for ((pin, _, _), &l) in h.ins.iter().zip(ins) {
            let net = emit(l, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
            d.connect(&net, &h.name, pin);
        }
        for (pin, sig, bit) in &h.outs {
            if let Some(l) = sig_bit.get(&(*sig, *bit)) {
                d.connect(&net_of[l], &h.name, pin);
            }
        }
    }
    // Debug signals: their nets exist and carry the mark.
    for &(_, _, l) in &debug_bits {
        let net = emit(l, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
        let _ = d.mark_debug(&net);
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
    let stats = Stats { regmap, luts: n_lut, ffs: reg_bits.len(), depth, ands: aig.nodes.iter().filter(|n| n.0 != u32::MAX).count() };
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
                SigInfo { name: "a".into(), hier: "a".into(), index: None, w: 3, drv: vec![Drv::Input; 3], init: vec![false; 3], debug: false },
                SigInfo { name: "b".into(), hier: "b".into(), index: None, w: 3, drv: vec![Drv::Input; 3], init: vec![false; 3], debug: false },
            ],
            ports: vec![
                Port { name: "a".into(), dir: PortDir::In, w: 3, node: None, sig: Some(0) },
                Port { name: "b".into(), dir: PortDir::In, w: 3, node: None, sig: Some(1) },
                Port { name: "s".into(), dir: PortDir::Out, w: 3, node: Some(s), sig: None },
            ],
            regs: vec![],
            latches: vec![],
            instances: vec![],
            hard: vec![],
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
