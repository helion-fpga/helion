//! Lut6 -> AIG bridge.
//!
//! Each `CellKind::Lut6 { init }` becomes an AIG fragment so the whole
//! equivalence flow has one encoding path (AIG -> CNF). The 64-bit INIT is
//! Shannon-split into four 4-LUT cofactors over inputs `I0..I3`
//! (`init[addr]` uses `{I5..I0}` as addr, I0 = LSB, matching
//! `helion_ir::CellKind::Lut6`), each cofactor synthesized with the tested
//! [`exact_synth`] (k <= 4, the same function the area campaign's
//! correctness rests on), then stitched with a fixed 2:1 mux tree on
//! `I4`/`I5`. Templates are memoized per INIT word: repeated INITs (the
//! common case in mapped netlists) share one template.
//!
//! Totality: if `exact_synth` cannot cover a cofactor within budget (not
//! observed; every 4-LUT is cheap), the template falls back to a fixed
//! Shannon mux tree over the 64 INIT bits, which always works.

use crate::aig_opt::{exact_synth, RawSynth, RInp};
use crate::{Aig, Lit};
use std::collections::HashMap;

/// Maximum AND budget tried per 4-LUT cofactor via [`exact_synth`].
/// Tried as increasing budgets 0..=COFACTOR_BUDGET (see [`synth_cofactor`]).
const COFACTOR_BUDGET: usize = 8;

/// Memoized per-INIT templates. Key = the 64-bit INIT word.
#[derive(Clone, Debug, Default)]
pub(crate) struct LutCache {
    map: HashMap<u64, RawSynth>,
}

impl LutCache {
    pub(crate) fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// Template for `init`, synthesizing and memoizing on first use.
    /// Returns an owned clone so callers avoid borrow entanglement with the
    /// cache while building their AIG.
    pub(crate) fn template(&mut self, init: u64) -> RawSynth {
        if let Some(t) = self.map.get(&init) {
            return t.clone();
        }
        let t = lut6_template(init);
        self.map.insert(init, t.clone());
        t
    }

    /// Number of distinct INIT words memoized.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Instantiate a template over six concrete input literals.
pub(crate) fn instantiate(aig: &mut Aig, tmpl: &RawSynth, inputs: [Lit; 6]) -> Lit {
    let mut nodes: Vec<Lit> = Vec::with_capacity(tmpl.ands.len());
    for (a, b) in &tmpl.ands {
        let la = resolve(aig, a, &inputs, &nodes);
        let lb = resolve(aig, b, &inputs, &nodes);
        nodes.push(aig.and_lit(la, lb));
    }
    resolve(aig, &tmpl.out, &inputs, &nodes)
}

fn resolve(_aig: &Aig, r: &RInp, inputs: &[Lit; 6], nodes: &[Lit]) -> Lit {
    match *r {
        RInp::Leaf(i, inv) => {
            let l = inputs[i];
            if inv {
                l.not()
            } else {
                l
            }
        }
        RInp::Node(j, inv) => {
            let l = nodes[j];
            if inv {
                l.not()
            } else {
                l
            }
        }
        RInp::Const(b) => {
            if b {
                Lit::c1()
            } else {
                Lit::c0()
            }
        }
    }
}

/// Flip edge polarity on a template input reference.
fn flip(r: RInp) -> RInp {
    match r {
        RInp::Leaf(i, inv) => RInp::Leaf(i, !inv),
        RInp::Node(j, inv) => RInp::Node(j, !inv),
        RInp::Const(b) => RInp::Const(!b),
    }
}

/// Builder for template AND structure (same shape as [`RawSynth`]).
struct TmplBuilder {
    ands: Vec<(RInp, RInp)>,
}

impl TmplBuilder {
    fn new() -> Self {
        Self { ands: Vec::new() }
    }

    fn push(&mut self, a: RInp, b: RInp) -> RInp {
        let idx = self.ands.len();
        self.ands.push((a, b));
        RInp::Node(idx, false)
    }

    /// `s ? a : b` = `(s&a) | (~s&b)`.
    fn mux(&mut self, s: RInp, a: RInp, b: RInp) -> RInp {
        let t1 = self.push(s, a);
        let t2 = self.push(flip(s), b);
        flip(self.push(flip(t1), flip(t2)))
    }

    /// Append a synthesized cofactor; its `Leaf(i)` (i < 4) already names the
    /// template's LUT input positions, so only node indices are rebased.
    fn append_raw(&mut self, raw: &RawSynth) -> RInp {
        let base = self.ands.len();
        for (a, b) in &raw.ands {
            let ra = match *a {
                RInp::Node(j, inv) => RInp::Node(base + j, inv),
                other => other,
            };
            let rb = match *b {
                RInp::Node(j, inv) => RInp::Node(base + j, inv),
                other => other,
            };
            self.ands.push((ra, rb));
        }
        match raw.out {
            RInp::Node(j, inv) => RInp::Node(base + j, inv),
            other => other,
        }
    }

    fn finish(self, out: RInp) -> RawSynth {
        RawSynth {
            ands: self.ands,
            out,
        }
    }
}

/// Synthesize one 4-LUT cofactor truth table, trying increasing AND budgets.
///
/// Note: budgets are tried small-to-large (not a single large budget).
/// `exact_synth`'s DFS shares one evaluation budget per call, so a large
/// budget can exhaust it exploring deep bad paths and return None where a
/// small budget quickly finds the minimal solution.
fn synth_cofactor(t: u16) -> Option<RawSynth> {
    for budget in 0..=COFACTOR_BUDGET {
        if let Some(raw) = exact_synth(t, 4, budget) {
            return Some(raw);
        }
    }
    None
}

/// Fixed Shannon mux tree over all 64 INIT bits. Total: always works, used
/// only if [`exact_synth`] fails a cofactor.
fn const_tree(b: &mut TmplBuilder, init: u64) -> RInp {
    let mut level: Vec<RInp> = (0..64)
        .map(|i| RInp::Const((init >> i) & 1 == 1))
        .collect();
    for s in 0..6 {
        let mut next = Vec::with_capacity(level.len() / 2);
        for pair in level.chunks_exact(2) {
            // pair[0] = select 0, pair[1] = select 1.
            next.push(b.mux(RInp::Leaf(s, false), pair[1], pair[0]));
        }
        level = next;
    }
    level[0]
}

/// Build the template for one 64-bit LUT6 INIT word.
///
/// `addr = I0 + 2*I1 + 4*I2 + 8*I3 + 16*I4 + 32*I5`; cofactor `c` (over
/// `I0..I3`) is bits `[16*c, 16*c+16)` with `c = I4 + 2*I5`. Output =
/// `mux(I4, mux(I5, c0, c2), mux(I5, c1, c3))`.
fn lut6_template(init: u64) -> RawSynth {
    let mut b = TmplBuilder::new();
    let mut cofactors = Vec::with_capacity(4);
    let mut ok = true;
    for c in 0..4 {
        let t = ((init >> (16 * c)) & 0xFFFF) as u16;
        match synth_cofactor(t) {
            Some(raw) => cofactors.push(b.append_raw(&raw)),
            None => {
                ok = false;
                break;
            }
        }
    }
    let out = if ok {
        // mux(s, a, b) = s ? a : b. m0 covers the i4=0 half (chunks c0/c2),
        // m1 the i4=1 half (chunks c1/c3).
        let m0 = b.mux(RInp::Leaf(5, false), cofactors[2], cofactors[0]);
        let m1 = b.mux(RInp::Leaf(5, false), cofactors[3], cofactors[1]);
        b.mux(RInp::Leaf(4, false), m1, m0)
    } else {
        const_tree(&mut b, init)
    };
    b.finish(out)
}

/// Direct INIT evaluation (the reference the bridge is tested against).
#[cfg(test)]
fn eval_init(init: u64, inputs: [bool; 6]) -> bool {
    let mut addr = 0u64;
    for (i, &b) in inputs.iter().enumerate() {
        if b {
            addr |= 1 << i;
        }
    }
    (init >> addr) & 1 == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::INC4_INIT;

    /// Template AIG must agree with direct INIT evaluation on all 64 combos.
    fn check_template_agrees(init: u64) {
        let mut cache = LutCache::new();
        let tmpl = cache.template(init);
        // Determinism: a second synthesis of the same INIT is identical.
        let tmpl2 = lut6_template(init);
        assert_eq!(
            tmpl.ands, tmpl2.ands,
            "template for init {init:#018x} is not deterministic"
        );
        for addr in 0..64u64 {
            let inputs: [bool; 6] = std::array::from_fn(|i| (addr >> i) & 1 == 1);
            let expected = eval_init(init, inputs);
            let mut aig = Aig::with_pis(Vec::new());
            let lits: [Lit; 6] = std::array::from_fn(|i| aig.pi_lit(&format!("i{i}")));
            let out = instantiate(&mut aig, &tmpl, lits);
            let got = aig.eval_lit(out, addr);
            assert_eq!(
                got, expected,
                "init {init:#018x} addr {addr}: template disagrees with direct eval"
            );
        }
    }

    #[test]
    fn lut_bridge_constants() {
        check_template_agrees(0x0000_0000_0000_0000);
        check_template_agrees(0xFFFF_FFFF_FFFF_FFFF);
    }

    #[test]
    fn lut_bridge_single_inputs() {
        // out = Ii and out = ~Ii for each position.
        for i in 0..6 {
            let mut init = 0u64;
            for addr in 0..64u64 {
                if (addr >> i) & 1 == 1 {
                    init |= 1 << addr;
                }
            }
            check_template_agrees(init);
            check_template_agrees(!init);
        }
    }

    #[test]
    fn lut_bridge_inc4_patterns() {
        // Real mapper INITs: 4-bit incrementer bit functions.
        for &init in &INC4_INIT {
            check_template_agrees(init);
        }
    }

    #[test]
    fn lut_bridge_xor_and_or() {
        // 6-input parity, 6-input AND, 6-input OR.
        let mut parity = 0u64;
        for addr in 0..64u64 {
            if addr.count_ones() % 2 == 1 {
                parity |= 1 << addr;
            }
        }
        check_template_agrees(parity);
        check_template_agrees(0x8000_0000_0000_0000); // AND6: only addr 63
        check_template_agrees(0xFFFF_FFFF_FFFF_FFFE); // OR6: all but addr 0
    }

    #[test]
    fn lut_bridge_mux21() {
        // 2:1 mux with I5 select, I0/I1 data (a real mapped pattern).
        let mut init = 0u64;
        for addr in 0..64u64 {
            let s = (addr >> 5) & 1;
            let v = if s == 1 { (addr >> 1) & 1 } else { addr & 1 };
            if v == 1 {
                init |= 1 << addr;
            }
        }
        check_template_agrees(init);
    }

    #[test]
    fn lut_bridge_random_battery() {
        // Deterministic PRNG (xorshift64*) so the battery is reproducible.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
            state
        };
        for _ in 0..40 {
            check_template_agrees(next());
        }
    }

    #[test]
    fn lut_bridge_synth_covers_typical() {
        // exact_synth should cover realistic cofactor shapes (not silently
        // fall back to the fat const tree): constants, single inputs,
        // INC4 mapper patterns, parity nibbles.
        let mut cases: Vec<u16> = vec![0x0000, 0xFFFF];
        for i in 0..4 {
            let mut t = 0u16;
            for addr in 0..16u16 {
                if (addr >> i) & 1 == 1 {
                    t |= 1 << addr;
                }
            }
            cases.push(t);
            cases.push(!t);
        }
        for &init in &INC4_INIT {
            for c in 0..4 {
                cases.push(((init >> (16 * c)) & 0xFFFF) as u16);
            }
        }
        // 4-bit parity nibble.
        let mut par = 0u16;
        for addr in 0..16u16 {
            if addr.count_ones() % 2 == 1 {
                par |= 1 << addr;
            }
        }
        cases.push(par);
        for t in cases {
            assert!(
                synth_cofactor(t).is_some(),
                "exact_synth failed on cofactor {t:#06x}, would use fat fallback"
            );
        }
    }

    #[test]
    fn lut_cache_memoizes() {
        let mut cache = LutCache::new();
        assert!(cache.is_empty());
        let _ = cache.template(0x1234_5678_9ABC_DEF0);
        let _ = cache.template(0x1234_5678_9ABC_DEF0);
        let _ = cache.template(0xFFFF_0000_FFFF_0000);
        assert_eq!(cache.len(), 2);
    }
}
