//! Tseitin CNF encoding of [`Aig`]s.
//!
//! Variables are DIMACS 1-based (`u32`); literals are signed `i32`
//! (`+v` = positive, `-v` = negative). Node 0 (const0) shares one constant
//! variable constrained false by a unit clause. Inversion on AIG edges folds
//! into literal polarity, so NOT costs no variables or clauses.

use crate::{Aig, Lit as AigLit};
use std::collections::HashMap;

/// Tseitin encoder: allocates DIMACS variables and accumulates clauses.
#[derive(Clone, Debug, Default)]
pub struct CnfEncoder {
    clauses: Vec<Vec<i32>>,
    next_var: u32,
    const0: u32,
}

impl CnfEncoder {
    /// New encoder with the shared const0 variable (unit clause `-c`).
    pub fn new() -> Self {
        let mut enc = Self {
            clauses: Vec::new(),
            next_var: 1,
            const0: 0,
        };
        let c = enc.new_var();
        enc.const0 = c;
        enc.clauses.push(vec![-(c as i32)]);
        enc
    }

    /// Allocate a fresh DIMACS variable (1-based).
    pub fn new_var(&mut self) -> u32 {
        let v = self.next_var;
        self.next_var += 1;
        v
    }

    /// Number of variables allocated so far.
    pub fn num_vars(&self) -> u32 {
        self.next_var - 1
    }

    /// All accumulated clauses (each a vec of signed DIMACS literals).
    pub fn clauses(&self) -> &[Vec<i32>] {
        &self.clauses
    }

    /// Fresh variable `o` constrained to `o = a AND b` (3 clauses).
    pub fn encode_and(&mut self, a: i32, b: i32) -> i32 {
        let o = self.new_var() as i32;
        self.clauses.push(vec![-a, -b, o]);
        self.clauses.push(vec![a, -o]);
        self.clauses.push(vec![b, -o]);
        o
    }

    /// Fresh variable `w` constrained to `w = a XOR b` (4 clauses).
    pub fn encode_xor(&mut self, a: i32, b: i32) -> i32 {
        let w = self.new_var() as i32;
        self.clauses.push(vec![-a, -b, -w]);
        self.clauses.push(vec![a, b, -w]);
        self.clauses.push(vec![-a, b, w]);
        self.clauses.push(vec![a, -b, w]);
        w
    }

    /// Fresh variable `m` constrained to `m = OR(lits)` (n+1 clauses).
    /// Requires at least one input literal.
    pub fn encode_or(&mut self, lits: &[i32]) -> i32 {
        let m = self.new_var() as i32;
        for &w in lits {
            self.clauses.push(vec![-w, m]);
        }
        let mut big: Vec<i32> = lits.to_vec();
        big.push(-m);
        self.clauses.push(big);
        m
    }

    /// Unit clause asserting `lit` true.
    pub fn assert_unit(&mut self, lit: i32) {
        self.clauses.push(vec![lit]);
    }

    /// Encode a whole AIG in node-id order (deterministic: node 0, then PIs
    /// 1..=n, then ANDs in push order). PI variables are shared by name via
    /// `pi_vars`, which is what makes a two-AIG miter a miter.
    /// Returns the signed literal for every node id (index = node id).
    pub fn encode_aig(
        &mut self,
        aig: &Aig,
        pi_vars: &mut HashMap<String, u32>,
    ) -> Vec<i32> {
        // Node ids are dense: 0, PIs 1..=n_pis, ANDs 1+n_pis+j.
        let mut node_lit = vec![0i32; 1 + aig.pis.len() + aig.ands.len()];
        node_lit[0] = self.const0 as i32;
        for (i, name) in aig.pis.iter().enumerate() {
            let v = match pi_vars.get(name) {
                Some(&v) => v,
                None => {
                    let v = self.new_var();
                    pi_vars.insert(name.clone(), v);
                    v
                }
            };
            node_lit[1 + i] = v as i32;
        }
        for (j, &(a, b)) in aig.ands.iter().enumerate() {
            let la = signed_lit(node_lit[a.node as usize], a.inv);
            let lb = signed_lit(node_lit[b.node as usize], b.inv);
            node_lit[1 + aig.pis.len() + j] = self.encode_and(la, lb);
        }
        node_lit
    }

    /// DIMACS text (`p cnf` header + clauses), for debugging and for
    /// cross-checking with `varisat-cli`.
    pub fn to_dimacs(&self) -> String {
        let mut s = format!("p cnf {} {}\n", self.num_vars(), self.clauses.len());
        for c in &self.clauses {
            for l in c {
                s.push_str(&format!("{l} "));
            }
            s.push_str("0\n");
        }
        s
    }

    /// Signed literal for helion [`AigLit`] given its node's variable literal.
    pub fn lit_signed(&self, lit: AigLit, node_lits: &[i32]) -> i32 {
        signed_lit(node_lits[lit.node as usize], lit.inv)
    }
}

/// Apply edge inversion to a node's (positive) variable literal.
pub(crate) fn signed_lit(var_lit: i32, inv: bool) -> i32 {
    if inv {
        -var_lit
    } else {
        var_lit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute-force: is there a PI assignment making `out` true? (PIs <= 8.)
    fn brute_force_sat(aig: &Aig, out: AigLit) -> bool {
        let n = aig.pis.len();
        for bits in 0..(1u64 << n) {
            if aig.eval_lit(out, bits) {
                return true;
            }
        }
        false
    }

    /// Encode a single-output AIG with its output unit-claused, solve with the
    /// varisat backend, and check SAT agrees with brute force.
    fn check_sat_iff_bruteforce(aig: &Aig, out: AigLit) {
        use crate::equiv::solver::{SolverBackend, VarisatBackend};
        let mut enc = CnfEncoder::new();
        let mut pi_vars = HashMap::new();
        let node_lits = enc.encode_aig(aig, &mut pi_vars);
        let out_signed = enc.lit_signed(out, &node_lits);
        enc.assert_unit(out_signed);
        let mut backend = VarisatBackend::new(enc.num_vars());
        for c in enc.clauses() {
            backend.add_clause(c);
        }
        let sat = backend.solve().expect("solver error in test");
        assert_eq!(
            sat,
            brute_force_sat(aig, out),
            "Tseitin SAT disagrees with brute force (pis={:?})",
            aig.pis
        );
    }

    fn aig_with_pis(pis: &[&str]) -> Aig {
        let mut a = Aig::with_pis(Vec::new());
        for p in pis {
            a.pi_lit(p);
        }
        a
    }

    #[test]
    fn tseitin_and_chain() {
        // out = a & b & c
        let mut a = aig_with_pis(&["a", "b", "c"]);
        let la = a.pi_lit("a");
        let lb = a.pi_lit("b");
        let lc = a.pi_lit("c");
        let t = a.and_lit(la, lb);
        let out = a.and_lit(t, lc);
        check_sat_iff_bruteforce(&a, out);
    }

    #[test]
    fn tseitin_or_via_demorgan() {
        // out = a | b | c = ~((~a) & (~b) & (~c))
        let mut a = aig_with_pis(&["a", "b", "c"]);
        let la = a.pi_lit("a").not();
        let lb = a.pi_lit("b").not();
        let lc = a.pi_lit("c").not();
        let t = a.and_lit(la, lb);
        let u = a.and_lit(t, lc);
        check_sat_iff_bruteforce(&a, u.not());
    }

    #[test]
    fn tseitin_xor_chain() {
        // out = a ^ b ^ c (parity)
        let mut a = aig_with_pis(&["a", "b", "c"]);
        let la = a.pi_lit("a");
        let lb = a.pi_lit("b");
        let lc = a.pi_lit("c");
        let xor = |g: &mut Aig, x: AigLit, y: AigLit| {
            let t1 = g.and_lit(x, y.not());
            let t2 = g.and_lit(x.not(), y);
            g.and_lit(t1.not(), t2.not()).not()
        };
        let t = xor(&mut a, la, lb);
        let out = xor(&mut a, t, lc);
        check_sat_iff_bruteforce(&a, out);
    }

    #[test]
    fn tseitin_majority_and_not() {
        // out = ~(maj(a,b,c)) where maj = (a&b)|(a&c)|(b&c)
        let mut a = aig_with_pis(&["a", "b", "c"]);
        let la = a.pi_lit("a");
        let lb = a.pi_lit("b");
        let lc = a.pi_lit("c");
        let ab = a.and_lit(la, lb);
        let ac = a.and_lit(la, lc);
        let bc = a.and_lit(lb, lc);
        // (ab | ac) = ~((~ab) & (~ac)); then | bc
        let or1 = a.and_lit(ab.not(), ac.not()).not();
        let maj = a.and_lit(or1.not(), bc.not()).not();
        check_sat_iff_bruteforce(&a, maj.not());
    }

    #[test]
    fn tseitin_const_outputs() {
        // out = c0 is UNSAT; out = c1 is SAT.
        let a = aig_with_pis(&["a", "b"]);
        check_sat_iff_bruteforce(&a, AigLit::c0());
        check_sat_iff_bruteforce(&a, AigLit::c1());
    }

    #[test]
    fn tseitin_shared_subexpression() {
        // out = (a & b) ^ (a & b) = 0, always UNSAT; exercises structural hashing.
        let mut a = aig_with_pis(&["a", "b"]);
        let la = a.pi_lit("a");
        let lb = a.pi_lit("b");
        let t1 = a.and_lit(la, lb);
        let t2 = a.and_lit(la, lb);
        assert_eq!(t1.node, t2.node, "structural hashing should share the AND");
        let x = a.and_lit(t1, t2.not());
        let y = a.and_lit(t1.not(), t2);
        let out = a.and_lit(x.not(), y.not()).not();
        check_sat_iff_bruteforce(&a, out);
        assert!(!brute_force_sat(&a, out));
    }

    #[test]
    fn tseitin_4bit_parity() {
        // 4-bit parity over 4 PIs: SAT (e.g. a=1 rest 0).
        let mut a = aig_with_pis(&["a", "b", "c", "d"]);
        let lits: Vec<AigLit> = ["a", "b", "c", "d"].iter().map(|p| a.pi_lit(p)).collect();
        let xor = |g: &mut Aig, x: AigLit, y: AigLit| {
            let t1 = g.and_lit(x, y.not());
            let t2 = g.and_lit(x.not(), y);
            g.and_lit(t1.not(), t2.not()).not()
        };
        let t = xor(&mut a, lits[0], lits[1]);
        let u = xor(&mut a, lits[2], lits[3]);
        let out = xor(&mut a, t, u);
        check_sat_iff_bruteforce(&a, out);
        assert!(brute_force_sat(&a, out));
    }

    #[test]
    fn dimacs_roundtrip_shape() {
        // o = a & b: 1 const unit + 2 PI vars + 1 AND var = 3 vars, 4 clauses.
        let mut a = aig_with_pis(&["x", "y"]);
        let lx = a.pi_lit("x");
        let ly = a.pi_lit("y");
        let o = a.and_lit(lx, ly);
        let mut enc = CnfEncoder::new();
        let mut pi_vars = HashMap::new();
        let node_lits = enc.encode_aig(&a, &mut pi_vars);
        let _ = enc.lit_signed(o, &node_lits);
        assert_eq!(enc.num_vars(), 3);
        assert_eq!(enc.clauses().len(), 4);
        let dimacs = enc.to_dimacs();
        assert!(dimacs.starts_with("p cnf 3 4\n"));
        assert_eq!(dimacs.lines().count(), 5);
    }
}
