//! Shift registers into `SRL16E` / `SRLC32E` (7-series flow, [`super::hard`]).
//!
//! Runs on the lowered netlist, like Vivado's shreg extraction (UG901):
//!
//! - **Fixed tap.** A chain of flops on one clock and one enable, with no
//!   reset, where each flop's D is the previous flop's Q and nothing else
//!   reads the inner stages, becomes one SRL addressed at its last stage
//!   (cascades of `SRLC32E` through `Q31` past 32 stages). Chains shorter
//!   than [`MIN_LEN`] stay flops.
//! - **Dynamic tap.** A chain whose stages are read through LUTs that pick
//!   stage `addr` (`sr[addr]`) becomes an SRL with the address on `A`. The
//!   match is exact: for every address value the LUT cone is reduced with
//!   the address bits held constant and must come down to one stage's Q.
//!
//! `(* shreg_extract = "no" *)`, `(* srl_style = "register" *)` and
//! `(* ASYNC_REG *)` keep a register's net (the lexer turns them into
//! `keep`), and a kept flop is never absorbed.

use super::lower::Stats;
use helion_ir::{intern, resolve, CellKind, Design, Spur};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

static OFF: AtomicBool = AtomicBool::new(false);

/// Keep shift registers in flops even in the hard flow: for a back end that
/// cannot build SRL cells yet. `HELION_V2_SRL=1` / `=0` overrides either way.
pub fn set_off(off: bool) {
    OFF.store(off, Ordering::Relaxed);
}

/// Is SRL inference on (given the hard flow is)?
pub fn on() -> bool {
    match std::env::var("HELION_V2_SRL") {
        Ok(v) => v != "0",
        Err(_) => !OFF.load(Ordering::Relaxed),
    }
}

/// Shortest chain built as an SRL (Vivado's `-shreg_min_size` default).
pub const MIN_LEN: usize = 3;

#[derive(Clone)]
struct Flop {
    cell: Spur,
    d: Spur,
    q: Spur,
    clk: Spur,
    ce: Option<Spur>,
}

/// What a net reduces to with the address bits held constant.
#[derive(Clone, Copy, PartialEq)]
enum Res {
    Const(bool),
    /// Value of net `.0`, inverted when `.1`.
    Proj(Spur, bool),
    Unknown,
}

struct Net {
    eps: Vec<(Spur, Spur)>,
}

struct Ctx {
    nets: HashMap<Spur, Net>,
    /// (cell, pin) -> net.
    pin: HashMap<(Spur, Spur), Spur>,
    /// LUT output net -> (cell, init).
    lut_of: HashMap<Spur, (Spur, u64)>,
    /// Nets that must stay (`keep` / `mark_debug`).
    pinned: HashSet<Spur>,
}

impl Ctx {
    fn loads(&self, net: Spur) -> impl Iterator<Item = &(Spur, Spur)> {
        self.nets.get(&net).into_iter().flat_map(|n| n.eps.iter())
    }
    fn lut_ins(&self, cell: Spur) -> [Option<Spur>; 6] {
        std::array::from_fn(|i| self.pin.get(&(cell, intern(&format!("I{i}")))).copied())
    }
}

/// One planned replacement.
struct Plan {
    /// Flops of the window, in shift order.
    flops: Vec<usize>,
    din: Spur,
    clk: Spur,
    ce: Option<Spur>,
    out: Spur,
    /// Dynamic tap: address nets by SRL address bit (net, inverted).
    addr: Option<Vec<(Spur, bool)>>,
    /// LUT cells that die with the window.
    dead: HashSet<Spur>,
}

pub fn infer(d: &mut Design, st: &mut Stats) {
    let mut ctx = Ctx { nets: HashMap::new(), pin: HashMap::new(), lut_of: HashMap::new(), pinned: HashSet::new() };
    let mut cell_pins: HashMap<Spur, Vec<Spur>> = HashMap::new();
    for n in &d.nets {
        if n.attrs.get("mark_debug").is_some() {
            ctx.pinned.insert(n.name);
        }
        for e in &n.endpoints {
            ctx.pin.insert((e.cell, e.pin), n.name);
            cell_pins.entry(e.cell).or_default().push(e.pin);
        }
        ctx.nets.insert(n.name, Net { eps: n.endpoints.iter().map(|e| (e.cell, e.pin)).collect() });
    }
    let (p_d, p_q, p_clk, p_ce, p_o) = (intern("D"), intern("Q"), intern("CLK"), intern("CE"), intern("O"));
    let mut flops: Vec<Flop> = Vec::new();
    for c in &d.cells {
        match c.kind {
            CellKind::Lut6 { init } => {
                if let Some(&o) = ctx.pin.get(&(c.name, p_o)) {
                    ctx.lut_of.insert(o, (c.name, init));
                }
            }
            CellKind::Hff => {
                let plain = cell_pins.get(&c.name).is_some_and(|ps| ps.iter().all(|p| [p_d, p_q, p_clk, p_ce].contains(p)))
                    && c.attrs.get("CLK_INV").map_or(true, |v| v != "1")
                    && c.attrs.get("INIT").map_or(true, |v| v == "0" || v == "1'b0")
                    && c.attrs.get("DONT_TOUCH").is_none();
                let (Some(&dn), Some(&q), Some(&clk)) = (ctx.pin.get(&(c.name, p_d)), ctx.pin.get(&(c.name, p_q)), ctx.pin.get(&(c.name, p_clk))) else { continue };
                if plain && !ctx.pinned.contains(&q) {
                    flops.push(Flop { cell: c.name, d: dn, q, clk, ce: ctx.pin.get(&(c.name, p_ce)).copied() });
                }
            }
            _ => {}
        }
    }
    // Chains: g -> f when f.D is g.Q on the same clock and enable, and f is
    // the only flop g feeds.
    let by_q: HashMap<Spur, usize> = flops.iter().enumerate().map(|(i, f)| (f.q, i)).collect();
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); flops.len()];
    let mut pred: Vec<Option<usize>> = vec![None; flops.len()];
    for (i, f) in flops.iter().enumerate() {
        if let Some(&g) = by_q.get(&f.d) {
            if g != i && flops[g].clk == f.clk && flops[g].ce == f.ce {
                succs[g].push(i);
                pred[i] = Some(g);
            }
        }
    }
    let link = |g: usize| (succs[g].len() == 1).then(|| succs[g][0]);
    let mut plans: Vec<Plan> = Vec::new();
    let mut seen = vec![false; flops.len()];
    for s in 0..flops.len() {
        if seen[s] || pred[s].is_some_and(|g| link(g) == Some(s)) {
            continue;
        }
        let mut chain = vec![s];
        seen[s] = true;
        while let Some(n) = link(*chain.last().unwrap()) {
            if seen[n] {
                break;
            }
            seen[n] = true;
            chain.push(n);
        }
        if chain.len() < MIN_LEN {
            continue;
        }
        if let Some(p) = dynamic(&ctx, &flops, &chain) {
            plans.push(p);
            continue;
        }
        // Fixed taps: cut the chain after every stage something else reads.
        let mut start = 0;
        for k in 0..chain.len() {
            let f = &flops[chain[k]];
            let next = chain.get(k + 1).map(|&n| flops[n].cell);
            let tapped = ctx.loads(f.q).any(|&(c, p)| !(c == f.cell && p == p_q) && !(Some(c) == next && p == p_d));
            if tapped || k + 1 == chain.len() {
                if k + 1 - start >= MIN_LEN {
                    let w: Vec<usize> = chain[start..=k].to_vec();
                    let f0 = &flops[w[0]];
                    plans.push(Plan { din: f0.d, clk: f0.clk, ce: f0.ce, out: f.q, flops: w, addr: None, dead: HashSet::new() });
                }
                start = k + 1;
            }
        }
    }
    if plans.is_empty() {
        return;
    }
    apply(d, st, &flops, plans);
}

/// Try a dynamic tap over the front of `chain`.
fn dynamic(ctx: &Ctx, flops: &[Flop], chain: &[usize]) -> Option<Plan> {
    let qs: HashMap<Spur, usize> = chain.iter().enumerate().map(|(k, &f)| (flops[f].q, k)).collect();
    // LUTs reading the stages, and everything they feed through LUTs.
    let mut front: Vec<Spur> = Vec::new();
    let mut read = 0;
    for &f in chain {
        let n = front.len();
        for &(c, _) in ctx.loads(flops[f].q) {
            if let Some(&o) = ctx.pin.get(&(c, intern("O"))) {
                if ctx.lut_of.get(&o).is_some_and(|l| l.0 == c) && !front.contains(&o) {
                    front.push(o);
                }
            }
        }
        read += usize::from(front.len() > n);
    }
    if read < 2 {
        return None;
    }
    let mut cands: Vec<Spur> = front.clone();
    let mut level = front;
    for _ in 0..8 {
        let mut next = Vec::new();
        for &n in &level {
            for &(c, _) in ctx.loads(n) {
                if let Some(&o) = ctx.pin.get(&(c, intern("O"))) {
                    if ctx.lut_of.get(&o).is_some_and(|l| l.0 == c) && !cands.contains(&o) {
                        cands.push(o);
                        next.push(o);
                    }
                }
            }
        }
        level = next;
        if cands.len() > 512 {
            return None;
        }
    }
    let mut best: Option<Plan> = None;
    for &out in cands.iter().rev() {
        // Cone of `out` through LUTs, stopping at chain stages.
        let mut cone: Vec<Spur> = Vec::new();
        let mut addr: Vec<Spur> = Vec::new();
        let mut stack = vec![out];
        let mut visited = HashSet::new();
        let mut ok = true;
        while let Some(n) = stack.pop() {
            if !visited.insert(n) {
                continue;
            }
            if qs.contains_key(&n) {
                continue;
            }
            match ctx.lut_of.get(&n) {
                Some(&(c, _)) => {
                    cone.push(c);
                    stack.extend(ctx.lut_ins(c).into_iter().flatten());
                }
                None => addr.push(n),
            }
            if cone.len() > 512 || addr.len() > 7 {
                ok = false;
                break;
            }
        }
        let m = addr.len();
        if !ok || m == 0 || (1usize << m) > chain.len() || best.as_ref().is_some_and(|b| b.flops.len() >= 1 << m) {
            continue;
        }
        // Stage picked by each address value.
        let mut pick = Vec::with_capacity(1 << m);
        for a in 0..1usize << m {
            let fixed: HashMap<Spur, bool> = addr.iter().enumerate().map(|(i, &n)| (n, (a >> i) & 1 == 1)).collect();
            match reduce(ctx, out, &fixed, &qs, &mut HashMap::new()) {
                Res::Proj(n, false) => pick.push(qs[&n]),
                _ => break,
            }
        }
        if pick.len() != 1 << m {
            continue;
        }
        // pick(a) = base ^ (bits of a moved to positions pos[i]).
        let base = pick[0];
        let mut pos = Vec::with_capacity(m);
        for i in 0..m {
            let dlt = pick[1 << i] ^ base;
            if !dlt.is_power_of_two() || pos.contains(&dlt.trailing_zeros()) {
                break;
            }
            pos.push(dlt.trailing_zeros());
        }
        if pos.len() != m || pos.iter().any(|&p| p as usize >= m) {
            continue;
        }
        if (0..1usize << m).any(|a| pick[a] != (0..m).fold(base, |v, i| v ^ (((a >> i) & 1) << pos[i]))) {
            continue;
        }
        let window: Vec<usize> = chain[..1 << m].to_vec();
        // LUTs that die with the window: `out`'s driver, then every cone LUT
        // all of whose readers die.
        let mut dead: HashSet<Spur> = HashSet::new();
        dead.insert(ctx.lut_of[&out].0);
        loop {
            let before = dead.len();
            for &c in &cone {
                let o = ctx.pin[&(c, intern("O"))];
                if !dead.contains(&c) && !ctx.pinned.contains(&o) && ctx.loads(o).all(|&(lc, lp)| lc == c && lp == intern("O") || dead.contains(&lc)) {
                    dead.insert(c);
                }
            }
            if dead.len() == before {
                break;
            }
        }
        // Each stage may be read only by the next stage and dying LUTs.
        let clean = window.iter().enumerate().all(|(k, &f)| {
            let fl = &flops[f];
            let next = window.get(k + 1).map(|&n| flops[n].cell);
            !ctx.pinned.contains(&fl.q)
                && ctx.loads(fl.q).all(|&(c, p)| (c == fl.cell && p == intern("Q")) || (Some(c) == next && p == intern("D")) || dead.contains(&c))
        });
        // The window's data, clock and enable must not come from a dying LUT.
        let f0 = &flops[window[0]];
        let feeds_dead = [Some(f0.d), Some(f0.clk), f0.ce].into_iter().flatten().any(|n| ctx.lut_of.get(&n).is_some_and(|l| dead.contains(&l.0)));
        if !clean || feeds_dead {
            continue;
        }
        let mut abits = vec![(addr[0], false); m];
        for i in 0..m {
            abits[pos[i] as usize] = (addr[i], (base >> pos[i]) & 1 == 1);
        }
        best = Some(Plan { din: f0.d, clk: f0.clk, ce: f0.ce, out, flops: window, addr: Some(abits), dead });
    }
    best
}

/// Reduce net `n` with the nets in `fixed` held constant.
fn reduce(ctx: &Ctx, n: Spur, fixed: &HashMap<Spur, bool>, qs: &HashMap<Spur, usize>, memo: &mut HashMap<Spur, Res>) -> Res {
    if let Some(&v) = fixed.get(&n) {
        return Res::Const(v);
    }
    if qs.contains_key(&n) {
        return Res::Proj(n, false);
    }
    if let Some(&r) = memo.get(&n) {
        return r;
    }
    let Some(&(cell, init)) = ctx.lut_of.get(&n) else { return Res::Unknown };
    let ins: Vec<Res> = ctx.lut_ins(cell).iter().map(|i| match i {
        Some(i) => reduce(ctx, *i, fixed, qs, memo),
        None => Res::Const(false),
    }).collect();
    let mut vars: Vec<Spur> = Vec::new();
    for r in &ins {
        match r {
            Res::Unknown => return Res::Unknown,
            Res::Proj(v, _) if !vars.contains(v) => vars.push(*v),
            _ => {}
        }
    }
    let mut tt = 0u64;
    for x in 0..1u32 << vars.len() {
        let mut idx = 0;
        for (i, r) in ins.iter().enumerate() {
            let b = match *r {
                Res::Const(b) => b,
                Res::Proj(v, inv) => ((x >> vars.iter().position(|&w| w == v).unwrap()) & 1 == 1) ^ inv,
                Res::Unknown => unreachable!(),
            };
            idx |= (b as usize) << i;
        }
        tt |= ((init >> idx) & 1) << x;
    }
    let full = (1u64 << (1u32 << vars.len())).wrapping_sub(1) | if vars.len() == 6 { u64::MAX } else { 0 };
    let r = if tt == 0 {
        Res::Const(false)
    } else if tt == full {
        Res::Const(true)
    } else {
        // Projection on one variable?
        (0..vars.len())
            .find_map(|j| {
                let pat = (0..1u32 << vars.len()).fold(0u64, |acc, x| acc | (((x >> j) & 1) as u64) << x);
                if tt == pat {
                    Some(Res::Proj(vars[j], false))
                } else if tt == !pat & full {
                    Some(Res::Proj(vars[j], true))
                } else {
                    None
                }
            })
            .unwrap_or(Res::Unknown)
    };
    memo.insert(n, r);
    r
}

fn fresh(d: &Design, base: &str) -> String {
    if d.cell(base).is_none() && d.net(base).is_none() {
        return base.to_string();
    }
    (1..).map(|k| format!("{base}{k}")).find(|n| d.cell(n).is_none() && d.net(n).is_none()).unwrap()
}

fn apply(d: &mut Design, st: &mut Stats, flops: &[Flop], plans: Vec<Plan>) {
    // Drop the absorbed flops and dying LUTs, and the drivers of the nets
    // the SRLs take over.
    let mut gone: HashSet<Spur> = HashSet::new();
    for p in &plans {
        gone.extend(p.flops.iter().map(|&f| flops[f].cell));
        gone.extend(p.dead.iter().copied());
    }
    let n_dead_luts: usize = plans.iter().map(|p| p.dead.len()).sum();
    let n_flops: usize = plans.iter().map(|p| p.flops.len()).sum();
    d.cells.retain(|c| !gone.contains(&c.name));
    for n in &mut d.nets {
        n.endpoints.retain(|e| !gone.contains(&e.cell));
    }
    d.nets.retain(|n| !n.endpoints.is_empty() || d.ports.iter().any(|p| p.name == n.name));
    d.rebuild_indexes();

    let mut consts: [Option<String>; 2] = [None, None];
    let mut new_luts = 0usize;
    let mut konst = |d: &mut Design, v: bool, new_luts: &mut usize| -> String {
        if let Some(n) = &consts[v as usize] {
            return n.clone();
        }
        let n = fresh(d, if v { "srl_vcc" } else { "srl_gnd" });
        let cell = fresh(d, &format!("u_{n}"));
        d.add_cell(&cell, CellKind::Lut6 { init: if v { u64::MAX } else { 0 } });
        d.connect(&n, &cell, "O");
        *new_luts += 1;
        consts[v as usize] = Some(n.clone());
        n
    };
    let mut n_srl = 0usize;
    for p in &plans {
        let out = resolve(p.out).to_string();
        let ce = match p.ce {
            Some(c) => resolve(c).to_string(),
            None => konst(d, true, &mut new_luts),
        };
        let len = p.flops.len();
        // Address of each stage SRL's Q.
        let (nets, abits): (Vec<String>, usize) = match &p.addr {
            Some(bits) => {
                let mut v = Vec::new();
                for &(n, inv) in bits {
                    let name = resolve(n).to_string();
                    if inv {
                        let o = fresh(d, &format!("{name}_srl_n"));
                        let c = fresh(d, &format!("{o}_lut"));
                        d.add_cell(&c, CellKind::Lut6 { init: 0x5555_5555_5555_5555 });
                        d.connect(&name, &c, "I0");
                        d.connect(&o, &c, "O");
                        new_luts += 1;
                        v.push(o);
                    } else {
                        v.push(name);
                    }
                }
                let m = v.len();
                (v, m)
            }
            None => (Vec::new(), 0),
        };
        let n_srls = len.div_ceil(32);
        let use16 = len <= 16;
        let mut din = resolve(p.din).to_string();
        let mut stage_q: Vec<String> = Vec::new();
        for j in 0..n_srls {
            let last = j + 1 == n_srls;
            let cell = fresh(d, &format!("{out}_srl{}", if n_srls > 1 { j.to_string() } else { String::new() }));
            let module = if use16 { "SRL16E" } else { "SRLC32E" };
            d.add_cell(&cell, CellKind::BlackBox { module: intern(module) });
            let _ = d.set_cell_attr(&cell, "INIT", if use16 { "16'h0000" } else { "32'h00000000" });
            d.connect(&resolve(p.clk).to_string(), &cell, "CLK");
            d.connect(&ce, &cell, "CE");
            d.connect(&din, &cell, "D");
            let q = if p.addr.is_none() && last || n_srls == 1 { out.clone() } else { fresh(d, &format!("{cell}_q")) };
            d.connect(&q, &cell, "Q");
            stage_q.push(q);
            // Fixed tap: the last stage; dynamic: the low address bits.
            let fixed = if last { (len - 32 * j) - 1 } else { 31 };
            for i in 0..if use16 { 4 } else { 5 } {
                let net = if p.addr.is_some() {
                    if i < abits { nets[i].clone() } else { konst(d, false, &mut new_luts) }
                } else {
                    konst(d, (fixed >> i) & 1 == 1, &mut new_luts)
                };
                d.connect(&net, &cell, &format!("A{i}"));
            }
            if !last {
                let q31 = fresh(d, &format!("{cell}_q31"));
                d.connect(&q31, &cell, "Q31");
                din = q31;
            }
            n_srl += 1;
        }
        if p.addr.is_some() && n_srls > 1 {
            // Pick the stage SRL with the address bits above A4 (I4, I5).
            let mut init = 0u64;
            for idx in 0..64u64 {
                let sel = (idx >> 4) as usize & 3;
                if sel < n_srls && (idx >> sel) & 1 == 1 {
                    init |= 1 << idx;
                }
            }
            let c = fresh(d, &format!("{out}_srlmux"));
            d.add_cell(&c, CellKind::Lut6 { init });
            for (i, q) in stage_q.iter().enumerate() {
                d.connect(q, &c, &format!("I{i}"));
            }
            for (k, n) in nets[5..].iter().enumerate() {
                d.connect(n, &c, &format!("I{}", 4 + k));
            }
            d.connect(&out, &c, "O");
            new_luts += 1;
        }
    }
    d.attrs.set("SRL_CELLS", n_srl.to_string());
    st.ffs -= n_flops;
    st.luts = st.luts + new_luts + n_srl - n_dead_luts;
    let driven: HashSet<Spur> = d.nets.iter().filter(|n| n.endpoints.iter().any(|e| resolve(e.pin) == "Q")).map(|n| n.name).collect();
    st.regmap.retain(|r| driven.contains(&intern(&r.net)));
}
