//! Symbolic execution of processes and subprograms.
//!
//! A process body is executed once over the dataflow graph: variables hold
//! their current value, signal assignments update a pending value (reads
//! always see the signal itself, as in VHDL), branches fork the state and
//! merge it with multiplexers. Clocked processes become registers whose D
//! input is the final pending value; combinational processes become
//! drivers, and any path that keeps an old value is reported as a latch.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use super::ast::*;
use super::dag::*;
use super::elab::*;
use super::expr::*;

const MAX_LOOP: usize = 1 << 16;

fn name_key(n: &Name) -> String {
    match n {
        Name::Id(s, _) => s.clone(),
        Name::Sel(p, f) => format!("{}.{f}", name_key(p)),
        Name::Call(p, a) => format!("{}({})", name_key(p), a.len()),
        Name::Slice(p, _) => format!("{}(..)", name_key(p)),
        Name::Attr(p, a, _) => format!("{}'{a}", name_key(p)),
        Name::OpSym(s) => s.clone(),
    }
}

fn event_of(e: &Expr) -> Option<Name> {
    match e {
        Expr::Name(n) => match &**n {
            Name::Attr(p, a, None) if a == "event" => Some((**p).clone()),
            _ => None,
        },
        Expr::Un(UnOp::Not, x) => match &**x {
            Expr::Name(n) => match &**n {
                Name::Attr(p, a, None) if a == "stable" => Some((**p).clone()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn level_of(e: &Expr) -> Option<(Name, bool)> {
    if let Expr::Bin(BinOp::Eq, a, b) = e {
        match (&**a, &**b) {
            (Expr::Name(n), Expr::Char(c)) | (Expr::Char(c), Expr::Name(n)) => {
                return Some(((**n).clone(), *c == '1'));
            }
            _ => {}
        }
    }
    None
}

fn and_opt(a: Option<Expr>, b: &Expr) -> Expr {
    match a {
        Some(x) => Expr::Bin(BinOp::And, Box::new(x), Box::new(b.clone())),
        None => b.clone(),
    }
}

/// Clock edge expression: (clock name, rising, extra enable condition).
pub fn edge_of(e: &Expr) -> Option<(Name, bool, Option<Expr>)> {
    match e {
        Expr::Name(n) => match &**n {
            Name::Call(p, args) if args.len() == 1 => {
                let f = match &**p {
                    Name::Id(f, _) => f.as_str(),
                    Name::Sel(_, f) => f.as_str(),
                    _ => return None,
                };
                let rising = match f {
                    "rising_edge" => true,
                    "falling_edge" => false,
                    _ => return None,
                };
                match &args[0].actual {
                    Expr::Name(c) => Some(((**c).clone(), rising, None)),
                    _ => None,
                }
            }
            _ => None,
        },
        Expr::Bin(BinOp::And, a, b) => {
            if let (Some(c1), Some((c2, lvl))) = (event_of(a), level_of(b)) {
                if name_key(&c1) == name_key(&c2) {
                    return Some((c1, lvl, None));
                }
            }
            if let (Some(c1), Some((c2, lvl))) = (event_of(b), level_of(a)) {
                if name_key(&c1) == name_key(&c2) {
                    return Some((c1, lvl, None));
                }
            }
            if !expr_has_edge(b) {
                if let Some((c, r, en)) = edge_of(a) {
                    return Some((c, r, Some(and_opt(en, b))));
                }
            }
            if !expr_has_edge(a) {
                if let Some((c, r, en)) = edge_of(b) {
                    return Some((c, r, Some(and_opt(en, a))));
                }
            }
            None
        }
        Expr::Un(UnOp::Cond, x) => edge_of(x),
        _ => None,
    }
}

fn name_has_edge(n: &Name) -> bool {
    match n {
        Name::Call(p, args) => {
            if let Name::Id(f, _) | Name::Sel(_, f) = &**p {
                if f == "rising_edge" || f == "falling_edge" {
                    return true;
                }
            }
            name_has_edge(p) || args.iter().any(|a| expr_has_edge(&a.actual))
        }
        Name::Attr(p, a, x) => {
            a == "event"
                || a == "stable"
                || name_has_edge(p)
                || x.as_deref().map(expr_has_edge).unwrap_or(false)
        }
        Name::Sel(p, _) | Name::Slice(p, _) => name_has_edge(p),
        _ => false,
    }
}

pub fn expr_has_edge(e: &Expr) -> bool {
    match e {
        Expr::Name(n) => name_has_edge(n),
        Expr::Bin(_, a, b) => expr_has_edge(a) || expr_has_edge(b),
        Expr::Un(_, a) | Expr::Qual(_, a) => expr_has_edge(a),
        Expr::Agg(es) => es.iter().any(|x| expr_has_edge(&x.val)),
        _ => false,
    }
}

fn seq_has_edge(s: &Seq) -> bool {
    match s {
        Seq::If { arms, els, .. } => {
            arms.iter()
                .any(|(c, b)| expr_has_edge(c) || b.iter().any(seq_has_edge))
                || els
                    .as_ref()
                    .map(|b| b.iter().any(seq_has_edge))
                    .unwrap_or(false)
        }
        Seq::Case { sel, arms, .. } => {
            expr_has_edge(sel) || arms.iter().any(|(_, b)| b.iter().any(seq_has_edge))
        }
        Seq::For { body, .. } | Seq::While { body, .. } => body.iter().any(seq_has_edge),
        Seq::SigAsg { rhs, .. } | Seq::VarAsg { rhs, .. } => rhs
            .iter()
            .any(|c| expr_has_edge(&c.val) || c.cond.as_ref().map(expr_has_edge).unwrap_or(false)),
        Seq::Wait { .. } => true,
        _ => false,
    }
}

fn count_waits(b: &[Seq]) -> usize {
    b.iter()
        .map(|s| match s {
            Seq::Wait { .. } => 1,
            Seq::If { arms, els, .. } => {
                arms.iter().map(|(_, x)| count_waits(x)).sum::<usize>()
                    + els.as_ref().map(|x| count_waits(x)).unwrap_or(0)
            }
            Seq::Case { arms, .. } => arms.iter().map(|(_, x)| count_waits(x)).sum(),
            Seq::For { body, .. } | Seq::While { body, .. } => count_waits(body),
            _ => 0,
        })
        .sum()
}

fn wrap_enable(en: Option<Expr>, body: Vec<Seq>, line: Line) -> Vec<Seq> {
    match en {
        Some(c) => vec![Seq::If {
            arms: vec![(c, body)],
            els: None,
            line,
        }],
        None => body,
    }
}

fn runs(mask: &[bool]) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < mask.len() {
        if !mask[i] {
            i += 1;
            continue;
        }
        let s = i;
        while i < mask.len() && mask[i] {
            i += 1;
        }
        v.push((s as u32, (i - s) as u32));
    }
    v
}

impl Elab {
    pub fn elab_process(
        &mut self,
        label: &str,
        _sens: &Sens,
        decls: &[Decl],
        body: &[Seq],
        line: Line,
    ) -> R<()> {
        let body: Vec<Seq> = body
            .iter()
            .filter(|s| !matches!(s, Seq::Null))
            .cloned()
            .collect();
        let waits = count_waits(&body);
        if waits > 0 {
            if let (
                Some(Seq::Wait {
                    until: Some(c),
                    has_for: false,
                    line: wl,
                    ..
                }),
                1,
            ) = (body.first(), waits)
            {
                let Some((clk, rising, en)) = edge_of(c) else {
                    return err(
                        *wl,
                        "`wait until` must name a clock edge (e.g. `wait until rising_edge(clk);`)",
                    );
                };
                let rest = wrap_enable(en, body[1..].to_vec(), *wl);
                return self.run_clocked(label, decls, &clk, rising, None, &rest, line);
            }
            return err(
                line,
                "wait statements are only supported as a single leading `wait until rising_edge(clk);`",
            );
        }
        if body.len() == 1 {
            if let Seq::If {
                arms,
                els,
                line: il,
            } = &body[0]
            {
                if let Some(k) = arms.iter().position(|(c, _)| edge_of(c).is_some()) {
                    if arms[..k]
                        .iter()
                        .any(|(c, b)| expr_has_edge(c) || b.iter().any(seq_has_edge))
                    {
                        return err(*il, "clock edge in an asynchronous branch");
                    }
                    if k + 1 < arms.len() || els.is_some() {
                        return err(
                            *il,
                            "an elsif/else after the clock edge branch is not synthesizable",
                        );
                    }
                    if arms[k].1.iter().any(seq_has_edge) {
                        return err(*il, "nested clock edge inside a clocked branch");
                    }
                    let (clk, rising, en) = edge_of(&arms[k].0).unwrap();
                    let clk_body = wrap_enable(en, arms[k].1.clone(), *il);
                    if k > 1 {
                        return err(
                            *il,
                            "more than one asynchronous set/reset branch is not supported",
                        );
                    }
                    let async_arm = if k == 1 { Some(arms[0].clone()) } else { None };
                    return self.run_clocked(
                        label,
                        decls,
                        &clk,
                        rising,
                        async_arm.as_ref(),
                        &clk_body,
                        line,
                    );
                }
            }
        }
        // Several `if rising_edge(clk) then ... end if;` in sequence.
        let edge_ifs = !body.is_empty()
            && body.iter().all(|s| matches!(s, Seq::If { arms, els: None, .. } if arms.len() == 1 && edge_of(&arms[0].0).is_some() && !arms[0].1.iter().any(seq_has_edge)));
        if edge_ifs {
            let mut clk_body = Vec::new();
            let mut clock: Option<(Name, bool)> = None;
            for s in &body {
                let Seq::If { arms, line: il, .. } = s else {
                    unreachable!()
                };
                let (clk, rising, en) = edge_of(&arms[0].0).unwrap();
                if let Some((c0, r0)) = &clock {
                    if name_key(c0) != name_key(&clk) || *r0 != rising {
                        return err(*il, "one process clocked by two different edges");
                    }
                }
                clock = Some((clk, rising));
                clk_body.extend(wrap_enable(en, arms[0].1.clone(), *il));
            }
            let (clk, rising) = clock.unwrap();
            return self.run_clocked(label, decls, &clk, rising, None, &clk_body, line);
        }
        if body.iter().any(seq_has_edge) {
            return err(
                line,
                "unsupported clocked process: the clock edge must be the outermost `if` (optionally after one asynchronous reset branch)",
            );
        }
        self.run_comb(label, decls, &body, line)
    }

    fn run_clocked(
        &mut self,
        label: &str,
        decls: &[Decl],
        clk: &Name,
        rising: bool,
        async_arm: Option<&(Expr, Vec<Seq>)>,
        body: &[Seq],
        line: Line,
    ) -> R<()> {
        if !rising {
            return err(line, "falling-edge registers are not supported yet");
        }
        self.push_px(PxKind::Clocked, line, format!("{}{label}", self.prefix));
        self.scopes.push(HashMap::new());
        let r = self.run_clocked_inner(decls, clk, async_arm, body, line);
        self.scopes.pop();
        self.px.pop();
        r
    }

    fn run_clocked_inner(
        &mut self,
        decls: &[Decl],
        clk: &Name,
        async_arm: Option<&(Expr, Vec<Seq>)>,
        body: &[Seq],
        line: Line,
    ) -> R<()> {
        self.elab_decls(decls)?;
        let (ct, cn) = self.eval_bits(&Expr::Name(Box::new(clk.clone())), None, line)?;
        if !ct.is_bit() {
            return err(line, "clock must be a std_logic signal");
        }
        let s0 = self.px.last().unwrap().st.clone();
        let mut arst: Option<(N, bool)> = None;
        let mut s_a: Option<St> = None;
        let mut drv_a: HashMap<u32, Vec<bool>> = HashMap::new();
        if let Some((c, ab)) = async_arm {
            let cn = self.cond(c, line)?;
            let (sig, high) = match self.dag.op(cn).clone() {
                Op::Not(x) => (x, false),
                _ => (cn, true),
            };
            if self.dag.as_const(sig).is_some() {
                return err(line, "asynchronous reset condition is constant");
            }
            arst = Some((sig, high));
            self.exec_seqs(ab)?;
            let px = self.px.last_mut().unwrap();
            s_a = Some(std::mem::replace(&mut px.st, s0.clone()));
            drv_a = std::mem::take(&mut px.driven);
        }
        self.exec_seqs(body)?;
        let px = self.px.last_mut().unwrap();
        let s_c = px.st.clone();
        let drv_c = std::mem::take(&mut px.driven);
        let var_nets = px.var_nets.clone();
        let mut nets: Vec<u32> = drv_a.keys().chain(drv_c.keys()).copied().collect();
        nets.sort();
        nets.dedup();
        for net in nets {
            let w = self.nets[net as usize].w;
            let ma = drv_a
                .get(&net)
                .cloned()
                .unwrap_or_else(|| vec![false; w as usize]);
            let mc = drv_c
                .get(&net)
                .cloned()
                .unwrap_or_else(|| vec![false; w as usize]);
            let all: Vec<bool> = ma.iter().zip(&mc).map(|(a, c)| *a || *c).collect();
            let rd = self.dag.read(net, w);
            let pc = s_c.pend.get(&net).copied().unwrap_or(rd);
            for (lo0, len0) in runs(&all) {
                // split by whether the async branch drives the bit
                let mut k = lo0;
                while k < lo0 + len0 {
                    let a = ma[k as usize];
                    let s = k;
                    while k < lo0 + len0 && ma[k as usize] == a {
                        k += 1;
                    }
                    let (lo, len) = (s, k - s);
                    let q = self.dag.slice(rd, lo, len);
                    let mut d = self.dag.slice(pc, lo, len);
                    let mut ar = None;
                    if let Some((sig, high)) = arst {
                        if a {
                            let pa = s_a.as_ref().unwrap().pend.get(&net).copied().unwrap_or(rd);
                            let rv = self.dag.slice(pa, lo, len);
                            if self.dag.as_const(rv).is_none() {
                                return err(
                                    line,
                                    format!(
                                        "asynchronous reset value of `{}` is not constant (asynchronous load is not supported)",
                                        self.nets[net as usize].name
                                    ),
                                );
                            }
                            ar = Some(Arst {
                                sig,
                                active_high: high,
                                val: rv,
                            });
                        } else {
                            let active = if high { sig } else { self.dag.not(sig) };
                            d = self.dag.mux(active, q, d);
                        }
                    }
                    let init =
                        self.nets[net as usize].init[lo as usize..(lo + len) as usize].to_vec();
                    let name = format!("{}[{}:{}]", self.nets[net as usize].name, lo + len - 1, lo);
                    self.regs.push(Reg {
                        name,
                        w: len,
                        clk: cn,
                        neg: false,
                        d,
                        init,
                        arst: ar,
                    });
                    let id = (self.regs.len() - 1) as u32;
                    self.add_driver(net, lo, len, DrvSrc::Reg(id), line);
                }
            }
        }
        // Process variables that keep state across clock edges.
        for (slot, vnet) in var_nets.iter().enumerate() {
            let Some(vnet) = *vnet else { continue };
            let w = self.nets[vnet as usize].w;
            let q = self.dag.read(vnet, w);
            let mut d = s_c.vars[slot];
            let mut ar = None;
            if let Some((sig, high)) = arst {
                let va = s_a.as_ref().unwrap().vars[slot];
                if va != s0.vars[slot] {
                    if self.dag.as_const(va).is_none() {
                        return err(
                            line,
                            "asynchronous reset value of a variable is not constant",
                        );
                    }
                    ar = Some(Arst {
                        sig,
                        active_high: high,
                        val: va,
                    });
                } else {
                    let active = if high { sig } else { self.dag.not(sig) };
                    d = self.dag.mux(active, q, d);
                }
            }
            if d == q && ar.is_none() {
                continue; // never assigned: keeps its initial value
            }
            let init = self.nets[vnet as usize].init.clone();
            let name = self.nets[vnet as usize].name.clone();
            self.regs.push(Reg {
                name,
                w,
                clk: cn,
                neg: false,
                d,
                init,
                arst: ar,
            });
            let id = (self.regs.len() - 1) as u32;
            self.add_driver(vnet, 0, w, DrvSrc::Reg(id), line);
        }
        Ok(())
    }

    fn run_comb(&mut self, label: &str, decls: &[Decl], body: &[Seq], line: Line) -> R<()> {
        self.push_px(PxKind::Comb, line, format!("{}{label}", self.prefix));
        self.scopes.push(HashMap::new());
        let r = (|| -> R<()> {
            self.elab_decls(decls)?;
            self.exec_seqs(body)?;
            let px = self.px.last_mut().unwrap();
            let st = px.st.clone();
            let drv = std::mem::take(&mut px.driven);
            let var_names = px.var_names.clone();
            let mut nets: Vec<u32> = drv.keys().copied().collect();
            nets.sort();
            for net in nets {
                let mask = &drv[&net];
                let p = st.pend[&net];
                let mut memo = HashMap::new();
                let tainted = self.dag.tainted_bits(
                    p,
                    &|op| matches!(op, Op::Hold(_) | Op::VHold(_)),
                    &mut memo,
                );
                let bad: Vec<usize> = (0..mask.len()).filter(|&i| mask[i] && tainted[i]).collect();
                if !bad.is_empty() {
                    let name = self.nets[net as usize].name.clone();
                    if let Some(v) = self.find_vhold(p) {
                        return err(
                            line,
                            format!(
                                "variable `{}` is read before it is assigned in this combinational process (that would be a latch); assign it first",
                                var_names.get(v as usize).cloned().unwrap_or_default()
                            ),
                        );
                    }
                    let what = if bad.len() == mask.len() {
                        String::new()
                    } else {
                        format!(" (bits {:?})", bad.iter().take(8).collect::<Vec<_>>())
                    };
                    return err(
                        line,
                        format!(
                            "latch inferred: `{name}`{what} is not assigned on every path through this combinational process; give it a default value or an else branch"
                        ),
                    );
                }
                for (lo, len) in runs(mask) {
                    let s = self.dag.slice(p, lo, len);
                    self.add_driver(net, lo, len, DrvSrc::Comb(s), line);
                }
            }
            Ok(())
        })();
        self.scopes.pop();
        self.px.pop();
        r
    }

    fn find_vhold(&self, n: N) -> Option<u32> {
        let mut stack = vec![n];
        let mut seen = std::collections::HashSet::new();
        while let Some(x) = stack.pop() {
            if !seen.insert(x) {
                continue;
            }
            if let Op::VHold(v) = self.dag.op(x) {
                return Some(*v);
            }
            stack.extend(self.dag.kids(x));
        }
        None
    }

    // ------------------------------------------------------------------
    // Statements

    pub fn exec_seqs(&mut self, body: &[Seq]) -> R<()> {
        for s in body {
            let d = self.dead();
            if self.dag.as_const(d) == Some(&[true]) {
                break;
            }
            self.exec(s)?;
        }
        Ok(())
    }

    fn exec(&mut self, s: &Seq) -> R<()> {
        match s {
            Seq::Null => Ok(()),
            Seq::SigAsg { target, rhs, line } => self.exec_asg(target, rhs, true, *line),
            Seq::VarAsg { target, rhs, line } => self.exec_asg(target, rhs, false, *line),
            Seq::SelAsg {
                sel,
                target,
                var,
                arms,
                line,
            } => {
                let arms: Vec<(Vec<Choice>, Vec<Seq>)> = arms
                    .iter()
                    .map(|(v, ch)| {
                        let rhs = vec![CondVal {
                            val: v.clone(),
                            cond: None,
                        }];
                        let st = if *var {
                            Seq::VarAsg {
                                target: target.clone(),
                                rhs,
                                line: *line,
                            }
                        } else {
                            Seq::SigAsg {
                                target: target.clone(),
                                rhs,
                                line: *line,
                            }
                        };
                        (ch.clone(), vec![st])
                    })
                    .collect();
                self.exec_case(sel, &arms, false, *line)
            }
            Seq::If { arms, els, line } => self.exec_if(arms, els.as_deref(), *line),
            Seq::Case {
                sel,
                arms,
                matching,
                line,
            } => self.exec_case(sel, arms, *matching, *line),
            Seq::For {
                label,
                var,
                range,
                body,
                line,
            } => self.exec_for(label, var, range, body, *line),
            Seq::While {
                label,
                cond,
                body,
                line,
            } => self.exec_while(label, cond.as_ref(), body, *line),
            Seq::Next { label, cond, line } => self.exec_jump(false, label, cond.as_ref(), *line),
            Seq::Exit { label, cond, line } => self.exec_jump(true, label, cond.as_ref(), *line),
            Seq::Return { val, line } => self.exec_return(val.as_ref(), *line),
            Seq::Wait { line, .. } => err(
                *line,
                "wait statements inside a process body are not supported",
            ),
            Seq::Call { name, line } => self.exec_call(name, *line),
        }
    }

    fn exec_asg(&mut self, target: &Expr, rhs: &[CondVal], sig: bool, line: Line) -> R<()> {
        if rhs.len() == 1 && rhs[0].cond.is_none() {
            if matches!(rhs[0].val, Expr::Open) {
                return Ok(());
            }
            let lv = self.lval(target, line)?;
            match (&lv.base, sig) {
                (LBase::Net(_), false) => return err(line, "`:=` assigns a signal (use `<=`)"),
                (LBase::Var(_), true) => return err(line, "`<=` assigns a variable (use `:=`)"),
                _ => {}
            }
            let v = self.eval(&rhs[0].val, Some(&lv.ty), line)?;
            let v = self.convert(v, &lv.ty, line)?;
            let n = bits_n(&v, line)?;
            return self.write_lval(&lv, n, line);
        }
        // Conditional assignment: an if/elsif chain.
        let mut arms = Vec::new();
        let mut els = None;
        for cv in rhs {
            let one = vec![CondVal {
                val: cv.val.clone(),
                cond: None,
            }];
            let st = if sig {
                Seq::SigAsg {
                    target: target.clone(),
                    rhs: one,
                    line,
                }
            } else {
                Seq::VarAsg {
                    target: target.clone(),
                    rhs: one,
                    line,
                }
            };
            match &cv.cond {
                Some(c) => arms.push((c.clone(), vec![st])),
                None => {
                    els = Some(vec![st]);
                    break;
                }
            }
        }
        self.exec_if(&arms, els.as_deref(), line)
    }

    fn pend_base(&mut self, net: u32, line: Line) -> R<N> {
        let w = self.nets[net as usize].w;
        match self.px.last().unwrap().kind {
            PxKind::Comb => Ok(self.dag.hold(net, w)),
            PxKind::Clocked => Ok(self.dag.read(net, w)),
            PxKind::Static => err(line, "signal assignment inside a function"),
        }
    }

    fn insert(&mut self, old: N, off: &Off, v: N, w: u32) -> N {
        let total = self.dag.w(old);
        match off {
            Off::Static(o) => {
                let o = *o;
                if o == 0 && w == total {
                    return v;
                }
                let hi = self.dag.slice(old, o + w, total - o - w);
                let lo = self.dag.slice(old, 0, o);
                self.dag.concat(vec![hi, v, lo])
            }
            Off::Dyn(alts) => {
                let mut a = alts.clone();
                a.sort_by_key(|x| x.1);
                let overlap = a.windows(2).any(|p| p[0].1 + w > p[1].1);
                if overlap {
                    let mut acc = old;
                    for (c, o) in a {
                        let ins = self.insert(acc, &Off::Static(o), v, w);
                        acc = self.dag.mux(c, ins, acc);
                    }
                    return acc;
                }
                let mut parts = Vec::new();
                let mut pos = 0u32;
                for (c, o) in a {
                    if o > pos {
                        parts.push(self.dag.slice(old, pos, o - pos));
                    }
                    let seg = self.dag.slice(old, o, w);
                    parts.push(self.dag.mux(c, v, seg));
                    pos = o + w;
                }
                if pos < total {
                    parts.push(self.dag.slice(old, pos, total - pos));
                }
                parts.reverse();
                self.dag.concat(parts)
            }
        }
    }

    pub fn write_lval(&mut self, lv: &LVal, v: N, line: Line) -> R<()> {
        let w = lv.ty.width()?;
        if self.dag.w(v) != w {
            return err(
                line,
                format!(
                    "internal: {}-bit value written to a {w}-bit target",
                    self.dag.w(v)
                ),
            );
        }
        let dead = self.dead();
        match lv.base {
            LBase::Var(slot) => {
                let old = self.px.last().unwrap().st.vars[slot as usize];
                let new = self.insert(old, &lv.off, v, w);
                let new = self.dag.mux(dead, old, new);
                self.px.last_mut().unwrap().st.vars[slot as usize] = new;
            }
            LBase::Net(net) => {
                let old = match self.px.last().unwrap().st.pend.get(&net) {
                    Some(&x) => x,
                    None => self.pend_base(net, line)?,
                };
                let new = self.insert(old, &lv.off, v, w);
                let new = self.dag.mux(dead, old, new);
                let nw = self.nets[net as usize].w as usize;
                let px = self.px.last_mut().unwrap();
                px.st.pend.insert(net, new);
                let m = px.driven.entry(net).or_insert_with(|| vec![false; nw]);
                let mut mark = |o: u32| {
                    for b in o..o + w {
                        m[b as usize] = true;
                    }
                };
                match &lv.off {
                    Off::Static(o) => mark(*o),
                    Off::Dyn(alts) => {
                        for (_, o) in alts {
                            mark(*o);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn merge(&mut self, c: N, a: St, b: St, line: Line) -> R<St> {
        let mut vars = Vec::with_capacity(a.vars.len());
        for (x, y) in a.vars.iter().zip(&b.vars) {
            vars.push(self.dag.mux(c, *x, *y));
        }
        let mut keys: Vec<u32> = a.pend.keys().chain(b.pend.keys()).copied().collect();
        keys.sort();
        keys.dedup();
        let mut pend = BTreeMap::new();
        for k in keys {
            let x = match a.pend.get(&k) {
                Some(&x) => x,
                None => self.pend_base(k, line)?,
            };
            let y = match b.pend.get(&k) {
                Some(&y) => y,
                None => self.pend_base(k, line)?,
            };
            pend.insert(k, self.dag.mux(c, x, y));
        }
        let ret = self.dag.mux(c, a.ret, b.ret);
        let brk = self.dag.mux(c, a.brk, b.brk);
        let cont = self.dag.mux(c, a.cont, b.cont);
        let retv = match (a.retv, b.retv) {
            (Some(x), Some(y)) if self.dag.w(x) == self.dag.w(y) => Some(self.dag.mux(c, x, y)),
            (Some(_), Some(_)) => {
                return err(
                    line,
                    "function returns values of different lengths on different paths",
                );
            }
            (Some(x), None) => Some(x),
            (None, y) => y,
        };
        Ok(St {
            vars,
            pend,
            ret,
            retv,
            brk,
            cont,
        })
    }

    fn exec_if(&mut self, arms: &[(Expr, Vec<Seq>)], els: Option<&[Seq]>, line: Line) -> R<()> {
        let Some((c0, b0)) = arms.first() else {
            if let Some(e) = els {
                self.exec_seqs(e)?;
            }
            return Ok(());
        };
        let c = self.cond(c0, line)?;
        match self.dag.as_const(c) {
            Some([true]) => return self.exec_seqs(b0),
            Some([false]) => return self.exec_if(&arms[1..], els, line),
            _ => {}
        }
        let s0 = self.px.last().unwrap().st.clone();
        self.exec_seqs(b0)?;
        let s1 = std::mem::replace(&mut self.px.last_mut().unwrap().st, s0);
        self.exec_if(&arms[1..], els, line)?;
        let s2 = self.px.last().unwrap().st.clone();
        let m = self.merge(c, s1, s2, line)?;
        self.px.last_mut().unwrap().st = m;
        Ok(())
    }

    fn choice_cond(&mut self, sv: &Val, ch: &Choice, matching: bool, line: Line) -> R<Option<N>> {
        let st = sv.ty();
        match ch {
            Choice::Others => Ok(None),
            Choice::Expr(e) => {
                if let Some(m) = meta_lit_s(e) {
                    if matching {
                        return Ok(Some(self.masked_eq_pub(sv, &m, line)?));
                    }
                    return Ok(Some(self.dag.bit(false)));
                }
                let cv = self.eval(e, Some(&st), line)?;
                if let (Val::B { n: a, .. }, Val::B { n: b, .. }) = (sv, &cv) {
                    if matches!(st, Ty::Arr(_)) && self.dag.w(*a) != self.dag.w(*b) {
                        return err(
                            line,
                            format!(
                                "case choice has {} bits, selector has {}",
                                self.dag.w(*b),
                                self.dag.w(*a)
                            ),
                        );
                    }
                }
                let r = self.compare(BinOp::Eq, sv.clone(), cv, line)?;
                Ok(Some(bits_n(&r, line)?))
            }
            Choice::Range(r) => {
                let (ir, _) = self.eval_range(r, line)?;
                let lo = self.int_val(ir.lo());
                let hi = self.int_val(ir.hi());
                let (lo, hi) = match &st {
                    Ty::Enum(_) => {
                        let w = st.width()?;
                        let a = self.dag.kval(ir.lo() as u128, w);
                        let b = self.dag.kval(ir.hi() as u128, w);
                        (
                            Val::B {
                                ty: st.clone(),
                                n: a,
                            },
                            Val::B {
                                ty: st.clone(),
                                n: b,
                            },
                        )
                    }
                    _ => (lo, hi),
                };
                let ge = self.compare(BinOp::Ge, sv.clone(), lo, line)?;
                let le = self.compare(BinOp::Le, sv.clone(), hi, line)?;
                let (g, l) = (bits_n(&ge, line)?, bits_n(&le, line)?);
                Ok(Some(self.dag.and(g, l)))
            }
        }
    }

    fn exec_case(
        &mut self,
        sel: &Expr,
        arms: &[(Vec<Choice>, Vec<Seq>)],
        matching: bool,
        line: Line,
    ) -> R<()> {
        let sv = self.eval(sel, None, line)?;
        let mut conds: Vec<Option<N>> = Vec::new();
        for (choices, _) in arms {
            let mut c = self.dag.bit(false);
            let mut others = false;
            for ch in choices {
                match self.choice_cond(&sv, ch, matching, line)? {
                    Some(x) => c = self.dag.or(c, x),
                    None => others = true,
                }
            }
            conds.push(if others { None } else { Some(c) });
        }
        // Static selector (or statically decided choices).
        let mut decided = true;
        for c in conds.iter().flatten() {
            if self.dag.as_const(*c).is_none() {
                decided = false;
            }
        }
        if decided {
            for (k, c) in conds.iter().enumerate() {
                match c {
                    Some(x) if self.dag.as_const(*x) == Some(&[true]) => {
                        return self.exec_seqs(&arms[k].1);
                    }
                    None => return self.exec_seqs(&arms[k].1),
                    _ => {}
                }
            }
            return Ok(());
        }
        let s0 = self.px.last().unwrap().st.clone();
        let mut states: Vec<(N, St)> = Vec::new();
        let mut others: Option<St> = None;
        for (k, (_, body)) in arms.iter().enumerate() {
            self.px.last_mut().unwrap().st = s0.clone();
            self.exec_seqs(body)?;
            let s = self.px.last().unwrap().st.clone();
            match conds[k] {
                Some(c) => states.push((c, s)),
                None => others = Some(s),
            }
        }
        // Without `others` the choices cover every value: the last arm is
        // the fallback.
        let mut acc = match others {
            Some(s) => s,
            None => match states.pop() {
                Some((_, s)) => s,
                None => s0,
            },
        };
        for (c, s) in states.into_iter().rev() {
            acc = self.merge(c, s, acc, line)?;
        }
        self.px.last_mut().unwrap().st = acc;
        Ok(())
    }

    fn loop_enter(&mut self, label: &Option<String>) -> (N, N) {
        let d = {
            let st = &self.px.last().unwrap().st;
            (st.brk, st.cont)
        };
        let outer = self.dag.or(d.0, d.1);
        let z = self.dag.bit(false);
        let px = self.px.last_mut().unwrap();
        px.st.brk = outer;
        px.st.cont = z;
        px.loops.push(label.clone());
        d
    }

    fn loop_leave(&mut self, saved: (N, N)) {
        let px = self.px.last_mut().unwrap();
        px.loops.pop();
        px.st.brk = saved.0;
        px.st.cont = saved.1;
    }

    fn exec_for(
        &mut self,
        label: &Option<String>,
        var: &str,
        range: &DiscRange,
        body: &[Seq],
        line: Line,
    ) -> R<()> {
        let (r, it) = self.eval_disc_range(range, line)?;
        if r.len() as usize > MAX_LOOP {
            return err(line, "for loop with too many iterations");
        }
        let saved = self.loop_enter(label);
        let mut res = Ok(());
        for i in r.iter() {
            let d = self.dead();
            if self.dag.as_const(d) == Some(&[true]) {
                break;
            }
            let z = self.dag.bit(false);
            self.px.last_mut().unwrap().st.cont = z;
            let v = match &it {
                Ty::Enum(_) => {
                    let w = it.width()?;
                    let n = self.dag.kval(i as u128, w);
                    Val::B { ty: it.clone(), n }
                }
                _ => self.int_val(i),
            };
            self.scopes
                .push(HashMap::from([(var.to_string(), Sym::Val(v))]));
            res = self.exec_seqs(body);
            self.scopes.pop();
            if res.is_err() {
                break;
            }
        }
        self.loop_leave(saved);
        res
    }

    fn exec_while(
        &mut self,
        label: &Option<String>,
        cond: Option<&Expr>,
        body: &[Seq],
        line: Line,
    ) -> R<()> {
        let saved = self.loop_enter(label);
        let mut n = 0usize;
        let res = loop {
            let d = self.dead();
            if self.dag.as_const(d) == Some(&[true]) {
                break Ok(());
            }
            let z = self.dag.bit(false);
            self.px.last_mut().unwrap().st.cont = z;
            if let Some(c) = cond {
                let cn = match self.cond(c, line) {
                    Ok(x) => x,
                    Err(e) => break Err(e),
                };
                match self.dag.as_const(cn) {
                    Some([false]) => break Ok(()),
                    Some(_) => {}
                    None => {
                        break err(
                            line,
                            "while-loop condition is not static (the iteration count must be known at synthesis time)",
                        );
                    }
                }
            }
            if let Err(e) = self.exec_seqs(body) {
                break Err(e);
            }
            n += 1;
            if n > MAX_LOOP {
                break err(
                    line,
                    "loop does not terminate within 65536 iterations at synthesis time",
                );
            }
        };
        self.loop_leave(saved);
        res
    }

    fn exec_jump(
        &mut self,
        exit: bool,
        label: &Option<String>,
        cond: Option<&Expr>,
        line: Line,
    ) -> R<()> {
        let loops = &self.px.last().unwrap().loops;
        if loops.is_empty() {
            return err(line, "next/exit outside a loop");
        }
        if let Some(l) = label {
            if loops.last().unwrap().as_deref() != Some(l.as_str()) {
                return err(
                    line,
                    format!("next/exit of an outer loop `{l}` is not supported"),
                );
            }
        }
        let d = self.dead();
        let mut g = self.dag.not(d);
        if let Some(c) = cond {
            let cn = self.cond(c, line)?;
            g = self.dag.and(g, cn);
        }
        let px_st = &self.px.last().unwrap().st;
        let (b, c) = (px_st.brk, px_st.cont);
        if exit {
            let nb = self.dag.or(b, g);
            self.px.last_mut().unwrap().st.brk = nb;
        } else {
            let nc = self.dag.or(c, g);
            self.px.last_mut().unwrap().st.cont = nc;
        }
        Ok(())
    }

    fn exec_return(&mut self, val: Option<&Expr>, line: Line) -> R<()> {
        let d = self.dead();
        let g = self.dag.not(d);
        if let Some(e) = val {
            let rt =
                self.ret_tys.last().cloned().ok_or_else(|| {
                    format!("line {line}: return with a value outside a function")
                })?;
            let v = self.eval(e, Some(&rt), line)?;
            let rt2 = self.constrain_from(&rt, &v, line)?;
            let v = self.convert(v, &rt2, line)?;
            let n = bits_n(&v, line)?;
            if let Some(slot) = self.ret_vtys.last_mut() {
                if slot.is_none() {
                    *slot = Some(rt2);
                }
            }
            let old = self.px.last().unwrap().st.retv;
            let nv = match old {
                None => n,
                Some(o) if self.dag.w(o) == self.dag.w(n) => self.dag.mux(g, n, o),
                Some(_) => return err(line, "function returns values of different lengths"),
            };
            self.px.last_mut().unwrap().st.retv = Some(nv);
        }
        let r = self.px.last().unwrap().st.ret;
        let nr = self.dag.or(r, g);
        self.px.last_mut().unwrap().st.ret = nr;
        Ok(())
    }

    fn exec_call(&mut self, name: &Name, line: Line) -> R<()> {
        let (p, args): (&Name, &[Assoc]) = match name {
            Name::Call(p, a) => (p, a),
            n => (n, &[]),
        };
        let f = match p {
            Name::Id(f, _) => f.clone(),
            Name::Sel(q, f) if self.is_pkg_prefix(q) => f.clone(),
            _ => return err(line, "unsupported procedure call"),
        };
        match self.lookup(&f).cloned() {
            Some(Sym::Subs(subs)) => {
                self.call_user(&subs, args, None, false, line)?;
                Ok(())
            }
            _ => match f.as_str() {
                "finish" | "stop" | "write" | "writeline" | "read" | "readline" | "deallocate"
                | "file_open" | "file_close" => err(line, format!("`{f}` is not synthesizable")),
                _ => err(line, format!("unknown procedure `{f}`")),
            },
        }
    }

    // ------------------------------------------------------------------
    // Subprogram calls (inlined)

    pub fn call_user(
        &mut self,
        subs: &[Rc<SubDef>],
        args: &[Assoc],
        want: Option<&Ty>,
        is_func: bool,
        line: Line,
    ) -> R<Option<Val>> {
        let _ = want;
        let nparams = |s: &SubDef| s.sp.params.iter().map(|p| p.names.len()).sum::<usize>();
        let nreq = |s: &SubDef| {
            s.sp.params
                .iter()
                .filter(|p| p.default.is_none())
                .map(|p| p.names.len())
                .sum::<usize>()
        };
        let cands: Vec<Rc<SubDef>> = subs
            .iter()
            .filter(|s| {
                s.sp.is_func == is_func && args.len() <= nparams(s) && args.len() >= nreq(s)
            })
            .cloned()
            .collect();
        let name = subs.first().map(|s| s.sp.name.clone()).unwrap_or_default();
        if cands.is_empty() {
            return err(
                line,
                format!(
                    "no {} `{name}` taking {} arguments",
                    if is_func { "function" } else { "procedure" },
                    args.len()
                ),
            );
        }
        let mut sub = cands[0].clone();
        if cands.len() > 1 && self.px.last().is_some() {
            // Overload resolution by argument types (positional arguments).
            let mut arg_tys = Vec::new();
            for a in args {
                let t = match &a.actual {
                    e if matches!(
                        e,
                        Expr::Char(_) | Expr::Str(_) | Expr::BitStr(_) | Expr::Agg(_)
                    ) =>
                    {
                        None
                    }
                    e => self.eval(e, None, line).ok().map(|v| v.ty()),
                };
                arg_tys.push(t);
            }
            'c: for c in &cands {
                let ptys = self.callee_param_types(c)?;
                for (k, t) in arg_tys.iter().enumerate() {
                    if let (Some(t), Some(pt)) = (t, ptys.get(k)) {
                        let ok = t.same_base(pt)
                            || (t.is_vec()
                                && pt.is_vec()
                                && matches!((t.arr(), pt.arr()), (Some(a), Some(b)) if a.kind == b.kind));
                        if !ok {
                            continue 'c;
                        }
                    }
                }
                sub = c.clone();
                break;
            }
        }
        let body = sub.sp.body.clone().ok_or_else(|| {
            format!("line {line}: subprogram `{name}` has no body (missing package body?)")
        })?;
        if self.call_depth > 64 {
            return err(
                line,
                format!("call depth of `{name}` exceeds 64 (unbounded recursion?)"),
            );
        }
        let ptys = self.callee_param_types(&sub)?;
        let params: Vec<(String, IfDecl)> = sub
            .sp
            .params
            .iter()
            .flat_map(|p| p.names.iter().map(move |n| (n.0.clone(), p.clone())))
            .collect();
        let mut acts: Vec<Option<Expr>> = vec![None; params.len()];
        for (k, a) in args.iter().enumerate() {
            let idx = match &a.formal {
                Some(Name::Id(f, _)) => params
                    .iter()
                    .position(|(n, _)| n == f)
                    .ok_or_else(|| format!("line {line}: `{name}` has no parameter `{f}`"))?,
                Some(_) => return err(line, "unsupported formal in call"),
                None => k,
            };
            acts[idx] = Some(a.actual.clone());
        }
        enum PA {
            In(Val),
            Sig(SigRef),
            Var(LVal, Option<N>),
        }
        let mut pas = Vec::new();
        for (k, (pn, p)) in params.iter().enumerate() {
            let pty = &ptys[k];
            let is_sig = p.class.as_deref() == Some("signal");
            match p.mode {
                Mode::In => {
                    let e = match &acts[k] {
                        Some(e) => e.clone(),
                        None => p.default.clone().ok_or_else(|| {
                            format!("line {line}: missing argument `{pn}` of `{name}`")
                        })?,
                    };
                    let v = self.eval(&e, Some(pty), line)?;
                    let t2 = self.constrain_from(pty, &v, line)?;
                    let v = self
                        .convert(v, &t2, line)
                        .map_err(|er| format!("{er} (argument `{pn}` of `{name}`)"))?;
                    pas.push(PA::In(v));
                }
                _ => {
                    let e = acts[k].clone().ok_or_else(|| {
                        format!("line {line}: missing argument `{pn}` of `{name}`")
                    })?;
                    let lv = self.lval(&e, line)?;
                    if is_sig {
                        let (LBase::Net(net), Off::Static(off)) = (&lv.base, &lv.off) else {
                            return err(
                                line,
                                format!("signal parameter `{pn}` needs a statically named signal"),
                            );
                        };
                        pas.push(PA::Sig(SigRef {
                            net: *net,
                            off: *off,
                            ty: lv.ty.clone(),
                        }));
                    } else {
                        let cur = if p.mode == Mode::InOut {
                            let v = self.eval(&e, None, line)?;
                            Some(bits_n(&v, line)?)
                        } else {
                            None
                        };
                        pas.push(PA::Var(lv, cur));
                    }
                }
            }
        }
        // Enter the callee.
        let caller_dead = self.dead();
        let depth = sub.depth.min(self.scopes.len());
        let tail = self.scopes.split_off(depth);
        self.scopes.push(HashMap::new());
        let rty = match &sub.sp.ret {
            Some(m) => match self.resolve_mark(m, line) {
                Ok(t) => Some(t),
                Err(e) => {
                    self.scopes.pop();
                    self.scopes.extend(tail);
                    return Err(e);
                }
            },
            None => None,
        };
        let px = self.px.last_mut().unwrap();
        let nvars0 = px.st.vars.len();
        let saved = (
            px.st.ret,
            px.st.retv,
            px.st.brk,
            px.st.cont,
            std::mem::take(&mut px.loops),
        );
        let z = self.dag.bit(false);
        let px = self.px.last_mut().unwrap();
        px.st.ret = if is_func { z } else { caller_dead };
        px.st.retv = None;
        px.st.brk = z;
        px.st.cont = z;
        self.call_depth += 1;
        self.ret_tys.push(rty.clone().unwrap_or(Ty::Bool));
        self.ret_vtys.push(None);
        let mut copy_out: Vec<(LVal, u32)> = Vec::new();
        let res = (|| -> R<Option<Val>> {
            for (k, pa) in pas.into_iter().enumerate() {
                let pn = &params[k].0;
                match pa {
                    PA::In(v) => self.declare(pn, Sym::Val(v)),
                    PA::Sig(s) => self.declare(pn, Sym::Sig(s)),
                    PA::Var(lv, cur) => {
                        let t = lv.ty.clone();
                        let w = t.width()?;
                        let init = match cur {
                            Some(n) => n,
                            None => {
                                let b = t.default_bits()?;
                                self.dag.konst(b)
                            }
                        };
                        let slot = self.new_var(pn, &t, w, init, line)?;
                        self.declare(pn, Sym::Var(slot, t));
                        copy_out.push((lv, slot));
                    }
                }
            }
            self.elab_decls(&sub.sp.decls)?;
            self.exec_seqs(&body)?;
            if is_func {
                let retv = self.px.last().unwrap().st.retv.ok_or_else(|| {
                    format!("line {line}: function `{name}` ends without returning a value")
                })?;
                let vt = self
                    .ret_vtys
                    .last()
                    .cloned()
                    .flatten()
                    .or(rty.clone())
                    .unwrap();
                return Ok(Some(Val::B { ty: vt, n: retv }));
            }
            Ok(None)
        })();
        let outs: Vec<(LVal, N)> = copy_out
            .into_iter()
            .map(|(lv, slot)| (lv, self.px.last().unwrap().st.vars[slot as usize]))
            .collect();
        self.call_depth -= 1;
        self.ret_tys.pop();
        self.ret_vtys.pop();
        let px = self.px.last_mut().unwrap();
        px.st.ret = saved.0;
        px.st.retv = saved.1;
        px.st.brk = saved.2;
        px.st.cont = saved.3;
        px.loops = saved.4;
        px.st.vars.truncate(nvars0);
        px.var_tys.truncate(nvars0);
        px.var_names.truncate(nvars0);
        px.var_nets.truncate(nvars0);
        self.scopes.pop();
        self.scopes.extend(tail);
        let r = res?;
        for (lv, n) in outs {
            self.write_lval(&lv, n, line)?;
        }
        Ok(r)
    }

    fn callee_param_types(&mut self, sub: &SubDef) -> R<Vec<Ty>> {
        let depth = sub.depth.min(self.scopes.len());
        let tail = self.scopes.split_off(depth);
        self.scopes.push(HashMap::new());
        let mut out = Vec::new();
        let mut res = Ok(());
        for p in &sub.sp.params {
            match self.resolve_subind(&p.ty) {
                Ok(t) => {
                    for _ in &p.names {
                        out.push(t.clone());
                    }
                }
                Err(e) => {
                    res = Err(e);
                    break;
                }
            }
        }
        self.scopes.pop();
        self.scopes.extend(tail);
        res.map(|_| out)
    }

    pub fn masked_eq_pub(&mut self, v: &Val, lit: &str, line: Line) -> R<N> {
        let Val::B { n, .. } = v else {
            return err(line, "bad operand");
        };
        let w = self.dag.w(*n) as usize;
        let chars: Vec<char> = lit.chars().collect();
        if chars.len() != w {
            return Ok(self.dag.bit(false));
        }
        let mut acc = self.dag.bit(true);
        for (k, c) in chars.iter().rev().enumerate() {
            let c = c.to_ascii_lowercase();
            match c {
                '-' => continue,
                '1' | 'h' | '0' | 'l' => {
                    let bit = self.dag.slice(*n, k as u32, 1);
                    let e = if matches!(c, '1' | 'h') {
                        bit
                    } else {
                        self.dag.not(bit)
                    };
                    acc = self.dag.and(acc, e);
                }
                _ => return Ok(self.dag.bit(false)),
            }
        }
        Ok(acc)
    }
}

fn meta_lit_s(e: &Expr) -> Option<String> {
    let s = match e {
        Expr::Char(c) => c.to_string(),
        Expr::Str(s) | Expr::BitStr(s) => s.clone(),
        _ => return None,
    };
    s.chars()
        .any(|c| matches!(c.to_ascii_lowercase(), 'u' | 'x' | 'z' | 'w' | '-'))
        .then_some(s)
}
