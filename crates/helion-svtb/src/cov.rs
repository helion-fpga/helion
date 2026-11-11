//! Static statement / branch line inventory (for line coverage: lines that
//! never ran still need to be listed).

use std::collections::BTreeMap;

use crate::ast::*;

/// Every executable statement line, with `Some(kind)` for branch points
/// (`if` / `assert`), keyed by `(file id, line)`.
pub fn statement_lines(unit: &SourceUnit) -> BTreeMap<(u32, u32), Option<&'static str>> {
    let mut out = BTreeMap::new();
    let mut sub = |s: &SubDecl, out: &mut BTreeMap<(u32, u32), Option<&'static str>>| body(&s.body, out);
    for m in &unit.modules {
        for it in &m.items {
            match it {
                ModItem::Initial(b, _) | ModItem::Final(b, _) | ModItem::Always(_, b, _) => body(b, &mut out),
                ModItem::Sub(s) => sub(s, &mut out),
                ModItem::Class(c) => class(c, &mut out),
                _ => {}
            }
        }
    }
    let mut pkg = |items: &[PkgItem], out: &mut BTreeMap<(u32, u32), Option<&'static str>>| {
        for it in items {
            match it {
                PkgItem::Sub(s) => body(&s.body, out),
                PkgItem::Class(c) => class(c, out),
                _ => {}
            }
        }
    };
    for p in &unit.packages {
        pkg(&p.items, &mut out);
    }
    pkg(&unit.unit, &mut out);
    out
}

fn class(c: &ClassDecl, out: &mut BTreeMap<(u32, u32), Option<&'static str>>) {
    for it in &c.items {
        match it {
            ClassItem::Method(m) => body(&m.body, out),
            ClassItem::Class(n) => class(n, out),
            _ => {}
        }
    }
}

fn body(b: &Body, out: &mut BTreeMap<(u32, u32), Option<&'static str>>) {
    for s in b.iter() {
        stmt(s, out);
    }
}

fn stmt(s: &S, out: &mut BTreeMap<(u32, u32), Option<&'static str>>) {
    let key = (s.loc.file, s.loc.line);
    let kind = match &s.k {
        SK::If { .. } => Some("if"),
        SK::Assert { .. } => Some("assert"),
        _ => None,
    };
    let e = out.entry(key).or_insert(None);
    if kind.is_some() {
        *e = kind;
    }
    match &s.k {
        SK::Block(b) | SK::Forever(b) | SK::While(_, b) | SK::DoWhile(b, _) | SK::Repeat(_, b) | SK::Delay(_, b) | SK::Event(_, b) | SK::Wait(_, b) | SK::Foreach { body: b, .. } => body(b, out),
        SK::For { init, step, body: b, .. } => {
            body(init, out);
            body(step, out);
            body(b, out);
        }
        SK::If { then, els, .. } => {
            body(then, out);
            if let Some(e) = els {
                body(e, out);
            }
        }
        SK::Case { items, default, .. } => {
            for (_, b) in items {
                body(b, out);
            }
            if let Some(d) = default {
                body(d, out);
            }
        }
        SK::Fork { decls, branches, .. } => {
            body(decls, out);
            for b in branches {
                body(b, out);
            }
        }
        SK::Assert { pass, fail, .. } => {
            if let Some(p) = pass {
                body(p, out);
            }
            if let Some(f) = fail {
                body(f, out);
            }
        }
        SK::RandCase(arms) => {
            for (_, b) in arms {
                body(b, out);
            }
        }
        _ => {}
    }
}
