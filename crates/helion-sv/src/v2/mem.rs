//! Hard blocks for the Xilinx 7-series back end (off unless [`super::set_hard`]):
//! memories become `RAMB18E1` / `RAMB36E1` cells, and instances of device
//! primitives (`DSP48E1`, `MMCME2_ADV`, ...) pass through as cells.
//!
//! A memory is an unpacked array with one dimension. It goes to block RAM
//! when every use is `mem[addr]`, it is written only in clocked processes
//! (whole elements or whole bytes of one), and each read is either loaded
//! straight into a register (`q <= mem[a]`, optionally under an enable) or
//! taken through an address register (`assign q = mem[a_q]`). Anything
//! else, including a read with no register in front of it, is built from
//! flops as before: elaboration is rerun with that memory excluded.
//!
//! Semantics kept exactly: a registered read sees the contents before the
//! clock edge (`READ_FIRST`); a read through an address register sees the
//! write of the same edge when it shares the write's address
//! (`WRITE_FIRST`), and a memory with such a read may have no other write.
//! Initial contents come from declarations, `initial` blocks and
//! `$readmemh` / `$readmemb`.

use super::ast::*;
use super::dag::{bits_for_u, Op, N};
use super::elab::{Elab, Sig, Ty, R};
use super::exec::{Env, LStep, Lv, Mode, Root};
use super::lower::{Drv, Hard, Reg};
use std::collections::{HashMap, HashSet};

/// Marks an error that asks for a rerun with one memory built from flops.
pub const RETRY: &str = "\u{1}flops:";

/// Memories smaller than this stay in flops.
const MIN_BITS: u32 = 1024;

pub struct WPort {
    proc: u32,
    clk: N,
    neg: bool,
    addr: N,
    /// Hidden signals: write data (element wide) and byte enables.
    wd: u32,
    we: u32,
}

pub struct RPort {
    /// Clocked process of a registered read; `None` outside one.
    proc: Option<(u32, N, bool)>,
    addr: N,
    /// Hidden signal standing for the value read.
    r: u32,
}

pub struct Mem {
    sig: u32,
    depth: u32,
    ew: u32,
    lo: i64,
    w: Vec<WPort>,
    r: Vec<RPort>,
}

#[derive(Default)]
pub struct MemState {
    pub on: bool,
    /// Memories to build from flops (flattened names).
    pub no_bram: HashSet<String>,
    mems: Vec<Mem>,
    of_sig: HashMap<u32, usize>,
    hidden: HashSet<u32>,
    /// Hidden write data: its value where nothing is written does not matter.
    dont_care: HashSet<u32>,
    /// The clocked process being run: (number, clock, falling edge).
    pub proc: Option<(u32, N, bool)>,
    nproc: u32,
    /// (memory, process) pairs with a blocking write.
    blocking: HashSet<(usize, u32)>,
    /// Set when an `initial` block took a branch that depends on a value.
    pub init_branch: bool,
}

/// A port of the block RAM as built.
struct LPort {
    clk: N,
    addr: N,
    write: Option<usize>,
    read: Option<Out>,
    write_first: bool,
}

enum Out {
    /// Registered read into register signal `q`, loading when `en`.
    Reg { q: u32, en: N },
    /// Read through an address register: hidden signal `r` shows the data.
    Direct { r: u32, init_addr: u64 },
}

fn clog2(x: u64) -> u32 {
    bits_for_u(x.saturating_sub(1) as u128)
}

fn hex(bits: &[bool]) -> String {
    let w = bits.len();
    let digits = w.div_ceil(4).max(1);
    let mut s = String::with_capacity(digits);
    for d in (0..digits).rev() {
        let mut v = 0;
        for b in 0..4 {
            if bits.get(d * 4 + b).copied().unwrap_or(false) {
                v |= 1 << b;
            }
        }
        s.push(char::from_digit(v, 16).unwrap().to_ascii_uppercase());
    }
    format!("{w}'h{s}")
}

impl Elab {
    pub fn next_proc(&mut self, clocked: Option<(N, bool)>) {
        self.mem.nproc += 1;
        self.mem.proc = clocked.map(|(c, n)| (self.mem.nproc, c, n));
    }

    pub fn is_hidden(&self, sig: u32) -> bool {
        self.mem.hidden.contains(&sig)
    }

    pub fn mem_of(&self, sig: u32) -> Option<usize> {
        self.mem.of_sig.get(&sig).copied()
    }

    pub fn mem_sig(&self, m: usize) -> u32 {
        self.mem.mems[m].sig
    }

    /// The memory an indexed name refers to, if it is one.
    pub fn mem_base(&mut self, b: &Expr, sc: usize) -> R<Option<usize>> {
        if self.mem.of_sig.is_empty() {
            return Ok(None);
        }
        let Expr::Id(path) = b else { return Ok(None) };
        Ok(match self.lookup_path(sc, path)? {
            super::elab::Sym::Sig(s) => self.mem_of(s),
            _ => None,
        })
    }

    pub fn retry<X>(&self, m: usize, why: &str) -> R<X> {
        Err(format!("{RETRY}{}\u{1}{why}", self.sigs[self.mem.mems[m].sig as usize].name))
    }

    fn hidden_sig(&mut self, name: String, w: u32) -> u32 {
        self.sigs.push(Sig { debug: false, hier: name.clone(), name, ty: Ty::bits(w, false), drv: vec![Drv::None; w as usize], init: vec![false; w as usize], line: self.line });
        let id = self.sigs.len() as u32 - 1;
        self.mem.hidden.insert(id);
        id
    }

    /// Called for every declared signal: a large one-dimensional unpacked
    /// array becomes a memory candidate.
    pub fn mem_candidate(&mut self, sig: u32) {
        if !self.mem.on || self.mem.no_bram.contains(&self.sigs[sig as usize].name) {
            return;
        }
        let Ty::Array { elem, lo, hi, packed: false, .. } = &*self.sigs[sig as usize].ty else { return };
        if matches!(&**elem, Ty::Array { packed: false, .. }) || self.sigs[sig as usize].ty.w() < MIN_BITS || hi == lo {
            return;
        }
        let m = Mem { sig, depth: (hi - lo + 1) as u32, ew: elem.w(), lo: *lo, w: Vec::new(), r: Vec::new() };
        self.mem.of_sig.insert(sig, self.mem.mems.len());
        self.mem.mems.push(m);
    }

    /// Element number (0 for the lowest index) of index value `i`.
    pub fn mem_addr(&mut self, m: usize, i: N, signed: bool) -> N {
        let lo = self.mem.mems[m].lo;
        if lo == 0 && !signed {
            return i;
        }
        let w = self.dag.w(i).max(bits_for_u(lo.unsigned_abs() as u128)) + 2;
        let x = self.dag.ext(i, w, signed);
        let k = self.kint(lo, w);
        self.dag.sub(x, k)
    }

    /// `mem[i]` as a value.
    pub fn mem_read(&mut self, m: usize, i: &Expr, sc: usize, env: &mut Env) -> R<N> {
        let iv = self.eval_self(i, sc, env)?;
        let addr = self.mem_addr(m, iv.n, iv.ty.signed());
        let (sig, ew, depth) = (self.mem.mems[m].sig, self.mem.mems[m].ew, self.mem.mems[m].depth);
        if env.mode == Mode::Init {
            let Some(a) = self.dag.const_u128(addr).filter(|a| *a < depth as u128) else { return self.err("initial block: memory index is not constant or out of range") };
            let off = self.mem_bit(m, a as u32) as usize;
            let bits = self.sigs[sig as usize].init[off..off + ew as usize].to_vec();
            return Ok(self.dag.konst(bits));
        }
        let proc = if env.mode == Mode::Seq { self.mem.proc } else { None };
        if let Some((p, ..)) = proc {
            if self.mem.blocking.contains(&(m, p)) {
                return self.retry(m, "it is read after a blocking write in the same block");
            }
        }
        let key = proc.map(|p| p.0);
        let r = match self.mem.mems[m].r.iter().find(|r| r.proc.map(|p| p.0) == key && r.addr == addr) {
            Some(r) => r.r,
            None => {
                let k = self.mem.mems[m].r.len();
                let name = format!("{}__rd{k}", self.sigs[sig as usize].name);
                let r = self.hidden_sig(name, ew);
                self.mem.mems[m].r.push(RPort { proc, addr, r });
                r
            }
        };
        Ok(self.dag.read(r, ew))
    }

    /// Bit offset of element `a` in the memory signal.
    fn mem_bit(&self, m: usize, a: u32) -> u32 {
        let mm = &self.mem.mems[m];
        self.sigs[mm.sig as usize].ty.offset(mm.lo + a as i64).unwrap()
    }

    /// Write `v` through `lv` (rooted at memory `m`, first step the element).
    pub(super) fn mem_write(&mut self, m: usize, lv: &Lv, v: N, nonblocking: bool, env: &mut Env) -> R<()> {
        let ew = self.mem.mems[m].ew;
        let LStep::Elem { idx: addr, .. } = lv.steps[0] else { return self.retry(m, "an element is written without an index") };
        let (mut lo, mut w) = (0, ew);
        for s in &lv.steps[1..] {
            let LStep::Fixed(l, sw) = s else { return self.retry(m, "a variable part of an element is written") };
            lo += l;
            w = *sw;
        }
        if env.mode == Mode::Init {
            let Some(a) = self.dag.const_u128(addr).filter(|a| *a < self.mem.mems[m].depth as u128) else {
                return self.err("initial block: memory index is not constant or out of range");
            };
            let Some(bits) = self.dag.as_const(v).map(|b| b.to_vec()) else { return self.err("initial block: the value is not constant") };
            let off = (self.mem_bit(m, a as u32) + lo) as usize;
            let sig = self.mem.mems[m].sig as usize;
            self.sigs[sig].init[off..off + w as usize].copy_from_slice(&bits);
            return Ok(());
        }
        let Some((p, clk, neg)) = self.mem.proc.filter(|_| env.mode == Mode::Seq) else {
            return self.retry(m, "it is written outside a clocked process");
        };
        if !(lo == 0 && w == ew) && !(lo % 8 == 0 && (w % 8 == 0 || lo + w == ew)) {
            return self.retry(m, "part of an element that is not whole bytes is written");
        }
        if !nonblocking {
            self.mem.blocking.insert((m, p));
        }
        let nb = ew.div_ceil(8);
        let k = match self.mem.mems[m].w.iter().position(|x| x.proc == p && x.addr == addr) {
            Some(k) => k,
            None => {
                let k = self.mem.mems[m].w.len();
                let name = self.sigs[self.mem.mems[m].sig as usize].name.clone();
                let wd = self.hidden_sig(format!("{name}__wd{k}"), ew);
                self.mem.dont_care.insert(wd);
                let we = self.hidden_sig(format!("{name}__we{k}"), nb);
                self.mem.mems[m].w.push(WPort { proc: p, clk, neg, addr, wd, we });
                k
            }
        };
        let (wd, we) = (self.mem.mems[m].w[k].wd, self.mem.mems[m].w[k].we);
        let lv_d = Lv { root: Root::Sig(wd), steps: vec![LStep::Fixed(lo, w)], ty: Ty::bits(w, false) };
        self.store_lv(&lv_d, v, true, env)?;
        let (b0, b1) = (lo / 8, (lo + w).div_ceil(8));
        let ones = self.dag.ones(b1 - b0);
        let lv_e = Lv { root: Root::Sig(we), steps: vec![LStep::Fixed(b0, b1 - b0)], ty: Ty::bits(b1 - b0, false) };
        self.store_lv(&lv_e, ones, true, env)
    }

    /// `initial` block: power-up values. Only constant code is allowed.
    pub fn initial(&mut self, body: &Stmt, sc: usize) -> R<()> {
        self.mem.init_branch = false;
        let mut env = Env { mode: Mode::Init, ..Default::default() };
        self.exec(body, sc, &mut env)?;
        if self.mem.init_branch {
            return self.err("initial block: a branch depends on a value that is not constant");
        }
        Ok(())
    }

    /// Store into a signal's power-up value (`initial` blocks).
    pub(super) fn init_store(&mut self, id: u32, lv: &Lv, v: N, env: &mut Env) -> R<()> {
        if let Some(m) = self.mem_of(id) {
            return self.mem_write(m, lv, v, false, env);
        }
        let mut off = 0;
        let mut w = self.sigs[id as usize].ty.w();
        for s in &lv.steps {
            let LStep::Fixed(l, sw) = s else { return self.err("initial block: index is not constant") };
            off += l;
            w = *sw;
        }
        let Some(bits) = self.dag.as_const(v).map(|b| b.to_vec()) else { return self.err("initial block: the value is not constant") };
        self.sigs[id as usize].init[off as usize..(off + w) as usize].copy_from_slice(&bits[..w as usize]);
        Ok(())
    }

    /// `$readmemh(file, mem [, start [, end]])` / `$readmemb`.
    pub fn readmem(&mut self, hexa: bool, args: &[Expr], sc: usize, env: &mut Env) -> R<()> {
        if env.mode != Mode::Init {
            return self.err("$readmemh / $readmemb is only supported in an initial block");
        }
        let (Some(Expr::Str(file)), Some(Expr::Id(path))) = (args.first(), args.get(1)) else {
            return self.err("$readmem needs a file name and a memory");
        };
        let super::elab::Sym::Sig(sig) = self.lookup_path(sc, path)? else { return self.err("$readmem needs a memory") };
        let ty = self.sigs[sig as usize].ty.clone();
        let Ty::Array { elem, lo, hi, asc, .. } = &*ty else { return self.err("$readmem needs an unpacked array") };
        let ew = elem.w() as usize;
        let _ = (hi, asc);
        // From the lowest address up, unless the range given runs down.
        let mut addr = match args.get(2) {
            Some(e) => self.const_int(e, sc)?,
            None => *lo,
        };
        let stop = match args.get(3) {
            Some(e) => Some(self.const_int(e, sc)?),
            None => None,
        };
        let step: i64 = if stop.map(|s| s < addr).unwrap_or(false) { -1 } else { 1 };
        let text = self.read_file(file)?;
        let mut words: Vec<&str> = Vec::new();
        for line in text.lines() {
            let line = line.split("//").next().unwrap_or("");
            words.extend(line.split_whitespace());
        }
        let radix = if hexa { 16 } else { 2 };
        for w in words {
            if let Some(a) = w.strip_prefix('@') {
                addr = i64::from_str_radix(a, 16).map_err(|_| format!("line {}: $readmem: bad address '{w}' in {file}", self.line))?;
                continue;
            }
            if stop.map(|s| (step > 0 && addr > s) || (step < 0 && addr < s)).unwrap_or(false) {
                break;
            }
            let Some(off) = ty.offset(addr) else {
                return self.err(format!("$readmem: address {addr} in {file} is outside the memory"));
            };
            let mut bits = Vec::with_capacity(ew);
            for c in w.chars().rev().filter(|c| *c != '_') {
                let d = c.to_digit(radix).unwrap_or(0); // x and z read as 0
                if !c.is_digit(radix) && !matches!(c, 'x' | 'X' | 'z' | 'Z') {
                    return self.err(format!("$readmem: bad value '{w}' in {file}"));
                }
                for b in 0..(if hexa { 4 } else { 1 }) {
                    bits.push((d >> b) & 1 == 1);
                }
            }
            bits.resize(ew, false);
            self.sigs[sig as usize].init[off as usize..off as usize + ew].copy_from_slice(&bits);
            addr += step;
        }
        Ok(())
    }

    fn read_file(&self, file: &str) -> R<String> {
        let mut tried = vec![std::path::PathBuf::from(file)];
        for d in &self.src_dirs {
            tried.push(d.join(file));
        }
        for p in &tried {
            if let Ok(t) = std::fs::read_to_string(p) {
                return Ok(t);
            }
        }
        self.err(format!("$readmem: cannot read '{file}' (looked in the working directory and next to the sources)"))
    }

    /// After a clocked process: the hidden write-port signals it assigned
    /// are its combinational outputs (0 where not assigned).
    pub fn hidden_comb(&mut self, id: u32, d: N) -> R<()> {
        let w = self.sigs[id as usize].ty.w();
        let mut d = d;
        if self.mem.dont_care.contains(&id) {
            // Where nothing is written the data is free: drop those muxes.
            let hold = self.dag.read(id, w);
            while let Op::Mux(_, a, b) = *self.dag.op(d) {
                if a == hold {
                    d = b;
                } else if b == hold {
                    d = a;
                } else {
                    break;
                }
            }
        }
        let mut memo = HashMap::new();
        let mut fix = |g: &mut super::dag::Dag, n: N| -> Option<N> { (*g.op(n) == Op::Read(id)).then(|| g.zero(w)) };
        let v = self.dag.subst(d, &mut fix, &mut memo);
        for b in 0..w {
            self.drive(id, b, Drv::Comb(v, b))?;
        }
        Ok(())
    }

    /// Build every memory candidate as block RAM.
    pub fn build_mems(&mut self) -> R<()> {
        let mems = std::mem::take(&mut self.mem.mems);
        for (m, mem) in mems.iter().enumerate() {
            if self.dag.nodes.iter().any(|n| matches!(n.op, Op::Read(s) | Op::Hold(s) if s == mem.sig)) {
                self.mem.mems = mems;
                return self.retry(m, "it is used other than as mem[index]");
            }
        }
        self.mem.mems = mems;
        // Which hidden read signals each node depends on (nodes are built
        // after their operands, so one pass in order covers the graph).
        let rsigs: HashSet<u32> = self.mem.mems.iter().flat_map(|m| m.r.iter().map(|r| r.r)).collect();
        let mut reads: Vec<Vec<u32>> = Vec::with_capacity(self.dag.nodes.len());
        for n in 0..self.dag.nodes.len() as N {
            let mut v: Vec<u32> = match self.dag.op(n) {
                Op::Read(s) if rsigs.contains(s) => vec![*s],
                _ => Vec::new(),
            };
            for k in self.dag.kids(n) {
                for &s in &reads[k as usize] {
                    if !v.contains(&s) {
                        v.push(s);
                    }
                }
            }
            reads.push(v);
        }
        let mut users: HashMap<u32, Vec<Option<usize>>> = HashMap::new();
        let note = |n: N, who: Option<usize>, users: &mut HashMap<u32, Vec<Option<usize>>>| {
            for &s in &reads[n as usize] {
                users.entry(s).or_default().push(who);
            }
        };
        for (ri, r) in self.regs.iter().enumerate() {
            note(r.d, Some(ri), &mut users);
            note(r.clk, None, &mut users);
        }
        let mut seen = HashSet::new();
        for s in &self.sigs {
            for d in &s.drv {
                if let Drv::Comb(n, _) = d {
                    if seen.insert(*n) {
                        note(*n, None, &mut users);
                    }
                }
            }
        }
        for l in &self.latches {
            note(l.d, None, &mut users);
            note(l.gate, None, &mut users);
        }
        for h in &self.hard {
            for (_, n, _) in &h.ins {
                if seen.insert(*n) {
                    note(*n, None, &mut users);
                }
            }
        }
        for m in 0..self.mem.mems.len() {
            self.build_mem(m, &users)?;
        }
        Ok(())
    }

    fn build_mem(&mut self, m: usize, users: &HashMap<u32, Vec<Option<usize>>>) -> R<()> {
        let mem = &self.mem.mems[m];
        if mem.r.is_empty() {
            return Ok(()); // nothing reads it: no hardware
        }
        let (ew, depth) = (mem.ew, mem.depth);
        let nw = mem.w.len();
        let mut ports: Vec<LPort> = mem.w.iter().enumerate().map(|(k, w)| LPort { clk: w.clk, addr: w.addr, write: Some(k), read: None, write_first: false }).collect();
        if mem.w.iter().any(|w| w.neg) {
            return self.retry(m, "it is written on a falling clock edge");
        }
        let rports: Vec<(Option<(u32, N, bool)>, N, u32)> = mem.r.iter().map(|r| (r.proc, r.addr, r.r)).collect();
        for (proc, addr, r) in rports {
            match proc {
                Some((_, clk, neg)) => {
                    let us = users.get(&r).cloned().unwrap_or_default();
                    let (q, en) = match us.as_slice() {
                        [Some(ri)] => match self.load_of(*ri, r, ew) {
                            Some(x) => x,
                            None => return self.retry(m, "a value read is not loaded straight into a whole register"),
                        },
                        [] => continue,
                        _ => return self.retry(m, "a value read is used other than by loading one register"),
                    };
                    if neg {
                        return self.retry(m, "it is read on a falling clock edge");
                    }
                    let one = self.dag.as_const(en) == Some(&[true]);
                    let shared = ports.iter().position(|p| p.write.is_some() && p.read.is_none() && one && p.clk == clk && p.addr == addr);
                    match shared {
                        Some(k) => ports[k].read = Some(Out::Reg { q, en }),
                        None => ports.push(LPort { clk, addr, write: None, read: Some(Out::Reg { q, en }), write_first: false }),
                    }
                }
                None => {
                    // The address must be a register's output.
                    let Some((ri, s)) = self.addr_reg(addr) else {
                        return self.retry(m, "a read has no register in front of it (asynchronous read)");
                    };
                    let (rd, clk, neg) = (self.regs[ri].d, self.regs[ri].clk, self.regs[ri].negedge);
                    if neg {
                        return self.retry(m, "its read address register is clocked on the falling edge");
                    }
                    let w = self.sigs[s as usize].ty.w();
                    let mut memo = HashMap::new();
                    let mut fix = |g: &mut super::dag::Dag, n: N| -> Option<N> { (*g.op(n) == Op::Read(s)).then_some(rd) };
                    let next = self.dag.subst(addr, &mut fix, &mut memo);
                    let init = self.dag.konst(self.sigs[s as usize].init.clone());
                    let mut memo = HashMap::new();
                    let mut fix0 = |g: &mut super::dag::Dag, n: N| -> Option<N> { (*g.op(n) == Op::Read(s) && g.w(n) == w).then_some(init) };
                    let ia = self.dag.subst(addr, &mut fix0, &mut memo);
                    let init_addr = self.dag.const_u128(ia).unwrap_or(0) as u64;
                    let out = Out::Direct { r, init_addr };
                    if nw == 0 {
                        ports.push(LPort { clk, addr: next, write: None, read: Some(out), write_first: false });
                        continue;
                    }
                    let k = ports.iter().position(|p| p.read.is_none() && p.write.is_some() && p.clk == clk && p.addr == next);
                    match k {
                        Some(k) if nw == 1 => {
                            ports[k].read = Some(out);
                            ports[k].write_first = true;
                        }
                        _ => return self.retry(m, "a read through an address register does not share the address of its only write"),
                    }
                }
            }
        }
        if ports.iter().any(|p| p.write_first) && ports.len() > 1 {
            return self.retry(m, "a write-first port and a second port would collide");
        }
        if ports.len() > 2 {
            return self.retry(m, "it needs more than two ports");
        }
        if !ports.iter().any(|p| p.read.is_some()) {
            return Ok(());
        }
        // Shape: data width per RAM (no parity bits) and primitive.
        let mut best: Option<(u32, u32, bool, u32)> = None; // (cost, cells, ramb36, width)
        for (big, widths) in [(false, &[1u32, 2, 4, 8, 16][..]), (true, &[1u32, 2, 4, 8, 16, 32][..])] {
            for &w in widths {
                let words = if big { 32768 } else { 16384 } / w;
                let cells = ew.div_ceil(w) * depth.div_ceil(words);
                let cost = cells * if big { 2 } else { 1 };
                if best.map(|b| (cost, cells) < (b.0, b.1)).unwrap_or(true) {
                    best = Some((cost, cells, big, w));
                }
            }
        }
        let (_, _, big, bw) = best.unwrap();
        let words = if big { 32768 } else { 16384 } / bw;
        let lw = clog2(words as u64);
        let ncol = ew.div_ceil(bw);
        let nrow = depth.div_ceil(words);
        let abits = clog2(depth as u64).max(1);
        let rbits = clog2(nrow as u64);
        let module = if big { "RAMB36E1" } else { "RAMB18E1" };
        let wcode = match bw {
            8 => 9,
            16 => 18,
            32 => 36,
            w => w,
        };
        let (n_addr, n_we, init_w) = if big { (16, 4, 36) } else { (14, 2, 18) };
        // The word address sits at pins [low + lw - 1 : low].
        let low = clog2(bw as u64);
        let mem = &self.mem.mems[m];
        let (msig, mname) = (mem.sig, self.sigs[mem.sig as usize].name.clone());
        let wps: Vec<(N, u32, u32)> = mem.w.iter().map(|w| (w.addr, w.wd, w.we)).collect();
        let nb = ew.div_ceil(8);
        // Element bits at power-up.
        let elem_init = |e: &Elab, a: u64| -> Vec<bool> {
            if a >= depth as u64 {
                return vec![false; ew as usize];
            }
            let off = e.mem_bit(m, a as u32) as usize;
            e.sigs[msig as usize].init[off..off + ew as usize].to_vec()
        };
        let mut hard: Vec<Hard> = Vec::new();
        let mut outs: Vec<Vec<Vec<u32>>> = Vec::new(); // per port, row, column: hidden data-out signal
        let zero = self.dag.bit(false);
        let one = self.dag.bit(true);
        for row in 0..nrow {
            for col in 0..ncol {
                let name = format!("{mname}_bram{}", row * ncol + col);
                let mut attrs = vec![("RAM_MODE".to_string(), "TDP".to_string())];
                for x in ["READ_WIDTH_A", "READ_WIDTH_B", "WRITE_WIDTH_A", "WRITE_WIDTH_B"] {
                    attrs.push((x.into(), wcode.to_string()));
                }
                attrs.push(("DOA_REG".into(), "0".into()));
                attrs.push(("DOB_REG".into(), "0".into()));
                // Contents: word `a` of this RAM is element `row * words + a`.
                let mut data = vec![false; (words * bw) as usize];
                for a in 0..words {
                    let e = (row * words + a) as u64;
                    if e >= depth as u64 {
                        break;
                    }
                    let bits = elem_init(self, e);
                    for k in 0..bw {
                        let eb = col * bw + k;
                        if eb < ew {
                            data[(a * bw + k) as usize] = bits[eb as usize];
                        }
                    }
                }
                for (r, chunk) in data.chunks(256).enumerate() {
                    if chunk.iter().any(|b| *b) {
                        attrs.push((format!("INIT_{r:02X}"), hex(chunk)));
                    }
                }
                let mut ins: Vec<(String, N, u32)> = Vec::new();
                let mut hout: Vec<(String, u32, u32)> = Vec::new();
                for (pi, p) in ports.iter().enumerate() {
                    let (s, ab, en_pin, clk_pin, di, dout, we_pin) = if pi == 0 {
                        ("A", "ADDRARDADDR", "ENARDEN", "CLKARDCLK", "DIADI", "DOADO", "WEA")
                    } else {
                        ("B", "ADDRBWRADDR", "ENBWREN", "CLKBWRCLK", "DIBDI", "DOBDO", "WEBWE")
                    };
                    attrs.push((format!("WRITE_MODE_{s}"), if p.write_first { "WRITE_FIRST" } else { "READ_FIRST" }.into()));
                    ins.push((clk_pin.into(), p.clk, 0));
                    let en = match &p.read {
                        Some(Out::Reg { en, .. }) => *en,
                        _ => one,
                    };
                    ins.push((en_pin.into(), en, 0));
                    let ctl: [&str; 3] = if pi == 0 { ["RSTRAMARSTRAM", "RSTREGARSTREG", "REGCEAREGCE"] } else { ["RSTRAMB", "RSTREGB", "REGCEB"] };
                    for x in ctl {
                        ins.push((x.into(), zero, 0));
                    }
                    // Address: word address at bits [lw + low - 1 : low]; the rest 1.
                    let aw = self.dag.w(p.addr);
                    let addr = if aw < abits { self.dag.zext(p.addr, abits) } else { p.addr };
                    for k in 0..n_addr {
                        let pin = format!("{ab}{k}");
                        if k >= low && k < low + lw && k - low < abits {
                            ins.push((pin, addr, k - low));
                        } else if k >= low && k < low + lw {
                            ins.push((pin, zero, 0));
                        } else {
                            ins.push((pin, one, 0));
                        }
                    }
                    // Write: data and byte enables, gated to this row and to addresses in range.
                    if let Some(wk) = p.write {
                        let (waddr, wd, we) = wps[wk];
                        let waw = self.dag.w(waddr);
                        let mut ok = if waw <= abits && depth == 1 << abits { one } else {
                            let d = self.dag.kval(depth as u128, waw.max(abits + 1));
                            let a = self.dag.zext(waddr, waw.max(abits + 1));
                            self.dag.ult(a, d)
                        };
                        if nrow > 1 {
                            let rsel = self.dag.slice(addr, lw, rbits.min(abits - lw));
                            let rk = self.dag.kval(row as u128, self.dag.w(rsel));
                            let hit = self.dag.eq(rsel, rk);
                            ok = self.dag.and(ok, hit);
                        }
                        let wdn = self.dag.read(wd, ew);
                        let wen = self.dag.read(we, nb);
                        for k in 0..bw {
                            let eb = col * bw + k;
                            if eb < ew {
                                ins.push((format!("{di}{k}"), wdn, eb));
                            } else {
                                ins.push((format!("{di}{k}"), zero, 0));
                            }
                        }
                        for k in 0..n_we {
                            // Byte lane `k` of this RAM is element byte `col * bw / 8 + k`;
                            // a narrower RAM uses every enable for its one byte.
                            let byte = if bw >= 16 { (col * bw) / 8 + k } else { (col * bw) / 8 };
                            let lane = if bw >= 16 && k >= bw / 8 { zero } else if byte < nb {
                                let b = self.dag.slice(wen, byte, 1);
                                self.dag.and(b, ok)
                            } else {
                                zero
                            };
                            ins.push((format!("{we_pin}{k}"), lane, 0));
                        }
                    } else {
                        for k in 0..n_we {
                            ins.push((format!("{we_pin}{k}"), zero, 0));
                        }
                    }
                    if p.read.is_some() {
                        let w = bw.min(ew - col * bw);
                        let o = self.hidden_sig(format!("{name}_do{s}"), w);
                        for k in 0..w {
                            hout.push((format!("{dout}{k}"), o, k));
                            self.sigs[o as usize].drv[k as usize] = Drv::Hard(self.hard.len() as u32 + hard.len() as u32);
                        }
                        while outs.len() <= pi {
                            outs.push(Vec::new());
                        }
                        while outs[pi].len() <= row as usize {
                            outs[pi].push(Vec::new());
                        }
                        outs[pi][row as usize].push(o);
                        // Output latch at power-up: what the RTL shows before the first edge.
                        let init_bits = match &p.read {
                            Some(Out::Reg { q, .. }) => self.sigs[*q as usize].init.clone(),
                            Some(Out::Direct { init_addr, .. }) => elem_init(self, *init_addr),
                            None => unreachable!(),
                        };
                        let mut ib = vec![false; init_w];
                        for k in 0..w {
                            ib[k as usize] = init_bits[(col * bw + k) as usize];
                        }
                        attrs.push((format!("INIT_{s}"), hex(&ib)));
                    }
                }
                if ports.len() == 1 {
                    ins.push(("ENBWREN".into(), zero, 0));
                    attrs.push(("WRITE_MODE_B".into(), "READ_FIRST".into()));
                }
                hard.push(Hard { name, module: module.into(), attrs, ins, outs: hout });
            }
        }
        // Read data: the RAM of each row, chosen by the row of the address
        // the data was read from.
        for (pi, p) in ports.iter().enumerate() {
            let Some(out) = &p.read else { continue };
            let rows: Vec<N> = (0..nrow as usize)
                .map(|row| {
                    let parts: Vec<N> = outs[pi][row].iter().rev().map(|&o| self.dag.read(o, self.sigs[o as usize].ty.w())).collect();
                    self.dag.concat(parts)
                })
                .collect();
            let aw = self.dag.w(p.addr);
            let addr = if aw < abits { self.dag.zext(p.addr, abits) } else { p.addr };
            let (target, v) = match out {
                Out::Reg { q, en } => {
                    let v = if nrow > 1 {
                        let rsel = self.dag.slice(addr, lw, rbits.min(abits - lw));
                        let sw = self.dag.w(rsel);
                        let sel = self.hidden_sig(format!("{mname}_bramsel{pi}"), sw);
                        let held = self.dag.read(sel, sw);
                        let d = self.dag.mux(*en, rsel, held);
                        self.regs.push(Reg { sig: sel, d, clk: p.clk, negedge: false, mask: vec![true; sw as usize], arst: None, arst_mask: vec![false; sw as usize] });
                        let ri = self.regs.len() as u32 - 1;
                        for b in 0..sw {
                            self.sigs[sel as usize].drv[b as usize] = Drv::Reg(ri);
                        }
                        self.select(&rows, held)
                    } else {
                        rows[0]
                    };
                    (*q, v)
                }
                Out::Direct { r, .. } => {
                    // `p.addr` is the next address; the data belongs to the registered one.
                    let reg_addr = self.mem.mems[m].r.iter().find(|x| x.r == *r).unwrap().addr;
                    let v = if nrow > 1 {
                        let ra = if self.dag.w(reg_addr) < abits { self.dag.zext(reg_addr, abits) } else { reg_addr };
                        let rsel = self.dag.slice(ra, lw, rbits.min(abits - lw));
                        self.select(&rows, rsel)
                    } else {
                        rows[0]
                    };
                    (*r, v)
                }
            };
            for b in 0..ew {
                self.sigs[target as usize].drv[b as usize] = Drv::Comb(v, b);
            }
        }
        self.hard.extend(hard);
        Ok(())
    }

    /// Register `ri` loads hidden read value `r` whole, or holds: returns
    /// (register signal, load enable).
    fn load_of(&mut self, ri: usize, r: u32, ew: u32) -> Option<(u32, N)> {
        let reg = &self.regs[ri];
        let q = reg.sig;
        if reg.arst.is_some() || !reg.mask.iter().all(|m| *m) || self.sigs[q as usize].ty.w() != ew || self.is_hidden(q) {
            return None;
        }
        let d = reg.d;
        let mut memo: HashMap<N, Option<N>> = HashMap::new();
        fn en(e: &mut Elab, n: N, r: u32, q: u32, memo: &mut HashMap<N, Option<N>>) -> Option<N> {
            if let Some(x) = memo.get(&n) {
                return *x;
            }
            let x = match e.dag.op(n).clone() {
                Op::Read(s) if s == r => Some(e.dag.bit(true)),
                Op::Read(s) if s == q => Some(e.dag.bit(false)),
                Op::Mux(c, a, b) => match (en(e, a, r, q, memo), en(e, b, r, q, memo)) {
                    (Some(x), Some(y)) => Some(e.dag.mux(c, x, y)),
                    _ => None,
                },
                _ => None,
            };
            memo.insert(n, x);
            x
        }
        en(self, d, r, q, &mut memo).map(|e| (q, e))
    }

    /// The register whose output an address is built from: (register, signal).
    fn addr_reg(&self, addr: N) -> Option<(usize, u32)> {
        let mut leaves = Vec::new();
        let mut stack = vec![addr];
        let mut seen = HashSet::new();
        while let Some(n) = stack.pop() {
            if !seen.insert(n) {
                continue;
            }
            match self.dag.op(n) {
                Op::Read(s) => leaves.push(*s),
                Op::Const(_) => {}
                Op::Hold(_) | Op::VHold(_) => return None,
                _ => stack.extend(self.dag.kids(n)),
            }
        }
        leaves.dedup();
        let [s] = leaves.as_slice() else { return None };
        let drv = &self.sigs[*s as usize].drv;
        let Some(Drv::Reg(ri)) = drv.first() else { return None };
        let reg = &self.regs[*ri as usize];
        (drv.iter().all(|d| *d == Drv::Reg(*ri)) && reg.mask.iter().all(|m| *m) && reg.arst.is_none()).then_some((*ri as usize, *s))
    }
}

/// Ports of the device primitives that pass through: (name, input, width).
pub fn prim_ports(module: &str) -> Option<&'static [(&'static str, bool, u32)]> {
    const RAMB18: &[(&str, bool, u32)] = &[
        ("ADDRARDADDR", true, 14), ("ADDRBWRADDR", true, 14), ("CLKARDCLK", true, 1), ("CLKBWRCLK", true, 1), ("DIADI", true, 16), ("DIBDI", true, 16),
        ("DIPADIP", true, 2), ("DIPBDIP", true, 2), ("ENARDEN", true, 1), ("ENBWREN", true, 1), ("REGCEAREGCE", true, 1), ("REGCEB", true, 1),
        ("RSTRAMARSTRAM", true, 1), ("RSTRAMB", true, 1), ("RSTREGARSTREG", true, 1), ("RSTREGB", true, 1), ("WEA", true, 2), ("WEBWE", true, 4),
        ("DOADO", false, 16), ("DOBDO", false, 16), ("DOPADOP", false, 2), ("DOPBDOP", false, 2),
    ];
    const RAMB36: &[(&str, bool, u32)] = &[
        ("ADDRARDADDR", true, 16), ("ADDRBWRADDR", true, 16), ("CASCADEINA", true, 1), ("CASCADEINB", true, 1), ("CLKARDCLK", true, 1), ("CLKBWRCLK", true, 1),
        ("DIADI", true, 32), ("DIBDI", true, 32), ("DIPADIP", true, 4), ("DIPBDIP", true, 4), ("ENARDEN", true, 1), ("ENBWREN", true, 1),
        ("INJECTDBITERR", true, 1), ("INJECTSBITERR", true, 1), ("REGCEAREGCE", true, 1), ("REGCEB", true, 1), ("RSTRAMARSTRAM", true, 1), ("RSTRAMB", true, 1),
        ("RSTREGARSTREG", true, 1), ("RSTREGB", true, 1), ("WEA", true, 4), ("WEBWE", true, 8),
        ("CASCADEOUTA", false, 1), ("CASCADEOUTB", false, 1), ("DBITERR", false, 1), ("DOADO", false, 32), ("DOBDO", false, 32), ("DOPADOP", false, 4),
        ("DOPBDOP", false, 4), ("ECCPARITY", false, 8), ("RDADDRECC", false, 9), ("SBITERR", false, 1),
    ];
    const DSP: &[(&str, bool, u32)] = &[
        ("A", true, 30), ("ACIN", true, 30), ("ALUMODE", true, 4), ("B", true, 18), ("BCIN", true, 18), ("C", true, 48), ("CARRYCASCIN", true, 1),
        ("CARRYIN", true, 1), ("CARRYINSEL", true, 3), ("CEA1", true, 1), ("CEA2", true, 1), ("CEAD", true, 1), ("CEALUMODE", true, 1), ("CEB1", true, 1),
        ("CEB2", true, 1), ("CEC", true, 1), ("CECARRYIN", true, 1), ("CECTRL", true, 1), ("CED", true, 1), ("CEINMODE", true, 1), ("CEM", true, 1),
        ("CEP", true, 1), ("CLK", true, 1), ("D", true, 25), ("INMODE", true, 5), ("MULTSIGNIN", true, 1), ("OPMODE", true, 7), ("PCIN", true, 48),
        ("RSTA", true, 1), ("RSTALLCARRYIN", true, 1), ("RSTALUMODE", true, 1), ("RSTB", true, 1), ("RSTC", true, 1), ("RSTCTRL", true, 1),
        ("RSTD", true, 1), ("RSTINMODE", true, 1), ("RSTM", true, 1), ("RSTP", true, 1),
        ("ACOUT", false, 30), ("BCOUT", false, 18), ("CARRYCASCOUT", false, 1), ("CARRYOUT", false, 4), ("MULTSIGNOUT", false, 1), ("OVERFLOW", false, 1),
        ("P", false, 48), ("PATTERNBDETECT", false, 1), ("PATTERNDETECT", false, 1), ("PCOUT", false, 48), ("UNDERFLOW", false, 1),
    ];
    const MMCM_ADV: &[(&str, bool, u32)] = &[
        ("CLKFBIN", true, 1), ("CLKIN1", true, 1), ("CLKIN2", true, 1), ("CLKINSEL", true, 1), ("DADDR", true, 7), ("DCLK", true, 1), ("DEN", true, 1),
        ("DI", true, 16), ("DWE", true, 1), ("PSCLK", true, 1), ("PSEN", true, 1), ("PSINCDEC", true, 1), ("PWRDWN", true, 1), ("RST", true, 1),
        ("CLKFBOUT", false, 1), ("CLKFBOUTB", false, 1), ("CLKFBSTOPPED", false, 1), ("CLKINSTOPPED", false, 1), ("CLKOUT0", false, 1),
        ("CLKOUT0B", false, 1), ("CLKOUT1", false, 1), ("CLKOUT1B", false, 1), ("CLKOUT2", false, 1), ("CLKOUT2B", false, 1), ("CLKOUT3", false, 1),
        ("CLKOUT3B", false, 1), ("CLKOUT4", false, 1), ("CLKOUT5", false, 1), ("CLKOUT6", false, 1), ("DO", false, 16), ("DRDY", false, 1),
        ("LOCKED", false, 1), ("PSDONE", false, 1),
    ];
    const MMCM_BASE: &[(&str, bool, u32)] = &[
        ("CLKFBIN", true, 1), ("CLKIN1", true, 1), ("PWRDWN", true, 1), ("RST", true, 1), ("CLKFBOUT", false, 1), ("CLKFBOUTB", false, 1),
        ("CLKOUT0", false, 1), ("CLKOUT0B", false, 1), ("CLKOUT1", false, 1), ("CLKOUT1B", false, 1), ("CLKOUT2", false, 1), ("CLKOUT2B", false, 1),
        ("CLKOUT3", false, 1), ("CLKOUT3B", false, 1), ("CLKOUT4", false, 1), ("CLKOUT5", false, 1), ("CLKOUT6", false, 1), ("LOCKED", false, 1),
    ];
    const PLL_ADV: &[(&str, bool, u32)] = &[
        ("CLKFBIN", true, 1), ("CLKIN1", true, 1), ("CLKIN2", true, 1), ("CLKINSEL", true, 1), ("DADDR", true, 7), ("DCLK", true, 1), ("DEN", true, 1),
        ("DI", true, 16), ("DWE", true, 1), ("PWRDWN", true, 1), ("RST", true, 1), ("CLKFBOUT", false, 1), ("CLKOUT0", false, 1), ("CLKOUT1", false, 1),
        ("CLKOUT2", false, 1), ("CLKOUT3", false, 1), ("CLKOUT4", false, 1), ("CLKOUT5", false, 1), ("DO", false, 16), ("DRDY", false, 1), ("LOCKED", false, 1),
    ];
    Some(match module {
        "RAMB18E1" => RAMB18,
        "RAMB36E1" => RAMB36,
        "DSP48E1" => DSP,
        "MMCME2_ADV" => MMCM_ADV,
        "MMCME2_BASE" => MMCM_BASE,
        "PLLE2_ADV" => PLL_ADV,
        "BUFG" => &[("I", true, 1), ("O", false, 1)],
        "BUFGCE" => &[("I", true, 1), ("CE", true, 1), ("O", false, 1)],
        "IDDR" => &[("C", true, 1), ("CE", true, 1), ("D", true, 1), ("R", true, 1), ("S", true, 1), ("Q1", false, 1), ("Q2", false, 1)],
        "ODDR" => &[("C", true, 1), ("CE", true, 1), ("D1", true, 1), ("D2", true, 1), ("R", true, 1), ("S", true, 1), ("Q", false, 1)],
        "SRL16E" => &[("A0", true, 1), ("A1", true, 1), ("A2", true, 1), ("A3", true, 1), ("CE", true, 1), ("CLK", true, 1), ("D", true, 1), ("Q", false, 1)],
        "SRLC32E" => &[("A", true, 5), ("CE", true, 1), ("CLK", true, 1), ("D", true, 1), ("Q", false, 1), ("Q31", false, 1)],
        "CARRY4" => &[("CI", true, 1), ("CYINIT", true, 1), ("DI", true, 4), ("S", true, 4), ("CO", false, 4), ("O", false, 4)],
        _ => return None,
    })
}

impl Elab {
    /// An instance of a device primitive: a hard cell with its parameters
    /// as attributes and one pin per port bit (`NAME<i>` for a bus).
    pub fn prim_instance(&mut self, d: &InstDecl, ports: &[(&str, bool, u32)], sc: usize, name: String) -> R<()> {
        let env = &mut Env::default();
        let mut attrs = Vec::new();
        for (p, arg) in &d.params {
            let Some(p) = p else { return self.err(format!("parameters of primitive '{}' must be given by name", d.module)) };
            let ParamArg::Expr(e) = arg else { return self.err(format!("parameter '{p}' needs a value")) };
            let v = match e {
                Expr::Str(s) | Expr::Real(s) => s.clone(),
                Expr::Unary("-", x) if matches!(**x, Expr::Real(_)) => match &**x {
                    Expr::Real(s) => format!("-{s}"),
                    _ => unreachable!(),
                },
                _ => {
                    let v = self.eval_self(e, sc, env)?;
                    let Some(bits) = self.dag.as_const(v.n).map(|b| b.to_vec()) else { return self.err(format!("parameter '{p}' is not constant")) };
                    if p.starts_with("INIT") || p.starts_with("SRVAL") || bits.len() > 32 {
                        hex(&bits)
                    } else {
                        let x = super::dag::to_u128(&bits).unwrap();
                        if v.ty.signed() && bits.last() == Some(&true) && bits.len() < 128 {
                            format!("{}", x as i128 - (1i128 << bits.len()))
                        } else {
                            format!("{x}")
                        }
                    }
                }
            };
            attrs.push((p.clone(), v));
        }
        if d.wildcard {
            return self.err(format!("'.*' on primitive '{}' is not supported", d.module));
        }
        let hi = self.hard.len() as u32;
        let (mut ins, mut outs) = (Vec::new(), Vec::new());
        let pin = |p: &str, w: u32, k: u32| if w == 1 { p.to_string() } else { format!("{p}{k}") };
        for c in &d.conns {
            let Some(port) = &c.port else { return self.err(format!("ports of primitive '{}' must be connected by name", d.module)) };
            let Some(&(_, input, w)) = ports.iter().find(|x| x.0 == port) else {
                return self.err(format!("primitive '{}' has no port '{port}'", d.module));
            };
            let Some(e) = &c.expr else { continue };
            if input {
                let v = self.eval_assign(e, w, sc, env)?;
                for k in 0..w {
                    ins.push((pin(port, w, k), v, k));
                }
            } else {
                let targets = self.static_targets(e, sc, env)?;
                let total: u32 = targets.iter().map(|t| t.2).sum();
                let mut hi_bit = total;
                for (sig, lo, tw, _) in targets {
                    hi_bit -= tw;
                    for i in 0..tw {
                        let k = hi_bit + i;
                        if k < w {
                            self.drive(sig, lo + i, Drv::Hard(hi))?;
                            outs.push((pin(port, w, k), sig, lo + i));
                        }
                    }
                }
            }
        }
        self.hard.push(Hard { name, module: d.module.clone(), attrs, ins, outs });
        Ok(())
    }
}
