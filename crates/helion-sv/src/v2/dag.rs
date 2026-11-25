//! Word-level dataflow graph with hash-consing and constant folding.
//!
//! Every node has an explicit width. Operands of binary arithmetic and
//! bitwise nodes always have the node's width; comparison nodes are 1 bit
//! wide with equal-width operands. Arithmetic is unsigned/two's complement:
//! signedness is lowered by the elaborator (sign extension, MSB flips), so
//! the emitted SystemVerilog never depends on `$signed` or context widths.

use std::collections::HashMap;

pub type N = u32;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// Constant, LSB first.
    Const(Vec<bool>),
    /// Value of a net (signal, port or register output).
    Read(u32),
    /// Placeholder for "keep the previous value" inside a combinational
    /// process (a latch if it reaches a driver).
    Hold(u32),
    /// Process variable read before any assignment (combinational context).
    VHold(u32),
    Slice(N, u32),
    /// MSB first.
    Concat(Vec<N>),
    Not(N),
    And(N, N),
    Or(N, N),
    Xor(N, N),
    Add(N, N),
    Sub(N, N),
    Mul(N, N),
    Udiv(N, N),
    Urem(N, N),
    Eq(N, N),
    Ult(N, N),
    Mux(N, N, N),
    Shl(N, N),
    Lshr(N, N),
    RedAnd(N),
    RedOr(N),
    RedXor(N),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub op: Op,
    pub w: u32,
}

#[derive(Default)]
pub struct Dag {
    pub nodes: Vec<Node>,
    memo: HashMap<(Op, u32), N>,
}

pub fn to_u128(bits: &[bool]) -> Option<u128> {
    if bits.len() > 128 && bits[128..].iter().any(|&b| b) {
        return None;
    }
    let mut v = 0u128;
    for (i, &b) in bits.iter().enumerate().take(128) {
        if b {
            v |= 1 << i;
        }
    }
    Some(v)
}

pub fn from_u128(v: u128, w: u32) -> Vec<bool> {
    (0..w).map(|i| i < 128 && (v >> i) & 1 == 1).collect()
}

impl Dag {
    pub fn w(&self, n: N) -> u32 {
        self.nodes[n as usize].w
    }
    pub fn op(&self, n: N) -> &Op {
        &self.nodes[n as usize].op
    }
    pub fn as_const(&self, n: N) -> Option<&[bool]> {
        match &self.nodes[n as usize].op {
            Op::Const(b) => Some(b),
            _ => None,
        }
    }
    pub fn const_u128(&self, n: N) -> Option<u128> {
        self.as_const(n).and_then(to_u128)
    }
    fn intern(&mut self, op: Op, w: u32) -> N {
        if let Some(&n) = self.memo.get(&(op.clone(), w)) {
            return n;
        }
        let n = self.nodes.len() as N;
        self.nodes.push(Node { op: op.clone(), w });
        self.memo.insert((op, w), n);
        n
    }

    pub fn konst(&mut self, bits: Vec<bool>) -> N {
        let w = bits.len() as u32;
        self.intern(Op::Const(bits), w)
    }
    pub fn kval(&mut self, v: u128, w: u32) -> N {
        self.konst(from_u128(v, w))
    }
    pub fn zero(&mut self, w: u32) -> N {
        self.konst(vec![false; w as usize])
    }
    pub fn ones(&mut self, w: u32) -> N {
        self.konst(vec![true; w as usize])
    }
    pub fn bit(&mut self, b: bool) -> N {
        self.konst(vec![b])
    }
    pub fn read(&mut self, net: u32, w: u32) -> N {
        self.intern(Op::Read(net), w)
    }
    pub fn hold(&mut self, net: u32, w: u32) -> N {
        self.intern(Op::Hold(net), w)
    }
    pub fn vhold(&mut self, var: u32, w: u32) -> N {
        self.intern(Op::VHold(var), w)
    }

    pub fn slice(&mut self, a: N, lo: u32, w: u32) -> N {
        let aw = self.w(a);
        assert!(lo + w <= aw, "slice [{lo}+:{w}] of {aw}-bit node");
        if w == 0 {
            return self.konst(Vec::new());
        }
        if lo == 0 && w == aw {
            return a;
        }
        match self.op(a).clone() {
            Op::Const(b) => self.konst(b[lo as usize..(lo + w) as usize].to_vec()),
            Op::Slice(x, l2) => self.slice(x, l2 + lo, w),
            Op::Concat(parts) => {
                // Walk parts LSB first.
                let mut off = 0u32;
                let mut picked: Vec<N> = Vec::new();
                for &p in parts.iter().rev() {
                    let pw = self.w(p);
                    let s = lo.max(off);
                    let e = (lo + w).min(off + pw);
                    if s < e {
                        picked.push(self.slice(p, s - off, e - s));
                    }
                    off += pw;
                }
                picked.reverse();
                self.concat(picked)
            }
            Op::Mux(s, x, y) => {
                let xs = self.slice(x, lo, w);
                let ys = self.slice(y, lo, w);
                self.mux(s, xs, ys)
            }
            Op::Not(x) => {
                let xs = self.slice(x, lo, w);
                self.not(xs)
            }
            Op::And(x, y) | Op::Or(x, y) | Op::Xor(x, y) => {
                let xs = self.slice(x, lo, w);
                let ys = self.slice(y, lo, w);
                match self.op(a) {
                    Op::And(..) => self.and(xs, ys),
                    Op::Or(..) => self.or(xs, ys),
                    _ => self.xor(xs, ys),
                }
            }
            Op::Add(..) | Op::Sub(..) | Op::Mul(..) if lo == 0 => {
                // Low bits of +,-,* depend only on low operand bits.
                let (x, y) = match self.op(a) {
                    Op::Add(x, y) | Op::Sub(x, y) | Op::Mul(x, y) => (*x, *y),
                    _ => unreachable!(),
                };
                let xs = self.slice(x, 0, w);
                let ys = self.slice(y, 0, w);
                match self.op(a) {
                    Op::Add(..) => self.add(xs, ys),
                    Op::Sub(..) => self.sub(xs, ys),
                    _ => self.mul(xs, ys),
                }
            }
            _ => self.intern(Op::Slice(a, lo), w),
        }
    }

    /// Bits `lo..lo + w` of `a` as they are: unlike [`Dag::slice`], the
    /// low bits of a sum or product do not become a narrower copy of it.
    pub fn bits(&mut self, a: N, lo: u32, w: u32) -> N {
        let (a, lo) = match *self.op(a) {
            Op::Slice(x, l) => (x, l + lo),
            _ => (a, lo),
        };
        match self.op(a) {
            Op::Add(..) | Op::Sub(..) | Op::Mul(..) if lo == 0 && w < self.w(a) => self.intern(Op::Slice(a, 0), w),
            _ => self.slice(a, lo, w),
        }
    }

    /// MSB-first concatenation.
    pub fn concat(&mut self, parts: Vec<N>) -> N {
        let mut flat: Vec<N> = Vec::new();
        for p in parts {
            if self.w(p) == 0 {
                continue;
            }
            if let Op::Concat(sub) = self.op(p).clone() {
                flat.extend(sub);
            } else {
                flat.push(p);
            }
        }
        // Merge adjacent constants and adjacent slices of the same node.
        let mut merged: Vec<N> = Vec::new();
        for p in flat {
            if let Some(&last) = merged.last() {
                if let (Some(a), Some(b)) = (
                    self.as_const(last).map(|x| x.to_vec()),
                    self.as_const(p).map(|x| x.to_vec()),
                ) {
                    // last is more significant
                    let mut bits = b;
                    bits.extend(a);
                    merged.pop();
                    let k = self.konst(bits);
                    merged.push(k);
                    continue;
                }
                let la = self.slice_parts(last);
                let pa = self.slice_parts(p);
                if let (Some((x, xl, xw)), Some((y, yl, yw))) = (la, pa) {
                    if x == y && yl + yw == xl {
                        merged.pop();
                        let s = self.slice(x, yl, xw + yw);
                        merged.push(s);
                        continue;
                    }
                }
            }
            merged.push(p);
        }
        match merged.len() {
            0 => self.konst(Vec::new()),
            1 => merged[0],
            _ => {
                let w = merged.iter().map(|&p| self.w(p)).sum();
                self.intern(Op::Concat(merged), w)
            }
        }
    }

    fn slice_parts(&self, n: N) -> Option<(N, u32, u32)> {
        match self.op(n) {
            Op::Slice(x, l) => Some((*x, *l, self.w(n))),
            Op::Const(_) | Op::Concat(_) => None,
            _ => Some((n, 0, self.w(n))),
        }
    }

    pub fn zext(&mut self, a: N, w: u32) -> N {
        let aw = self.w(a);
        if aw == w {
            return a;
        }
        if aw > w {
            return self.slice(a, 0, w);
        }
        let z = self.zero(w - aw);
        self.concat(vec![z, a])
    }

    pub fn sext(&mut self, a: N, w: u32) -> N {
        let aw = self.w(a);
        if aw == w {
            return a;
        }
        if aw > w {
            return self.slice(a, 0, w);
        }
        if aw == 0 {
            return self.zero(w);
        }
        let msb = self.slice(a, aw - 1, 1);
        let mut parts = vec![msb; (w - aw) as usize];
        parts.push(a);
        self.concat(parts)
    }

    pub fn ext(&mut self, a: N, w: u32, signed: bool) -> N {
        if signed {
            self.sext(a, w)
        } else {
            self.zext(a, w)
        }
    }

    pub fn not(&mut self, a: N) -> N {
        let w = self.w(a);
        if let Some(b) = self.as_const(a) {
            let nb: Vec<bool> = b.iter().map(|x| !x).collect();
            return self.konst(nb);
        }
        match self.op(a).clone() {
            Op::Not(x) => x,
            Op::Concat(parts) => {
                let np: Vec<N> = parts.into_iter().map(|p| self.not(p)).collect();
                self.concat(np)
            }
            _ => self.intern(Op::Not(a), w),
        }
    }

    fn bitwise(&mut self, kind: u8, a: N, b: N) -> N {
        let w = self.w(a);
        assert_eq!(w, self.w(b), "bitwise width mismatch");
        if let (Some(x), Some(y)) = (self.as_const(a), self.as_const(b)) {
            let r: Vec<bool> = x
                .iter()
                .zip(y)
                .map(|(&p, &q)| match kind {
                    0 => p & q,
                    1 => p | q,
                    _ => p ^ q,
                })
                .collect();
            return self.konst(r);
        }
        // Constant-operand identities.
        for (c, o) in [(a, b), (b, a)] {
            if let Some(cb) = self.as_const(c) {
                let all0 = cb.iter().all(|x| !x);
                let all1 = cb.iter().all(|&x| x);
                match kind {
                    0 if all0 => return c,
                    0 if all1 => return o,
                    1 if all1 => return c,
                    1 if all0 => return o,
                    2 if all0 => return o,
                    2 if all1 => return self.not(o),
                    _ => {}
                }
            }
        }
        if a == b {
            return match kind {
                0 | 1 => a,
                _ => self.zero(w),
            };
        }
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let op = match kind {
            0 => Op::And(a, b),
            1 => Op::Or(a, b),
            _ => Op::Xor(a, b),
        };
        self.intern(op, w)
    }
    pub fn and(&mut self, a: N, b: N) -> N {
        self.bitwise(0, a, b)
    }
    pub fn or(&mut self, a: N, b: N) -> N {
        self.bitwise(1, a, b)
    }
    pub fn xor(&mut self, a: N, b: N) -> N {
        self.bitwise(2, a, b)
    }

    pub fn add(&mut self, a: N, b: N) -> N {
        let w = self.w(a);
        assert_eq!(w, self.w(b), "add width mismatch");
        if w <= 128 {
            if let (Some(x), Some(y)) = (self.const_u128(a), self.const_u128(b)) {
                return self.kval(x.wrapping_add(y), w);
            }
        }
        if self
            .as_const(b)
            .map(|c| c.iter().all(|x| !x))
            .unwrap_or(false)
        {
            return a;
        }
        if self
            .as_const(a)
            .map(|c| c.iter().all(|x| !x))
            .unwrap_or(false)
        {
            return b;
        }
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        if w > PREFIX_MIN_W && !carry_arith() {
            return self.prefix_add(a, b, false).0;
        }
        self.intern(Op::Add(a, b), w)
    }

    /// `a + b + 1` on a carry chain, as (sum, carry out): one extra low
    /// bit of `1 + 1` produces the carry in, one extra high bit of `0 + 0`
    /// receives the carry out.
    fn carry_add_plus_one(&mut self, a: N, b: N) -> (N, N) {
        let w = self.w(a);
        let one = self.bit(true);
        let zero = self.bit(false);
        let x = self.concat(vec![zero, a, one]);
        let y = self.concat(vec![zero, b, one]);
        let s = self.add(x, y);
        (self.slice(s, 1, w), self.slice(s, w + 1, 1))
    }

    /// Parallel-prefix (Kogge-Stone) adder over 1-bit gates: logarithmic
    /// depth, so wide adders, counters and comparators meet a 100 MHz clock
    /// in LUT logic (a ripple chain of LUTs does not). Returns (sum, carry
    /// out). `cin` is a constant carry in.
    pub fn prefix_add(&mut self, a: N, b: N, cin: bool) -> (N, N) {
        let w = self.w(a);
        let mut p0 = Vec::with_capacity(w as usize);
        let mut g = Vec::with_capacity(w as usize);
        for i in 0..w {
            let ai = self.slice(a, i, 1);
            let bi = self.slice(b, i, 1);
            p0.push(self.xor(ai, bi));
            g.push(self.and(ai, bi));
        }
        if cin {
            // generate at bit 0 including the carry in: g0 | p0
            g[0] = self.or(g[0], p0[0]);
        }
        let mut gg = g.clone();
        let mut pp = p0.clone();
        let mut d = 1usize;
        while d < w as usize {
            let (g2, p2) = (gg.clone(), pp.clone());
            for i in d..w as usize {
                let t = self.and(p2[i], g2[i - d]);
                gg[i] = self.or(g2[i], t);
                pp[i] = self.and(p2[i], p2[i - d]);
            }
            d *= 2;
        }
        let mut bits = Vec::with_capacity(w as usize);
        for i in 0..w as usize {
            let c = if i == 0 { self.bit(cin) } else { gg[i - 1] };
            bits.push(self.xor(p0[i], c));
        }
        bits.reverse();
        let sum = self.concat(bits);
        (sum, gg[w as usize - 1])
    }
    pub fn sub(&mut self, a: N, b: N) -> N {
        let w = self.w(a);
        assert_eq!(w, self.w(b), "sub width mismatch");
        if w <= 128 {
            if let (Some(x), Some(y)) = (self.const_u128(a), self.const_u128(b)) {
                return self.kval(x.wrapping_sub(y), w);
            }
        }
        if self
            .as_const(b)
            .map(|c| c.iter().all(|x| !x))
            .unwrap_or(false)
        {
            return a;
        }
        if a == b {
            return self.zero(w);
        }
        if w > PREFIX_MIN_W {
            // a - b = a + ~b + 1
            let nb = self.not(b);
            if carry_arith() {
                return self.carry_add_plus_one(a, nb).0;
            }
            return self.prefix_add(a, nb, true).0;
        }
        self.intern(Op::Sub(a, b), w)
    }
    pub fn mul(&mut self, a: N, b: N) -> N {
        let w = self.w(a);
        assert_eq!(w, self.w(b), "mul width mismatch");
        if w <= 128 {
            if let (Some(x), Some(y)) = (self.const_u128(a), self.const_u128(b)) {
                return self.kval(x.wrapping_mul(y), w);
            }
        }
        for (c, o) in [(a, b), (b, a)] {
            if let Some(v) = self.const_u128(c) {
                if v == 0 {
                    return self.zero(w);
                }
                if v == 1 {
                    return o;
                }
                if v.is_power_of_two() {
                    let k = v.trailing_zeros();
                    let amt = self.kval(k as u128, bits_for_u(k as u128).max(1));
                    return self.shl(o, amt);
                }
            }
        }
        if w > PREFIX_MIN_W && carry_arith() {
            // The netlist builds a Wallace tree with one carry chain.
            return self.intern(Op::Mul(a, b), w);
        }
        // Shift-and-add over named partial sums (no `*` in the emitted SV).
        let (a, b) = if self.as_const(a).is_some() {
            (b, a)
        } else {
            (a, b)
        };
        let mut terms: Vec<N> = Vec::new();
        for j in 0..w {
            let bj = self.slice(b, j, 1);
            if self.as_const(bj) == Some(&[false]) {
                continue;
            }
            let sh = self.kval(j as u128, bits_for_u(j as u128).max(1));
            let a_sh = self.shl(a, sh);
            let rep = self.concat(vec![bj; w as usize]);
            terms.push(self.and(a_sh, rep));
        }
        // Sum the partial products as a balanced tree: the depth is the
        // logarithm of their number, not their number.
        while terms.len() > 1 {
            let mut next = Vec::with_capacity(terms.len().div_ceil(2));
            for pair in terms.chunks(2) {
                next.push(if pair.len() == 2 { self.add(pair[0], pair[1]) } else { pair[0] });
            }
            terms = next;
        }
        terms.pop().unwrap_or_else(|| self.zero(w))
    }
    pub fn udiv(&mut self, a: N, b: N) -> N {
        let w = self.w(a);
        if w <= 128 {
            if let (Some(x), Some(y)) = (self.const_u128(a), self.const_u128(b)) {
                return self.kval(if y == 0 { u128::MAX } else { x / y }, w);
            }
        }
        if let Some(v) = self.const_u128(b) {
            if v == 1 {
                return a;
            }
            if v.is_power_of_two() {
                let k = v.trailing_zeros();
                let amt = self.kval(k as u128, bits_for_u(k as u128).max(1));
                return self.lshr(a, amt);
            }
        }
        self.divmod(a, b).0
    }

    /// Restoring division, built from subtract/compare/mux (no `/` in the
    /// emitted SV). Division by zero gives all-ones quotient, remainder `a`.
    pub fn divmod(&mut self, a: N, b: N) -> (N, N) {
        let w = self.w(a);
        let bx = self.zext(b, w + 1);
        let mut r = self.zero(w + 1);
        let mut q: Vec<N> = Vec::with_capacity(w as usize);
        for i in (0..w).rev() {
            let ai = self.slice(a, i, 1);
            let rl = self.slice(r, 0, w);
            r = self.concat(vec![rl, ai]);
            let lt = self.ult(r, bx);
            let ge = self.not(lt);
            let d = self.sub(r, bx);
            r = self.mux(ge, d, r);
            q.push(ge);
        }
        let qn = self.concat(q);
        let rn = self.slice(r, 0, w);
        (qn, rn)
    }
    pub fn urem(&mut self, a: N, b: N) -> N {
        let w = self.w(a);
        if w <= 128 {
            if let (Some(x), Some(y)) = (self.const_u128(a), self.const_u128(b)) {
                return self.kval(if y == 0 { x } else { x % y }, w);
            }
        }
        if let Some(v) = self.const_u128(b) {
            if v.is_power_of_two() {
                let k = v.trailing_zeros();
                let lo = self.slice(a, 0, k.min(w));
                return self.zext(lo, w);
            }
        }
        self.divmod(a, b).1
    }

    pub fn eq(&mut self, a: N, b: N) -> N {
        assert_eq!(self.w(a), self.w(b), "eq width mismatch");
        if a == b {
            return self.bit(true);
        }
        if let (Some(x), Some(y)) = (self.as_const(a), self.as_const(b)) {
            let r = x == y;
            return self.bit(r);
        }
        if self.w(a) == 1 {
            // a == 1 -> a ; a == 0 -> !a
            for (c, o) in [(a, b), (b, a)] {
                if let Some(cb) = self.as_const(c) {
                    return if cb[0] { o } else { self.not(o) };
                }
            }
        }
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        self.intern(Op::Eq(a, b), 1)
    }
    pub fn ne(&mut self, a: N, b: N) -> N {
        let e = self.eq(a, b);
        self.not(e)
    }
    pub fn ult(&mut self, a: N, b: N) -> N {
        let w = self.w(a);
        assert_eq!(w, self.w(b), "ult width mismatch");
        if a == b {
            return self.bit(false);
        }
        if w <= 128 {
            if let (Some(x), Some(y)) = (self.const_u128(a), self.const_u128(b)) {
                return self.bit(x < y);
            }
        }
        if self.const_u128(b) == Some(0) {
            return self.bit(false);
        }
        if w > PREFIX_MIN_W {
            // a < b  <=>  no carry out of a + ~b + 1
            let nb = self.not(b);
            let (_, co) = if carry_arith() { self.carry_add_plus_one(a, nb) } else { self.prefix_add(a, nb, true) };
            return self.not(co);
        }
        self.intern(Op::Ult(a, b), 1)
    }
    pub fn ule(&mut self, a: N, b: N) -> N {
        let g = self.ult(b, a);
        self.not(g)
    }
    /// Signed less-than via MSB flip.
    pub fn slt(&mut self, a: N, b: N) -> N {
        let w = self.w(a);
        let m = {
            let mut bits = vec![false; w as usize];
            bits[w as usize - 1] = true;
            self.konst(bits)
        };
        let fa = self.xor(a, m);
        let fb = self.xor(b, m);
        self.ult(fa, fb)
    }

    pub fn mux(&mut self, s: N, a: N, b: N) -> N {
        assert_eq!(self.w(s), 1, "mux select must be 1 bit");
        let w = self.w(a);
        assert_eq!(w, self.w(b), "mux arm width mismatch");
        if a == b {
            return a;
        }
        if let Some(c) = self.as_const(s) {
            return if c[0] { a } else { b };
        }
        if let Op::Not(x) = self.op(s).clone() {
            return self.mux(x, b, a);
        }
        if w == 1 {
            if let (Some(x), Some(y)) = (self.as_const(a), self.as_const(b)) {
                return if x[0] && !y[0] { s } else { self.not(s) };
            }
        }
        // Mux(s, Mux(s, x, _), y) => Mux(s, x, y)
        if let Op::Mux(s2, x, _) = self.op(a).clone() {
            if s2 == s {
                return self.mux(s, x, b);
            }
        }
        if let Op::Mux(s2, _, y) = self.op(b).clone() {
            if s2 == s {
                return self.mux(s, a, y);
            }
        }
        self.intern(Op::Mux(s, a, b), w)
    }

    pub fn shl(&mut self, a: N, amt: N) -> N {
        let w = self.w(a);
        if let Some(k) = self.const_u128(amt) {
            if k >= w as u128 {
                return self.zero(w);
            }
            let k = k as u32;
            if k == 0 {
                return a;
            }
            let lo = self.slice(a, 0, w - k);
            let z = self.zero(k);
            return self.concat(vec![lo, z]);
        }
        self.barrel(a, amt, true)
    }
    pub fn lshr(&mut self, a: N, amt: N) -> N {
        let w = self.w(a);
        if let Some(k) = self.const_u128(amt) {
            if k >= w as u128 {
                return self.zero(w);
            }
            let k = k as u32;
            if k == 0 {
                return a;
            }
            let hi = self.slice(a, k, w - k);
            let z = self.zero(k);
            return self.concat(vec![z, hi]);
        }
        self.barrel(a, amt, false)
    }

    /// Logarithmic shifter for a variable amount (no variable shifts in the
    /// emitted SV). Amounts >= width give zero.
    fn barrel(&mut self, a: N, amt: N, left: bool) -> N {
        let w = self.w(a);
        let aw = self.w(amt);
        let mut acc = a;
        let stages = bits_for_u(w as u128).min(aw);
        for k in 0..stages {
            let sh = 1u128 << k;
            let kn = self.kval(sh, bits_for_u(sh).max(1));
            let shifted = if left {
                self.shl(acc, kn)
            } else {
                self.lshr(acc, kn)
            };
            let bit = self.slice(amt, k, 1);
            acc = self.mux(bit, shifted, acc);
        }
        if aw > stages {
            let hi = self.slice(amt, stages, aw - stages);
            let big = self.red_or(hi);
            let z = self.zero(w);
            acc = self.mux(big, z, acc);
        }
        acc
    }

    pub fn red_or(&mut self, a: N) -> N {
        if self.w(a) == 1 {
            return a;
        }
        if let Some(c) = self.as_const(a) {
            let r = c.iter().any(|&x| x);
            return self.bit(r);
        }
        if self.w(a) == 0 {
            return self.bit(false);
        }
        self.intern(Op::RedOr(a), 1)
    }
    pub fn red_and(&mut self, a: N) -> N {
        if self.w(a) == 1 {
            return a;
        }
        if let Some(c) = self.as_const(a) {
            let r = c.iter().all(|&x| x);
            return self.bit(r);
        }
        if self.w(a) == 0 {
            return self.bit(true);
        }
        self.intern(Op::RedAnd(a), 1)
    }
    pub fn red_xor(&mut self, a: N) -> N {
        if self.w(a) == 1 {
            return a;
        }
        if let Some(c) = self.as_const(a) {
            let r = c.iter().filter(|&&x| x).count() % 2 == 1;
            return self.bit(r);
        }
        if self.w(a) == 0 {
            return self.bit(false);
        }
        self.intern(Op::RedXor(a), 1)
    }

    /// Operands of a node, in order.
    pub fn kids(&self, n: N) -> Vec<N> {
        match self.op(n) {
            Op::Const(_) | Op::Read(_) | Op::Hold(_) | Op::VHold(_) => vec![],
            Op::Slice(a, _) | Op::Not(a) | Op::RedAnd(a) | Op::RedOr(a) | Op::RedXor(a) => vec![*a],
            Op::Concat(p) => p.clone(),
            Op::And(a, b)
            | Op::Or(a, b)
            | Op::Xor(a, b)
            | Op::Add(a, b)
            | Op::Sub(a, b)
            | Op::Mul(a, b)
            | Op::Udiv(a, b)
            | Op::Urem(a, b)
            | Op::Eq(a, b)
            | Op::Ult(a, b)
            | Op::Shl(a, b)
            | Op::Lshr(a, b) => vec![*a, *b],
            Op::Mux(s, a, b) => vec![*s, *a, *b],
        }
    }

    /// Rebuild `n` with `f` applied to leaves (memoized). `f` returns a
    /// replacement for a leaf or `None` to keep it.
    pub fn subst(
        &mut self,
        n: N,
        f: &mut dyn FnMut(&mut Dag, N) -> Option<N>,
        memo: &mut HashMap<N, N>,
    ) -> N {
        // Iterative post-order to avoid deep recursion on long chains.
        let mut stack: Vec<(N, bool)> = vec![(n, false)];
        while let Some((x, done)) = stack.pop() {
            if memo.contains_key(&x) {
                continue;
            }
            let kids = self.kids(x);
            if !done {
                stack.push((x, true));
                for k in kids {
                    if !memo.contains_key(&k) {
                        stack.push((k, false));
                    }
                }
                continue;
            }
            let r = if kids.is_empty() {
                f(self, x).unwrap_or(x)
            } else {
                let nk: Vec<N> = kids.iter().map(|k| memo[k]).collect();
                if nk == kids { x } else { self.rebuild(x, &nk) }
            };
            memo.insert(x, r);
        }
        memo[&n]
    }

    fn rebuild(&mut self, n: N, k: &[N]) -> N {
        let w = self.w(n);
        match self.op(n).clone() {
            Op::Slice(_, lo) => self.slice(k[0], lo, w),
            Op::Concat(_) => self.concat(k.to_vec()),
            Op::Not(_) => self.not(k[0]),
            Op::And(..) => self.and(k[0], k[1]),
            Op::Or(..) => self.or(k[0], k[1]),
            Op::Xor(..) => self.xor(k[0], k[1]),
            Op::Add(..) => self.add(k[0], k[1]),
            Op::Sub(..) => self.sub(k[0], k[1]),
            Op::Mul(..) => self.mul(k[0], k[1]),
            Op::Udiv(..) => self.udiv(k[0], k[1]),
            Op::Urem(..) => self.urem(k[0], k[1]),
            Op::Eq(..) => self.eq(k[0], k[1]),
            Op::Ult(..) => self.ult(k[0], k[1]),
            Op::Mux(..) => self.mux(k[0], k[1], k[2]),
            Op::Shl(..) => self.shl(k[0], k[1]),
            Op::Lshr(..) => self.lshr(k[0], k[1]),
            Op::RedAnd(_) => self.red_and(k[0]),
            Op::RedOr(_) => self.red_or(k[0]),
            Op::RedXor(_) => self.red_xor(k[0]),
            Op::Const(_) | Op::Read(_) | Op::Hold(_) | Op::VHold(_) => n,
        }
    }

    /// For each bit of `n`, whether it may come from a `Hold`/`VHold` leaf
    /// (`pred` selects which leaves count).
    pub fn tainted_bits(
        &self,
        n: N,
        pred: &dyn Fn(&Op) -> bool,
        memo: &mut HashMap<N, Vec<bool>>,
    ) -> Vec<bool> {
        let mut stack: Vec<(N, bool)> = vec![(n, false)];
        while let Some((x, done)) = stack.pop() {
            if memo.contains_key(&x) {
                continue;
            }
            let kids = self.kids(x);
            if !done {
                stack.push((x, true));
                for k in kids {
                    if !memo.contains_key(&k) {
                        stack.push((k, false));
                    }
                }
                continue;
            }
            let w = self.w(x) as usize;
            let r: Vec<bool> = match self.op(x) {
                op if kids.is_empty() => vec![pred(op); w],
                Op::Slice(a, lo) => memo[a][*lo as usize..*lo as usize + w].to_vec(),
                Op::Concat(parts) => {
                    let mut v = Vec::with_capacity(w);
                    for p in parts.iter().rev() {
                        v.extend(memo[p].iter().copied());
                    }
                    v
                }
                Op::Mux(s, a, b) => {
                    if memo[s][0] {
                        vec![true; w]
                    } else {
                        memo[a]
                            .iter()
                            .zip(&memo[b])
                            .map(|(p, q)| *p || *q)
                            .collect()
                    }
                }
                Op::Not(a) => memo[a].clone(),
                Op::And(a, b) | Op::Or(a, b) | Op::Xor(a, b) => memo[a]
                    .iter()
                    .zip(&memo[b])
                    .map(|(p, q)| *p || *q)
                    .collect(),
                _ => {
                    let any = kids.iter().any(|k| memo[k].iter().any(|&t| t));
                    vec![any; w]
                }
            };
            memo.insert(x, r);
        }
        memo[&n].clone()
    }
}

/// Adders/comparators wider than this become parallel-prefix gate logic.
pub(super) const PREFIX_MIN_W: u32 = 4;

thread_local! {
    static CARRY_ARITH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Keep wide adders, subtractors and comparators as `+` on named buses, for
/// a back end that maps them to dedicated carry chains, instead of
/// lowering them to parallel-prefix gates. The Xilinx flow sets it; it also
/// selects the netlist's Wallace-tree multiplier and its deeper logic
/// optimization (AND-tree balancing, two-level rules, more cuts).
pub fn set_carry_arith(on: bool) {
    CARRY_ARITH.with(|c| c.set(on));
}

pub fn carry_arith() -> bool {
    CARRY_ARITH.with(|c| c.get())
}

pub fn bits_for_u(v: u128) -> u32 {
    128 - v.leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding() {
        let mut d = Dag::default();
        let a = d.read(0, 8);
        let k = d.kval(3, 8);
        let s = d.add(a, k);
        let lo = d.slice(s, 0, 4);
        assert_eq!(d.w(lo), 4);
        let z = d.zero(8);
        assert_eq!(d.and(a, z), z);
        let x = d.slice(a, 4, 4);
        let y = d.slice(a, 0, 4);
        assert_eq!(d.concat(vec![x, y]), a);
        let one = d.bit(true);
        assert_eq!(d.mux(one, a, z), a);
    }
}
