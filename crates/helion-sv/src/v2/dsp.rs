//! DSP48E1 inference for the Xilinx 7-series flow (with [`super::hard`]).
//!
//! A product that fits the slice's 25 x 18 signed multiplier becomes a
//! `DSP48E1` cell of the form an instantiated one takes (`mem.rs`:
//! parameters as attributes, every input pin driven), and registers
//! around it move into the slice when they behave exactly like the slice's
//! own (UG479). A wider product becomes several cells.
//!
//! `Mul` is modulo 2^W at its width W with operands the elaborator has
//! already extended, so only an operand's value modulo 2^W matters: it
//! goes on a port as its low `e` bits sign-extended, `e` the fewest bits
//! that sign-extend to it (a zero-extended `u`-bit operand takes `u + 1`).
//! A takes `e <= 25`, B `e <= 18`, either operand on either port. The
//! product of `e1`- and `e2`-bit signed values fits in `e1 + e2` bits, so
//! result bit `i` is `P[min(i, e1 + e2 - 1)]`, for any W.
//!
//! A wider product is split (see [`split`]): each operand becomes chunks of
//! 24 (A) or 17 (B) unsigned bits under a signed top chunk, every pair of
//! chunks below the result's width is one partial product, and the fabric
//! adds them on carry chains. Partial products take no slice registers.
//! (The PCOUT cascade would add them in the slices, but no design here
//! has run one through the back end and x7v yet.)
//!
//! The fabric keeps a product that is at most one adder: a 1-bit operand
//! (one partial-product row), or a non-negative constant with two nonzero
//! bits (3, 5, 6, 10, ...: a shift and an add, 19 LUTs for 16 bits). A
//! power of two never gets here (`Dag::mul` makes it a shift). Measured
//! fabric costs of the rest, 16 bits wide: x * 7 62 LUTs, x * -3 67,
//! 2 x 16 bits 51, 3 x 8 34, 8 x 8 115, 16 x 16 482; each is one DSP.
//!
//! Registers move in only on one rising clock with no gate, no
//! asynchronous reset and a power-up value of 0 (the slice's registers
//! start at 0). A hold in the next value becomes the clock enable and an
//! outermost synchronous reset to 0 the reset pin: in the RTL and in the
//! slice the reset wins over the enable. Shapes:
//! - P: `p <= a * b`, or `p <= p + a * b` (OPMODE Z = P) with `p` at most
//!   48 bits, the product used nowhere else;
//! - M: `m <= a * b` read only by `p <= m` or `p <= p + m` (then P too);
//! - A, B: an operand that is the output of a register nothing else reads,
//!   beside constant 0 bits only, unless that register is the P or M
//!   register of a product that gets its own DSP48E1 (`q <= p1 * c` after
//!   `p1 <= a * b` reads the first slice's P, in either source order).
//!
//! Products past [`DSP_SITES`] stay in the fabric.

use super::dag::{Dag, Op, N};
use super::lower::{clock_shape, wire, Drv, Flat, Hard, SigInfo};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

/// DSP48E1 sites on the part: 220 on the xc7z020 (ZedBoard). Products past
/// it, after the instantiated DSP48E1 cells, stay in the fabric.
// shortcut: one part's count, first come first served in build order (upstream first, then node order); set it per part when another one builds.
pub static DSP_SITES: AtomicUsize = AtomicUsize::new(220);

/// A register's next value as `rst ? 0 : en ? core : hold`.
struct Peel {
    core: N,
    en: N,
    rst: N,
    /// The select structure from the register's input down to `core`,
    /// with how often the structure itself refers to each node.
    uses: Vec<(N, u32)>,
    /// How often the structure refers to `hold`.
    holds: u32,
}

/// A register that can be a slice register: its next value and clock.
struct SliceReg {
    ri: u32,
    peel: Peel,
    clk: (N, u32),
    /// The register's own output (the hold).
    hold: N,
}

/// How often each node and each signal is read by the live design
/// (`lower` builds only what the outputs, hard cells and debug nets reach).
fn refs(flat: &Flat) -> (Vec<u32>, Vec<u32>) {
    let dag = &flat.dag;
    let mut refs = vec![0u32; dag.nodes.len()];
    let mut live = vec![false; dag.nodes.len()];
    let (mut sig_live, mut reg_live) = (vec![false; flat.sigs.len()], vec![false; flat.regs.len()]);
    let mut stack: Vec<N> = Vec::new();
    let mut sigs: Vec<u32> = (0..flat.sigs.len() as u32).filter(|&s| flat.sigs[s as usize].debug).collect();
    let root = |n: N, refs: &mut Vec<u32>, stack: &mut Vec<N>| {
        refs[n as usize] += 1;
        stack.push(n);
    };
    for n in flat.ports.iter().filter_map(|p| p.node) {
        root(n, &mut refs, &mut stack);
    }
    for h in &flat.hard {
        for &(_, n, _) in &h.ins {
            root(n, &mut refs, &mut stack);
        }
    }
    loop {
        if let Some(n) = stack.pop() {
            if std::mem::replace(&mut live[n as usize], true) {
                continue;
            }
            for k in dag.kids(n) {
                root(k, &mut refs, &mut stack);
            }
            if let Op::Read(s) = dag.op(n) {
                sigs.push(*s);
            }
        } else if let Some(s) = sigs.pop() {
            if std::mem::replace(&mut sig_live[s as usize], true) {
                continue;
            }
            let mut seen = HashSet::new();
            for d in &flat.sigs[s as usize].drv {
                match *d {
                    Drv::Comb(m, _) if seen.insert(m) => root(m, &mut refs, &mut stack),
                    Drv::Reg(r) if !std::mem::replace(&mut reg_live[r as usize], true) => {
                        let r = &flat.regs[r as usize];
                        for n in [r.d, r.clk].into_iter().chain(r.arst.map(|a| a.0)) {
                            root(n, &mut refs, &mut stack);
                        }
                    }
                    Drv::Latch(l) => {
                        let l = &flat.latches[l as usize];
                        root(l.d, &mut refs, &mut stack);
                        root(l.gate, &mut refs, &mut stack);
                    }
                    _ => {}
                }
            }
        } else {
            break;
        }
    }
    let mut reads: Vec<u32> = flat.sigs.iter().map(|s| s.debug as u32).collect();
    for (n, node) in dag.nodes.iter().enumerate() {
        if let Op::Read(s) = node.op {
            reads[s as usize] += refs[n];
        }
    }
    (refs, reads)
}

fn count(refs: &[u32], n: N) -> u32 {
    refs.get(n as usize).copied().unwrap_or(u32::MAX)
}

/// Every node of the structure is referred to only by the structure.
fn exclusive(refs: &[u32], uses: &[(N, u32)]) -> bool {
    let mut c: HashMap<N, u32> = HashMap::new();
    for &(n, k) in uses {
        *c.entry(n).or_default() += k;
    }
    c.iter().all(|(&n, &k)| count(refs, n) == k)
}

/// `n` read once, followed through whole-signal aliases (`assign s = x`)
/// read once, to the node that computes it.
fn sole(flat: &Flat, refs: &[u32], reads: &[u32], mut n: N) -> Option<N> {
    loop {
        if count(refs, n) != 1 {
            return None;
        }
        let Op::Read(s) = *flat.dag.op(n) else { return Some(n) };
        let sg = &flat.sigs[s as usize];
        let Some(&Drv::Comb(m, 0)) = sg.drv.first() else { return Some(n) };
        if reads[s as usize] != 1 || flat.dag.w(m) != sg.w || sg.drv.iter().enumerate().any(|(i, d)| *d != Drv::Comb(m, i as u32)) {
            return Some(n);
        }
        n = m;
    }
}

/// Load enable of `n` as a select tree over `hold` (0) and other leaves
/// (1); `None` when `hold` is not in it (then `n` is a leaf).
fn tree(dag: &mut Dag, n: N, hold: N, leaves: &mut Vec<N>, uses: &mut Vec<(N, u32)>, holds: &mut u32, memo: &mut HashMap<N, Option<N>>) -> Option<N> {
    if n == hold {
        return Some(dag.bit(false));
    }
    if let Some(r) = memo.get(&n) {
        return *r;
    }
    let r = match *dag.op(n) {
        Op::Mux(s, a, b) => {
            let ea = tree(dag, a, hold, leaves, uses, holds, memo);
            let eb = tree(dag, b, hold, leaves, uses, holds, memo);
            if ea.is_none() && eb.is_none() {
                None
            } else {
                for (arm, e) in [(a, ea), (b, eb)] {
                    if e.is_none() {
                        leaves.push(arm);
                    }
                    match arm == hold {
                        true => *holds += 1,
                        false => uses.push((arm, 1)),
                    }
                }
                let one = dag.bit(true);
                Some(dag.mux(s, ea.unwrap_or(one), eb.unwrap_or(one)))
            }
        }
        _ => None,
    };
    memo.insert(n, r);
    r
}

fn peel(dag: &mut Dag, d: N, hold: N) -> Option<Peel> {
    let zero = |dag: &Dag, n: N| dag.as_const(n).is_some_and(|b| b.iter().all(|x| !x));
    let mut uses = vec![(d, 1)];
    let (mut top, mut rst) = (d, dag.bit(false));
    if let Op::Mux(s, a, b) = *dag.op(d) {
        if zero(dag, a) != zero(dag, b) {
            (top, rst) = if zero(dag, a) { (b, s) } else { (a, dag.not(s)) };
            uses.push((top, 1));
        }
    }
    let (mut leaves, mut holds) = (Vec::new(), 0);
    let en = match tree(dag, top, hold, &mut leaves, &mut uses, &mut holds, &mut HashMap::new()) {
        Some(en) => en,
        None => {
            leaves.push(top);
            dag.bit(true)
        }
    };
    leaves.sort_unstable();
    leaves.dedup();
    let [core] = leaves[..] else { return None };
    Some(Peel { core, en, rst, uses, holds })
}

/// Register `ri` as a slice register: every bit of its signal is its flop,
/// on a plain rising clock, with no asynchronous reset and power-up 0.
fn slice_reg(flat: &mut Flat, ri: u32) -> Option<SliceReg> {
    let r = &flat.regs[ri as usize];
    let s = &flat.sigs[r.sig as usize];
    if r.negedge || r.arst.is_some() || s.debug || s.init.iter().any(|b| *b) || s.drv.iter().any(|d| *d != Drv::Reg(ri)) {
        return None;
    }
    let (clk, gates) = clock_shape(flat, r.clk);
    if !gates.is_empty() {
        return None;
    }
    let (d, sig, w) = (r.d, r.sig, s.w);
    let hold = flat.dag.read(sig, w);
    peel(&mut flat.dag, d, hold).map(|peel| SliceReg { ri, peel, clk, hold })
}

/// An operand: `e` and the driver of each of its bits.
struct Operand {
    n: N,
    e: u32,
    bits: Vec<(N, u32)>,
}

fn operand(flat: &Flat, x: N) -> Operand {
    let w = flat.dag.w(x);
    let bits: Vec<(N, u32)> = (0..w).map(|i| wire(flat, x, i)).collect();
    let key = |b: (N, u32)| match flat.dag.as_const(b.0) {
        Some(c) => (N::MAX, c[b.1 as usize] as u32),
        None => b,
    };
    let zw = bits.iter().rposition(|&b| key(b) != (N::MAX, 0)).map_or(0, |i| i as u32 + 1);
    let mut sw = w;
    while sw > 1 && key(bits[sw as usize - 2]) == key(bits[w as usize - 1]) {
        sw -= 1;
    }
    Operand { n: x, e: sw.min(zw + 1), bits }
}

impl Operand {
    /// At most one adder in the fabric (see the module docs).
    fn cheap(&self, flat: &Flat) -> bool {
        let bits = &self.bits[..self.e as usize];
        let rows = bits.iter().filter(|b| flat.dag.as_const(b.0).map_or(true, |c| c[b.1 as usize])).count();
        let konst = |b: &(N, u32)| flat.dag.as_const(b.0).map(|c| c[b.1 as usize]);
        rows <= 1 || (rows == 2 && bits.iter().all(|b| konst(b).is_some()) && konst(&bits[bits.len() - 1]) == Some(false))
    }

    /// The register whose output this operand is: (register, its signal).
    fn reg(&self, flat: &Flat) -> Option<(u32, u32)> {
        let mut found = None;
        for &(n, i) in &self.bits[..self.e as usize] {
            match *flat.dag.op(n) {
                // A constant 1 would pass through the slice register, which starts and resets at 0.
                Op::Const(_) if flat.dag.as_const(n).is_some_and(|c| !c[i as usize]) => {}
                Op::Read(s) => {
                    let Drv::Reg(r) = flat.sigs[s as usize].drv[i as usize] else { return None };
                    if found.is_some_and(|f| f != (r, s)) {
                        return None;
                    }
                    found = Some((r, s));
                }
                _ => return None,
            }
        }
        found
    }
}

/// Every read of signal `s` is inside operand `x` (through its slices and
/// concatenations) and those are used by nothing else; the product refers
/// to `x` `uses` times.
fn private(flat: &Flat, refs: &[u32], reads: &[u32], x: N, uses: u32, s: u32) -> bool {
    let mut seen: HashMap<N, u32> = HashMap::new();
    let mut stack = vec![x; uses as usize];
    while let Some(n) = stack.pop() {
        let c = seen.entry(n).or_default();
        *c += 1;
        if *c == 1 && matches!(flat.dag.op(n), Op::Slice(..) | Op::Concat(..)) {
            stack.extend(flat.dag.kids(n));
        }
    }
    let mut r = 0;
    for (&n, &c) in &seen {
        match *flat.dag.op(n) {
            Op::Read(t) if t != s => {}
            Op::Read(_) | Op::Slice(..) | Op::Concat(..) if count(refs, n) != c => return false,
            Op::Read(_) => r += c,
            _ => {}
        }
    }
    r == reads[s as usize]
}

/// An operand of `e` bits as chunks `(lo, len, signed)` of at most `c`
/// unsigned bits under a signed top chunk of at most `c + 1` bits: its
/// value is the sum of each chunk shifted to `lo` (two's complement).
fn chunks(e: u32, c: u32) -> Vec<(u32, u32, bool)> {
    let n = (e - 1).div_ceil(c).max(1);
    (0..n).map(|k| (k * c, if k + 1 < n { c } else { e - k * c }, k + 1 == n)).collect()
}

/// `m`, a product wider than one multiplier, as partial products of
/// chunks that fit (24 unsigned bits on A, 17 on B, the top chunks
/// signed), each modulo 2^(W - its shift), added up the result in the
/// fabric's carry chains. Returns the partial products.
fn split(dag: &mut Dag, m: N, a: &Operand, b: &Operand) -> Vec<N> {
    let w = dag.w(m);
    let terms = |p: &Operand, q: &Operand| -> Vec<(u32, (u32, u32, bool), (u32, u32, bool))> {
        let mut t: Vec<_> = chunks(p.e, 24).into_iter().flat_map(|x| chunks(q.e, 17).into_iter().map(move |y| (x.0 + y.0, x, y))).filter(|t| t.0 < w).collect();
        t.sort_by_key(|t| t.0);
        t
    };
    let (ta, tb) = (terms(a, b), terms(b, a));
    let (t, p, q) = if tb.len() < ta.len() { (tb, b, a) } else { (ta, a, b) };
    let mut parts = Vec::new();
    // The sum from bit `at` up, over the bits below it that are final (LSB last).
    let (mut at, mut sum, mut done) = (0, dag.zero(w), Vec::new());
    for (off, (lx, nx, sx), (ly, ny, sy)) in t {
        let ww = w - off;
        let x = dag.slice(p.n, lx, nx);
        let x = dag.ext(x, ww, sx);
        let y = dag.slice(q.n, ly, ny);
        let y = dag.ext(y, ww, sy);
        let pp = dag.mul(x, y);
        if matches!(dag.op(pp), Op::Mul(..)) {
            parts.push(pp);
        }
        if off > at {
            done.insert(0, dag.bits(sum, 0, off - at));
        }
        let hi = dag.bits(sum, off - at, ww);
        (at, sum) = (off, dag.add(hi, pp));
    }
    done.insert(0, sum);
    let v = dag.concat(done);
    dag.nodes[m as usize] = dag.nodes[v as usize].clone();
    parts
}

const ATTRS: [(&str, &str); 7] = [
    ("A_INPUT", "DIRECT"),
    ("B_INPUT", "DIRECT"),
    ("USE_DPORT", "FALSE"),
    ("USE_MULT", "MULTIPLY"),
    ("USE_SIMD", "ONE48"),
    ("USE_PATTERN_DETECT", "NO_PATDET"),
    ("AUTORESET_PATDET", "NO_RESET"),
];

/// `n` is a product that gets a DSP48E1 of its own, unsplit, when its turn
/// comes (a split one's partial products take no slice registers).
fn takes_dsp(flat: &Flat, n: N) -> bool {
    let Op::Mul(x, y) = *flat.dag.op(n) else { return false };
    let (a, b) = (operand(flat, x), operand(flat, y));
    !a.cheap(flat) && !b.cheap(flat) && ((a.e <= 25 && b.e <= 18) || (b.e <= 25 && a.e <= 18))
}

/// `muls` with each after every other one its operands read, through logic
/// and registers: a slice once built narrows what its readers see to
/// `sext(P)`, so a product is judged only after those it reads, whatever
/// the source order. Node order breaks ties, and picks among a cycle's
/// products once only cycles are left.
fn upstream_first(flat: &Flat, muls: &[N]) -> Vec<N> {
    let set: HashSet<N> = muls.iter().copied().collect();
    let ups: Vec<HashSet<N>> = muls
        .iter()
        .map(|&m| {
            let (mut ups, mut seen, mut stack) = (HashSet::new(), HashSet::new(), flat.dag.kids(m));
            while let Some(n) = stack.pop() {
                if !seen.insert(n) {
                    continue;
                }
                if set.contains(&n) {
                    ups.insert(n);
                    continue;
                }
                stack.extend(flat.dag.kids(n));
                if let Op::Read(s) = *flat.dag.op(n) {
                    for d in &flat.sigs[s as usize].drv {
                        match *d {
                            Drv::Comb(c, _) => stack.push(c),
                            Drv::Reg(r) => stack.push(flat.regs[r as usize].d),
                            Drv::Latch(l) => stack.push(flat.latches[l as usize].d),
                            _ => {}
                        }
                    }
                }
            }
            ups.remove(&m);
            ups
        })
        .collect();
    let (mut left, mut out): (Vec<usize>, Vec<N>) = ((0..muls.len()).collect(), Vec::new());
    let mut done: HashSet<N> = HashSet::new();
    while !left.is_empty() {
        // Nothing ready means a cycle of products: take one whose unbuilt
        // inputs all sit on a cycle with it, so nothing outside feeds it.
        let feeds = |u: N, k: usize| {
            let (mut st, mut seen) = (vec![u], HashSet::new());
            while let Some(v) = st.pop() {
                if v == muls[k] {
                    return true;
                }
                if seen.insert(v) {
                    st.extend(ups[muls.iter().position(|&x| x == v).unwrap()].iter().filter(|w| !done.contains(*w)));
                }
            }
            false
        };
        let i = left
            .iter()
            .position(|&k| ups[k].is_subset(&done))
            .or_else(|| left.iter().position(|&k| ups[k].iter().all(|&u| done.contains(&u) || feeds(u, k))))
            .unwrap_or(0);
        let k = left.remove(i);
        done.insert(muls[k]);
        out.push(muls[k]);
    }
    out
}

/// Infer DSP48E1 cells for every product that fits (see the module docs).
pub fn infer(flat: &mut Flat) {
    let (mut refs, mut reads) = refs(flat);
    let mut muls: Vec<N> = (0..flat.dag.nodes.len() as N).filter(|&n| matches!(flat.dag.op(n), Op::Mul(..)) && refs[n as usize] > 0).collect();
    if muls.is_empty() {
        return;
    }
    muls = upstream_first(flat, &muls);
    // Registers that could be the slice's P or M register, by the node
    // their core reads (`p <= x` or `p <= p + x`).
    let mut cands: Vec<SliceReg> = Vec::new();
    for ri in 0..flat.regs.len() as u32 {
        if count(&refs, flat.regs[ri as usize].d) == 0 {
            continue;
        }
        let Some(sr) = slice_reg(flat, ri) else { continue };
        // A register reading itself to hold is no other reader: it goes
        // into the slice with the register.
        if sr.peel.holds > 0 {
            refs[sr.hold as usize] -= sr.peel.holds;
            reads[flat.regs[ri as usize].sig as usize] -= sr.peel.holds;
        }
        cands.push(sr);
    }
    let mut by_input: HashMap<N, Vec<(usize, bool)>> = HashMap::new();
    // Register -> the node its core reads, to leave a product's own P or M
    // register out of another product's A/B ports whatever the node order.
    let mut stage_of: HashMap<u32, N> = HashMap::new();
    for (k, sr) in cands.iter().enumerate() {
        let (x, acc) = match *flat.dag.op(sr.peel.core) {
            Op::Add(a, b) if a == sr.hold => (b, true),
            Op::Add(a, b) if b == sr.hold => (a, true),
            _ => (sr.peel.core, false),
        };
        if let Some(x) = sole(flat, &refs, &reads, x) {
            by_input.entry(x).or_default().push((k, acc));
            stage_of.insert(sr.ri, x);
        }
    }
    // `m <= a * b; p <= m`: p is that product's P register behind its M.
    // One hop, from the map as built: `p2 <= p` is never that slice's.
    let mp: Vec<(u32, N)> = cands
        .iter()
        .filter_map(|c| {
            let Op::Read(s) = *flat.dag.op(*stage_of.get(&c.ri)?) else { return None };
            let m = cands.iter().find(|m| flat.regs[m.ri as usize].sig == s && m.clk == c.clk)?;
            stage_of.get(&m.ri).copied().filter(|&n| takes_dsp(flat, n)).map(|n| (c.ri, n))
        })
        .collect();
    stage_of.extend(mp);
    let mut built = 0usize;
    let mut room = DSP_SITES.load(Ordering::Relaxed).saturating_sub(flat.hard.iter().filter(|h| h.module == "DSP48E1").count());
    // Partial products of a split one: combinational, as the sum reads them.
    let mut parts: HashSet<N> = HashSet::new();
    let mut next = 0;
    while let Some(&m) = muls.get(next) {
        next += 1;
        let Op::Mul(x, y) = *flat.dag.op(m) else { continue };
        let w = flat.dag.w(m);
        let (mut a, mut b) = (operand(flat, x), operand(flat, y));
        if a.cheap(flat) || b.cheap(flat) || room == 0 {
            continue;
        }
        if a.e > 25 || b.e > 18 {
            std::mem::swap(&mut a, &mut b);
        }
        if a.e > 25 || b.e > 18 {
            let pp = split(&mut flat.dag, m, &a, &b);
            parts.extend(pp.iter().copied());
            muls.extend(pp);
            continue;
        }
        let part = parts.contains(&m);
        // P side: the one register loading `input` or adding it to itself,
        // with the select structure and `input` used by nothing else.
        let stage = |flat: &Flat, refs: &[u32], input: N| -> Option<(usize, bool)> {
            let [(k, acc)] = by_input.get(&input)?.as_slice() else { return None };
            let c = &cands[*k];
            let x = match *flat.dag.op(c.peel.core) {
                Op::Add(p, q) if *acc => if p == c.hold { q } else { p },
                _ => c.peel.core,
            };
            let wide = *acc && flat.sigs[flat.regs[c.ri as usize].sig as usize].w > 48;
            (!wide && exclusive(refs, &c.peel.uses) && sole(flat, refs, &reads, x) == Some(input)).then_some((*k, *acc))
        };
        let (mut mreg, mut preg) = (None, stage(flat, &refs, m).filter(|_| !part));
        if let Some((k, false)) = preg {
            // `m <= a * b` read only by the next stage: the M register.
            let s = flat.regs[cands[k].ri as usize].sig;
            if reads[s as usize] == 1 {
                let rm = flat.dag.read(s, flat.sigs[s as usize].w);
                if let Some(next) = stage(flat, &refs, rm).filter(|n| cands[n.0].clk == cands[k].clk) {
                    (mreg, preg) = (Some(k), Some(next));
                }
            }
        }
        let mut clk = preg.map(|(k, _)| cands[k].clk);
        // A and B: an operand that is a register's output, on the same clock.
        let own: Vec<u32> = [mreg, preg.map(|p| p.0)].into_iter().flatten().map(|k| cands[k].ri).collect();
        let port = |flat: &mut Flat, op: &Operand, clk: &mut Option<(N, u32)>| -> (Vec<(N, u32)>, Option<(N, N)>) {
            let pins: Vec<(N, u32)> = (0..op.e).map(|j| op.bits[j as usize]).collect();
            let uses = (x == op.n) as u32 + (y == op.n) as u32;
            let Some((ri, s)) = op.reg(flat).filter(|&(ri, s)| !part && !own.contains(&ri) && !stage_of.get(&ri).is_some_and(|&n| takes_dsp(flat, n)) && private(flat, &refs, &reads, op.n, uses, s)) else {
                return (pins, None);
            };
            let Some(sr) = slice_reg(flat, ri) else { return (pins, None) };
            if clk.is_some_and(|c| c != sr.clk) {
                return (pins, None);
            }
            *clk = Some(sr.clk);
            let pins = pins.into_iter().map(|(n, i)| if *flat.dag.op(n) == Op::Read(s) { (sr.peel.core, i) } else { (n, i) }).collect();
            (pins, Some((sr.peel.en, sr.peel.rst)))
        };
        let (pa, ra) = port(flat, &a, &mut clk);
        let (pb, rb) = port(flat, &b, &mut clk);
        // The cell.
        let base = match preg {
            Some((k, _)) => flat.sigs[flat.regs[cands[k].ri as usize].sig as usize].name.clone(),
            None => "mul".to_string(),
        };
        let name = format!("{base}_dsp{built}");
        built += 1;
        room -= 1;
        let acc = preg.is_some_and(|p| p.1);
        let wp = match preg {
            Some((k, _)) => flat.sigs[flat.regs[cands[k].ri as usize].sig as usize].w,
            None => w,
        };
        let np = if acc { wp } else { wp.min(a.e + b.e) };
        let dag = &mut flat.dag;
        let (zero, one) = (dag.bit(false), dag.bit(true));
        let mut ins: Vec<(String, N, u32)> = Vec::new();
        if let Some((n, i)) = clk {
            ins.push(("CLK".into(), n, i));
        }
        for (p, pins, k) in [("A", &pa, 30), ("B", &pb, 18)] {
            for j in 0..k {
                let (n, i) = pins[j.min(pins.len() - 1)];
                ins.push((format!("{p}{j}"), n, i));
            }
        }
        let opmode: u32 = if acc { 0b0100101 } else { 0b0000101 };
        for (p, k) in [("C", 48), ("D", 25), ("INMODE", 5), ("OPMODE", 7), ("ALUMODE", 4), ("CARRYINSEL", 3), ("ACIN", 30), ("BCIN", 18), ("PCIN", 48)] {
            for j in 0..k {
                let v = if p == "OPMODE" && (opmode >> j) & 1 == 1 { one } else { zero };
                ins.push((format!("{p}{j}"), v, 0));
            }
        }
        // (enable, reset) of each register; an unused one is enabled and not reset.
        let ctl = |c: Option<(N, N)>| c.unwrap_or((one, zero));
        let pc = |k: Option<usize>| k.map(|k| (cands[k].peel.en, cands[k].peel.rst));
        let (ea, eb, em, ep) = (ctl(ra), ctl(rb), ctl(pc(mreg)), ctl(pc(preg.map(|p| p.0))));
        for (p, v) in [
            ("CARRYIN", zero),
            ("CARRYCASCIN", zero),
            ("MULTSIGNIN", zero),
            ("CEA1", one),
            ("CEA2", ea.0),
            ("CEB1", one),
            ("CEB2", eb.0),
            ("CEM", em.0),
            ("CEP", ep.0),
            ("CEAD", zero),
            ("CEC", zero),
            ("CED", zero),
            ("CECTRL", zero),
            ("CEINMODE", zero),
            ("CEALUMODE", zero),
            ("CECARRYIN", zero),
            ("RSTA", ea.1),
            ("RSTB", eb.1),
            ("RSTM", em.1),
            ("RSTP", ep.1),
            ("RSTC", zero),
            ("RSTD", zero),
            ("RSTCTRL", zero),
            ("RSTINMODE", zero),
            ("RSTALUMODE", zero),
            ("RSTALLCARRYIN", zero),
        ] {
            ins.push((p.into(), v, 0));
        }
        let mut attrs: Vec<(String, String)> = ATTRS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        for (k, on) in [("AREG", ra.is_some()), ("BREG", rb.is_some()), ("ACASCREG", ra.is_some()), ("BCASCREG", rb.is_some()), ("MREG", mreg.is_some()), ("PREG", preg.is_some())] {
            attrs.push((k.into(), (on as u32).to_string()));
        }
        for k in ["ADREG", "CREG", "DREG", "INMODEREG", "OPMODEREG", "ALUMODEREG", "CARRYINREG", "CARRYINSELREG"] {
            attrs.push((k.into(), "0".into()));
        }
        // The product, on a signal the cell drives.
        let hi = flat.hard.len() as u32;
        let psig = flat.sigs.len() as u32;
        flat.sigs.push(SigInfo {
            name: format!("{name}_P"),
            hier: format!("{name}.P"),
            index: None,
            w: np,
            drv: vec![Drv::Hard(hi); np as usize],
            init: vec![false; np as usize],
            debug: false,
        });
        let outs = (0..np).map(|i| (format!("P{i}"), psig, i)).collect();
        for (_, n, _) in &ins {
            match refs.get_mut(*n as usize) {
                Some(r) => *r += 1,
                None => {
                    refs.resize(*n as usize + 1, 0);
                    refs[*n as usize] = 1;
                }
            }
        }
        flat.hard.push(Hard { name, module: "DSP48E1".into(), attrs, ins, outs });
        let pr = flat.dag.read(psig, np);
        let v = flat.dag.sext(pr, wp);
        match preg {
            Some((k, _)) => {
                let s = flat.regs[cands[k].ri as usize].sig as usize;
                for (i, d) in flat.sigs[s].drv.iter_mut().enumerate() {
                    *d = Drv::Comb(v, i as u32);
                }
            }
            None => flat.dag.nodes[m as usize] = flat.dag.nodes[v as usize].clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::chunks;

    #[test]
    fn chunks_cover_the_operand_and_fit_their_port() {
        for c in [17, 24] {
            for e in 2..=130 {
                let ch = chunks(e, c);
                let (top, low) = ch.split_last().unwrap();
                let mut at = 0;
                for &(lo, len, signed) in low {
                    assert_eq!((lo, len, signed), (at, c, false), "e {e} c {c}");
                    at += c;
                }
                assert_eq!((top.0, top.2), (at, true), "e {e} c {c}");
                assert!(top.0 + top.1 == e && (2..=c + 1).contains(&top.1), "e {e} c {c}: top {top:?}");
            }
        }
        assert_eq!(chunks(33, 24), [(0, 24, false), (24, 9, true)]);
        assert_eq!(chunks(18, 17), [(0, 18, true)]);
    }
}
