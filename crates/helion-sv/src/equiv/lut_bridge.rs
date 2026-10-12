//! Lut6 -> AIG bridge.
//!
//! Each `CellKind::Lut6 { init }` becomes an AIG fragment so the whole
//! equivalence flow has one encoding path (AIG -> CNF). The 64-bit INIT is
//! decomposed by direct Shannon expansion (`init[addr]` uses `{I5..I0}` as
//! addr, I0 = LSB, matching `helion_ir::CellKind::Lut6`) with constant
//! simplifications. Templates are memoized per INIT word: repeated INITs
//! (the common case in mapped netlists) share one template.
//!
//! The decomposition is direct (no search): predictable, fast, total, and
//! deterministic. Structural hashing in `Aig::and_lit` shares identical
//! subtrees when a template is instantiated.

use crate::aig_opt::{RawSynth, RInp};
use crate::{Aig, Lit};
use std::collections::HashMap;


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

    fn finish(self, out: RInp) -> RawSynth {
        RawSynth {
            ands: self.ands,
            out,
        }
    }
}

/// Direct Shannon decomposition of a k-LUT into an AIG template.
///
/// `init` holds the `2^k` truth-table bits; inputs are template leaves
/// `base..base+k` (`init[addr]` uses `{I_{k-1}..I_0}` as addr, I_0 = LSB,
/// matching `CellKind::Lut6`). Splits on the highest input with constant
/// simplifications:
/// - independent of select -> recurse on the shared half;
/// - `s ? h : 0` = `s & h`, `s ? 0 : l` = `~s & l` (1 AND);
/// - `s ? h : 1` = `h | ~s`, `s ? 1 : l` = `~s | l` (2 ANDs via De Morgan);
/// - else a full 2:1 mux (3 ANDs).
///
/// Predictable, fast (no search), total, and deterministic. Structural
/// hashing in `Aig::and_lit` shares identical subtrees at instantiation.
fn shannon_tree(b: &mut TmplBuilder, init: u64, k: usize, base: usize) -> RInp {
    if k == 0 {
        return RInp::Const(init & 1 == 1);
    }
    let half_bits = 1u32 << (k - 1);
    let mask = if half_bits >= 64 {
        u64::MAX
    } else {
        (1u64 << half_bits) - 1
    };
    let lo = init & mask;
    let hi = (init >> half_bits) & mask;
    if lo == hi {
        return shannon_tree(b, lo, k - 1, base);
    }
    let lo_r = shannon_tree(b, lo, k - 1, base);
    let hi_r = shannon_tree(b, hi, k - 1, base);
    let s = RInp::Leaf(base + k - 1, false);
    // out = s ? hi : lo.
    match (lo_r, hi_r) {
        (RInp::Const(false), RInp::Const(false)) => RInp::Const(false),
        (RInp::Const(true), RInp::Const(true)) => RInp::Const(true),
        (RInp::Const(false), h) => b.push(s, h),
        (RInp::Const(true), h) => flip(b.push(flip(s), flip(h))),
        (l, RInp::Const(false)) => b.push(flip(s), l),
        (l, RInp::Const(true)) => flip(b.push(s, flip(l))),
        (l, h) => b.mux(s, h, l),
    }
}

/// Build the template for one 64-bit LUT6 INIT word via direct Shannon
/// decomposition (see [`shannon_tree`]).
fn lut6_template(init: u64) -> RawSynth {
    let mut b = TmplBuilder::new();
    let out = shannon_tree(&mut b, init, 6, 0);
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
    fn lut_bridge_templates_stay_small() {
        // Direct Shannon decomposition must keep templates small: constants
        // and wires need 0 ANDs, simple functions a handful, and even
        // 6-bit parity stays well under 100 ANDs. Cases: full 64-bit INITs.
        let cases: Vec<u64> = vec![
            0x0000_0000_0000_0000,
            0xFFFF_FFFF_FFFF_FFFF,
            0xFFFF_0000_FFFF_0000, // I4
            0x8000_0000_0000_0000, // AND6
            0x6996_6996_6996_6996, // 6-bit parity (replicated nibble)
            0xCCCC_CCCC_AAAA_AAAA, // mux21 pattern
        ];
        for init in cases {
            let mut cache = LutCache::new();
            let tmpl = cache.template(init);
            assert!(
                tmpl.ands.len() < 100,
                "init {init:#018x}: template has {} ANDs, expected small Shannon tree",
                tmpl.ands.len()
            );
        }
        // Wires and constants collapse to (near-)zero ANDs.
        let wire = LutCache::new().template(0xFFFF_0000_FFFF_0000);
        assert!(
            wire.ands.len() <= 2,
            "I4 wire should be ~0 ANDs, got {}",
            wire.ands.len()
        );
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
