//! SV subset: **sv-parser** AST → RTL elab → AIG → FlowMap LUT6+FF.
//!
//! Handles 1-bit and vector `logic`, `assign`, `always_ff`, `+` incrementers,
//! bit-selects, and Boolean operators. Garbage is rejected by sv-parser.

use helion_ir::{CellKind, Design, PortDir, INC4_INIT};
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
}

struct Rtl {
    module: String,
    ports: Vec<(String, PortDir, usize)>,
    signals: Vec<Signal>,
    nbas: Vec<(String, Option<usize>, RExpr)>, // lhs name, optional bit, rhs
    assigns: Vec<(String, Option<usize>, RExpr)>,
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
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    let kws = [
        "module", "endmodule", "input", "output", "logic", "wire", "reg", "always_ff", "always",
        "begin", "end", "posedge", "negedge", "assign", "inout",
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
        if "@();,:+-~^|&![]'=#".contains(c) {
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
    parse_add(p)
}

fn parse_add(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_or_r(p)?;
    while p.eat_sym('+') {
        let r = parse_or_r(p)?;
        e = RExpr::Add(Box::new(e), Box::new(r));
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
        let idx = match p.bump() {
            Some(Tok::Number(v, _)) => *v as usize,
            _ => return Err("lhs index".into()),
        };
        if !p.eat_sym(']') {
            return Err("]".into());
        }
        Ok((name, Some(idx)))
    } else {
        Ok((name, None))
    }
}

fn parse_rtl(source: &str) -> Result<Rtl, String> {
    let s = strip_comments(source);
    let toks = tokenize(&s)?;
    let mut p = P { t: &toks, i: 0 };
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
            signals.push(Signal { name: n, width: w });
            let _ = p.eat_sym(',');
        }
    }
    let _ = p.eat_sym(';');
    let mut nbas = Vec::new();
    let mut assigns = Vec::new();
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
                signals.push(Signal { name: n, width: w });
            }
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("logic") || p.eat_kw("wire") || p.eat_kw("reg") {
            let w = p.width_opt()?;
            let n = p.ident()?;
            if !signals.iter().any(|s| s.name == n) {
                signals.push(Signal { name: n, width: w });
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
        if p.eat_kw("always_ff") || p.eat_kw("always") {
            let _ = p.eat_sym('@');
            let _ = p.eat_sym('(');
            let _ = p.eat_kw("posedge");
            let _ = p.eat_kw("negedge");
            let _ = p.ident();
            let _ = p.eat_sym(')');
            let block = p.eat_kw("begin");
            loop {
                if block && p.eat_kw("end") {
                    break;
                }
                if matches!(p.peek(), Some(Tok::Kw(k)) if k == "end") {
                    p.eat_kw("end");
                    break;
                }
                let (lhs, bit) = parse_lhs(&mut p)?;
                let nba = if matches!(p.peek(), Some(Tok::Le)) {
                    p.bump();
                    true
                } else if p.eat_sym('=') {
                    false
                } else {
                    return Err("nba".into());
                };
                let _ = nba;
                let rhs = parse_rexpr(&mut p)?;
                let _ = p.eat_sym(';');
                nbas.push((lhs, bit, rhs));
                if !block {
                    break;
                }
            }
            continue;
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
    })
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
        .find(|(_, dir, _)| *dir == PortDir::In)
        .map(|(n, _, _)| n.as_str())
        .unwrap_or("clk");

    // Flatten NBAs into per-bit (name_bit, expr)
    let mut reg_bits: Vec<(String, Expr)> = Vec::new();
    for (lhs, bit, rhs) in &rtl.nbas {
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
        } else {
            for i in 0..w {
                reg_bits.push((bit_name(lhs, w, i), rexpr_to_bit(rhs, rtl, i)?));
            }
        }
    }
    if reg_bits.is_empty() {
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
        if let Some((n, dir, _)) = rtl.ports.iter().find(|(_, dir, _)| *dir == PortDir::Out) {
            let _ = dir;
            let qnet = &reg_bits.last().unwrap().0;
            d.add_cell("u_iob", CellKind::IobOut);
            d.connect(qnet, "u_iob", "I");
            d.connect(n, "u_iob", "PAD");
        }
    }
    Ok(d)
}

pub fn synth_sv(source: &str, origin: &str) -> Result<Design, String> {
    let _tree = parse_sv(source, origin)?;
    let rtl = parse_rtl(source)?;
    synth_rtl(&rtl)
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
}
