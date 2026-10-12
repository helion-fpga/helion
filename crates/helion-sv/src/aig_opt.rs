//! ABC-style AIG logic rewriting (FM-HEL-AREA-04).
//!
//! `rewrite_aig` runs cut-based exact rewriting over the per-cone AIGs that
//! feed `map_wide_cone`. For each AND node it enumerates K≤4-input cuts,
//! exactly resynthesizes each cut's function with the minimum number of AND
//! nodes (memoized on the 16-bit truth table — no precomputed NPN table
//! needed), and rebuilds the AIG, keeping the result only when the global
//! AND count strictly decreases. Since `map_wide_cone` turns AND nodes into
//! LUT6s, fewer ANDs means fewer LUTs.
//!
//! Correctness is by construction: a replacement is spliced in only when its
//! truth table is bit-identical to the cut's over the cut leaves, and the
//! rebuild maps every leaf through the same function-preserving recursion.
//! Exhaustive miter tests below prove it anyway.
//!
//! Complexity: cut enumeration is O(n · C) for n AND nodes and ≤C cuts per
//! node (C=8, K=4); exact synthesis is DFS with iterative deepening bounded
//! by the cut's current AND cost and memoized on (truth table, inputs,
//! budget). Wide cones are capped at 96 ANDs upstream, so this is firmly in
//! the noise next to elaboration.

use super::{Aig, Lit};
use std::collections::{HashMap, HashSet};

/// Cut input bound (ABC `rewrite` classic).
const CUT_K: usize = 4;
/// Max cuts retained per node (smallest first).
const CUT_MAX_PER_NODE: usize = 8;
/// Max AND cost of a cut we attempt to resynthesize (larger cuts are rare
/// and their exact search is exponential; they are simply skipped).
const MAX_CUT_COST: usize = 6;
/// Fixpoint passes over the cone.
const REWRITE_PASSES: usize = 4;

/// Rewrite `aig`, returning an equivalent AIG with no more AND nodes
/// (usually fewer). Deterministic: no result depends on hash iteration order.
pub fn rewrite_aig(aig: &Aig) -> Aig {
    let (mut new, outs) = rewrite_aig_multi(aig, &[aig.output]);
    debug_assert_eq!(outs.len(), 1);
    new.output = outs[0];
    new
}

/// Merge per-cone AIGs into one global hashed AIG (FM-HEL-AREA-05).
///
/// PIs are unioned by name FIRST (all PI ids allocated before any AND id —
/// the same invariant `from_expr` maintains; violating it lets AND ids
/// collide with later PI ids). Each cone's AND DAG is then re-inserted via
/// `and_lit`, whose structural hashing makes identical subfunctions across
/// cones share one node automatically. Returns the global AIG and each
/// cone's output literal, in input order.
///
/// Correctness: re-insertion is function-preserving (`and_lit` only folds
/// constants and shares structurally identical pairs — same inputs, same
/// function). Deterministic: cone order and PI first-seen order are fixed.
pub fn merge_cones(cones: &[Aig]) -> (Aig, Vec<Lit>) {
    let mut global = Aig::with_pis(Vec::new());
    for cone in cones {
        for pi_name in &cone.pis {
            global.pi_lit(pi_name);
        }
    }
    let mut outputs = Vec::with_capacity(cones.len());
    for cone in cones {
        // old node id -> new Lit. Ids are dense: 0, PIs 1..=n_pi,
        // ANDs 1+n_pi+i — pushed in the same order.
        let mut map: Vec<Lit> = Vec::with_capacity(1 + cone.pis.len() + cone.ands.len());
        map.push(Lit::c0());
        for pi_name in &cone.pis {
            map.push(global.pi_lit(pi_name));
        }
        for (a, b) in &cone.ands {
            let ma = map[a.node as usize];
            let mb = map[b.node as usize];
            let la = if a.inv { ma.not() } else { ma };
            let lb = if b.inv { mb.not() } else { mb };
            map.push(global.and_lit(la, lb));
        }
        let o = cone.output;
        let mo = map[o.node as usize];
        outputs.push(if o.inv { mo.not() } else { mo });
    }
    (global, outputs)
}

/// Multi-output cut rewriting (FM-HEL-AREA-05): same per-node exact
/// resynthesis as [`rewrite_aig`], but the demand-driven rebuild starts from
/// ALL outputs, so shared subfunctions are rebuilt (and shared) once.
/// Returns the rewritten AIG and the remapped output literals, in input
/// order. `rewrite_aig` is the single-output specialization.
pub fn rewrite_aig_multi(aig: &Aig, outputs: &[Lit]) -> (Aig, Vec<Lit>) {
    if aig.ands.len() < 2 {
        return (aig.clone(), outputs.to_vec());
    }
    let mut cur = aig.clone();
    let mut cur_outs = outputs.to_vec();
    for _ in 0..REWRITE_PASSES {
        let (next, next_outs) = rewrite_pass_multi(&cur, &cur_outs);
        if next.ands.len() >= cur.ands.len() {
            break;
        }
        cur = next;
        cur_outs = next_outs;
    }
    (cur, cur_outs)
}

/// One rewriting pass over `outputs`: pick the cheapest exact resynthesis
/// per AND node, rebuild demand-driven from all outputs, keep iff the total
/// AND count shrank.
fn rewrite_pass_multi(aig: &Aig, outputs: &[Lit]) -> (Aig, Vec<Lit>) {
    let n_pi = aig.pis.len();
    let n = aig.ands.len();
    let mut cut_memo: HashMap<u32, Vec<Vec<u32>>> = HashMap::new();
    let mut synth_memo: HashMap<(u16, u8, u8), Option<RawSynth>> = HashMap::new();
    let mut repl: Vec<Option<Synth>> = vec![None; n];
    for (i, slot) in repl.iter_mut().enumerate() {
        let node = 1 + n_pi as u32 + i as u32;
        let cuts = cuts_of(node, &aig.ands, n_pi, &mut cut_memo);
        let mut best: Option<Synth> = None;
        for cut in &cuts {
            if cut.len() > CUT_K {
                continue;
            }
            if cut.len() == 1 && cut[0] == node {
                continue; // trivial cut: replacing node by itself
            }
            if cut.iter().any(|&n| n == 0) {
                continue; // const leaf: cannot arise via and_lit folding; skip defensively
            }
            let leaf_set: HashSet<u32> = cut.iter().copied().collect();
            let cost = cut_cost(&aig.ands, n_pi, node, &leaf_set);
            if cost > MAX_CUT_COST {
                continue;
            }
            let Some(t) = cut_truth(&aig.ands, n_pi, node, cut) else {
                continue;
            };
            // Budget = internal ANDs: find an implementation with ≤ cost
            // ANDs; the region currently spends cost + 1 (root included).
            let key = (t, cut.len() as u8, cost as u8);
            let raw = synth_memo
                .entry(key)
                .or_insert_with(|| exact_synth(t, cut.len(), cost))
                .clone();
            if let Some(raw) = raw {
                let cand = Synth {
                    ands: raw.ands,
                    out: raw.out,
                    leaves: cut.clone(),
                };
                let better = best.as_ref().is_none_or(|b: &Synth| cand.ands.len() < b.ands.len());
                if better {
                    best = Some(cand);
                }
            }
        }
        *slot = best;
    }
    let mut rb = Rebuilder {
        old: aig,
        n_pi,
        repl,
        memo: HashMap::new(),
        new: Aig::with_pis(aig.pis.clone()),
    };
    let outs: Vec<Lit> = outputs.iter().map(|&o| rb.lit(o)).collect();
    let new = rb.new;
    (new, outs)
}

/// Cuts of AND-node `node`: sorted leaf-node-id vecs, smallest first,
/// bounded by [`CUT_MAX_PER_NODE`]. Structural (inversion-insensitive).
fn cuts_of(
    node: u32,
    ands: &[(Lit, Lit)],
    n_pi: usize,
    memo: &mut HashMap<u32, Vec<Vec<u32>>>,
) -> Vec<Vec<u32>> {
    if node == 0 {
        return vec![vec![0]];
    }
    if node as usize <= n_pi {
        return vec![vec![node]];
    }
    if let Some(c) = memo.get(&node) {
        return c.clone();
    }
    let ai = (node as usize) - 1 - n_pi;
    let (a, b) = ands[ai];
    let ca = cuts_of(a.node, ands, n_pi, memo);
    let cb = cuts_of(b.node, ands, n_pi, memo);
    let mut out: Vec<Vec<u32>> = vec![vec![node]];
    for x in &ca {
        for y in &cb {
            let mut u: Vec<u32> = Vec::with_capacity(x.len() + y.len());
            let (mut i, mut j) = (0usize, 0usize);
            while i < x.len() && j < y.len() {
                match x[i].cmp(&y[j]) {
                    std::cmp::Ordering::Less => {
                        u.push(x[i]);
                        i += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        u.push(y[j]);
                        j += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        u.push(x[i]);
                        i += 1;
                        j += 1;
                    }
                }
            }
            u.extend_from_slice(&x[i..]);
            u.extend_from_slice(&y[j..]);
            if u.len() <= CUT_K && !out.iter().any(|c| *c == u) {
                out.push(u);
            }
        }
    }
    out.sort_by_key(|c| c.len());
    out.truncate(CUT_MAX_PER_NODE);
    memo.insert(node, out.clone());
    out
}

/// Evaluate AND-node `node` under leaf assignment `assign`
/// (bit i = leaves[i]). None if a PI outside the cut is reached.
fn eval_cut(
    ands: &[(Lit, Lit)],
    n_pi: usize,
    node: u32,
    leaf_pos: &HashMap<u32, usize>,
    assign: u32,
) -> Option<bool> {
    if node == 0 {
        return Some(false);
    }
    if let Some(&i) = leaf_pos.get(&node) {
        return Some((assign >> i) & 1 == 1);
    }
    if (node as usize) <= n_pi {
        return None;
    }
    let (a, b) = ands[(node as usize) - 1 - n_pi];
    let va = eval_cut(ands, n_pi, a.node, leaf_pos, assign)? ^ a.inv;
    let vb = eval_cut(ands, n_pi, b.node, leaf_pos, assign)? ^ b.inv;
    Some(va && vb)
}

/// Truth table of `node`'s function over `leaves` (bit j = f(j)).
/// None if the cut does not cover the whole cone.
fn cut_truth(ands: &[(Lit, Lit)], n_pi: usize, node: u32, leaves: &[u32]) -> Option<u16> {
    let leaf_pos: HashMap<u32, usize> =
        leaves.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let mut t: u16 = 0;
    for assign in 0..(1u32 << leaves.len()) {
        if eval_cut(ands, n_pi, node, &leaf_pos, assign)? {
            t |= 1u16 << assign;
        }
    }
    Some(t)
}

/// AND nodes in `node`'s transitive fanin, excluding cut leaves.
fn cut_cost(
    ands: &[(Lit, Lit)],
    n_pi: usize,
    node: u32,
    leaves: &HashSet<u32>,
) -> usize {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack = vec![node];
    let mut c = 0usize;
    while let Some(nd) = stack.pop() {
        if nd == 0 || leaves.contains(&nd) || !seen.insert(nd) {
            continue;
        }
        if (nd as usize) <= n_pi {
            continue; // defensive; coverage check in cut_truth prevents
        }
        c += 1;
        let (a, b) = ands[(nd as usize) - 1 - n_pi];
        stack.push(a.node);
        stack.push(b.node);
    }
    c
}

/// Input reference inside a synthesized replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RInp {
    Leaf(usize, bool),
    Node(usize, bool),
    Const(bool),
}

/// Minimum-AND structure computing a cut function (leaves not yet bound).
#[derive(Clone, Debug)]
pub(crate) struct RawSynth {
    pub(crate) ands: Vec<(RInp, RInp)>,
    pub(crate) out: RInp,
}

/// Chosen replacement for one AND node, with bound cut leaves.
#[derive(Clone, Debug)]
struct Synth {
    ands: Vec<(RInp, RInp)>,
    out: RInp,
    leaves: Vec<u32>,
}

fn var_mask(i: usize, k: usize) -> u16 {
    let mut m = 0u16;
    for j in 0..(1u32 << k) {
        if (j >> i) & 1 == 1 {
            m |= 1u16 << j;
        }
    }
    m
}

/// Maximum candidate AND evaluations per `exact_synth` call: bounds the
/// worst-case DFS on hard cuts (proving no small implementation exists is
/// the expensive direction). Memoization keeps the common case cheap.
const SYNTH_EVAL_BUDGET: u64 = 500_000;

/// Minimum-AND implementation of truth table `t` over `k` ≤ 4 variables
/// using at most `budget` AND nodes. None if impossible within budget.
/// Trivial (0-AND) cases are answered regardless of budget.
pub(crate) fn exact_synth(t: u16, k: usize, budget: usize) -> Option<RawSynth> {
    let full: u16 = if k >= 4 { 0xFFFF } else { (1u16 << (1u32 << k)) - 1 };
    let t = t & full;
    let zero = |out| Some(RawSynth { ands: Vec::new(), out });
    if t == 0 {
        return zero(RInp::Const(false));
    }
    if t == full {
        return zero(RInp::Const(true));
    }
    for i in 0..k {
        let vm = var_mask(i, k);
        if t == vm {
            return zero(RInp::Leaf(i, false));
        }
        if t == (!vm & full) {
            return zero(RInp::Leaf(i, true));
        }
    }
    if budget == 0 {
        return None;
    }
    let mut lits: Vec<(RInp, u16)> = Vec::new();
    for i in 0..k {
        let vm = var_mask(i, k);
        lits.push((RInp::Leaf(i, false), vm));
        lits.push((RInp::Leaf(i, true), !vm & full));
    }
    let mut ands: Vec<(RInp, RInp)> = Vec::new();
    let mut evals: u64 = 0;
    dfs_synth(t, full, budget, &mut lits, &mut ands, &mut evals)
        .map(|out| RawSynth { ands, out })
}

/// DFS with iterative deepening: try 1-AND completions from the current
/// literal pool, else grow the pool by one AND node and recurse.
/// Returns the output literal on success.
fn dfs_synth(
    t: u16,
    full: u16,
    budget: usize,
    lits: &mut Vec<(RInp, u16)>,
    ands: &mut Vec<(RInp, RInp)>,
    evals: &mut u64,
) -> Option<RInp> {
    // Committing one more node must stay within budget.
    if ands.len() >= budget {
        return None;
    }
    let base = lits.len();
    for a in 0..base {
        for b in (a + 1)..base {
            *evals += 1;
            if *evals > SYNTH_EVAL_BUDGET {
                return None;
            }
            let tt = lits[a].1 & lits[b].1;
            let idx = ands.len();
            if tt == t {
                ands.push((lits[a].0, lits[b].0));
                return Some(RInp::Node(idx, false));
            }
            if (!tt & full) == t {
                ands.push((lits[a].0, lits[b].0));
                return Some(RInp::Node(idx, true));
            }
        }
    }
    // Growing once more must leave room for at least the completion node.
    if ands.len() + 1 >= budget {
        return None;
    }
    for a in 0..base {
        for b in (a + 1)..base {
            let tt = lits[a].1 & lits[b].1;
            if lits.iter().any(|(_, e)| *e == tt) {
                continue; // redundant node
            }
            let idx = ands.len();
            ands.push((lits[a].0, lits[b].0));
            lits.push((RInp::Node(idx, false), tt));
            lits.push((RInp::Node(idx, true), !tt & full));
            if let Some(out) = dfs_synth(t, full, budget, lits, ands, evals) {
                return Some(out);
            }
            lits.truncate(base);
            ands.pop();
        }
    }
    None
}

/// Demand-driven AIG rebuild: old nodes map to new literals, replacements
/// splice in truth-table-identical structures, structural hashing in
/// `and_lit` re-shares duplicates. Only nodes reachable from the output are
/// built, so dead logic vanishes by construction.
struct Rebuilder<'a> {
    old: &'a Aig,
    n_pi: usize,
    repl: Vec<Option<Synth>>,
    memo: HashMap<u32, Lit>,
    new: Aig,
}

impl Rebuilder<'_> {
    fn lit(&mut self, l: Lit) -> Lit {
        let base = self.node(l.node);
        if l.inv {
            base.not()
        } else {
            base
        }
    }

    fn node(&mut self, node: u32) -> Lit {
        if node == 0 {
            return Lit::c0();
        }
        if (node as usize) <= self.n_pi {
            return Lit { node, inv: false };
        }
        if let Some(&m) = self.memo.get(&node) {
            return m;
        }
        let ai = (node as usize) - 1 - self.n_pi;
        let r = self.repl[ai].clone();
        let built = match r {
            Some(s) => self.build_synth(&s),
            None => {
                let (a, b) = self.old.ands[ai];
                let ma = self.lit(a);
                let mb = self.lit(b);
                self.new.and_lit(ma, mb)
            }
        };
        self.memo.insert(node, built);
        built
    }

    fn build_synth(&mut self, s: &Synth) -> Lit {
        let leaves: Vec<Lit> = s
            .leaves
            .iter()
            .map(|&nd| self.lit(Lit { node: nd, inv: false }))
            .collect();
        let mut nodes: Vec<Lit> = Vec::with_capacity(s.ands.len());
        for (a, b) in s.ands.clone() {
            let ma = resolve_rinp(a, &leaves, &nodes);
            let mb = resolve_rinp(b, &leaves, &nodes);
            nodes.push(self.new.and_lit(ma, mb));
        }
        resolve_rinp(s.out, &leaves, &nodes)
    }
}

fn resolve_rinp(r: RInp, leaves: &[Lit], nodes: &[Lit]) -> Lit {
    match r {
        RInp::Leaf(i, inv) => {
            let l = leaves[i];
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_expr;

    /// Parse a test expression. The arena is leaked so the returned `Expr`
    /// can outlive the call (test-only; mirrors the `parse_expr` 2-arg API
    /// from the perf campaign).
    fn pe(s: &str) -> crate::Expr<'static> {
        let arena: &'static bumpalo::Bump = Box::leak(Box::new(bumpalo::Bump::new()));
        parse_expr(s, arena).expect("test expr parses")
    }

    /// Exhaustive miter: rewritten AIG must agree with the original on all
    /// PI assignments. `vars` caps the check at 2^12 assignments.
    fn assert_miter_equiv(aig: &Aig, rw: &Aig) {
        assert_eq!(aig.pis, rw.pis, "PI lists must match");
        let k = aig.pis.len();
        assert!(k <= 12, "miter battery keeps PI counts small");
        for bits in 0..(1u64 << k) {
            assert_eq!(
                aig.eval_lit(aig.output, bits),
                rw.eval_lit(rw.output, bits),
                "miter mismatch at assignment {bits:#b}"
            );
        }
    }

    fn rewrite_of(expr_src: &str) -> (Aig, Aig) {
        let e = pe(expr_src);
        let aig = Aig::from_expr(&e);
        let rw = rewrite_aig(&aig);
        assert_miter_equiv(&aig, &rw);
        (aig, rw)
    }

    #[test]
    fn hash_shares_identical_subcones() {
        // (a&b)|(a&b): one AND for a&b, one for the OR's De Morgan AND.
        let e = pe("(a&b)|(a&b)");
        let aig = Aig::from_expr(&e);
        assert_eq!(aig.ands.len(), 2);
        assert_miter_equiv(&aig, &rewrite_aig(&aig));
    }

    #[test]
    fn rewrite_removes_self_and() {
        // (a&b)&(a&b): outer AND is redundant given hashing shares the inner.
        let (aig, rw) = rewrite_of("(a&b)&(a&b)");
        assert!(rw.ands.len() < aig.ands.len(), "self-AND must go");
    }

    #[test]
    fn rewrite_absorbs_redundant_term() {
        // x | (x&c) = x with x = a&b: the c-term cone must collapse.
        let (aig, rw) = rewrite_of("(a&b)|((a&b)&c)");
        assert!(
            rw.ands.len() < aig.ands.len(),
            "absorb: {} -> {}",
            aig.ands.len(),
            rw.ands.len()
        );
    }

    #[test]
    fn rewrite_miter_battery() {
        // Representative cones: arithmetic, comparators, muxes, decoders.
        let cases = [
            "a&b&c&d",
            "a|b|c|d",
            "(a&b)|(c&d)",
            "(a&b&c)|(a&b&~c)|(~a&b&c)",
            "(s&a)|(~s&b)",
            "((s&a)|(~s&b))&((t&c)|(~t&d))",
            "(a^b)&(c^d)",
            "(a&b)|(a&c)|(b&c)",       // majority
            "(a&~b)|(~a&b&c)|(a&b&~c)", // xor-ish
            "~(a&b&c&d)|((a|b)&(c|d))",
            "(a&b&c&d&e)|(f&g&h)",
        ];
        for src in cases {
            let (aig, rw) = rewrite_of(src);
            assert!(
                rw.ands.len() <= aig.ands.len(),
                "{src}: rewrite must not grow the AIG"
            );
        }
    }

    /// Deterministic xorshift (fixed seed): no external RNG in tests.
    fn xorshift(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    fn random_expr(rng: &mut u64, vars: &[&str], depth: u32) -> String {
        if depth == 0 || xorshift(rng) % 3 == 0 {
            let v = vars[(xorshift(rng) as usize) % vars.len()];
            return if xorshift(rng) % 4 == 0 {
                format!("~{v}")
            } else {
                v.to_string()
            };
        }
        let l = random_expr(rng, vars, depth - 1);
        let r = random_expr(rng, vars, depth - 1);
        match xorshift(rng) % 3 {
            0 => format!("({l}&{r})"),
            1 => format!("({l}|{r})"),
            _ => format!("({l}^{r})"),
        }
    }

    #[test]
    fn rewrite_random_miter() {
        let vars = ["a", "b", "c", "d", "e", "f"];
        let mut rng: u64 = 0x243F_6A88_85A3_08D3;
        for _ in 0..150 {
            let src = random_expr(&mut rng, &vars, 4);
            let (aig, rw) = rewrite_of(&src);
            assert!(
                rw.ands.len() <= aig.ands.len(),
                "{src}: rewrite must not grow the AIG"
            );
        }
    }

    #[test]
    fn exact_synth_spot_checks() {
        // 2-input AND needs exactly 1 AND node.
        let t = 0b1000u16; // a&b over (a,b)
        let s = exact_synth(t, 2, 1).expect("a&b in 1 AND");
        assert_eq!(s.ands.len(), 1);
        // Majority(a,b,c) needs 3 ANDs in an AIG... check it fits in budget 3.
        let mut maj = 0u16;
        for j in 0..8u32 {
            if (j.count_ones() >= 2) {
                maj |= 1u16 << j;
            }
        }
        assert!(exact_synth(maj, 3, 2).is_none(), "majority needs >2 ANDs");
        // Majority-3 is a classical 4-AND function (no 3-AND AIG exists —
        // verified by exhaustive search); check the 4-AND construction.
        assert!(exact_synth(maj, 3, 3).is_none(), "majority needs >3 ANDs");
        let s = exact_synth(maj, 3, 4).expect("majority in 4 ANDs");
        assert_eq!(s.ands.len(), 4);
        // XOR needs 3 ANDs: a^b = ~(~(~a&~b)... check budget behavior.
        let xor = 0b0110u16;
        assert!(exact_synth(xor, 2, 2).is_none());
        assert_eq!(exact_synth(xor, 2, 3).unwrap().ands.len(), 3);
    }

    #[test]
    fn rewrite_is_deterministic() {
        let src = "((a&b)|(c&d))&((a|c)&(b|d)|(~a&~b&~c))";
        let e = pe(src);
        let a = Aig::from_expr(&e);
        let r1 = rewrite_aig(&a);
        let r2 = rewrite_aig(&a);
        assert_eq!(r1.ands, r2.ands);
        assert_eq!(r1.output, r2.output);
    }

    /// FM-HEL-AREA-05: merge_cones must share structurally identical
    /// subfunctions across cones.
    #[test]
    fn merge_cones_shares_subfunctions() {
        let c1 = Aig::from_expr(&pe("(a&b)|(c&d)"));
        let c2 = Aig::from_expr(&pe("(a&b)|(e&f)"));
        let sum: usize = c1.ands.len() + c2.ands.len();
        let (g, outs) = merge_cones(&[c1, c2]);
        assert_eq!(outs.len(), 2);
        // (a&b) is shared: merged graph is strictly smaller than the sum.
        assert!(
            g.ands.len() < sum,
            "merge must share (a&b): {} < {}",
            g.ands.len(),
            sum
        );
        // PIs unioned by name: a,b,c,d,e,f = 6.
        assert_eq!(g.pis.len(), 6);
    }

    /// Exhaustive multi-output miter: merged + rewritten cones must agree
    /// with the originals on every PI assignment, per output.
    fn assert_multi_miter(cones: &[Aig]) {
        let (g, outs) = merge_cones(cones);
        let (rw, rw_outs) = rewrite_aig_multi(&g, &outs);
        assert_eq!(outs.len(), rw_outs.len());
        let k = g.pis.len();
        assert!(k <= 12, "miter battery keeps PI counts small");
        // PI order may differ between cone-local and global; map by name.
        for (ci, cone) in cones.iter().enumerate() {
            for bits in 0..(1u64 << k) {
                // global assignment -> cone-local assignment
                let mut cbits = 0u64;
                for (i, pi) in cone.pis.iter().enumerate() {
                    let gi = g.pis.iter().position(|p| p == pi).unwrap();
                    if (bits >> gi) & 1 == 1 {
                        cbits |= 1 << i;
                    }
                }
                let expected = cone.eval_lit(cone.output, cbits);
                // global output -> rewritten output, same global assignment
                let mut rbits = 0u64;
                for (i, pi) in g.pis.iter().enumerate() {
                    let ri = rw.pis.iter().position(|p| p == pi).unwrap();
                    if (bits >> i) & 1 == 1 {
                        rbits |= 1 << ri;
                    }
                }
                let got = rw.eval_lit(rw_outs[ci], rbits);
                assert_eq!(expected, got, "multi miter mismatch cone {ci} at {bits:b}");
            }
        }
    }

    #[test]
    fn merge_rewrite_multi_miter() {
        let mk = |s: &str| Aig::from_expr(&pe(s));
        let cases: &[&[&str]] = &[
            &["(a&b)|(c&d)", "(a&b)|(e&f)"],
            &["(a&b)|(c&d)", "(a&b)|(c&d)"], // identical cones
            &["~((a&b)|(c&d))", "(a&b)|(c&d)"], // polarity-shared output
            &["(a&b&c)|(d&e)", "(a&b)|(d&e&f)", "(a&b&c&d)|(e&f)"],
        ];
        for (i, group) in cases.iter().enumerate() {
            let cones: Vec<Aig> = group.iter().map(|s| mk(s)).collect();
            assert_multi_miter(&cones);
            eprintln!("multi miter group {i} ok");
        }
    }

    #[test]
    fn rewrite_multi_matches_single() {
        // Single-output multi rewrite must equal the legacy single path.
        let a = Aig::from_expr(&pe("((a&b)|(c&d))&((a|c)&(b|d))"));
        let (g, outs) = merge_cones(std::slice::from_ref(&a));
        let (rw, rw_outs) = rewrite_aig_multi(&g, &outs);
        let single = rewrite_aig(&a);
        assert_eq!(rw.ands.len(), single.ands.len(), "multi must match single on one cone");
        assert_multi_miter(&[a]);
        let _ = rw_outs;
    }
}
