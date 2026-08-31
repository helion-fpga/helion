//! SV subset: **sv-parser** AST → RTL elab → AIG → FlowMap LUT6+FF.
//!
//! Handles 1-bit and vector `logic`, `assign`, `always_ff`, `+` incrementers,
//! bit-selects, and Boolean operators. Garbage is rejected by sv-parser.

use helion_ir::{CellKind, Design, PortDir};
use std::collections::HashMap;
use std::path::Path;
use sv_parser::parse_sv_str;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Const(bool),
    Var(String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Xor(Box<Expr>, Box<Expr>),
}

/// AIG: node 0 is const0. Edges are (node, inverted).
#[derive(Clone, Debug)]
pub struct Aig {
    pub pis: Vec<String>,
    pub ands: Vec<(Lit, Lit)>,
    pub output: Lit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lit {
    pub node: u32,
    pub inv: bool,
}

impl Lit {
    pub fn c0() -> Self {
        Self { node: 0, inv: false }
    }
    pub fn c1() -> Self {
        Self { node: 0, inv: true }
    }
    pub fn not(self) -> Self {
        Self {
            node: self.node,
            inv: !self.inv,
        }
    }
}

impl Aig {
    pub fn from_expr(e: &Expr) -> Self {
        let mut a = Self {
            pis: Vec::new(),
            ands: Vec::new(),
            output: Lit::c0(),
        };
        // Allocate every PI first so AND node ids (1+n_pis+i) cannot collide
        // with later PIs — that collision folded `q2 ^ (q0 & q1)` to const 0.
        a.collect_pis(e);
        a.output = a.expr(e);
        a
    }

    fn collect_pis(&mut self, e: &Expr) {
        match e {
            Expr::Const(_) => {}
            Expr::Var(s) => {
                let _ = self.pi(s);
            }
            Expr::Not(x) => self.collect_pis(x),
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Xor(a, b) => {
                self.collect_pis(a);
                self.collect_pis(b);
            }
        }
    }

    fn pi(&mut self, name: &str) -> Lit {
        if let Some(i) = self.pis.iter().position(|p| p == name) {
            return Lit {
                node: 1 + i as u32,
                inv: false,
            };
        }
        self.pis.push(name.into());
        Lit {
            node: self.pis.len() as u32,
            inv: false,
        }
    }

    fn and_lit(&mut self, a: Lit, b: Lit) -> Lit {
        if a.node == 0 && !a.inv {
            return Lit::c0();
        }
        if b.node == 0 && !b.inv {
            return Lit::c0();
        }
        if a.node == 0 && a.inv {
            return b;
        }
        if b.node == 0 && b.inv {
            return a;
        }
        self.ands.push((a, b));
        Lit {
            node: 1 + self.pis.len() as u32 + (self.ands.len() as u32 - 1),
            inv: false,
        }
    }

    fn expr(&mut self, e: &Expr) -> Lit {
        match e {
            Expr::Const(false) => Lit::c0(),
            Expr::Const(true) => Lit::c1(),
            Expr::Var(s) => self.pi(s),
            Expr::Not(x) => self.expr(x).not(),
            Expr::And(a, b) => {
                let la = self.expr(a);
                let lb = self.expr(b);
                self.and_lit(la, lb)
            }
            Expr::Or(a, b) => {
                let la = self.expr(a);
                let lb = self.expr(b);
                self.and_lit(la.not(), lb.not()).not()
            }
            Expr::Xor(a, b) => {
                let la = self.expr(a);
                let lb = self.expr(b);
                let t1 = self.and_lit(la, lb.not());
                let t2 = self.and_lit(la.not(), lb);
                self.and_lit(t1.not(), t2.not()).not()
            }
        }
    }

    fn eval_lit(&self, lit: Lit, pi_bits: u64) -> bool {
        let v = if lit.node == 0 {
            false
        } else if (lit.node as usize) <= self.pis.len() {
            let i = (lit.node - 1) as u32;
            (pi_bits >> i) & 1 == 1
        } else {
            let ai = (lit.node as usize) - 1 - self.pis.len();
            let (a, b) = self.ands[ai];
            self.eval_lit(a, pi_bits) && self.eval_lit(b, pi_bits)
        };
        v ^ lit.inv
    }

    /// FlowMap for a single cone with ≤6 PIs: truth table in LUT6 INIT (I0 = LSB = pis[0]).
    pub fn flowmap_lut6(&self) -> u64 {
        let k = self.pis.len().min(6);
        let width = 1u32 << k.max(1);
        let mut pat = 0u64;
        for addr in 0..width {
            if self.eval_lit(self.output, addr as u64) {
                pat |= 1u64 << addr;
            }
        }
        if k == 0 {
            return if self.eval_lit(self.output, 0) {
                u64::MAX
            } else {
                0
            };
        }
        let mut acc = 0u64;
        let w = width as u64;
        let mut sh = 0;
        while sh < 64 {
            acc |= (pat & ((1u64 << w) - 1)) << sh;
            sh += w;
        }
        acc
    }
}

pub fn parse_sv(source: &str, origin: &str) -> Result<sv_parser::SyntaxTree, String> {
    let defines = HashMap::new();
    parse_sv_str(source, origin, &defines, &[""], false, false)
        .map(|(tree, _)| tree)
        .map_err(|e| format!("sv-parser: {e}"))
}

/// Recursive-descent Boolean subset used after AST extraction.
pub fn parse_expr(s: &str) -> Result<Expr, String> {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    fn parse_or(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        let mut e = parse_xor(chars, i)?;
        while *i < chars.len() && (chars[*i] == '|' && peek_not_pipe(chars, *i)) {
            *i += 1;
            let r = parse_xor(chars, i)?;
            e = Expr::Or(Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn peek_not_pipe(chars: &[char], i: usize) -> bool {
        chars.get(i + 1) != Some(&'|')
    }
    fn parse_xor(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        let mut e = parse_and(chars, i)?;
        while *i < chars.len() && chars[*i] == '^' {
            *i += 1;
            let r = parse_and(chars, i)?;
            e = Expr::Xor(Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn parse_and(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        let mut e = parse_un(chars, i)?;
        while *i < chars.len() && chars[*i] == '&' && chars.get(*i + 1) != Some(&'&') {
            *i += 1;
            let r = parse_un(chars, i)?;
            e = Expr::And(Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn parse_un(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        if *i < chars.len() && chars[*i] == '~' {
            *i += 1;
            return Ok(Expr::Not(Box::new(parse_un(chars, i)?)));
        }
        if *i < chars.len() && chars[*i] == '!' {
            *i += 1;
            return Ok(Expr::Not(Box::new(parse_un(chars, i)?)));
        }
        parse_atom(chars, i)
    }
    fn parse_atom(chars: &[char], i: &mut usize) -> Result<Expr, String> {
        if *i < chars.len() && chars[*i] == '(' {
            *i += 1;
            let e = parse_or(chars, i)?;
            if *i >= chars.len() || chars[*i] != ')' {
                return Err("missing )".into());
            }
            *i += 1;
            return Ok(e);
        }
        if *i + 3 < chars.len() && chars[*i] == '1' && chars[*i + 1] == '\'' {
            let b = chars[*i + 3];
            *i += 4;
            return Ok(Expr::Const(b == '1'));
        }
        if *i < chars.len()
            && (chars[*i] == '0' || chars[*i] == '1')
            && !chars.get(*i + 1).map(|c| c.is_alphanumeric()).unwrap_or(false)
        {
            let c = chars[*i] == '1';
            *i += 1;
            return Ok(Expr::Const(c));
        }
        if *i < chars.len() && (chars[*i].is_ascii_alphabetic() || chars[*i] == '_') {
            let mut n = String::new();
            while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '_') {
                n.push(chars[*i]);
                *i += 1;
            }
            return Ok(Expr::Var(n));
        }
        Err(format!("expr at {i}: {:?}", chars.get(*i..)))
    }
    let e = parse_or(&chars, &mut i)?;
    Ok(e)
}

#[derive(Clone, Debug)]
struct Signal {
    name: String,
    width: usize,
    depth: usize,
}

#[derive(Clone, Debug)]
enum RExpr {
    Const { val: u128, width: usize }, // width kept for based-number round-trip
    Ident(String),
    Bit(String, usize),
    Not(Box<RExpr>),
    And(Box<RExpr>, Box<RExpr>),
    Or(Box<RExpr>, Box<RExpr>),
    Xor(Box<RExpr>, Box<RExpr>),
    Add(Box<RExpr>, Box<RExpr>),
    Mul(Box<RExpr>, Box<RExpr>),
    Mux(Box<RExpr>, Box<RExpr>, Box<RExpr>),
    Eq(Box<RExpr>, Box<RExpr>),
    Ne(Box<RExpr>, Box<RExpr>),
    Lt(Box<RExpr>, Box<RExpr>),
}

#[derive(Clone, Debug)]
struct Inst {
    module: String,
    name: String,
    /// child port → parent net (ident)
    conns: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
struct Rtl {
    module: String,
    ports: Vec<(String, PortDir, usize)>,
    signals: Vec<Signal>,
    nbas: Vec<(String, Option<usize>, RExpr)>, // lhs name, optional bit, rhs
    assigns: Vec<(String, Option<usize>, RExpr)>,
    insts: Vec<Inst>,
}

fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Number(u128, usize),
    Kw(String),
    Sym(char),
    Le, // <=
    Eq, // ==
    Ne, // !=
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    let kws = [
        "module", "endmodule", "input", "output", "logic", "wire", "reg", "always_ff", "always",
        "begin", "end", "posedge", "negedge", "assign", "inout", "if", "else", "always_comb",
        "case", "endcase", "default",
    ];
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '<' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Le);
            i += 2;
            continue;
        }
        if c == '=' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Eq);
            i += 2;
            continue;
        }
        if c == '!' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Ne);
            i += 2;
            continue;
        }
        if "@();,:+-~^|&![]'=#?*<>.".contains(c) {
            out.push(Tok::Sym(c));
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let mut n = String::new();
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                n.push(chars[i]);
                i += 1;
            }
            if kws.contains(&n.as_str()) {
                out.push(Tok::Kw(n));
            } else {
                out.push(Tok::Ident(n));
            }
            continue;
        }
        if c.is_ascii_digit() {
            let mut n = String::new();
            while i < chars.len() && chars[i].is_ascii_digit() {
                n.push(chars[i]);
                i += 1;
            }
            if chars.get(i) == Some(&'\'') {
                i += 1;
                let width: usize = n.parse().unwrap_or(1);
                let mut base = 10;
                if i < chars.len() {
                    match chars[i].to_ascii_lowercase() {
                        'b' => {
                            base = 2;
                            i += 1;
                        }
                        'h' => {
                            base = 16;
                            i += 1;
                        }
                        'd' => {
                            base = 10;
                            i += 1;
                        }
                        'o' => {
                            base = 8;
                            i += 1;
                        }
                        _ => {}
                    }
                }
                let mut digits = String::new();
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    digits.push(chars[i]);
                    i += 1;
                }
                let val = u128::from_str_radix(&digits, base).unwrap_or(0);
                out.push(Tok::Number(val, width));
            } else {
                let val: u128 = n.parse().unwrap_or(0);
                out.push(Tok::Number(val, 32));
            }
            continue;
        }
        return Err(format!("bad char {c:?} at {i}"));
    }
    Ok(out)
}

struct P<'a> {
    t: &'a [Tok],
    i: usize,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.t.get(self.i)
    }
    fn bump(&mut self) -> Option<&'a Tok> {
        let t = self.t.get(self.i)?;
        self.i += 1;
        Some(t)
    }
    fn eat_kw(&mut self, k: &str) -> bool {
        match self.peek() {
            Some(Tok::Kw(s)) if s == k => {
                self.i += 1;
                true
            }
            _ => false,
        }
    }
    fn eat_sym(&mut self, c: char) -> bool {
        match self.peek() {
            Some(Tok::Sym(s)) if *s == c => {
                self.i += 1;
                true
            }
            _ => false,
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        match self.bump() {
            Some(Tok::Ident(s)) => Ok(s.clone()),
            other => Err(format!("expected ident, got {other:?}")),
        }
    }
    fn width_opt(&mut self) -> Result<usize, String> {
        if !self.eat_sym('[') {
            return Ok(1);
        }
        let msb = match self.bump() {
            Some(Tok::Number(v, _)) => *v as usize,
            _ => return Err("msb".into()),
        };
        if !self.eat_sym(':') {
            return Err("range :".into());
        }
        let lsb = match self.bump() {
            Some(Tok::Number(v, _)) => *v as usize,
            _ => return Err("lsb".into()),
        };
        if !self.eat_sym(']') {
            return Err("]".into());
        }
        Ok(msb.max(lsb) - msb.min(lsb) + 1)
    }
}

fn parse_rexpr(p: &mut P) -> Result<RExpr, String> {
    let e = parse_cmp(p)?;
    if p.eat_sym('?') {
        let t = parse_rexpr(p)?;
        if !p.eat_sym(':') {
            return Err("ternary :".into());
        }
        let f = parse_rexpr(p)?;
        return Ok(RExpr::Mux(Box::new(e), Box::new(t), Box::new(f)));
    }
    Ok(e)
}

fn parse_cmp(p: &mut P) -> Result<RExpr, String> {
    let e = parse_add(p)?;
    if matches!(p.peek(), Some(Tok::Eq)) {
        p.bump();
        return Ok(RExpr::Eq(Box::new(e), Box::new(parse_add(p)?)));
    }
    if matches!(p.peek(), Some(Tok::Ne)) {
        p.bump();
        return Ok(RExpr::Ne(Box::new(e), Box::new(parse_add(p)?)));
    }
    if p.eat_sym('<') {
        return Ok(RExpr::Lt(Box::new(e), Box::new(parse_add(p)?)));
    }
    if p.eat_sym('>') {
        // a > b  ≡  b < a
        return Ok(RExpr::Lt(Box::new(parse_add(p)?), Box::new(e)));
    }
    Ok(e)
}

fn parse_add(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_mul(p)?;
    while p.eat_sym('+') {
        let r = parse_mul(p)?;
        e = RExpr::Add(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_mul(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_or_r(p)?;
    while p.eat_sym('*') {
        let r = parse_or_r(p)?;
        e = RExpr::Mul(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_or_r(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_xor_r(p)?;
    while p.eat_sym('|') {
        let r = parse_xor_r(p)?;
        e = RExpr::Or(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_xor_r(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_and_r(p)?;
    while p.eat_sym('^') {
        let r = parse_and_r(p)?;
        e = RExpr::Xor(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_and_r(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_un_r(p)?;
    while p.eat_sym('&') {
        let r = parse_un_r(p)?;
        e = RExpr::And(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_un_r(p: &mut P) -> Result<RExpr, String> {
    if p.eat_sym('~') || p.eat_sym('!') {
        return Ok(RExpr::Not(Box::new(parse_un_r(p)?)));
    }
    parse_atom_r(p)
}

fn parse_atom_r(p: &mut P) -> Result<RExpr, String> {
    if p.eat_sym('(') {
        let e = parse_rexpr(p)?;
        if !p.eat_sym(')') {
            return Err(")".into());
        }
        return Ok(e);
    }
    match p.bump() {
        Some(Tok::Number(v, w)) => Ok(RExpr::Const {
            val: *v,
            width: *w,
        }),
        Some(Tok::Ident(s)) => {
            let name = s.clone();
            if p.eat_sym('[') {
                let idx = match p.bump() {
                    Some(Tok::Number(v, _)) => *v as usize,
                    _ => return Err("bit index".into()),
                };
                if !p.eat_sym(']') {
                    return Err("]".into());
                }
                Ok(RExpr::Bit(name, idx))
            } else {
                Ok(RExpr::Ident(name))
            }
        }
        other => Err(format!("atom {other:?}")),
    }
}

fn parse_port_dir(p: &mut P) -> Option<PortDir> {
    if p.eat_kw("input") {
        Some(PortDir::In)
    } else if p.eat_kw("output") {
        Some(PortDir::Out)
    } else if p.eat_kw("inout") {
        Some(PortDir::In)
    } else {
        None
    }
}

fn skip_logic(p: &mut P) {
    let _ = p.eat_kw("logic");
    let _ = p.eat_kw("wire");
    let _ = p.eat_kw("reg");
}

fn parse_lhs(p: &mut P) -> Result<(String, Option<usize>), String> {
    let name = p.ident()?;
    if p.eat_sym('[') {
        match p.bump() {
            Some(Tok::Number(v, _)) => {
                let idx = *v as usize;
                if !p.eat_sym(']') {
                    return Err("]".into());
                }
                Ok((name, Some(idx)))
            }
            Some(Tok::Ident(_)) => {
                if !p.eat_sym(']') {
                    return Err("]".into());
                }
                Ok((name, None))
            }
            _ => Err("lhs index".into()),
        }
    } else {
        Ok((name, None))
    }
}

type Nba = (String, Option<usize>, RExpr);

fn parse_nba(p: &mut P) -> Result<Nba, String> {
    let (lhs, bit) = parse_lhs(p)?;
    if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
    } else if !p.eat_sym('=') {
        return Err("nba".into());
    }
    let rhs = parse_rexpr(p)?;
    let _ = p.eat_sym(';');
    Ok((lhs, bit, rhs))
}

fn parse_seq_block(p: &mut P, block: bool) -> Result<Vec<Nba>, String> {
    let mut v = Vec::new();
    loop {
        if block && p.eat_kw("end") {
            break;
        }
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "end") {
            p.eat_kw("end");
            break;
        }
        v.extend(parse_seq_item(p)?);
        if !block {
            break;
        }
    }
    Ok(v)
}

fn parse_seq_item(p: &mut P) -> Result<Vec<Nba>, String> {
    if p.eat_kw("if") {
        if !p.eat_sym('(') {
            return Err("if (".into());
        }
        let cond = parse_rexpr(p)?;
        if !p.eat_sym(')') {
            return Err("if )".into());
        }
        let then_b = p.eat_kw("begin");
        let then_s = parse_seq_block(p, then_b)?;
        let else_s = if p.eat_kw("else") {
            let eb = p.eat_kw("begin");
            parse_seq_block(p, eb)?
        } else {
            Vec::new()
        };
        let mut out = Vec::new();
        for (lhs, bit, rhs) in then_s {
            let other = else_s
                .iter()
                .find(|(l, b, _)| l == &lhs && b == &bit)
                .map(|(_, _, r)| r.clone())
                .unwrap_or_else(|| RExpr::Ident(lhs.clone()));
            out.push((lhs, bit, RExpr::Mux(Box::new(cond.clone()), Box::new(rhs), Box::new(other))));
        }
        for (lhs, bit, rhs) in else_s {
            if out.iter().any(|(l, b, _)| l == &lhs && b == &bit) {
                continue;
            }
            out.push((
                lhs.clone(),
                bit,
                RExpr::Mux(
                    Box::new(cond.clone()),
                    Box::new(RExpr::Ident(lhs.clone())),
                    Box::new(rhs),
                ),
            ));
        }
        return Ok(out);
    }
    if p.eat_kw("case") {
        if !p.eat_sym('(') {
            return Err("case (".into());
        }
        let sel = parse_rexpr(p)?;
        if !p.eat_sym(')') {
            return Err("case )".into());
        }
        let mut arms: Vec<(Option<RExpr>, Vec<Nba>)> = Vec::new();
        let mut def: Vec<Nba> = Vec::new();
        while !p.eat_kw("endcase") {
            if p.peek().is_none() {
                return Err("unterminated case".into());
            }
            if p.eat_kw("default") {
                let _ = p.eat_sym(':');
                let b = p.eat_kw("begin");
                def = parse_seq_block(p, b)?;
                continue;
            }
            let item = parse_rexpr(p)?;
            if !p.eat_sym(':') {
                return Err("case :".into());
            }
            let b = p.eat_kw("begin");
            let body = parse_seq_block(p, b)?;
            arms.push((Some(item), body));
        }
        let mut out = Vec::new();
        for (item, body) in arms.into_iter().rev() {
            let item = item.unwrap();
            let cond = RExpr::Eq(Box::new(sel.clone()), Box::new(item));
            for (lhs, bit, rhs) in body {
                let other = out
                    .iter()
                    .chain(def.iter())
                    .find(|(l, b, _)| l == &lhs && b == &bit)
                    .map(|(_, _, r)| r.clone())
                    .unwrap_or_else(|| RExpr::Ident(lhs.clone()));
                if let Some(existing) = out.iter_mut().find(|(l, b, _)| l == &lhs && b == &bit) {
                    existing.2 = RExpr::Mux(Box::new(cond.clone()), Box::new(rhs), Box::new(existing.2.clone()));
                } else {
                    out.push((lhs, bit, RExpr::Mux(Box::new(cond.clone()), Box::new(rhs), Box::new(other))));
                }
            }
        }
        for (lhs, bit, rhs) in def {
            if !out.iter().any(|(l, b, _)| l == &lhs && b == &bit) {
                out.push((lhs, bit, rhs));
            }
        }
        return Ok(out);
    }
    Ok(vec![parse_nba(p)?])
}

fn parse_source(source: &str) -> Result<Vec<Rtl>, String> {
    let s = strip_comments(source);
    let toks = tokenize(&s)?;
    let mut p = P { t: &toks, i: 0 };
    let mut mods = Vec::new();
    while p.peek().is_some() {
        if p.eat_sym(';') {
            continue;
        }
        mods.push(parse_one_module(&mut p)?);
    }
    if mods.is_empty() {
        return Err("no module".into());
    }
    Ok(mods)
}

fn parse_rtl(source: &str) -> Result<Rtl, String> {
    parse_source(source)?
        .into_iter()
        .next()
        .ok_or_else(|| "no module".into())
}

fn parse_one_module(mut p: &mut P) -> Result<Rtl, String> {
    if !p.eat_kw("module") {
        return Err("expected module".into());
    }
    let module = p.ident()?;
    let mut ports = Vec::new();
    let mut signals = Vec::new();
    if p.eat_sym('(') {
        loop {
            if p.eat_sym(')') {
                break;
            }
            let dir = parse_port_dir(&mut p).ok_or("port dir")?;
            skip_logic(&mut p);
            let w = p.width_opt()?;
            let n = p.ident()?;
            ports.push((n.clone(), dir, w));
            signals.push(Signal { name: n, width: w, depth: 0 });
            let _ = p.eat_sym(',');
        }
    }
    let _ = p.eat_sym(';');
    let mut nbas = Vec::new();
    let mut assigns = Vec::new();
    let mut insts = Vec::new();
    while !p.eat_kw("endmodule") {
        if p.peek().is_none() {
            return Err("unterminated module".into());
        }
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "input" || k == "output") {
            let dir = parse_port_dir(&mut p).unwrap();
            skip_logic(&mut p);
            let w = p.width_opt()?;
            let n = p.ident()?;
            ports.push((n.clone(), dir, w));
            if !signals.iter().any(|s| s.name == n) {
                signals.push(Signal { name: n, width: w, depth: 0 });
            }
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("logic") || p.eat_kw("wire") || p.eat_kw("reg") {
            let w = p.width_opt()?;
            let n = p.ident()?;
            let mut depth = 0usize;
            if p.eat_sym('[') {
                let hi = match p.bump() {
                    Some(Tok::Number(v, _)) => *v as usize,
                    _ => return Err("mem msb".into()),
                };
                if !p.eat_sym(':') {
                    return Err("mem :".into());
                }
                let lo = match p.bump() {
                    Some(Tok::Number(v, _)) => *v as usize,
                    _ => return Err("mem lsb".into()),
                };
                if !p.eat_sym(']') {
                    return Err("mem ]".into());
                }
                depth = hi.max(lo) - hi.min(lo) + 1;
            }
            if !signals.iter().any(|s| s.name == n) {
                signals.push(Signal { name: n, width: w, depth });
            }
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("assign") {
            let (lhs, bit) = parse_lhs(&mut p)?;
            if !p.eat_sym('=') {
                return Err("assign =".into());
            }
            let rhs = parse_rexpr(&mut p)?;
            let _ = p.eat_sym(';');
            assigns.push((lhs, bit, rhs));
            continue;
        }
        if p.eat_kw("always_comb") {
            let block = p.eat_kw("begin");
            let stmts = parse_seq_block(&mut p, block)?;
            assigns.extend(stmts);
            continue;
        }
        if p.eat_kw("always_ff") || p.eat_kw("always") {
            let _ = p.eat_sym('@');
            let _ = p.eat_sym('(');
            let _ = p.eat_kw("posedge");
            let _ = p.eat_kw("negedge");
            let _ = p.ident();
            let _ = p.eat_sym(')');
            let block = p.eat_kw("begin");
            nbas.extend(parse_seq_block(&mut p, block)?);
            continue;
        }
        if matches!(p.peek(), Some(Tok::Ident(_))) {
            if let Ok(inst) = parse_inst(&mut p) {
                insts.push(inst);
                continue;
            }
        }
        // skip unknown token to avoid infinite loop
        p.bump();
    }
    Ok(Rtl {
        module,
        ports,
        signals,
        nbas,
        assigns,
        insts,
    })
}

fn parse_inst(p: &mut P) -> Result<Inst, String> {
    let start = p.i;
    let module = match p.bump() {
        Some(Tok::Ident(s)) => s.clone(),
        _ => {
            p.i = start;
            return Err("inst module".into());
        }
    };
    if p.eat_sym('#') {
        let mut depth = 0i32;
        if p.eat_sym('(') {
            depth = 1;
        }
        while depth > 0 {
            match p.bump() {
                Some(Tok::Sym('(')) => depth += 1,
                Some(Tok::Sym(')')) => depth -= 1,
                None => break,
                _ => {}
            }
        }
    }
    let name = match p.bump() {
        Some(Tok::Ident(s)) => s.clone(),
        _ => {
            p.i = start;
            return Err("inst name".into());
        }
    };
    if !p.eat_sym('(') {
        p.i = start;
        return Err("inst (".into());
    }
    let mut conns = Vec::new();
    let mut pos = 0usize;
    loop {
        if p.eat_sym(')') {
            break;
        }
        if p.peek().is_none() {
            p.i = start;
            return Err("inst )".into());
        }
        if p.eat_sym('.') {
            let port = p.ident().map_err(|_| {
                p.i = start;
                "inst port".to_string()
            })?;
            if !p.eat_sym('(') {
                p.i = start;
                return Err("inst .port(".into());
            }
            let net = p.ident().map_err(|_| {
                p.i = start;
                "inst net".to_string()
            })?;
            if !p.eat_sym(')') {
                p.i = start;
                return Err("inst .port)".into());
            }
            conns.push((port, net));
        } else {
            let net = match p.bump() {
                Some(Tok::Ident(s)) => s.clone(),
                _ => {
                    p.i = start;
                    return Err("inst pos".into());
                }
            };
            conns.push((format!("#{pos}"), net));
            pos += 1;
        }
        let _ = p.eat_sym(',');
    }
    let _ = p.eat_sym(';');
    Ok(Inst { module, name, conns })
}

fn sig_width(rtl: &Rtl, name: &str) -> usize {
    rtl.signals
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.width)
        .or_else(|| rtl.ports.iter().find(|(n, _, _)| n == name).map(|(_, _, w)| *w))
        .unwrap_or(1)
}

fn bit_name(sig: &str, width: usize, bit: usize) -> String {
    if width == 1 {
        sig.to_string()
    } else {
        format!("{sig}_{bit}")
    }
}

fn inc_bit_expr(sig: &str, width: usize, i: usize) -> Expr {
    let qi = Expr::Var(bit_name(sig, width, i));
    if i == 0 {
        return Expr::Not(Box::new(qi));
    }
    let mut and = Expr::Var(bit_name(sig, width, 0));
    for j in 1..i {
        and = Expr::And(
            Box::new(and),
            Box::new(Expr::Var(bit_name(sig, width, j))),
        );
    }
    Expr::Xor(Box::new(and), Box::new(qi))
}

fn rexpr_is_plus_one(e: &RExpr) -> bool {
    match e {
        RExpr::Add(_, r) => matches!(r.as_ref(), RExpr::Const { val, .. } if *val == 1),
        _ => false,
    }
}

fn rexpr_ident(e: &RExpr) -> Option<String> {
    match e {
        RExpr::Ident(s) => Some(s.clone()),
        RExpr::Add(l, _) => rexpr_ident(l),
        _ => None,
    }
}

fn rexpr_to_bit(e: &RExpr, rtl: &Rtl, bit: usize) -> Result<Expr, String> {
    match e {
        RExpr::Const { val, width } => {
            let _ = width;
            Ok(Expr::Const((val >> bit) & 1 == 1))
        }
        RExpr::Ident(s) => {
            let w = sig_width(rtl, s);
            Ok(Expr::Var(bit_name(s, w, bit.min(w.saturating_sub(1)))))
        }
        RExpr::Bit(s, i) => {
            let w = sig_width(rtl, s);
            Ok(Expr::Var(bit_name(s, w, *i)))
        }
        RExpr::Not(x) => Ok(Expr::Not(Box::new(rexpr_to_bit(x, rtl, bit)?))),
        RExpr::And(a, b) => Ok(Expr::And(
            Box::new(rexpr_to_bit(a, rtl, bit)?),
            Box::new(rexpr_to_bit(b, rtl, bit)?),
        )),
        RExpr::Or(a, b) => Ok(Expr::Or(
            Box::new(rexpr_to_bit(a, rtl, bit)?),
            Box::new(rexpr_to_bit(b, rtl, bit)?),
        )),
        RExpr::Xor(a, b) => Ok(Expr::Xor(
            Box::new(rexpr_to_bit(a, rtl, bit)?),
            Box::new(rexpr_to_bit(b, rtl, bit)?),
        )),
        RExpr::Add(_, _) => Err("add must be expanded as incrementer".into()),
        RExpr::Mul(_, _) => Err("mul is a DSP primitive, not a LUT cone".into()),
        RExpr::Mux(c, t, f) => {
            let cv = rexpr_to_bit(c, rtl, 0)?;
            let tv = rexpr_to_bit(t, rtl, bit)?;
            let fv = rexpr_to_bit(f, rtl, bit)?;
            Ok(Expr::Or(
                Box::new(Expr::And(Box::new(cv.clone()), Box::new(tv))),
                Box::new(Expr::And(Box::new(Expr::Not(Box::new(cv))), Box::new(fv))),
            ))
        }
        RExpr::Eq(a, b) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            Ok(cmp_eq_bits(a, b, rtl, true)?)
        }
        RExpr::Ne(a, b) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            Ok(Expr::Not(Box::new(cmp_eq_bits(a, b, rtl, true)?)))
        }
        RExpr::Lt(a, b) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            // unsigned: MSB-first: (a_msb < b_msb) | (eq_msb & lower)
            lt_bits(a, b, rtl)
        }
    }
}

fn cmp_eq_bits(a: &RExpr, b: &RExpr, rtl: &Rtl, _eq: bool) -> Result<Expr, String> {
    let wa = rexpr_width(a, rtl);
    let wb = rexpr_width(b, rtl);
    let w = wa.max(wb).max(1);
    let mut acc: Option<Expr> = None;
    for i in 0..w {
        let ai = rexpr_to_bit(a, rtl, i)?;
        let bi = rexpr_to_bit(b, rtl, i)?;
        let xnor = Expr::Not(Box::new(Expr::Xor(Box::new(ai), Box::new(bi))));
        acc = Some(match acc {
            None => xnor,
            Some(p) => Expr::And(Box::new(p), Box::new(xnor)),
        });
    }
    Ok(acc.unwrap_or(Expr::Const(true)))
}

fn lt_bits(a: &RExpr, b: &RExpr, rtl: &Rtl) -> Result<Expr, String> {
    let w = rexpr_width(a, rtl).max(rexpr_width(b, rtl)).max(1);
    let mut acc = Expr::Const(false);
    let mut eq_so_far = Expr::Const(true);
    for i in (0..w).rev() {
        let ai = rexpr_to_bit(a, rtl, i)?;
        let bi = rexpr_to_bit(b, rtl, i)?;
        let a0b1 = Expr::And(Box::new(Expr::Not(Box::new(ai.clone()))), Box::new(bi.clone()));
        acc = Expr::Or(
            Box::new(acc),
            Box::new(Expr::And(Box::new(eq_so_far.clone()), Box::new(a0b1))),
        );
        let xnor = Expr::Not(Box::new(Expr::Xor(Box::new(ai), Box::new(bi))));
        eq_so_far = Expr::And(Box::new(eq_so_far), Box::new(xnor));
    }
    Ok(acc)
}

fn rexpr_width(e: &RExpr, rtl: &Rtl) -> usize {
    match e {
        RExpr::Ident(s) | RExpr::Bit(s, _) => sig_width(rtl, s),
        RExpr::Const { width, .. } => (*width).max(1),
        RExpr::Not(x) => rexpr_width(x, rtl).min(1).max(1),
        RExpr::Eq(_, _) | RExpr::Ne(_, _) | RExpr::Lt(_, _) => 1,
        RExpr::And(a, b) | RExpr::Or(a, b) | RExpr::Xor(a, b) | RExpr::Add(a, b) | RExpr::Mul(a, b) => {
            rexpr_width(a, rtl).max(rexpr_width(b, rtl))
        }
        RExpr::Mux(_, t, f) => rexpr_width(t, rtl).max(rexpr_width(f, rtl)),
    }
}

fn sig_depth(rtl: &Rtl, name: &str) -> usize {
    rtl.signals
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.depth)
        .unwrap_or(0)
}

fn is_mac_rhs(e: &RExpr) -> bool {
    match e {
        RExpr::Mul(_, _) => true,
        RExpr::Add(l, _) => matches!(l.as_ref(), RExpr::Mul(_, _)),
        _ => false,
    }
}

fn drive_target(e: &RExpr, rtl: &Rtl) -> Result<(String, usize), String> {
    match e {
        RExpr::Ident(s) => {
            let w = sig_width(rtl, s);
            if w == 1 {
                Ok((bit_name(s, 1, 0), 0))
            } else {
                Err(format!("vector {s} as 1-bit assign"))
            }
        }
        RExpr::Bit(s, i) => {
            let w = sig_width(rtl, s);
            Ok((bit_name(s, w, *i), *i))
        }
        _ => Err("assign rhs not a name/bitsel".into()),
    }
}

fn synth_rtl(rtl: &Rtl) -> Result<Design, String> {
    let mut d = Design::new(&rtl.module);
    for (n, dir, _) in &rtl.ports {
        d.add_port(n, *dir);
    }
    let clk = rtl
        .ports
        .iter()
        .find(|(n, dir, _)| *dir == PortDir::In && n == "clk")
        .or_else(|| rtl.ports.iter().find(|(_, dir, _)| *dir == PortDir::In))
        .map(|(n, _, _)| n.as_str())
        .unwrap_or("clk");

    // Flatten NBAs into per-bit (name_bit, expr)
    let mut reg_bits: Vec<(String, Expr)> = Vec::new();
    let mut n_mac = 0usize;
    let mut n_bram = 0usize;
    for (lhs, bit, rhs) in &rtl.nbas {
        if sig_depth(rtl, lhs) > 0 {
            d.add_cell(format!("u_bram{n_bram}"), CellKind::Bram18);
            n_bram += 1;
            continue;
        }
        if is_mac_rhs(rhs) {
            d.add_cell(format!("u_mac{n_mac}"), CellKind::Mac27);
            n_mac += 1;
            continue;
        }
        let w = sig_width(rtl, lhs);
        if let Some(b) = bit {
            let e = if rexpr_is_plus_one(rhs) {
                inc_bit_expr(lhs, w, *b)
            } else {
                rexpr_to_bit(rhs, rtl, 0)?
            };
            reg_bits.push((bit_name(lhs, w, *b), e));
        } else if rexpr_is_plus_one(rhs) && rexpr_ident(rhs).as_deref() == Some(lhs.as_str()) {
            for i in 0..w {
                reg_bits.push((bit_name(lhs, w, i), inc_bit_expr(lhs, w, i)));
            }
        } else if let RExpr::Mux(cond, t, f) = rhs {
            if rexpr_is_plus_one(f)
                && rexpr_ident(f).as_deref() == Some(lhs.as_str())
                && matches!(t.as_ref(), RExpr::Const { val: 0, .. })
            {
                for i in 0..w {
                    let inc = inc_bit_expr(lhs, w, i);
                    let c = rexpr_to_bit(cond, rtl, 0)?;
                    reg_bits.push((
                        bit_name(lhs, w, i),
                        Expr::And(Box::new(Expr::Not(Box::new(c))), Box::new(inc)),
                    ));
                }
            } else {
                for i in 0..w {
                    reg_bits.push((bit_name(lhs, w, i), rexpr_to_bit(rhs, rtl, i)?));
                }
            }
        } else {
            for i in 0..w {
                reg_bits.push((bit_name(lhs, w, i), rexpr_to_bit(rhs, rtl, i)?));
            }
        }
    }
    if reg_bits.is_empty() && n_mac == 0 && n_bram == 0 {
        return Err("no registered bits".into());
    }

    let single_q = reg_bits.len() == 1 && reg_bits[0].0 == "q";
    for (i, (bitn, expr)) in reg_bits.iter().enumerate() {
        let aig = Aig::from_expr(expr);
        let init = aig.flowmap_lut6();
        let (lut, ff, dnet, qnet) = if single_q {
            ("u_lut".into(), "u_ff".into(), "d".into(), "q".into())
        } else {
            (
                format!("u_lut{i}"),
                format!("u_ff{i}"),
                format!("d{i}"),
                bitn.clone(),
            )
        };
        d.add_cell(&lut, CellKind::Lut6 { init });
        d.add_cell(&ff, CellKind::Hff);
        d.connect(clk, &ff, "CLK");
        d.connect(&dnet, &lut, "O");
        d.connect(&dnet, &ff, "D");
        d.connect(&qnet, &ff, "Q");
        for (pin, pi) in aig.pis.iter().enumerate() {
            if pin >= 6 {
                return Err(format!("cone {bitn} has >6 inputs"));
            }
            d.connect(pi, &lut, format!("I{pin}"));
        }
    }

    // Output IOBs from assigns
    let mut iob_n = 0usize;
    for (lhs, bit, rhs) in &rtl.assigns {
        let is_out = rtl
            .ports
            .iter()
            .any(|(n, dir, _)| n == lhs && *dir == PortDir::Out);
        if !is_out {
            continue;
        }
        let (qnet, _) = if let Some(b) = bit {
            let w = sig_width(rtl, lhs);
            (bit_name(lhs, w, *b), *b)
        } else {
            drive_target(rhs, rtl)?
        };
        let iob = if iob_n == 0 {
            "u_iob".to_string()
        } else {
            format!("u_iob{iob_n}")
        };
        iob_n += 1;
        d.add_cell(&iob, CellKind::IobOut);
        d.connect(&qnet, &iob, "I");
        d.connect(lhs, &iob, "PAD");
    }
    if iob_n == 0 {
        // default: last register bit to first output
        if let (Some((n, _, _)), Some((qnet, _))) = (
            rtl.ports.iter().find(|(_, dir, _)| *dir == PortDir::Out),
            reg_bits.last(),
        ) {
            d.add_cell("u_iob", CellKind::IobOut);
            d.connect(qnet, "u_iob", "I");
            d.connect(n, "u_iob", "PAD");
        }
    }
    Ok(d)
}

fn rewrite_rexpr(e: &RExpr, subst: &HashMap<String, String>) -> RExpr {
    let id = |s: &str| subst.get(s).cloned().unwrap_or_else(|| s.to_string());
    match e {
        RExpr::Const { val, width } => RExpr::Const {
            val: *val,
            width: *width,
        },
        RExpr::Ident(s) => RExpr::Ident(id(s)),
        RExpr::Bit(s, i) => RExpr::Bit(id(s), *i),
        RExpr::Not(x) => RExpr::Not(Box::new(rewrite_rexpr(x, subst))),
        RExpr::And(a, b) => RExpr::And(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Or(a, b) => RExpr::Or(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Xor(a, b) => RExpr::Xor(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Add(a, b) => RExpr::Add(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Mul(a, b) => RExpr::Mul(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Mux(c, t, f) => RExpr::Mux(
            Box::new(rewrite_rexpr(c, subst)),
            Box::new(rewrite_rexpr(t, subst)),
            Box::new(rewrite_rexpr(f, subst)),
        ),
        RExpr::Eq(a, b) => RExpr::Eq(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Ne(a, b) => RExpr::Ne(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Lt(a, b) => RExpr::Lt(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
    }
}

fn flatten_module(mods: &HashMap<String, Rtl>, name: &str) -> Result<Rtl, String> {
    let src = mods
        .get(name)
        .ok_or_else(|| format!("unknown module {name}"))?;
    let mut out = Rtl {
        module: src.module.clone(),
        ports: src.ports.clone(),
        signals: src.signals.clone(),
        nbas: src.nbas.clone(),
        assigns: src.assigns.clone(),
        insts: Vec::new(),
    };
    for inst in &src.insts {
        let child = flatten_module(mods, &inst.module)?;
        let prefix = format!("{}_", inst.name);
        let mut subst: HashMap<String, String> = HashMap::new();
        for s in &child.signals {
            subst.insert(s.name.clone(), format!("{prefix}{}", s.name));
        }
        for (i, (pname, _, _)) in child.ports.iter().enumerate() {
            if let Some((_, net)) = inst.conns.iter().find(|(p, _)| p == pname)
                .or_else(|| inst.conns.iter().find(|(p, _)| p == &format!("#{i}")))
            {
                subst.insert(pname.clone(), net.clone());
            }
        }
        for s in &child.signals {
            let mapped = subst.get(&s.name).cloned().unwrap();
            if !out.signals.iter().any(|x| x.name == mapped) {
                out.signals.push(Signal {
                    name: mapped,
                    width: s.width,
                    depth: s.depth,
                });
            }
        }
        for (lhs, bit, rhs) in &child.nbas {
            let lhs = subst.get(lhs).cloned().unwrap_or_else(|| format!("{prefix}{lhs}"));
            out.nbas.push((lhs, *bit, rewrite_rexpr(rhs, &subst)));
        }
        for (lhs, bit, rhs) in &child.assigns {
            let lhs = subst.get(lhs).cloned().unwrap_or_else(|| format!("{prefix}{lhs}"));
            out.assigns.push((lhs, *bit, rewrite_rexpr(rhs, &subst)));
        }
    }
    Ok(out)
}

pub fn synth_sv(source: &str, origin: &str) -> Result<Design, String> {
    let _tree = parse_sv(source, origin)?;
    let mods = parse_source(source)?;
    let map: HashMap<String, Rtl> = mods.iter().map(|m| (m.module.clone(), m.clone())).collect();
    let instantiated: HashMap<String, ()> = mods
        .iter()
        .flat_map(|m| m.insts.iter().map(|i| (i.module.clone(), ())))
        .collect();
    let top = mods
        .iter()
        .rev()
        .find(|m| !instantiated.contains_key(&m.module))
        .or_else(|| mods.last())
        .ok_or_else(|| "no top module".to_string())?;
    let flat = flatten_module(&map, &top.module)?;
    synth_rtl(&flat)
}

pub fn synth_sv_path(path: &Path) -> Result<Design, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    synth_sv(&src, &path.display().to_string())
}

pub fn lut_init_of(source: &str) -> Result<u64, String> {
    match synth_sv(source, "t.sv")?.cell("u_lut").unwrap().kind {
        CellKind::Lut6 { init } => Ok(init),
        _ => Err("no lut".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::INC4_INIT;

    fn wrap(rhs: &str) -> String {
        format!(
            "module blinky(input logic clk, output logic led);\n  logic q;\n  always_ff @(posedge clk) q <= {rhs};\n  assign led = q;\nendmodule\n"
        )
    }

    #[test]
    fn sv_parser_rejects_garbage() {
        assert!(parse_sv("this is not verilog @@@", "bad.sv").is_err());
    }

    #[test]
    fn inverter_and_buffer_and_zero_differ() {
        let inv = lut_init_of(&wrap("~q")).expect("inv");
        let buf = lut_init_of(&wrap("q")).expect("buf");
        let z = lut_init_of(&wrap("1'b0")).expect("zero");
        assert_eq!(inv, 0x5555_5555_5555_5555);
        assert_eq!(buf, 0xAAAA_AAAA_AAAA_AAAA);
        assert_eq!(z, 0);
        assert_ne!(inv, buf);
        assert_ne!(buf, z);
    }

    #[test]
    fn aig_flowmap_and() {
        let e = parse_expr("a&b").unwrap();
        let aig = Aig::from_expr(&e);
        assert_eq!(aig.pis.len(), 2);
        let init = aig.flowmap_lut6();
        // I0=a I1=b: AND is 1 only when addr & 3 == 3 → bits 3,7,11,...
        assert_eq!(init & 0b1111, 0b1000);
    }

    #[test]
    fn incrementer_matches_gold_init() {
        let src = r#"
module counter(input logic clk, output logic led);
  logic [3:0] cnt;
  always_ff @(posedge clk) cnt <= cnt + 1;
  assign led = cnt[3];
endmodule
"#;
        let d = synth_sv(src, "c.sv").unwrap();
        let inits = d.lut_inits();
        assert_eq!(inits, INC4_INIT.to_vec(), "synth incrementer INIT {inits:#x?}");
        assert_eq!(d.net_on("u_iob", "I"), Some("cnt_3"));
        assert_eq!(d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count(), 4);
    }

    #[test]
    fn incrementer_plus_one_b1() {
        let src = r#"
module counter(input logic clk, output logic led);
  logic [3:0] cnt;
  always_ff @(posedge clk) begin
    cnt <= cnt + 1'b1;
  end
  assign led = cnt[3];
endmodule
"#;
        let d = synth_sv(src, "c.sv").unwrap();
        assert_eq!(d.lut_inits(), INC4_INIT.to_vec());
    }

    #[test]
    fn reset_if_else_is_not_a_noop() {
        let src = r#"
module counter(input logic clk, input logic rst, output logic led);
  logic [3:0] cnt;
  always_ff @(posedge clk) begin
    if (rst) cnt <= 0;
    else cnt <= cnt + 1;
  end
  assign led = cnt[3];
endmodule
"#;
        let d = synth_sv(src, "r.sv").unwrap();
        let inits = d.lut_inits();
        assert_eq!(inits.len(), 4);
        assert_ne!(
            inits[0], INC4_INIT[0],
            "rst must occupy a LUT pin so INIT is not the bare incrementer"
        );
        assert!(d.ports.iter().any(|p| p.name == "rst"));
    }

    #[test]
    fn mul_infers_mac27() {
        let src = r#"
module mac(input logic clk, input logic [26:0] a, input logic [26:0] b, output logic [47:0] p);
  always_ff @(posedge clk) p <= a * b + 0;
endmodule
"#;
        let d = synth_sv(src, "m.sv").unwrap();
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "a*b must map to MAC27, cells {:?}",
            d.cells.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        assert!(d.lut_inits().is_empty(), "DSP path must not bitblast a 27x27 mul");
    }

    #[test]
    fn mem_infers_bram18() {
        let src = r#"
module ram(input logic clk, input logic [7:0] din, input logic [8:0] addr);
  logic [7:0] mem [0:511];
  always_ff @(posedge clk) mem[addr] <= din;
endmodule
"#;
        let d = synth_sv(src, "ram.sv").unwrap();
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Bram18)));
    }

    #[test]
    fn hierarchy_flattens_to_inverter() {
        let src = r#"
module tog(input logic clk, output logic q);
  always_ff @(posedge clk) q <= ~q;
endmodule
module top(input logic clk, output logic led);
  tog u0(.clk(clk), .q(led));
endmodule
"#;
        let d = synth_sv(src, "h.sv").unwrap();
        assert_eq!(d.name, "top");
        let inits = d.lut_inits();
        assert_eq!(inits, vec![0x5555_5555_5555_5555]);
    }

    #[test]
    fn always_comb_and_eq_case() {
        let src = r#"
module m(input logic clk, output logic led);
  logic [1:0] s;
  logic q;
  always_ff @(posedge clk) begin
    case (s)
      2'd0: s <= 2'd1;
      2'd1: s <= 2'd2;
      default: s <= 2'd0;
    endcase
    q <= (s == 2'd2);
  end
  always_comb led = q;
endmodule
"#;
        let d = synth_sv(src, "c.sv").unwrap();
        assert!(d.lut_inits().len() >= 2, "case/eq must produce LUTs {:?}", d.lut_inits());
        assert!(d.cell("u_iob").is_some() || d.cells.iter().any(|c| matches!(c.kind, CellKind::IobOut)));
    }
}
