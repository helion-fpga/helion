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
    /// Output of latch `.0`.
    Latch(u32),
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
    /// The power-up value was given (initializer or `initial`): bits
    /// nothing drives hold it.
    pub has_init: bool,
    /// Keep a net of this signal's name (`mark_debug`).
    pub debug: bool,
}

/// The flops of one clocked process for one signal: bit `i` is a flop when
/// `mask[i]`, with data input bit `i` of `d`.
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

/// A read `mem[addr]` of a whole memory signal with a variable address
/// (recorded only with hard-block inference on).
#[derive(Clone, Debug)]
pub struct MemRead {
    /// The value read.
    pub node: N,
    pub sig: u32,
    /// The address as written in the source.
    pub addr: N,
    pub geo: MemGeo,
}

/// A nonblocking write `mem[addr] <= val` straight into the stored value
/// of a memory signal (recorded only with hard-block inference on).
#[derive(Clone, Debug)]
pub struct MemWrite {
    /// The memory's new value when the write happens.
    pub node: N,
    pub sig: u32,
    pub addr: N,
    pub val: N,
    pub geo: MemGeo,
}

/// Shape of a memory: `count` words of `ew` bits, indices `lo..=hi`,
/// `asc` when declared `[lo:hi]` (word `i` then sits at position `hi - i`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemGeo {
    pub count: u32,
    pub ew: u32,
    pub lo: i64,
    pub hi: i64,
    pub asc: bool,
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
    /// Memory accesses, for block RAM inference.
    pub mem_reads: Vec<MemRead>,
    pub mem_writes: Vec<MemWrite>,
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
fn wire(flat: &Flat, mut n: N, mut bit: u32) -> (N, u32) {
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
fn clock_shape(flat: &Flat, clk: N) -> ((N, u32), Vec<(N, u32)>) {
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
    /// Hard-block inference is on.
    hard: bool,
    /// Multipliers put on `Mac27` blocks.
    macs: Vec<MacInf>,
    /// Memories put on `Bram18` blocks, and the signals they replace:
    /// a read register is port `.1` of RAM `.0`.
    rams: Vec<RamInf>,
    ram_of_q: HashMap<u32, (usize, usize)>,
    ram_of_mem: HashMap<u32, usize>,
    /// Registers that may be a multiplier's `P` register, and those
    /// decided (`true`: on the block).
    mac_regs: HashMap<u32, MacRegCand>,
    mac_reg_done: HashMap<u32, bool>,
    /// Additions tried as a multiplier's `+ C`.
    fused_tried: std::collections::HashSet<N>,
    /// Flops the blocks need besides the design's registers (a deep RAM's
    /// registered bank select): Q literal (an input of the logic), D, clock
    /// and net name.
    extra_ffs: Vec<(Lit, Lit, Lit, String)>,
}

/// A multiplication on a `Mac27`: operand and result literals.
struct MacInf {
    a: Vec<Lit>,
    b: Vec<Lit>,
    /// The addend on `C` (`FALSE` bits are left unconnected).
    c: Vec<Lit>,
    signed: bool,
    p: Vec<Lit>,
    /// `PREG=1`: the register's clock, load enable (`CE`) and synchronous
    /// reset (`RST`); `p` are then the register's own bits.
    reg: Option<(Lit, Lit, Lit)>,
}

/// A register that may be a multiplier's `P` register: its data input is
/// a multiplexer tree over one `load` (the product `mul`, or `mul + c`),
/// the constant 0 and its own value.
struct MacRegCand {
    load: N,
    mul: N,
    c: Option<N>,
}

/// Most multiplier blocks one product is built from.
const MAX_MAC_PARTS: usize = 16;
/// Most block RAMs one memory is built from (read ports x depth banks of
/// 1024 words x width slices of 16 bits).
const MAX_RAM_BLOCKS: u32 = 64;

/// A memory on `Bram18` blocks: the memory signal, its geometry, the
/// writes (one write port), and one read port per register that reads it.
/// Every port has its own blocks; all of them take every write.
struct RamInf {
    mem: u32,
    /// The register whose clock the blocks take: the memory's own, or for
    /// a ROM (no writes) its first read register's.
    reg_mem: u32,
    geo: MemGeo,
    /// The writes, all at one address (one write port).
    writes: Vec<MemWrite>,
    ports: Vec<RamPort>,
    /// Write pins, built with the first used port's pins: low address
    /// bits, data, the enable of each depth bank, clock.
    wpins: Option<(Vec<Lit>, Vec<Lit>, Vec<Lit>, Lit)>,
}

struct RamPort {
    q: u32,
    reg_q: u32,
    read: MemRead,
    /// Once the register is read: its value, the `DO` of each depth bank
    /// (all slices, `ew` bits) and the registered bank select.
    dout: Option<Vec<Lit>>,
    bank_do: Vec<Vec<Lit>>,
    sel: Vec<Lit>,
    /// Low read address bits and read enable.
    pins: Option<(Vec<Lit>, Lit)>,
}

impl RamInf {
    fn abits(&self) -> u32 {
        self.geo.count.trailing_zeros()
    }
    /// Address bits each block takes, and bank-select bits.
    fn split(&self) -> (u32, u32) {
        let lo = self.abits().min(10);
        (lo, self.abits() - lo)
    }
    fn banks(&self) -> u32 {
        1 << self.split().1
    }
    fn slices(&self) -> u32 {
        self.geo.ew.div_ceil(16)
    }
}

/// Whether node `n` is the constant 0.
fn is_zero(flat: &Flat, n: N) -> bool {
    matches!(flat.dag.op(n), Op::Const(b) if b.iter().all(|x| !x))
}

/// Whether the operands of a product of `w` bits fit one multiplier: at
/// most 32 bits of result, 16-bit values (both zero-extended: unsigned,
/// both sign-extended: signed) with two or more varying bits each.
/// Returns the sign mode.
fn fit16(x: &[Lit], y: &[Lit], w: usize) -> Option<bool> {
    let lo = w.min(16);
    let zext = |v: &[Lit]| v[lo..].iter().all(|&l| l == FALSE);
    let sext = |v: &[Lit]| w > 16 && v[15..].iter().all(|&l| l == v[15]);
    let varying = |v: &[Lit]| v[..lo].iter().filter(|&&l| l > TRUE).count() >= 2;
    if !(varying(x) && varying(y)) {
        return None;
    }
    if zext(x) && zext(y) {
        Some(false)
    } else if sext(x) && sext(y) {
        Some(true)
    } else {
        None
    }
}

/// Registers whose value may live in a multiplier's `P` register.
fn infer_mac_regs(flat: &Flat) -> HashMap<u32, MacRegCand> {
    let mut out = HashMap::new();
    for (ri, r) in flat.regs.iter().enumerate() {
        let s = &flat.sigs[r.sig as usize];
        if s.w > 32
            || flat.dag.w(r.d) != s.w
            || r.negedge
            || r.arst.is_some()
            || !r.mask.iter().all(|b| *b)
            || !clock_shape(flat, r.clk).1.is_empty()
            || s.init.iter().any(|b| *b)
            || s.drv.iter().any(|d| *d != Drv::Reg(ri as u32))
        {
            continue;
        }
        let mut leaves = Vec::new();
        mux_leaves(&flat.dag, r.d, N::MAX, &mut leaves);
        let loads: Vec<N> = leaves.iter().copied().filter(|&l| !is_zero(flat, l) && !is_whole_read(flat, l, r.sig)).collect();
        let [load] = loads.as_slice() else { continue };
        let is_mul = |m: N| matches!(flat.dag.op(m), Op::Mul(..)) && flat.dag.w(m) == s.w;
        // (`load` stays the leaf the register's input names; the product
        // may come through wires: `assign m = a * b; r <= m;`.)
        let value = whole_wire(flat, *load);
        let cand = match flat.dag.op(value) {
            Op::Mul(..) if is_mul(value) => MacRegCand { load: *load, mul: value, c: None },
            Op::Add(x, y) if flat.dag.w(value) == s.w => {
                let (x, y) = (whole_wire(flat, *x), whole_wire(flat, *y));
                if is_mul(x) {
                    MacRegCand { load: *load, mul: x, c: Some(y) }
                } else if is_mul(y) {
                    MacRegCand { load: *load, mul: y, c: Some(x) }
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        out.insert(r.sig, cand);
    }
    out
}

/// Follow whole reads of combinationally driven signals (every bit `i`
/// of the signal bit `i` of one node as wide) down to that node.
fn whole_wire(flat: &Flat, mut n: N) -> N {
    for _ in 0..64 {
        let Op::Read(sig) = flat.dag.op(n) else { break };
        let s = &flat.sigs[*sig as usize];
        if flat.dag.w(n) != s.w {
            break;
        }
        let Some(Drv::Comb(m, 0)) = s.drv.first().copied() else { break };
        if flat.dag.w(m) != s.w || s.drv.iter().enumerate().any(|(i, d)| *d != Drv::Comb(m, i as u32)) {
            break;
        }
        n = m;
    }
    n
}

/// Error prefix asking `lower` to build memory signal `n` from flops.
const RETRY: &str = "\u{0}retry-ram:";

/// Leaves of a tree of multiplexers (`stop` is a leaf even if it is one).
fn mux_leaves(dag: &Dag, n: N, stop: N, out: &mut Vec<N>) {
    match dag.op(n) {
        Op::Mux(_, a, b) if n != stop => {
            let (a, b) = (*a, *b);
            mux_leaves(dag, a, stop, out);
            mux_leaves(dag, b, stop, out);
        }
        _ => {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
}

/// Whether node `n` is the whole value of signal `sig`.
fn is_whole_read(flat: &Flat, n: N, sig: u32) -> bool {
    matches!(flat.dag.op(n), Op::Read(s) if *s == sig) && flat.dag.w(n) == flat.sigs[sig as usize].w
}

/// Memories that can go on block RAMs (see `super::set_hard_blocks`),
/// except those in `excluded`.
fn infer_rams(flat: &Flat, excluded: &std::collections::HashSet<u32>) -> Vec<RamInf> {
    let mut out = Vec::new();
    let mut mems: Vec<u32> = flat.mem_writes.iter().map(|w| w.sig).collect();
    // ROMs: memories nothing drives, with contents from an initializer or
    // an `initial` block, read by registers.
    let is_rom = |m: u32| {
        let s = &flat.sigs[m as usize];
        s.has_init && s.drv.iter().all(|d| *d == Drv::None)
    };
    mems.extend(flat.mem_reads.iter().map(|r| r.sig).filter(|&m| is_rom(m)));
    mems.sort_unstable();
    mems.dedup();
    'mem: for m in mems {
        if excluded.contains(&m) {
            continue;
        }
        let writes: Vec<&MemWrite> = flat.mem_writes.iter().filter(|w| w.sig == m).collect();
        let mut reads: Vec<&MemRead> = flat.mem_reads.iter().filter(|r| r.sig == m).collect();
        reads.sort_by_key(|r| r.node);
        reads.dedup_by_key(|r| r.node);
        let rom = writes.is_empty();
        if (rom && !is_rom(m)) || reads.is_empty() || writes.iter().any(|x| x.addr != writes[0].addr || x.geo != writes[0].geo) {
            continue;
        }
        let g = if rom { reads[0].geo } else { writes[0].geo };
        let abits = g.count.trailing_zeros();
        if reads.iter().any(|r| r.geo != g) || g.lo != 0 || !g.count.is_power_of_two() || g.count < 2 || g.ew == 0 || g.hi != g.count as i64 - 1 {
            continue;
        }
        if writes.iter().any(|x| flat.dag.w(x.addr) != abits) || reads.iter().any(|r| flat.dag.w(r.addr) != abits) || writes.iter().any(|x| flat.dag.w(x.val) != g.ew) {
            continue;
        }
        let sm = &flat.sigs[m as usize];
        if sm.w != g.count * g.ew {
            continue;
        }
        let root;
        let rm;
        if rom {
            // The clock of the first register that reads it (every read
            // port must share it).
            let Some(ri) = flat.regs.iter().position(|rq| reads.iter().any(|r| {
                let mut leaves = Vec::new();
                mux_leaves(&flat.dag, rq.d, r.node, &mut leaves);
                leaves.contains(&r.node)
            })) else {
                continue;
            };
            rm = ri as u32;
            root = clock_shape(flat, flat.regs[ri].clk).0;
        } else {
            let Some(Drv::Reg(r0)) = sm.drv.first().copied() else { continue };
            rm = r0;
            if sm.drv.iter().any(|d| *d != Drv::Reg(rm)) {
                continue;
            }
            let reg = &flat.regs[rm as usize];
            if reg.negedge || reg.arst.is_some() || !reg.mask.iter().all(|b| *b) || !clock_shape(flat, reg.clk).1.is_empty() {
                continue;
            }
            let mut leaves = Vec::new();
            mux_leaves(&flat.dag, reg.d, writes[0].node, &mut leaves);
            let is_write = |l: N| writes.iter().any(|x| x.node == l);
            if !leaves.iter().any(|&l| is_write(l)) || leaves.iter().any(|&l| !is_write(l) && !is_whole_read(flat, l, m)) {
                continue;
            }
            root = clock_shape(flat, reg.clk).0;
        }
        // The registers that read the memory, one read port each.
        let mut ports: Vec<RamPort> = Vec::new();
        for (ri, rq) in flat.regs.iter().enumerate() {
            let q = rq.sig;
            let sq = &flat.sigs[q as usize];
            // (A read is itself a multiplexer tree over the words: a leaf.)
            let found = reads.iter().find_map(|r| {
                let mut leaves = Vec::new();
                mux_leaves(&flat.dag, rq.d, r.node, &mut leaves);
                leaves.contains(&r.node).then_some((r, leaves))
            });
            let Some((r, leaves)) = found else { continue };
            let ok = leaves.iter().all(|&l| l == r.node || is_whole_read(flat, l, q))
                && sq.w == g.ew
                && sq.drv.iter().all(|d| *d == Drv::Reg(ri as u32))
                && sq.init.iter().all(|b| !b)
                && !rq.negedge
                && rq.arst.is_none()
                && rq.mask.iter().all(|b| *b)
                && clock_shape(flat, rq.clk) == (root, Vec::new());
            if !ok {
                continue 'mem;
            }
            ports.push(RamPort { q, reg_q: ri as u32, read: (*r).clone(), dout: None, bank_do: Vec::new(), sel: Vec::new(), pins: None });
        }
        // Every read is some register's.
        if reads.iter().any(|r| !ports.iter().any(|p| p.read.node == r.node)) {
            continue;
        }
        let ram = RamInf { mem: m, reg_mem: rm, geo: g, writes: writes.iter().map(|x| (*x).clone()).collect(), ports, wpins: None };
        if ram.ports.len() as u32 * ram.banks() * ram.slices() > MAX_RAM_BLOCKS {
            continue;
        }
        out.push(ram);
    }
    out
}

impl<'a> Blaster<'a> {
    /// Literal of `n`, a multiplexer tree over `on` and `off` leaves, being `on`.
    fn chosen(&mut self, n: N, on: &dyn Fn(N) -> bool, off: &dyn Fn(N) -> bool) -> Result<Lit, String> {
        if on(n) {
            return Ok(TRUE);
        }
        if off(n) {
            return Ok(FALSE);
        }
        let Op::Mux(s, a, b) = self.flat.dag.op(n).clone() else { return Err("internal: not a multiplexer tree".into()) };
        let sel = self.bit(s, 0)?;
        let x = self.chosen(a, on, off)?;
        let y = self.chosen(b, on, off)?;
        Ok(self.aig.mux(sel, x, y))
    }

    /// Data written by a multiplexer tree over the writes `ws` (`None`:
    /// no write on any path, the data does not matter).
    fn write_data(&mut self, n: N, ws: &[MemWrite], mem: u32) -> Result<Option<Vec<Lit>>, String> {
        if let Some(w) = ws.iter().find(|w| w.node == n) {
            return Ok(Some(self.all(w.val)?));
        }
        if is_whole_read(self.flat, n, mem) {
            return Ok(None);
        }
        let Op::Mux(s, a, b) = self.flat.dag.op(n).clone() else { return Err("internal: not a multiplexer tree".into()) };
        let x = self.write_data(a, ws, mem)?;
        let y = self.write_data(b, ws, mem)?;
        Ok(match (x, y) {
            (Some(x), Some(y)) => {
                let sel = self.bit(s, 0)?;
                Some(x.iter().zip(&y).map(|(p, q)| self.aig.mux(sel, *p, *q)).collect())
            }
            (x, y) => x.or(y),
        })
    }

    /// The value of read port `p` of RAM `k`: with one depth bank the
    /// blocks' `DO`; with several, every bank's `DO` (inputs of the logic)
    /// chosen by the bank select registered with the read.
    fn ram_port_out(&mut self, k: usize, p: usize) -> Vec<Lit> {
        if let Some(v) = &self.rams[k].ports[p].dout {
            return v.clone();
        }
        let (ew, banks, hb) = (self.rams[k].geo.ew, self.rams[k].banks(), self.rams[k].split().1);
        let bank_do: Vec<Vec<Lit>> = (0..banks).map(|_| (0..ew).map(|_| self.aig.input()).collect()).collect();
        let sel: Vec<Lit> = (0..hb).map(|_| self.aig.input()).collect();
        let mut v = bank_do[0].clone();
        if banks > 1 {
            for bit in 0..ew as usize {
                // A multiplexer tree on the select bits, LSB first.
                let mut level: Vec<Lit> = bank_do.iter().map(|d| d[bit]).collect();
                for &s in &sel {
                    level = level.chunks(2).map(|c| self.aig.mux(s, c[1], c[0])).collect();
                }
                v[bit] = level[0];
            }
        }
        let port = &mut self.rams[k].ports[p];
        port.bank_do = bank_do;
        port.sel = sel;
        port.dout = Some(v.clone());
        v
    }

    /// Build the pins of read port `p` of RAM `k` (its output is used), and
    /// the RAM's write pins if not yet built.
    fn ram_pins(&mut self, k: usize, p: usize) -> Result<(), String> {
        let flat = self.flat;
        let (ws, mem, rm, (lo, hb)) = {
            let ri = &self.rams[k];
            (ri.writes.clone(), ri.mem, ri.reg_mem, ri.split())
        };
        let ((cn, cb), _) = clock_shape(flat, flat.regs[rm as usize].clk);
        let clk = self.bit(cn, cb)?;
        if self.rams[k].wpins.is_none() && ws.is_empty() {
            // A ROM: no write pins, WE off.
            self.rams[k].wpins = Some((Vec::new(), Vec::new(), vec![FALSE; 1 << hb], clk));
        }
        if self.rams[k].wpins.is_none() {
            let waddr = self.all(ws[0].addr)?;
            let d = flat.regs[rm as usize].d;
            let di = self.write_data(d, &ws, mem)?.ok_or("internal: a RAM without a write")?;
            let we = self.chosen(d, &|l| ws.iter().any(|w| w.node == l), &|l| is_whole_read(flat, l, mem))?;
            // Each depth bank writes when the high address bits are its own.
            let mut bank_we = Vec::new();
            for b in 0..1u32 << hb {
                let mut e = we;
                for h in 0..hb {
                    let a = waddr[(lo + h) as usize];
                    e = self.aig.and(e, if (b >> h) & 1 == 1 { a } else { a ^ 1 });
                }
                bank_we.push(e);
            }
            self.rams[k].wpins = Some((waddr[..lo as usize].to_vec(), di, bank_we, clk));
        }
        let (r, q, rq) = {
            let port = &self.rams[k].ports[p];
            (port.read.clone(), port.q, port.reg_q)
        };
        let raddr = self.all(r.addr)?;
        let rnode = r.node;
        let re = self.chosen(flat.regs[rq as usize].d, &|l| l == rnode, &|l| is_whole_read(flat, l, q))?;
        // The bank select: the high read address bits, registered with
        // the read (held while RE is low).
        let sel = self.rams[k].ports[p].sel.clone();
        let qname = flat.sigs[q as usize].name.clone();
        for (h, &sq) in sel.iter().enumerate() {
            let d = self.aig.mux(re, raddr[lo as usize + h], sq);
            self.extra_ffs.push((sq, d, clk, format!("{qname}_bank{h}")));
        }
        self.rams[k].ports[p].pins = Some((raddr[..lo as usize].to_vec(), re));
        Ok(())
    }

    /// Operands of `mul` on one multiplier block ([`fit16`]), and the sign
    /// mode.
    fn mac_operands(&mut self, mul: N) -> Result<Option<(Vec<Lit>, Vec<Lit>, bool)>, String> {
        let Op::Mul(a, b) = self.flat.dag.op(mul).clone() else { return Ok(None) };
        let w = self.flat.dag.w(mul) as usize;
        if w > 32 {
            return Ok(None);
        }
        let (x, y) = (self.all(a)?, self.all(b)?);
        let lo = w.min(16);
        Ok(fit16(&x, &y, w).map(|s| (x[..lo].to_vec(), y[..lo].to_vec(), s)))
    }

    /// `a * b + c` (one operand of addition `n` a product of the same
    /// width that fits one multiplier) on a multiplier with `C`.
    fn fused_add(&mut self, n: N) -> Result<Option<Vec<Lit>>, String> {
        if let Some(v) = self.whole.get(&n) {
            return Ok(Some(v.clone()));
        }
        if !self.fused_tried.insert(n) {
            return Ok(None);
        }
        let Op::Add(a, b) = self.flat.dag.op(n).clone() else { return Ok(None) };
        let w = self.flat.dag.w(n);
        if w > 32 {
            return Ok(None);
        }
        for (m, c) in [(a, b), (b, a)] {
            if !matches!(self.flat.dag.op(m), Op::Mul(..)) || self.flat.dag.w(m) != w {
                continue;
            }
            if let Some((x, y, signed)) = self.mac_operands(m)? {
                let c = self.all(c)?;
                let p: Vec<Lit> = (0..w).map(|_| self.aig.input()).collect();
                self.macs.push(MacInf { a: x, b: y, c, signed, p: p.clone(), reg: None });
                self.whole.insert(n, p.clone());
                return Ok(Some(p));
            }
        }
        Ok(None)
    }

    /// The product of `x` and `y` (`w` bits) on multiplier blocks: one
    /// block when both fit 16 bits ([`fit16`]; a result wider than 32 bits
    /// is then P zero- or sign-extended); else 16 x 16 partial products of
    /// the operands' 16-bit pieces, each on a block, summed with adders.
    /// `None`: a constant operand, or more than [`MAX_MAC_PARTS`] blocks.
    fn mul_on_blocks(&mut self, x: &[Lit], y: &[Lit], w: usize) -> Option<Vec<Lit>> {
        if let Some(signed) = fit16(x, y, w) {
            let lo = w.min(16);
            let np = w.min(32);
            let p: Vec<Lit> = (0..np).map(|_| self.aig.input()).collect();
            self.macs.push(MacInf { a: x[..lo].to_vec(), b: y[..lo].to_vec(), c: Vec::new(), signed, p: p.clone(), reg: None });
            let ext = if signed { p[31.min(np - 1)] } else { FALSE };
            return Some((0..w).map(|i| if i < np { p[i] } else { ext }).collect());
        }
        let varying = |v: &[Lit]| v.iter().filter(|&&l| l > TRUE).count() >= 2;
        if !(varying(x) && varying(y)) {
            return None;
        }
        // Each operand as 16-bit pieces, fewest first: an operand
        // zero-extended from u bits has ceil(u / 16) unsigned pieces; one
        // sign-extended from s bits (s < w) has ceil(s / 16), all unsigned
        // but the top one, which is signed (it holds the sign). A block
        // multiplies two pieces: unsigned x unsigned unsigned, else signed
        // (SIGNED=1), an unsigned piece u beside a signed one t read as
        // signed, u - 2^16 u[15], so the block is short by u[15] * t * 2^16,
        // added in logic. This takes ceil(n / 16) * ceil(m / 16) blocks for
        // n- and m-bit operands (fewer when the result is narrower), the
        // fewest 16 x 16 blocks that cover the n x m partial products.
        let rep = |v: &[Lit]| -> (usize, bool) {
            let used = v.iter().take(w).rposition(|&l| l != FALSE).map_or(0, |i| i + 1);
            let top = v[w - 1];
            if top == FALSE {
                return (used, false);
            }
            let s = (0..w).rev().find(|&i| v[i] != top).map_or(1, |i| i + 2);
            if s < w {
                (s, true)
            } else {
                (w, false)
            }
        };
        let ((ux, sx), (uy, sy)) = (rep(x), rep(y));
        // Bits 16i.. of piece i: past the operand's u bits 0, or for a
        // signed one its sign.
        let piece = |v: &[Lit], u: usize, signed: bool, i: usize| -> Vec<Lit> {
            (16 * i..16 * i + 16).map(|k| if k < u { v[k] } else if signed { v[u - 1] } else { FALSE }).collect()
        };
        let (kx, ky) = (ux.div_ceil(16), uy.div_ceil(16));
        // (shift, x piece, y piece, x piece signed, y piece signed)
        let mut parts: Vec<(usize, Vec<Lit>, Vec<Lit>, bool, bool)> = Vec::new();
        for i in 0..kx {
            for j in 0..ky {
                let sh = 16 * (i + j);
                let (a, b) = (piece(x, ux, sx, i), piece(y, uy, sy, j));
                if sh < w && a.iter().any(|&l| l != FALSE) && b.iter().any(|&l| l != FALSE) {
                    parts.push((sh, a, b, sx && i == kx - 1, sy && j == ky - 1));
                }
            }
        }
        if parts.len() > MAX_MAC_PARTS {
            return None;
        }
        let mut acc = vec![FALSE; w];
        // acc += v << sh (v extended by `ext` above its length).
        let add_at = |bl: &mut Self, acc: &mut Vec<Lit>, sh: usize, v: &[Lit], ext: Lit| {
            let mut c = FALSE;
            for j in sh..w {
                let t = v.get(j - sh).copied().unwrap_or(ext);
                let s1 = bl.aig.xor(acc[j], t);
                let s2 = bl.aig.xor(s1, c);
                let g = bl.aig.and(acc[j], t);
                let h = bl.aig.and(s1, c);
                c = bl.aig.or(g, h);
                acc[j] = s2;
            }
        };
        for (sh, a, b, sa, sb) in parts {
            let signed = sa || sb;
            let p: Vec<Lit> = (0..(w - sh).min(32)).map(|_| self.aig.input()).collect();
            self.macs.push(MacInf { a: a.clone(), b: b.clone(), c: Vec::new(), signed, p: p.clone(), reg: None });
            let ext = if signed && p.len() == 32 { p[31] } else { FALSE };
            add_at(self, &mut acc, sh, &p, ext);
            // An unsigned piece read as signed: add its bit 15 times the
            // signed piece, 16 bits up.
            for (u, t, fix) in [(&a, &b, sb && !sa), (&b, &a, sa && !sb)] {
                if fix && u[15] != FALSE && sh + 16 < w {
                    let m: Vec<Lit> = t.iter().map(|&l| self.aig.and(l, u[15])).collect();
                    let e = m[15];
                    add_at(self, &mut acc, sh + 16, &m, e);
                }
            }
        }
        Some(acc)
    }

    /// Put register `sig` (a [`MacRegCand`]) on a multiplier's `P`
    /// register if its product fits one block. Called before any of its
    /// bits is built as a flop.
    fn decide_mac_reg(&mut self, sig: u32) -> Result<bool, String> {
        if let Some(&d) = self.mac_reg_done.get(&sig) {
            return Ok(d);
        }
        self.mac_reg_done.insert(sig, false);
        let flat = self.flat;
        let Some(cand) = self.mac_regs.get(&sig) else { return Ok(false) };
        let (load, mul, c) = (cand.load, cand.mul, cand.c);
        let Some((a, b, signed)) = self.mac_operands(mul)? else { return Ok(false) };
        let c = match c {
            Some(c) => self.all(c)?,
            None => Vec::new(),
        };
        let Drv::Reg(ri) = flat.sigs[sig as usize].drv[0] else { return Ok(false) };
        let r = &flat.regs[ri as usize];
        let ce = self.chosen(r.d, &|l| l == load, &|l| is_zero(flat, l) || is_whole_read(flat, l, sig))?;
        let rst = self.chosen(r.d, &|l| is_zero(flat, l) && l != load, &|l| l == load || is_whole_read(flat, l, sig))?;
        let ((cn, cb), _) = clock_shape(flat, r.clk);
        let clk = self.bit(cn, cb)?;
        // P is the register: its bits are the register's own literals.
        let p: Vec<Lit> = (0..flat.sigs[sig as usize].w).map(|i| self.read_bit(sig, i)).collect::<Result<_, _>>()?;
        self.macs.push(MacInf { a, b, c, signed, p, reg: Some((clk, ce, rst)) });
        self.mac_reg_done.insert(sig, true);
        Ok(true)
    }

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
            Drv::Reg(_) if self.ram_of_q.contains_key(&sig) => {
                // The register is a read port of a RAM.
                let (k, p) = self.ram_of_q[&sig];
                self.ram_port_out(k, p)[bit as usize]
            }
            Drv::Reg(_) if self.ram_of_mem.contains_key(&sig) => {
                // Something besides the RAM's read port reads the memory.
                return Err(format!("{RETRY}{sig}"));
            }
            Drv::Reg(_) => {
                let l = self.aig.input();
                self.pi_name.push((l, sig, bit));
                self.live_regs.push((sig, bit));
                // A flop that powers up at 1 stores the inverted value.
                if s.init[bit as usize] { l ^ 1 } else { l }
            }
            Drv::None if self.ram_of_mem.contains_key(&sig) => {
                // Something besides the ROM's read port reads the memory.
                return Err(format!("{RETRY}{sig}"));
            }
            Drv::None if s.has_init => {
                if s.init[bit as usize] {
                    TRUE
                } else {
                    FALSE
                }
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
        if self.hard && matches!(self.flat.dag.op(n), Op::Add(..)) {
            if let Some(v) = self.fused_add(n)? {
                self.memo.insert((n, i), v[i as usize]);
                return Ok(v[i as usize]);
            }
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
                    // On multiplier blocks.
                    if self.hard {
                        if let Some(p) = self.mul_on_blocks(&x, &y, w) {
                            let l = p[i as usize];
                            self.whole.insert(n, p);
                            self.memo.insert((n, i), l);
                            return Ok(l);
                        }
                    }
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
    // A memory that turns out to be read elsewhere is built from flops.
    let mut excluded = std::collections::HashSet::new();
    loop {
        match lower_with(flat, &excluded) {
            Err(e) if e.starts_with(RETRY) => {
                let sig: u32 = e[RETRY.len()..].parse().map_err(|_| "internal: bad retry".to_string())?;
                excluded.insert(sig);
            }
            r => return r,
        }
    }
}

fn lower_with(flat: &Flat, excluded: &std::collections::HashSet<u32>) -> Result<(Design, Stats, Vec<String>), String> {
    let hard = super::hard_blocks();
    let rams = if hard { infer_rams(flat, excluded) } else { Vec::new() };
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
        hard,
        macs: Vec::new(),
        ram_of_q: rams.iter().enumerate().flat_map(|(k, r)| r.ports.iter().enumerate().map(move |(p, port)| (port.q, (k, p)))).collect(),
        ram_of_mem: rams.iter().enumerate().map(|(k, r)| (r.mem, k)).collect(),
        rams,
        mac_regs: if hard { infer_mac_regs(flat) } else { HashMap::new() },
        mac_reg_done: HashMap::new(),
        fused_tried: Default::default(),
        extra_ffs: Vec::new(),
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
    }
    let mut reg_bits: Vec<RegBit> = Vec::new();
    let mut done = 0usize;
    loop {
        // RAMs whose output is used need their pins, which may use more
        // registers.
        if done >= bl.live_regs.len() {
            let pending = (0..bl.rams.len()).flat_map(|k| (0..bl.rams[k].ports.len()).map(move |p| (k, p))).find(|&(k, p)| bl.rams[k].ports[p].dout.is_some() && bl.rams[k].ports[p].pins.is_none());
            let Some((k, p)) = pending else { break };
            bl.ram_pins(k, p)?;
            continue;
        }
        let (sig, bit) = bl.live_regs[done];
        done += 1;
        // A register on a multiplier's P register is no flop.
        if bl.mac_regs.contains_key(&sig) && bl.decide_mac_reg(sig)? {
            continue;
        }
        let Drv::Reg(ri) = flat.sigs[sig as usize].drv[bit as usize] else { unreachable!() };
        let r = &flat.regs[ri as usize];
        let mut d = bl.bit(r.d, bit)?;
        if flat.sigs[sig as usize].init[bit as usize] {
            d ^= 1;
        }
        let ((cn, cb), enables) = clock_shape(flat, r.clk);
        if !enables.is_empty() {
            // Gated clock: keep the stored value unless every gate is open.
            let q = bl.sig_bit[&(sig, bit)] ^ (flat.sigs[sig as usize].init[bit as usize] as u32);
            let mut en = TRUE;
            for (en_n, en_b) in enables {
                let e = bl.bit(en_n, en_b)?;
                en = bl.aig.and(en, e);
            }
            d = bl.aig.mux(en, d, q);
        }
        let clk = bl.bit(cn, cb)?;
        let arst = match r.arst {
            Some((n, level)) if r.arst_mask[bit as usize] => {
                let l = bl.bit(n, 0)?;
                // An inverted literal is the same signal with the other level.
                Some((l & !1, level ^ (l & 1 == 1)))
            }
            _ => None,
        };
        reg_bits.push(RegBit { sig, bit, d, clk, negedge: r.negedge, arst });
    }
    let Blaster { aig, pi_name, hints, undriven, macs, rams, extra_ffs, .. } = bl;
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
        let mut n = bit_name(&s.name, s.w, bit);
        if matches!(s.drv[bit as usize], Drv::Reg(_)) && s.init[bit as usize] {
            // This flop stores the complement; do not call that net by the signal's name.
            n.push_str("_n");
        }
        taken.insert(n.clone());
        net_of.insert(l, n);
    }
    // Names the design itself gives nets and flops: registers (read or
    // not, with and without the complement's `_n`) and ports. A net made
    // up for a hard block takes none of them (nor, for a flop, a cell
    // name `<name>_reg` of a register).
    let mut fixed: std::collections::HashSet<String> = taken.clone();
    for rb in &reg_bits {
        let s = &flat.sigs[rb.sig as usize];
        let n = bit_name(&s.name, s.w, rb.bit);
        fixed.insert(format!("{n}_n"));
        fixed.insert(n);
    }
    for p in &flat.ports {
        fixed.insert(p.name.clone());
        for i in 0..p.w {
            fixed.insert(bit_name(&p.name, p.w, i));
        }
    }
    let made_up = |base: String, taken: &mut std::collections::HashSet<String>| -> String {
        let mut n = base.clone();
        let mut k = 1;
        while fixed.contains(&n) || taken.contains(&n) {
            n = format!("{base}_{k}");
            k += 1;
        }
        taken.insert(n.clone());
        n
    };
    // Outputs of hard blocks: a RAM's data out carries the name of the
    // register it replaces (with several depth banks, each bank's has the
    // bank's number, and the register is the bank chosen by the select).
    let mut ram_hint: Vec<(Lit, String)> = Vec::new();
    for r in &rams {
        for port in &r.ports {
            let Some(dout) = &port.dout else { continue };
            let s = &flat.sigs[port.q as usize];
            for (bk, bank) in port.bank_do.iter().enumerate() {
                for (b, &l) in bank.iter().enumerate() {
                    let n = if port.bank_do.len() == 1 {
                        // The register's own name: it is not a flop any more.
                        let n = bit_name(&s.name, s.w, b as u32);
                        taken.insert(n.clone());
                        n
                    } else {
                        made_up(format!("{}_bank{bk}_do{b}", s.name), &mut taken)
                    };
                    net_of.insert(l, n);
                }
            }
            if port.bank_do.len() > 1 {
                for (b, &l) in dout.iter().enumerate() {
                    ram_hint.push((l, bit_name(&s.name, s.w, b as u32)));
                }
            }
        }
    }
    let mut extra_ffs = extra_ffs;
    for (q, _, _, name) in extra_ffs.iter_mut() {
        *name = made_up(std::mem::take(name), &mut taken);
        net_of.insert(*q, name.clone());
    }
    for (k, m) in macs.iter().enumerate() {
        if m.reg.is_some() {
            // P is the register, already named.
            continue;
        }
        for (b, &l) in m.p.iter().enumerate() {
            let n = made_up(format!("u_mul{k}_p{b}"), &mut taken);
            net_of.insert(l, n);
        }
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
    for (l, n) in ram_hint {
        if l > TRUE && !net_of.contains_key(&l) {
            hint.entry(l).or_insert(n);
        }
    }
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
    // Multipliers.
    for (k, m) in macs.iter().enumerate() {
        let cell = format!("u_mul{k}");
        let mut pins: Vec<(String, String)> = Vec::new();
        for (name, lits) in [("A", &m.a), ("B", &m.b), ("C", &m.c)] {
            for (i, &l) in lits.iter().enumerate() {
                if l != FALSE {
                    pins.push((emit(l, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut), format!("{name}{i}")));
                }
            }
        }
        if let Some((clk, ce, rst)) = m.reg {
            // An unconnected CE reads 1, an unconnected RST 0.
            pins.push((emit(clk, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut), "CLK".into()));
            if ce != TRUE {
                pins.push((emit(ce, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut), "CE".into()));
            }
            if rst != FALSE {
                pins.push((emit(rst, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut), "RST".into()));
            }
        }
        d.add_cell(&cell, CellKind::Mac27);
        if m.signed {
            d.set_cell_attr(&cell, "SIGNED", "1")?;
        }
        if m.reg.is_some() {
            d.set_cell_attr(&cell, "PREG", "1")?;
        }
        for (net, pin) in pins {
            d.connect(&net, &cell, &pin);
        }
        for (i, l) in m.p.iter().enumerate() {
            d.connect(&net_of[l], &cell, &format!("P{i}"));
        }
    }
    // Block RAMs: per read port, per depth bank, per 16-bit slice.
    for r in &rams {
        let Some((waddr, di, bank_we, clk)) = &r.wpins else { continue };
        let sm = &flat.sigs[r.mem as usize];
        let g = r.geo;
        let total = r.ports.iter().filter(|p| p.dout.is_some()).count() as u32 * r.banks() * r.slices();
        // Contents: word `a` sits at position `hi - a` when declared ascending.
        // Bits `16 * slice ..` of word `a`, built bit by bit (a word may be
        // wider than any integer).
        let slice_bits = |a: u32, slice: u32| -> u64 {
            let pos = if g.asc { g.hi as u32 - a } else { a };
            (0..16u32).filter(|k| 16 * slice + k < g.ew).fold(0u64, |v, k| v | ((sm.init[(pos * g.ew + 16 * slice + k) as usize] as u64) << k))
        };
        let words = g.count.min(1024);
        for (pi, port) in r.ports.iter().enumerate() {
            let (Some(_), Some((raddr, re))) = (&port.dout, &port.pins) else { continue };
            for bank in 0..r.banks() {
                for slice in 0..r.slices() {
                    let cell = if total == 1 { format!("{}_ram", sm.name) } else { format!("{}_ram_r{pi}_b{bank}_s{slice}", sm.name) };
                    let mut pins: Vec<(String, String)> = Vec::new();
                    let mut pin = |l: Lit, name: String, skip: Lit, d: &mut Design| {
                        if l != skip {
                            pins.push((emit(l, &aig, &map, d, &mut net_of, &hint, &mut taken, &mut n_lut), name));
                        }
                    };
                    for (i, &l) in waddr.iter().enumerate() {
                        pin(l, format!("WADDR{i}"), FALSE, &mut d);
                    }
                    for (i, &l) in raddr.iter().enumerate() {
                        pin(l, format!("RADDR{i}"), FALSE, &mut d);
                    }
                    let bits = 16 * slice..(16 * slice + 16).min(g.ew);
                    for (i, k) in bits.clone().enumerate() {
                        if let Some(&l) = di.get(k as usize) {
                            pin(l, format!("DI{i}"), FALSE, &mut d);
                        }
                    }
                    // An unconnected WE reads 0, an unconnected RE reads 1.
                    pin(bank_we[bank as usize], "WE".into(), FALSE, &mut d);
                    pin(*re, "RE".into(), TRUE, &mut d);
                    pin(*clk, "CLK".into(), u32::MAX, &mut d);
                    d.add_cell(&cell, CellKind::Bram18);
                    let base = bank * 1024;
                    let slice_word = |a: u32| slice_bits(base + a, slice);
                    let mut init: Vec<u64> = (0..words.div_ceil(4)).map(|w| (0..4).fold(0u64, |v, k| if 4 * w + k < words { v | slice_word(4 * w + k) << (16 * k) } else { v })).collect();
                    while init.last() == Some(&0) {
                        init.pop();
                    }
                    if !init.is_empty() {
                        d.set_cell_attr(&cell, "INIT", init.iter().map(|w| format!("{w:x}")).collect::<Vec<_>>().join(","))?;
                    }
                    for (net, pin) in pins {
                        d.connect(&net, &cell, &pin);
                    }
                    for (i, k) in bits.enumerate() {
                        d.connect(&net_of[&port.bank_do[bank as usize][k as usize]], &cell, &format!("DO{i}"));
                    }
                }
            }
        }
    }
    // Flops the blocks need (a deep RAM's bank select).
    for (q, dl, clk, name) in &extra_ffs {
        let dnet = emit(*dl, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
        let cnet = emit(*clk, &aig, &map, &mut d, &mut net_of, &hint, &mut taken, &mut n_lut);
        let cell = format!("{name}_reg");
        d.add_cell(&cell, CellKind::Hff);
        d.connect(&dnet, &cell, "D");
        d.connect(&net_of[q], &cell, "Q");
        d.connect(&cnet, &cell, "CLK");
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
    let stats = Stats { regmap, luts: n_lut, ffs: reg_bits.len() + extra_ffs.len(), depth, ands: aig.nodes.iter().filter(|n| n.0 != u32::MAX).count() };
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
                SigInfo { name: "a".into(), hier: "a".into(), index: None, w: 3, drv: vec![Drv::Input; 3], init: vec![false; 3], has_init: false, debug: false },
                SigInfo { name: "b".into(), hier: "b".into(), index: None, w: 3, drv: vec![Drv::Input; 3], init: vec![false; 3], has_init: false, debug: false },
            ],
            ports: vec![
                Port { name: "a".into(), dir: PortDir::In, w: 3, node: None, sig: Some(0) },
                Port { name: "b".into(), dir: PortDir::In, w: 3, node: None, sig: Some(1) },
                Port { name: "s".into(), dir: PortDir::Out, w: 3, node: Some(s), sig: None },
            ],
            regs: vec![],
            latches: vec![],
            instances: vec![],
            mem_reads: vec![],
            mem_writes: vec![],
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

