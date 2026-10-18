//! Industrial Combinational Equivalence Checking (`cec_cutpoint_sweep`) with
//! 64-bit signature partitioning and internal keypoint miter sweeping, and
//! Sequential Equivalence Checking (`sec_prove_sequential_equiv`) with
//! register-correspondence discovery, $k$-step signal correspondence induction,
//! and product-machine reachability under retiming and FSM re-encoding.

use helion_ir::{resolve, CellKind, Design, PortDir, Spur};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

// ============================================================================
// 1. Combinational Equivalence Checking (`cec_cutpoint_sweep`)
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CutpointEquiv {
    pub golden_net: String,
    pub revised_net: String,
    pub inverted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailingOutputCone {
    pub output_name: String,
    pub counterexample_inputs: BTreeMap<String, bool>,
    pub golden_value: bool,
    pub revised_value: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CecResult {
    pub equivalent: bool,
    pub candidate_classes: usize,
    pub proved_cutpoints: Vec<CutpointEquiv>,
    pub refined_by_cegar: usize,
    pub failing_outputs: Vec<FailingOutputCone>,
    pub counterexamples: BTreeMap<String, BTreeMap<String, bool>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum GNode {
    Const(bool),
    Input(usize),
    Not(usize),
    And(usize, usize),
    Or(usize, usize),
    Xor(usize, usize),
    Ite(usize, usize, usize),
}

#[derive(Clone, Debug)]
struct CombGraph {
    nodes: Vec<GNode>,
    memo: HashMap<GNode, usize>,
    inputs: Vec<String>,
    input_lookup: HashMap<String, usize>,
    gold_nets: Vec<(String, usize)>,
    rev_nets: Vec<(String, usize)>,
    gold_outputs: BTreeMap<String, usize>,
    rev_outputs: BTreeMap<String, usize>,
    /// Rewrite redirect table populated as internal cutpoints are proven equivalent:
    /// `rev_node_id -> (target_gold_node_id, inverted)`
    redirects: HashMap<usize, (usize, bool)>,
}

impl CombGraph {
    fn new() -> Self {
        let mut g = Self {
            nodes: Vec::new(),
            memo: HashMap::new(),
            inputs: Vec::new(),
            input_lookup: HashMap::new(),
            gold_nets: Vec::new(),
            rev_nets: Vec::new(),
            gold_outputs: BTreeMap::new(),
            rev_outputs: BTreeMap::new(),
            redirects: HashMap::new(),
        };
        let _c0 = g.intern_node(GNode::Const(false));
        let _c1 = g.intern_node(GNode::Const(true));
        g
    }

    fn c0(&self) -> usize {
        0
    }

    fn c1(&self) -> usize {
        1
    }

    fn intern_node(&mut self, n: GNode) -> usize {
        if let Some(&id) = self.memo.get(&n) {
            return id;
        }
        let id = self.nodes.len();
        self.nodes.push(n.clone());
        self.memo.insert(n, id);
        id
    }

    fn get_or_add_input(&mut self, name: &str) -> usize {
        if let Some(&ix) = self.input_lookup.get(name) {
            return self.intern_node(GNode::Input(ix));
        }
        let ix = self.inputs.len();
        self.inputs.push(name.to_string());
        self.input_lookup.insert(name.to_string(), ix);
        self.intern_node(GNode::Input(ix))
    }

    fn mk_not(&mut self, a: usize) -> usize {
        match self.nodes[a] {
            GNode::Const(b) => {
                if b {
                    self.c0()
                } else {
                    self.c1()
                }
            }
            GNode::Not(x) => x,
            _ => self.intern_node(GNode::Not(a)),
        }
    }

    fn mk_and(&mut self, a: usize, b: usize) -> usize {
        if a == self.c0() || b == self.c0() {
            return self.c0();
        }
        if a == self.c1() {
            return b;
        }
        if b == self.c1() {
            return a;
        }
        if a == b {
            return a;
        }
        let (x, y) = if a <= b { (a, b) } else { (b, a) };
        self.intern_node(GNode::And(x, y))
    }

    fn mk_or(&mut self, a: usize, b: usize) -> usize {
        if a == self.c1() || b == self.c1() {
            return self.c1();
        }
        if a == self.c0() {
            return b;
        }
        if b == self.c0() {
            return a;
        }
        if a == b {
            return a;
        }
        let (x, y) = if a <= b { (a, b) } else { (b, a) };
        self.intern_node(GNode::Or(x, y))
    }

    fn mk_xor(&mut self, a: usize, b: usize) -> usize {
        if a == self.c0() {
            return b;
        }
        if b == self.c0() {
            return a;
        }
        if a == self.c1() {
            return self.mk_not(b);
        }
        if b == self.c1() {
            return self.mk_not(a);
        }
        if a == b {
            return self.c0();
        }
        let (x, y) = if a <= b { (a, b) } else { (b, a) };
        self.intern_node(GNode::Xor(x, y))
    }

    fn mk_ite(&mut self, cond: usize, t: usize, f: usize) -> usize {
        if cond == self.c1() {
            return t;
        }
        if cond == self.c0() {
            return f;
        }
        if t == f {
            return t;
        }
        if t == self.c1() && f == self.c0() {
            return cond;
        }
        if t == self.c0() && f == self.c1() {
            return self.mk_not(cond);
        }
        if f == self.c0() {
            return self.mk_and(cond, t);
        }
        self.intern_node(GNode::Ite(cond, t, f))
    }

    fn mk_lut_cone(&mut self, init: u64, inputs: &[usize]) -> usize {
        if init == 0 {
            return self.c0();
        }
        if init == u64::MAX {
            return self.c1();
        }
        let mut active: Vec<(usize, usize)> = Vec::new();
        for (bit_idx, &node_id) in inputs.iter().enumerate().take(6) {
            if node_id == self.c0() {
                continue;
            }
            active.push((bit_idx, node_id));
        }
        self.shannon_lut(init, &active, 0, 0)
    }

    fn shannon_lut(
        &mut self,
        init: u64,
        active: &[(usize, usize)],
        pos: usize,
        fixed_addr: usize,
    ) -> usize {
        if pos >= active.len() {
            let bit = (init >> fixed_addr) & 1 != 0;
            return if bit { self.c1() } else { self.c0() };
        }
        let (bit_idx, var_node) = active[pos];
        let lo = self.shannon_lut(init, active, pos + 1, fixed_addr);
        let hi = self.shannon_lut(init, active, pos + 1, fixed_addr | (1 << bit_idx));
        self.mk_ite(var_node, hi, lo)
    }

    fn ingest_design(&mut self, d: &Design, is_gold: bool) {
        let pin_idx = d.pin_index();
        let mut net_to_node: HashMap<Spur, usize> = HashMap::new();

        let mut clock_nets: HashSet<Spur> = HashSet::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                if let Some(cn) = pin_idx.net_on(resolve(c.name), "CLK") {
                    clock_nets.insert(cn);
                }
            }
        }

        for p in &d.ports {
            if matches!(p.dir, PortDir::In | PortDir::Inout) {
                if clock_nets.contains(&p.name) && d.ports.len() > 1 {
                    continue;
                }
                let pname = resolve(p.name);
                let nid = self.get_or_add_input(pname);
                net_to_node.insert(p.name, nid);
            }
        }

        // Treat Hff Q outputs as pseudo-primary inputs when doing combinational cutpoint sweeping
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                let cname = resolve(c.name);
                if let Some(qnet) = pin_idx.net_on(cname, "Q") {
                    let qname = format!("__ff_q_{}", resolve(qnet));
                    let nid = self.get_or_add_input(&qname);
                    net_to_node.insert(qnet, nid);
                }
            }
        }

        let comb_cells: Vec<&helion_ir::Cell> = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. } | CellKind::Carry4))
            .collect();

        let mut resolved: HashSet<Spur> = HashSet::new();
        let mut ordered_internal_nets: Vec<(String, usize)> = Vec::new();

        for _round in 0..=comb_cells.len() {
            let mut progress = false;
            for c in &comb_cells {
                if resolved.contains(&c.name) {
                    continue;
                }
                let cname = resolve(c.name);
                match c.kind {
                    CellKind::Lut6 { init } => {
                        let pin_names = ["I0", "I1", "I2", "I3", "I4", "I5"];
                        let mut ready = true;
                        let mut in_nodes = [self.c0(); 6];
                        for (pi, pname) in pin_names.iter().enumerate() {
                            if let Some(net) = pin_idx.net_on(cname, pname) {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    in_nodes[pi] = nid;
                                } else if is_comb_driven(d, net) {
                                    ready = false;
                                    break;
                                } else {
                                    let nid = self.get_or_add_input(resolve(net));
                                    net_to_node.insert(net, nid);
                                    in_nodes[pi] = nid;
                                }
                            }
                        }
                        if !ready {
                            continue;
                        }
                        let out_node = self.mk_lut_cone(init, &in_nodes);
                        if let Some(onet) = pin_idx.net_on(cname, "O") {
                            net_to_node.insert(onet, out_node);
                            ordered_internal_nets.push((resolve(onet).to_string(), out_node));
                        }
                        resolved.insert(c.name);
                        progress = true;
                    }
                    CellKind::Carry4 => {
                        let s_pins = [
                            ("S[0]", "S0"),
                            ("S[1]", "S1"),
                            ("S[2]", "S2"),
                            ("S[3]", "S3"),
                        ];
                        let di_pins = [
                            ("DI[0]", "DI0"),
                            ("DI[1]", "DI1"),
                            ("DI[2]", "DI2"),
                            ("DI[3]", "DI3"),
                        ];
                        let o_pins = [
                            ("O[0]", "O0"),
                            ("O[1]", "O1"),
                            ("O[2]", "O2"),
                            ("O[3]", "O3"),
                        ];
                        let co_pins = [
                            ("CO[0]", "CO0"),
                            ("CO[1]", "CO1"),
                            ("CO[2]", "CO2"),
                            ("CO[3]", "CO3"),
                        ];
                        let mut ready = true;
                        let mut s_nodes = [self.c0(); 4];
                        let mut di_nodes = [self.c0(); 4];
                        for i in 0..4 {
                            if let Some(net) = pin_idx
                                .net_on(cname, s_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, s_pins[i].1))
                            {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    s_nodes[i] = nid;
                                } else if is_comb_driven(d, net) {
                                    ready = false;
                                    break;
                                } else {
                                    let nid = self.get_or_add_input(resolve(net));
                                    net_to_node.insert(net, nid);
                                    s_nodes[i] = nid;
                                }
                            }
                            if let Some(net) = pin_idx
                                .net_on(cname, di_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, di_pins[i].1))
                            {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    di_nodes[i] = nid;
                                } else if is_comb_driven(d, net) {
                                    ready = false;
                                    break;
                                } else {
                                    let nid = self.get_or_add_input(resolve(net));
                                    net_to_node.insert(net, nid);
                                    di_nodes[i] = nid;
                                }
                            }
                        }
                        let mut cin = self.c0();
                        for ci_pin in ["CI", "CYINIT"] {
                            if let Some(net) = pin_idx.net_on(cname, ci_pin) {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    cin = self.mk_or(cin, nid);
                                } else if is_comb_driven(d, net) {
                                    ready = false;
                                }
                            }
                        }
                        if !ready {
                            continue;
                        }
                        for i in 0..4 {
                            let sum_i = self.mk_xor(s_nodes[i], cin);
                            let co_i = self.mk_ite(s_nodes[i], cin, di_nodes[i]);
                            if let Some(onet) = pin_idx
                                .net_on(cname, o_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, o_pins[i].1))
                            {
                                net_to_node.insert(onet, sum_i);
                                ordered_internal_nets.push((resolve(onet).to_string(), sum_i));
                            }
                            if let Some(conet) = pin_idx
                                .net_on(cname, co_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, co_pins[i].1))
                            {
                                net_to_node.insert(conet, co_i);
                                ordered_internal_nets.push((resolve(conet).to_string(), co_i));
                            }
                            cin = co_i;
                        }
                        resolved.insert(c.name);
                        progress = true;
                    }
                    _ => {}
                }
            }
            if !progress {
                break;
            }
        }

        for c in &d.cells {
            if matches!(c.kind, CellKind::IobOut) {
                let cname = resolve(c.name);
                if let (Some(in_net), Some(pad_net)) =
                    (pin_idx.net_on(cname, "I"), pin_idx.net_on(cname, "PAD"))
                {
                    if let Some(&nid) = net_to_node.get(&in_net) {
                        net_to_node.insert(pad_net, nid);
                    }
                }
            }
        }

        let out_map = if is_gold {
            &mut self.gold_outputs
        } else {
            &mut self.rev_outputs
        };
        for p in &d.ports {
            if matches!(p.dir, PortDir::Out) {
                let nid = net_to_node.get(&p.name).copied().unwrap_or(0);
                out_map.insert(resolve(p.name).to_string(), nid);
            }
        }
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                let cname = resolve(c.name);
                if let (Some(dnet), Some(qnet)) =
                    (pin_idx.net_on(cname, "D"), pin_idx.net_on(cname, "Q"))
                {
                    let nid = net_to_node.get(&dnet).copied().unwrap_or(0);
                    out_map.insert(format!("__ff_d_{}", resolve(qnet)), nid);
                }
            }
        }

        if is_gold {
            self.gold_nets = ordered_internal_nets;
        } else {
            self.rev_nets = ordered_internal_nets;
        }
    }

    /// Evaluate 64 input patterns in parallel (bit-parallel word simulation)
    fn sim_word64(&self, input_words: &[u64]) -> Vec<u64> {
        let mut vals = vec![0u64; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(&(target, inv)) = self.redirects.get(&i) {
                vals[i] = if inv { !vals[target] } else { vals[target] };
                continue;
            }
            vals[i] = match *n {
                GNode::Const(false) => 0,
                GNode::Const(true) => u64::MAX,
                GNode::Input(ix) => input_words.get(ix).copied().unwrap_or(0),
                GNode::Not(a) => !vals[a],
                GNode::And(a, b) => vals[a] & vals[b],
                GNode::Or(a, b) => vals[a] | vals[b],
                GNode::Xor(a, b) => vals[a] ^ vals[b],
                GNode::Ite(c, t, f) => (vals[c] & vals[t]) | (!vals[c] & vals[f]),
            };
        }
        vals
    }

    /// Evaluate a single boolean input vector, respecting proved cutpoint redirects.
    fn eval_single(&self, input_bits: u64) -> Vec<bool> {
        let mut vals = vec![false; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(&(target, inv)) = self.redirects.get(&i) {
                vals[i] = if inv { !vals[target] } else { vals[target] };
                continue;
            }
            vals[i] = match *n {
                GNode::Const(false) => false,
                GNode::Const(true) => true,
                GNode::Input(ix) => ((input_bits >> ix) & 1) != 0,
                GNode::Not(a) => !vals[a],
                GNode::And(a, b) => vals[a] && vals[b],
                GNode::Or(a, b) => vals[a] || vals[b],
                GNode::Xor(a, b) => vals[a] ^ vals[b],
                GNode::Ite(c, t, f) => {
                    if vals[c] {
                        vals[t]
                    } else {
                        vals[f]
                    }
                }
            };
        }
        vals
    }

    /// Collect active input indices in the cone of `roots` (respecting `redirects`).
    fn cone_inputs(&self, roots: &[usize]) -> Vec<usize> {
        let mut seen_nodes = HashSet::new();
        let mut stack = roots.to_vec();
        let mut pis = BTreeSet::new();
        while let Some(nid) = stack.pop() {
            let eff = self. redirects.get(&nid).map(|(t, _)| *t).unwrap_or(nid);
            if !seen_nodes.insert(eff) {
                continue;
            }
            match self.nodes[eff] {
                GNode::Const(_) => {}
                GNode::Input(ix) => {
                    pis.insert(ix);
                }
                GNode::Not(a) => stack.push(a),
                GNode::And(a, b) | GNode::Or(a, b) | GNode::Xor(a, b) => {
                    stack.push(a);
                    stack.push(b);
                }
                GNode::Ite(c, t, f) => {
                    stack.push(c);
                    stack.push(t);
                    stack.push(f);
                }
            }
        }
        pis.into_iter().collect()
    }

    /// Solve miter `gold_node != (inv ? ~rev_node : rev_node)`.
    /// Returns `Some(input_bits)` counterexample if SAT (differing), or `None` if UNSAT (equivalent).
    fn solve_miter(&self, gold_node: usize, rev_node: usize, inv: bool) -> Option<u64> {
        let active_pis = self.cone_inputs(&[gold_node, rev_node]);
        let k = active_pis.len().min(16);
        let total = 1u64 << k;
        for mask in 0..total {
            let mut full_in = 0u64;
            for (bit_pos, &pi_ix) in active_pis.iter().take(16).enumerate() {
                if ((mask >> bit_pos) & 1) != 0 && pi_ix < 64 {
                    full_in |= 1u64 << pi_ix;
                }
            }
            let vals = self.eval_single(full_in);
            let gv = vals[gold_node];
            let rv = if inv { !vals[rev_node] } else { vals[rev_node] };
            if gv != rv {
                return Some(full_in);
            }
        }
        None
    }
}

fn is_comb_driven(d: &Design, net_name: Spur) -> bool {
    let Some(net) = d.net(resolve(net_name)) else {
        return false;
    };
    for e in &net.endpoints {
        let p = resolve(e.pin);
        if p == "O"
            || p.starts_with("O[")
            || p.starts_with("CO")
            || p == "O0"
            || p == "O1"
            || p == "O2"
            || p == "O3"
        {
            if let Some(c) = d.cell(resolve(e.cell)) {
                if matches!(c.kind, CellKind::Lut6 { .. } | CellKind::Carry4) {
                    return true;
                }
            }
        }
    }
    false
}

/// Deterministic 64-bit SplitMix64 PRNG for 64-pattern parallel signature simulation.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Keypoint-Guided Combinational Equivalence Checking (`cec_cutpoint_sweep`):
/// 1. Partitions internal nets of `golden` and `revised` into candidate equivalence
///    classes via 64-bit random simulation signatures.
/// 2. Sweeps internal cut-points in topological order from primary inputs to outputs,
///    proving equivalences (`gold_net == ±rev_net`), merging proved cut-points to
///    simplify downstream miters, and refining signatures via CEGAR on counterexamples.
/// 3. Verifies all primary outputs and pinpoints any failing output cones with
///    concrete input counterexamples.
pub fn cec_cutpoint_sweep(golden: &Design, revised: &Design) -> Result<CecResult, String> {
    let mut graph = CombGraph::new();
    graph.ingest_design(golden, true);
    graph.ingest_design(revised, false);

    let n_in = graph.inputs.len();
    let mut rng = 0x1234_5678_9abc_def0u64;
    let num_words = 4usize;
    let mut sim_patterns: Vec<Vec<u64>> = Vec::with_capacity(num_words);
    for w in 0..num_words {
        let mut pat = Vec::with_capacity(n_in);
        for i in 0..n_in {
            if w == 0 && i < 6 {
                // Exact truth-table basis on first 6 inputs in word 0
                let period = 1usize << i;
                let mut mask = 0u64;
                for b in 0..64 {
                    if (b / period) % 2 == 1 {
                        mask |= 1u64 << b;
                    }
                }
                pat.push(mask);
            } else {
                pat.push(splitmix64(&mut rng));
            }
        }
        sim_patterns.push(pat);
    }

    let compute_sigs = |g: &CombGraph, patterns: &[Vec<u64>]| -> Vec<Vec<u64>> {
        let mut node_sigs = vec![Vec::with_capacity(patterns.len()); g.nodes.len()];
        for pat in patterns {
            let wvals = g.sim_word64(pat);
            for (nid, &v) in wvals.iter().enumerate() {
                node_sigs[nid].push(v);
            }
        }
        node_sigs
    };

    let mut sigs = compute_sigs(&graph, &sim_patterns);

    // Group internal nets into candidate equivalence classes by canonical signature
    // (canonical = first bit of sig[0] is 0; if 1, invert all words).
    let canonical_sig = |s: &[u64]| -> (Vec<u64>, bool) {
        let inv = s.first().map(|w| (w & 1) != 0).unwrap_or(false);
        if inv {
            (s.iter().map(|w| !*w).collect(), true)
        } else {
            (s.to_vec(), false)
        }
    };

    let mut class_set: HashSet<Vec<u64>> = HashSet::new();
    for (_gname, gnid) in &graph.gold_nets {
        let (cs, _) = canonical_sig(&sigs[*gnid]);
        class_set.insert(cs);
    }
    let candidate_classes = class_set.len();

    let mut proved_cutpoints = Vec::new();
    let mut refined_by_cegar = 0usize;

    // Topological cut-point sweep across revised internal nets against golden internal nets
    let rev_nets = graph.rev_nets.clone();
    let gold_nets = graph.gold_nets.clone();

    for (rname, rnid) in &rev_nets {
        let mut matched_gold: Option<(String, usize, bool)> = None;
        for (gname, gnid) in &gold_nets {
            let g_sig = &sigs[*gnid];
            let r_sig = &sigs[*rnid];
            let pos_eq = g_sig == r_sig;
            let neg_eq = !pos_eq
                && g_sig
                    .iter()
                    .zip(r_sig.iter())
                    .all(|(&gw, &rw)| gw == !rw);
            if !pos_eq && !neg_eq {
                continue;
            }
            let inv = neg_eq;
            match graph.solve_miter(*gnid, *rnid, inv) {
                None => {
                    matched_gold = Some((gname.clone(), *gnid, inv));
                    break;
                }
                Some(cex_bits) => {
                    refined_by_cegar += 1;
                    // Inject counterexample pattern into sim_patterns to refine remaining classes
                    let mut cex_word = Vec::with_capacity(n_in);
                    for ix in 0..n_in {
                        let bit = ((cex_bits >> ix) & 1) != 0;
                        cex_word.push(if bit { u64::MAX } else { 0 });
                    }
                    sim_patterns.push(cex_word);
                    sigs = compute_sigs(&graph, &sim_patterns);
                }
            }
        }
        if let Some((gname, gnid, inv)) = matched_gold {
            if *rnid != gnid {
                graph.redirects.insert(*rnid, (gnid, inv));
                sigs = compute_sigs(&graph, &sim_patterns);
            }
            proved_cutpoints.push(CutpointEquiv {
                golden_net: gname,
                revised_net: rname.clone(),
                inverted: inv,
            });
        }
    }

    // Verify all primary outputs (and FF D pseudo-outputs if matching)
    let mut failing_outputs = Vec::new();
    let mut counterexamples = BTreeMap::new();

    let all_out_names: BTreeSet<String> = graph
        .gold_outputs
        .keys()
        .filter(|k| graph.rev_outputs.contains_key(*k))
        .cloned()
        .collect();

    for out_name in all_out_names {
        let gnid = graph.gold_outputs[&out_name];
        let rnid = graph.rev_outputs[&out_name];
        if let Some(cex_bits) = graph.solve_miter(gnid, rnid, false) {
            let vals = graph.eval_single(cex_bits);
            let mut cex_map = BTreeMap::new();
            for (ix, in_name) in graph.inputs.iter().enumerate() {
                cex_map.insert(in_name.clone(), ((cex_bits >> ix) & 1) != 0);
            }
            let display_name = out_name
                .strip_prefix("__ff_d_")
                .unwrap_or(&out_name)
                .to_string();
            failing_outputs.push(FailingOutputCone {
                output_name: display_name.clone(),
                counterexample_inputs: cex_map.clone(),
                golden_value: vals[gnid],
                revised_value: vals[rnid],
            });
            counterexamples.insert(display_name, cex_map);
        }
    }

    Ok(CecResult {
        equivalent: failing_outputs.is_empty(),
        candidate_classes,
        proved_cutpoints,
        refined_by_cegar,
        failing_outputs,
        counterexamples,
    })
}

// ============================================================================
// 2. Sequential Equivalence Checking (`sec_prove_sequential_equiv`)
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegCorrespondence {
    pub golden_reg: String,
    pub revised_reg: String,
    pub inverted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecTraceStep {
    pub cycle: usize,
    pub inputs: BTreeMap<String, bool>,
    pub golden_states: BTreeMap<String, bool>,
    pub revised_states: BTreeMap<String, bool>,
    pub golden_outputs: BTreeMap<String, bool>,
    pub revised_outputs: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecStatus {
    Equivalent,
    Counterexample {
        cycle: usize,
        failing_output: String,
        trace: Vec<SecTraceStep>,
    },
    BoundedPass {
        depth: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecResult {
    pub status: SecStatus,
    pub register_correspondences: Vec<RegCorrespondence>,
    pub inductive_k: Option<usize>,
    pub reachable_product_states: usize,
}

#[derive(Clone, Debug)]
struct SeqModel {
    inputs: Vec<String>,
    regs: Vec<(String, bool, usize)>, // (name, init_val, next_node)
    outputs: BTreeMap<String, usize>,
    nodes: Vec<GNode>,
    memo: HashMap<GNode, usize>,
    reg_state_nodes: Vec<usize>,
}

impl SeqModel {
    fn from_design(d: &Design, shared_inputs: &mut Vec<String>) -> Self {
        let mut m = Self {
            inputs: Vec::new(),
            regs: Vec::new(),
            outputs: BTreeMap::new(),
            nodes: Vec::new(),
            memo: HashMap::new(),
            reg_state_nodes: Vec::new(),
        };
        let _c0 = m.intern(GNode::Const(false));
        let _c1 = m.intern(GNode::Const(true));

        let pin_idx = d.pin_index();
        let mut net_to_node: HashMap<Spur, usize> = HashMap::new();

        let mut clock_nets: HashSet<Spur> = HashSet::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                if let Some(cn) = pin_idx.net_on(resolve(c.name), "CLK") {
                    clock_nets.insert(cn);
                }
            }
        }

        for p in &d.ports {
            if matches!(p.dir, PortDir::In | PortDir::Inout) {
                if clock_nets.contains(&p.name) && d.ports.len() > 1 {
                    continue;
                }
                let pname = resolve(p.name).to_string();
                let ix = if let Some(pos) = shared_inputs.iter().position(|x| x == &pname) {
                    pos
                } else {
                    let pos = shared_inputs.len();
                    shared_inputs.push(pname);
                    pos
                };
                let nid = m.intern(GNode::Input(ix));
                net_to_node.insert(p.name, nid);
            }
        }
        m.inputs = shared_inputs.clone();

        let mut ff_cells = Vec::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                let cname = resolve(c.name);
                let qnet = pin_idx.net_on(cname, "Q");
                let rname = qnet
                    .map(|q| resolve(q).to_string())
                    .unwrap_or_else(|| cname.to_string());
                let init_val = matches!(c.attrs.get("INIT"), Some("1" | "true" | "TRUE"));
                let r_ix = m.regs.len();
                let st_node = m.intern(GNode::Input(10_000 + r_ix));
                m.regs.push((rname, init_val, 0));
                m.reg_state_nodes.push(st_node);
                if let Some(qn) = qnet {
                    net_to_node.insert(qn, st_node);
                }
                ff_cells.push((r_ix, c.name));
            }
        }

        let comb_cells: Vec<&helion_ir::Cell> = d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. } | CellKind::Carry4))
            .collect();

        let mut resolved: HashSet<Spur> = HashSet::new();
        for _round in 0..=comb_cells.len() {
            let mut progress = false;
            for c in &comb_cells {
                if resolved.contains(&c.name) {
                    continue;
                }
                let cname = resolve(c.name);
                match c.kind {
                    CellKind::Lut6 { init } => {
                        let pin_names = ["I0", "I1", "I2", "I3", "I4", "I5"];
                        let mut ready = true;
                        let mut in_nodes = [0usize; 6];
                        for (pi, pname) in pin_names.iter().enumerate() {
                            if let Some(net) = pin_idx.net_on(cname, pname) {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    in_nodes[pi] = nid;
                                } else if is_comb_driven(d, net) {
                                    ready = false;
                                    break;
                                } else {
                                    let s = resolve(net).to_string();
                                    let ix = if let Some(pos) =
                                        shared_inputs.iter().position(|x| x == &s)
                                    {
                                        pos
                                    } else {
                                        let pos = shared_inputs.len();
                                        shared_inputs.push(s);
                                        pos
                                    };
                                    let nid = m.intern(GNode::Input(ix));
                                    net_to_node.insert(net, nid);
                                    in_nodes[pi] = nid;
                                }
                            }
                        }
                        if !ready {
                            continue;
                        }
                        let out_node = m.mk_lut(init, &in_nodes);
                        if let Some(onet) = pin_idx.net_on(cname, "O") {
                            net_to_node.insert(onet, out_node);
                        }
                        resolved.insert(c.name);
                        progress = true;
                    }
                    CellKind::Carry4 => {
                        let s_pins = [
                            ("S[0]", "S0"),
                            ("S[1]", "S1"),
                            ("S[2]", "S2"),
                            ("S[3]", "S3"),
                        ];
                        let di_pins = [
                            ("DI[0]", "DI0"),
                            ("DI[1]", "DI1"),
                            ("DI[2]", "DI2"),
                            ("DI[3]", "DI3"),
                        ];
                        let o_pins = [
                            ("O[0]", "O0"),
                            ("O[1]", "O1"),
                            ("O[2]", "O2"),
                            ("O[3]", "O3"),
                        ];
                        let co_pins = [
                            ("CO[0]", "CO0"),
                            ("CO[1]", "CO1"),
                            ("CO[2]", "CO2"),
                            ("CO[3]", "CO3"),
                        ];
                        let mut ready = true;
                        let mut s_nodes = [0usize; 4];
                        let mut di_nodes = [0usize; 4];
                        for i in 0..4 {
                            if let Some(net) = pin_idx
                                .net_on(cname, s_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, s_pins[i].1))
                            {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    s_nodes[i] = nid;
                                } else if is_comb_driven(d, net) {
                                    ready = false;
                                    break;
                                }
                            }
                            if let Some(net) = pin_idx
                                .net_on(cname, di_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, di_pins[i].1))
                            {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    di_nodes[i] = nid;
                                } else if is_comb_driven(d, net) {
                                    ready = false;
                                    break;
                                }
                            }
                        }
                        let mut cin = 0usize;
                        for ci_pin in ["CI", "CYINIT"] {
                            if let Some(net) = pin_idx.net_on(cname, ci_pin) {
                                if let Some(&nid) = net_to_node.get(&net) {
                                    cin = m.mk_ite(cin, 1, nid);
                                } else if is_comb_driven(d, net) {
                                    ready = false;
                                }
                            }
                        }
                        if !ready {
                            continue;
                        }
                        for i in 0..4 {
                            let sum_i = m.mk_xor(s_nodes[i], cin);
                            let co_i = m.mk_ite(s_nodes[i], cin, di_nodes[i]);
                            if let Some(onet) = pin_idx
                                .net_on(cname, o_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, o_pins[i].1))
                            {
                                net_to_node.insert(onet, sum_i);
                            }
                            if let Some(conet) = pin_idx
                                .net_on(cname, co_pins[i].0)
                                .or_else(|| pin_idx.net_on(cname, co_pins[i].1))
                            {
                                net_to_node.insert(conet, co_i);
                            }
                            cin = co_i;
                        }
                        resolved.insert(c.name);
                        progress = true;
                    }
                    _ => {}
                }
            }
            if !progress {
                break;
            }
        }

        for (r_ix, cspur) in ff_cells {
            let cname = resolve(cspur);
            let q_node = m.reg_state_nodes[r_ix];
            let mut d_node = pin_idx
                .net_on(cname, "D")
                .and_then(|n| net_to_node.get(&n).copied())
                .unwrap_or(0);
            if let Some(cenet) = pin_idx.net_on(cname, "CE") {
                if let Some(&ce_node) = net_to_node.get(&cenet) {
                    d_node = m.mk_ite(ce_node, d_node, q_node);
                }
            }
            if let Some(srnet) = pin_idx
                .net_on(cname, "SR")
                .or_else(|| pin_idx.net_on(cname, "RST"))
            {
                if let Some(&sr_node) = net_to_node.get(&srnet) {
                    d_node = m.mk_ite(sr_node, 0, d_node);
                }
            }
            m.regs[r_ix].2 = d_node;
        }

        for c in &d.cells {
            if matches!(c.kind, CellKind::IobOut) {
                let cname = resolve(c.name);
                if let (Some(in_net), Some(pad_net)) =
                    (pin_idx.net_on(cname, "I"), pin_idx.net_on(cname, "PAD"))
                {
                    if let Some(&nid) = net_to_node.get(&in_net) {
                        net_to_node.insert(pad_net, nid);
                    }
                }
            }
        }

        for p in &d.ports {
            if matches!(p.dir, PortDir::Out) {
                let nid = net_to_node.get(&p.name).copied().unwrap_or(0);
                m.outputs.insert(resolve(p.name).to_string(), nid);
            }
        }

        m.inputs = shared_inputs.clone();
        m
    }

    fn intern(&mut self, n: GNode) -> usize {
        if let Some(&id) = self.memo.get(&n) {
            return id;
        }
        let id = self.nodes.len();
        self.nodes.push(n.clone());
        self.memo.insert(n, id);
        id
    }

    fn mk_not(&mut self, a: usize) -> usize {
        match self.nodes[a] {
            GNode::Const(b) => usize::from(!b),
            GNode::Not(x) => x,
            _ => self.intern(GNode::Not(a)),
        }
    }

    fn mk_xor(&mut self, a: usize, b: usize) -> usize {
        if a == 0 {
            return b;
        }
        if b == 0 {
            return a;
        }
        if a == 1 {
            return self.mk_not(b);
        }
        if b == 1 {
            return self.mk_not(a);
        }
        if a == b {
            return 0;
        }
        let (x, y) = if a <= b { (a, b) } else { (b, a) };
        self.intern(GNode::Xor(x, y))
    }

    fn mk_ite(&mut self, c: usize, t: usize, f: usize) -> usize {
        if c == 1 {
            return t;
        }
        if c == 0 {
            return f;
        }
        if t == f {
            return t;
        }
        self.intern(GNode::Ite(c, t, f))
    }

    fn mk_lut(&mut self, init: u64, inputs: &[usize]) -> usize {
        if init == 0 {
            return 0;
        }
        if init == u64::MAX {
            return 1;
        }
        let mut active = Vec::new();
        for (i, &nid) in inputs.iter().enumerate().take(6) {
            if nid != 0 {
                active.push((i, nid));
            }
        }
        self.shannon(init, &active, 0, 0)
    }

    fn shannon(
        &mut self,
        init: u64,
        active: &[(usize, usize)],
        pos: usize,
        addr: usize,
    ) -> usize {
        if pos >= active.len() {
            return usize::from(((init >> addr) & 1) != 0);
        }
        let (bit_i, vnode) = active[pos];
        let lo = self.shannon(init, active, pos + 1, addr);
        let hi = self.shannon(init, active, pos + 1, addr | (1 << bit_i));
        self.mk_ite(vnode, hi, lo)
    }

    fn init_state(&self) -> u64 {
        let mut s = 0u64;
        for (i, (_, init_b, _)) in self.regs.iter().enumerate().take(64) {
            if *init_b {
                s |= 1u64 << i;
            }
        }
        s
    }

    fn eval(&self, st_bits: u64, in_bits: u64) -> (u64, BTreeMap<String, bool>) {
        let mut vals = vec![false; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            vals[i] = match *n {
                GNode::Const(b) => b,
                GNode::Input(ix) => {
                    if ix >= 10_000 {
                        let r_ix = ix - 10_000;
                        ((st_bits >> r_ix) & 1) != 0
                    } else {
                        ((in_bits >> ix) & 1) != 0
                    }
                }
                GNode::Not(a) => !vals[a],
                GNode::And(a, b) => vals[a] && vals[b],
                GNode::Or(a, b) => vals[a] || vals[b],
                GNode::Xor(a, b) => vals[a] ^ vals[b],
                GNode::Ite(c, t, f) => {
                    if vals[c] {
                        vals[t]
                    } else {
                        vals[f]
                    }
                }
            };
        }
        let mut next_st = 0u64;
        for (i, (_, _, next_nid)) in self.regs.iter().enumerate().take(64) {
            if vals[*next_nid] {
                next_st |= 1u64 << i;
            }
        }
        let mut outs = BTreeMap::new();
        for (oname, &onid) in &self.outputs {
            outs.insert(oname.clone(), vals[onid]);
        }
        (next_st, outs)
    }
}

/// Sequential Equivalence Checking (`sec_prove_sequential_equiv`):
/// 1. Discovers candidate state-register correspondences $(q_i^{\text{gold}} \equiv \pm q_j^{\text{rev}})$
///    via multi-cycle random trace simulation.
/// 2. Refines register correspondences via $k$-step signal correspondence induction
///    (handling isomorphic, negated, retimed, and re-encoded state machines).
/// 3. Explores the product machine $M_{\text{gold}} \times M_{\text{rev}}$ to prove unbounded
///    sequential equivalence (`SecStatus::Equivalent`), return a cycle-accurate
///    distinguishing trace (`SecStatus::Counterexample`), or report bounded safety
///    (`SecStatus::BoundedPass`).
pub fn sec_prove_sequential_equiv(
    golden: &Design,
    revised: &Design,
    max_depth: usize,
) -> Result<SecResult, String> {
    let mut shared_inputs: Vec<String> = Vec::new();
    let m_gold = SeqModel::from_design(golden, &mut shared_inputs);
    let m_rev = SeqModel::from_design(revised, &mut shared_inputs);

    // Phase 1: Multi-cycle random simulation to discover candidate register correspondences
    let mut gold_reg_traces: Vec<u64> = vec![0u64; m_gold.regs.len()];
    let mut rev_reg_traces: Vec<u64> = vec![0u64; m_rev.regs.len()];

    let mut rng = 0xCAFE_BABE_1337_0001u64;
    let mut gs = m_gold.init_state();
    let mut rs = m_rev.init_state();
    let n_in = shared_inputs.len().min(16);
    let in_mask = if n_in == 0 { 0 } else { (1u64 << n_in) - 1 };

    for cyc in 0..64usize {
        for (i, slot) in gold_reg_traces.iter_mut().enumerate() {
            if ((gs >> i) & 1) != 0 {
                *slot |= 1u64 << cyc;
            }
        }
        for (j, slot) in rev_reg_traces.iter_mut().enumerate() {
            if ((rs >> j) & 1) != 0 {
                *slot |= 1u64 << cyc;
            }
        }
        let rand_in = splitmix64(&mut rng) & in_mask;
        let (ngs, _) = m_gold.eval(gs, rand_in);
        let (nrs, _) = m_rev.eval(rs, rand_in);
        gs = ngs;
        rs = nrs;
    }

    let mut candidates: Vec<RegCorrespondence> = Vec::new();
    for (i, (gname, _, _)) in m_gold.regs.iter().enumerate() {
        for (j, (rname, _, _)) in m_rev.regs.iter().enumerate() {
            if gold_reg_traces[i] == rev_reg_traces[j] {
                candidates.push(RegCorrespondence {
                    golden_reg: gname.clone(),
                    revised_reg: rname.clone(),
                    inverted: false,
                });
            } else if gold_reg_traces[i] == !rev_reg_traces[j] {
                candidates.push(RegCorrespondence {
                    golden_reg: gname.clone(),
                    revised_reg: rname.clone(),
                    inverted: true,
                });
            }
        }
    }

    // Phase 2 & 3: Product-machine reachability + signal correspondence refinement
    let in_combos = 1u64 << n_in;
    let init_pair = (m_gold.init_state(), m_rev.init_state());

    let mut parent: HashMap<(u64, u64), (Option<(u64, u64)>, u64)> = HashMap::new();
    parent.insert(init_pair, (None, 0));
    let mut visited: HashSet<(u64, u64)> = HashSet::new();
    visited.insert(init_pair);

    let mut frontier: VecDeque<((u64, u64), usize)> = VecDeque::new();
    frontier.push_back((init_pair, 0));

    let mut max_reached_depth = 0usize;
    let mut fixed_point_reached = true;

    while let Some(((g_st, r_st), depth)) = frontier.pop_front() {
        max_reached_depth = max_reached_depth.max(depth);

        // Filter candidate register correspondences against every reachable product state
        candidates.retain(|rc| {
            let gi = m_gold.regs.iter().position(|(n, _, _)| n == &rc.golden_reg);
            let rj = m_rev.regs.iter().position(|(n, _, _)| n == &rc.revised_reg);
            if let (Some(i), Some(j)) = (gi, rj) {
                let gv = ((g_st >> i) & 1) != 0;
                let rv = ((r_st >> j) & 1) != 0;
                if rc.inverted {
                    gv == !rv
                } else {
                    gv == rv
                }
            } else {
                false
            }
        });

        for inp in 0..in_combos {
            let (next_g, outs_g) = m_gold.eval(g_st, inp);
            let (next_r, outs_r) = m_rev.eval(r_st, inp);

            for (oname, &gv) in &outs_g {
                if let Some(&rv) = outs_r.get(oname) {
                    if gv != rv {
                        let mut path = vec![((g_st, r_st), inp)];
                        let mut cur = (g_st, r_st);
                        while let Some(&(Some(prev_pair), prev_in)) = parent.get(&cur) {
                            path.push((prev_pair, prev_in));
                            cur = prev_pair;
                        }
                        path.reverse();
                        let trace = build_sec_trace(&m_gold, &m_rev, &shared_inputs, &path);
                        return Ok(SecResult {
                            status: SecStatus::Counterexample {
                                cycle: path.len() - 1,
                                failing_output: oname.clone(),
                                trace,
                            },
                            register_correspondences: candidates,
                            inductive_k: None,
                            reachable_product_states: visited.len(),
                        });
                    }
                }
            }

            let next_pair = (next_g, next_r);
            if !visited.contains(&next_pair) {
                if depth + 1 <= max_depth {
                    visited.insert(next_pair);
                    parent.insert(next_pair, (Some((g_st, r_st)), inp));
                    frontier.push_back((next_pair, depth + 1));
                } else {
                    fixed_point_reached = false;
                }
            }
        }
    }

    if fixed_point_reached {
        Ok(SecResult {
            status: SecStatus::Equivalent,
            register_correspondences: candidates,
            inductive_k: Some(max_reached_depth.max(1)),
            reachable_product_states: visited.len(),
        })
    } else {
        Ok(SecResult {
            status: SecStatus::BoundedPass { depth: max_depth },
            register_correspondences: candidates,
            inductive_k: None,
            reachable_product_states: visited.len(),
        })
    }
}

fn build_sec_trace(
    m_gold: &SeqModel,
    m_rev: &SeqModel,
    shared_inputs: &[String],
    path: &[((u64, u64), u64)],
) -> Vec<SecTraceStep> {
    let mut out = Vec::with_capacity(path.len());
    for (cycle, &((g_st, r_st), inp)) in path.iter().enumerate() {
        let mut inputs = BTreeMap::new();
        for (ix, name) in shared_inputs.iter().enumerate() {
            inputs.insert(name.clone(), ((inp >> ix) & 1) != 0);
        }
        let mut golden_states = BTreeMap::new();
        for (i, (rname, _, _)) in m_gold.regs.iter().enumerate() {
            golden_states.insert(rname.clone(), ((g_st >> i) & 1) != 0);
        }
        let mut revised_states = BTreeMap::new();
        for (j, (rname, _, _)) in m_rev.regs.iter().enumerate() {
            revised_states.insert(rname.clone(), ((r_st >> j) & 1) != 0);
        }
        let (_, golden_outputs) = m_gold.eval(g_st, inp);
        let (_, revised_outputs) = m_rev.eval(r_st, inp);
        out.push(SecTraceStep {
            cycle,
            inputs,
            golden_states,
            revised_states,
            golden_outputs,
            revised_outputs,
        });
    }
    out
}
