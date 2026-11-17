//! LUT RAM (7-series distributed RAM in SLICEM LUTs) for the memories block
//! RAM does not take: an asynchronous read, more ports than block RAM has,
//! or fewer than [`super::mem::MIN_BITS`] bits. Like block RAM it is built
//! only with [`super::set_hard`] on.
//!
//! The memory needs one write port (rising edge, whole elements or whole
//! bytes); it may have any number of reads. Every read is asynchronous: a
//! read inside a clocked process sees the contents before the edge, which is
//! what the LUT shows until the write lands at the edge. Cells (UG953), by
//! depth:
//!
//! * up to 32: `RAM32M` (LUTs A-C hold 32 x 2 each and read anywhere, LUT D
//!   reads at the write address), `RAM32X1S` / `RAM32X1D` for one bit;
//! * up to 64: `RAM64M`, `RAM64X1S` / `RAM64X1D`;
//! * up to 128, or deeper in banks of 128 picked by the high address bits:
//!   `RAM128X1D` per bit and read address (`RAM128X1S` when every read is
//!   at the write address);
//! * up to 256 with every read at the write address: `RAM256X1S`.
//!
//! Contents are the memory's power-up value. Writes outside the memory do
//! nothing; a read address wider than the cells needs reads aliased, as the
//! flop-built memory does.

use super::dag::{bits_for_u, N};
use super::elab::{Elab, R};
use super::lower::{Drv, Hard};
use std::collections::HashMap;

fn clog2(x: u64) -> u32 {
    bits_for_u(x.saturating_sub(1) as u128)
}

fn hex(bits: &[bool]) -> String {
    let digits = bits.len().div_ceil(4).max(1);
    let s: String = (0..digits)
        .rev()
        .map(|d| {
            let v = (0..4).filter(|b| bits.get(d * 4 + b).copied().unwrap_or(false)).fold(0, |v, b| v | 1 << b);
            char::from_digit(v, 16).unwrap().to_ascii_uppercase()
        })
        .collect();
    format!("{}'h{s}", bits.len())
}

/// A cell being built.
struct Cell {
    module: &'static str,
    attrs: Vec<(String, String)>,
    ins: Vec<(String, N, u32)>,
    /// (pin, read address index, data bit).
    outs: Vec<(String, usize, u32)>,
}

impl Elab {
    /// Build memory `m` as LUT RAM, or ask for it in flops. `why` says why
    /// block RAM did not take it; `users` is [`Elab::build_mems`]' map of
    /// what reads each hidden read signal.
    pub(super) fn build_lutram(&mut self, m: usize, users: &HashMap<u32, Vec<Option<usize>>>, why: &str) -> R<()> {
        let mem = &self.mem.mems[m];
        if mem.w.len() != 1 {
            let n = mem.w.len();
            return self.retry(m, &format!("{why}; LUT RAM needs one write port, it has {n}"));
        }
        if mem.w[0].neg {
            return self.retry(m, &format!("{why}; LUT RAM is written on the rising edge only"));
        }
        let (depth, ew, msig, partial) = (mem.depth, mem.ew, mem.sig, mem.partial);
        let (wclk, waddr, wd, we) = (mem.w[0].clk, mem.w[0].addr, mem.w[0].wd, mem.w[0].we);
        let mname = self.sigs[msig as usize].name.clone();
        // Each address read and used, once; the reads it serves.
        let mut addrs: Vec<N> = Vec::new();
        let mut reads: Vec<(u32, usize)> = Vec::new();
        for r in &mem.r {
            if users.get(&r.r).map_or(true, |u| u.is_empty()) {
                continue;
            }
            let k = addrs.iter().position(|a| *a == r.addr).unwrap_or_else(|| {
                addrs.push(r.addr);
                addrs.len() - 1
            });
            reads.push((r.r, k));
        }
        if addrs.is_empty() {
            return Ok(());
        }
        let free: Vec<usize> = (0..addrs.len()).filter(|&k| addrs[k] != waddr).collect();
        let at_w = (0..addrs.len()).find(|&k| addrs[k] == waddr);
        let (bank, cbits) = match depth {
            0..=32 => (32u32, 5u32),
            33..=64 => (64, 6),
            129..=256 if free.is_empty() => (256, 8),
            _ => (128, 7),
        };
        let nbank = depth.div_ceil(bank);
        let hbits = clog2(nbank as u64);
        let init: Vec<Vec<bool>> = (0..depth)
            .map(|a| {
                let off = self.sigs[msig as usize].ty.offset(mem.lo + a as i64).unwrap() as usize;
                self.sigs[msig as usize].init[off..off + ew as usize].to_vec()
            })
            .collect();
        let bit_at = |b: u32, a: u32, eb: u32| init.get((b * bank + a) as usize).map_or(false, |e| e[eb as usize]);
        let one = self.dag.bit(true);
        // Addresses: cell bits [0, cbits) and bank bits above.
        let full = |e: &mut Elab, a: N| {
            let w = e.dag.w(a);
            if w < cbits + hbits { e.dag.zext(a, cbits + hbits) } else { a }
        };
        let wa = full(self, waddr);
        let ras: Vec<N> = addrs.iter().map(|&a| full(self, a)).collect();
        // Write enable per lane and bank: writes outside the memory do nothing.
        let waw = self.dag.w(waddr);
        let ok = if waw < 32 && (1u64 << waw) <= depth as u64 {
            one
        } else {
            let w2 = waw.max(bits_for_u(depth as u128)) + 1;
            let a = self.dag.zext(waddr, w2);
            let d = self.dag.kval(depth as u128, w2);
            self.dag.ult(a, d)
        };
        let lanes: Vec<(u32, u32)> = if partial { (0..ew.div_ceil(8)).map(|b| (b * 8, (b * 8 + 8).min(ew))).collect() } else { vec![(0, ew)] };
        let wen = self.dag.read(we, ew.div_ceil(8));
        let wdn = self.dag.read(wd, ew);
        let mut cells: Vec<(u32, Cell)> = Vec::new(); // (bank, cell)
        for b in 0..nbank {
            let hit = if nbank > 1 {
                let hb = self.dag.slice(wa, cbits, hbits);
                let k = self.dag.kval(b as u128, hbits);
                self.dag.eq(hb, k)
            } else {
                one
            };
            for &(l0, l1) in &lanes {
                let lane_we = self.dag.slice(wen, l0 / 8, 1);
                let x = self.dag.and(lane_we, ok);
                let wen_bl = self.dag.and(x, hit);
                let common = |ins: &mut Vec<(String, N, u32)>| {
                    ins.push(("WCLK".into(), wclk, 0));
                    ins.push(("WE".into(), wen_bl, 0));
                };
                if bank >= 128 || (ew == 1 && free.len() <= 1) {
                    // One bit per cell: X1S, or X1D per free read address.
                    let (sname, dname) = match bank {
                        32 => ("RAM32X1S", "RAM32X1D"),
                        64 => ("RAM64X1S", "RAM64X1D"),
                        128 => ("RAM128X1S", "RAM128X1D"),
                        _ => ("RAM256X1S", "RAM256X1D"),
                    };
                    for eb in l0..l1 {
                        let contents: Vec<bool> = (0..bank).map(|a| bit_at(b, a, eb)).collect();
                        let mut base = vec![("D".to_string(), wdn, eb)];
                        common(&mut base);
                        for i in 0..cbits {
                            base.push((format!("A{i}"), wa, i));
                        }
                        let attrs = vec![("INIT".to_string(), hex(&contents))];
                        if free.is_empty() {
                            let k = at_w.unwrap();
                            cells.push((b, Cell { module: sname, attrs, ins: base, outs: vec![("O".into(), k, eb)] }));
                            continue;
                        }
                        for (i, &k) in free.iter().enumerate() {
                            let mut ins = base.clone();
                            for j in 0..cbits {
                                ins.push((format!("DPRA{j}"), ras[k], j));
                            }
                            let mut outs = vec![("DPO".to_string(), k, eb)];
                            if let (0, Some(kw)) = (i, at_w) {
                                outs.push(("SPO".into(), kw, eb));
                            }
                            cells.push((b, Cell { module: dname, attrs: attrs.clone(), ins, outs }));
                        }
                    }
                    continue;
                }
                // RAM32M / RAM64M: slots of `bpl` bits per LUT; reads at the
                // write address may take port D, others only A-C.
                let bpl = if bank == 32 { 2 } else { 1 };
                let mut fs: Vec<(usize, u32)> = Vec::new();
                let mut ws: Vec<(usize, u32)> = Vec::new();
                for k in 0..addrs.len() {
                    for g in (l0..l1).step_by(bpl as usize) {
                        if Some(k) == at_w { ws.push((k, g)) } else { fs.push((k, g)) }
                    }
                }
                let ncells = fs.len().div_ceil(3).max((fs.len() + ws.len()).div_ceil(4));
                let (mut fs, mut ws) = (fs.into_iter(), ws.into_iter());
                for _ in 0..ncells {
                    let d_slot = ws.next();
                    let mut abc: Vec<(usize, u32)> = fs.by_ref().take(3).collect();
                    while abc.len() < 3 {
                        match ws.next() {
                            Some(s) => abc.push(s),
                            None => break,
                        }
                    }
                    let mut c = Cell { module: if bank == 32 { "RAM32M" } else { "RAM64M" }, attrs: Vec::new(), ins: Vec::new(), outs: Vec::new() };
                    common(&mut c.ins);
                    for (p, slot) in ['A', 'B', 'C', 'D'].into_iter().zip(abc.into_iter().map(Some).chain([None, None, None]).take(3).chain([d_slot])) {
                        // Port D is written even when nothing reads it: it holds the lane's first bits.
                        let (k, g) = match slot {
                            Some(s) => (Some(s.0), s.1),
                            None if p == 'D' => (None, l0),
                            None => continue,
                        };
                        let addr = if p == 'D' { wa } else { ras[k.unwrap()] };
                        for i in 0..cbits {
                            c.ins.push((format!("ADDR{p}{i}"), addr, i));
                        }
                        let mut contents = vec![false; 64];
                        for j in 0..bpl {
                            let eb = (g + j).min(l1 - 1); // an odd last bit fills both halves
                            let pin = if bpl == 1 { format!("DI{p}") } else { format!("DI{p}{j}") };
                            c.ins.push((pin, wdn, eb));
                            if let Some(k) = k.filter(|_| g + j < l1) {
                                c.outs.push((if bpl == 1 { format!("DO{p}") } else { format!("DO{p}{j}") }, k, eb));
                            }
                            for a in 0..bank {
                                contents[(a * bpl + j) as usize] = k.is_some() && bit_at(b, a, eb);
                            }
                        }
                        c.attrs.push((format!("INIT_{p}"), hex(&contents)));
                    }
                    cells.push((b, c));
                }
            }
        }
        // Outputs: one hidden signal per read address and bank.
        let mut outs: Vec<Vec<u32>> = Vec::new();
        for k in 0..addrs.len() {
            outs.push((0..nbank).map(|b| self.hidden_sig(format!("{mname}_lr{k}_{b}"), ew)).collect());
        }
        let n0 = self.hard.len();
        for (i, (b, c)) in cells.into_iter().enumerate() {
            let hi = (n0 + i) as u32;
            let name = format!("{mname}_lutram{i}");
            let mut hout = Vec::new();
            for (pin, k, eb) in c.outs {
                let s = outs[k][b as usize];
                self.sigs[s as usize].drv[eb as usize] = Drv::Hard(hi);
                hout.push((pin, s, eb));
            }
            self.hard.push(Hard { name, module: c.module.into(), attrs: c.attrs, ins: c.ins, outs: hout });
        }
        for (r, k) in reads {
            let rows: Vec<N> = outs[k].iter().map(|&s| self.dag.read(s, ew)).collect();
            let v = if nbank > 1 {
                let hb = self.dag.slice(ras[k], cbits, hbits);
                self.select(&rows, hb)
            } else {
                rows[0]
            };
            for bit in 0..ew {
                self.sigs[r as usize].drv[bit as usize] = Drv::Comb(v, bit);
            }
        }
        Ok(())
    }
}
