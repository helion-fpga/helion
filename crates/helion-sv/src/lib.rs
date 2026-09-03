//! SV frontend: preprocess + helion-sv elab → AIG → FlowMap LUT6+FF.
//!
//! Large cores (Ibex, PicoRV32) are ingested via `` `define ``/`ifdef`
//! preprocess and skip of packages/typedefs; Helion-legal always_ff / assign
//! still map to LUT/FF. Unknown instances become empty blackboxes.

mod preprocess;
pub use preprocess::preprocess_sv;

use helion_ir::{CellKind, Design, PortDir};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use sv_parser::{parse_sv_str, Define, DefineText};

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
        let mut memo: HashMap<u32, bool> = HashMap::new();
        self.eval_lit_memo(lit, pi_bits, &mut memo)
    }

    fn eval_lit_memo(&self, lit: Lit, pi_bits: u64, memo: &mut HashMap<u32, bool>) -> bool {
        let v = if lit.node == 0 {
            false
        } else if (lit.node as usize) <= self.pis.len() {
            let i = (lit.node - 1) as u32;
            (pi_bits >> i) & 1 == 1
        } else if let Some(&cached) = memo.get(&lit.node) {
            cached
        } else {
            let ai = (lit.node as usize) - 1 - self.pis.len();
            let (a, b) = self.ands[ai];
            let computed =
                self.eval_lit_memo(a, pi_bits, memo) && self.eval_lit_memo(b, pi_bits, memo);
            memo.insert(lit.node, computed);
            computed
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

/// UG900 Compilation tab over helion-sv (include + `define), not xvlog.
#[derive(Clone, Debug, Default)]
pub struct SvCompileOpts {
    /// `define NAME[=VALUE]` pairs for sv-parser.
    pub defines: Vec<(String, String)>,
    /// Verilog include directories for sv-parser.
    pub include_paths: Vec<String>,
}

/// UG900 Elaboration snapshot stats from helion-sv (not xelab).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvElabReport {
    pub top: String,
    pub cells: usize,
    pub luts: usize,
    pub ffs: usize,
}

pub fn parse_sv(source: &str, origin: &str) -> Result<sv_parser::SyntaxTree, String> {
    parse_sv_opts(source, origin, &SvCompileOpts::default())
}

pub fn parse_sv_opts(
    source: &str,
    origin: &str,
    opts: &SvCompileOpts,
) -> Result<sv_parser::SyntaxTree, String> {
    let mut defines = HashMap::new();
    for (k, v) in &opts.defines {
        let text = if v.is_empty() {
            None
        } else {
            Some(DefineText::new(v.clone(), None))
        };
        defines.insert(k.clone(), Some(Define::new(k.clone(), Vec::new(), text)));
    }
    let inc: Vec<String> = if opts.include_paths.is_empty() {
        vec!["".into()]
    } else {
        opts.include_paths.clone()
    };
    parse_sv_str(source, origin, &defines, &inc, false, false)
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
    keep: bool,
    mark_debug: bool,
}

#[derive(Clone, Debug)]
enum RExpr {
    Const { val: u128, width: usize, care: u128 }, // care bits: 1 = specified (casez)
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
    params: Vec<(String, u128)>,
}

#[derive(Clone, Debug)]
struct Rtl {
    module: String,
    ports: Vec<(String, PortDir, usize)>,
    signals: Vec<Signal>,
    nbas: Vec<(String, Option<usize>, RExpr)>, // lhs name, optional bit, rhs
    assigns: Vec<(String, Option<usize>, RExpr)>,
    insts: Vec<Inst>,
    params: Vec<(String, u128)>,
    toks: Vec<Tok>,
    mem_inits: HashMap<String, BTreeMap<usize, u128>>,
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
    /// based number with x/z/? don't-care bits (casez).
    Pat { val: u128, care: u128, width: usize },
    Kw(String),
    Str(String),
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
        "case", "casez", "casex", "endcase", "default", "generate", "endgenerate", "genvar",
        "for", "int", "parameter", "localparam", "initial", "package", "endpackage", "typedef",
        "import", "export", "struct", "enum", "packed", "signed", "unsigned", "function",
        "endfunction", "task", "endtask", "return", "always_latch", "unique", "priority",
        "automatic", "void", "const", "var", "ref", "static", "extern", "virtual", "pure",
        "interface", "endinterface", "modport", "clocking", "property", "endproperty",
        "assert", "assume", "cover", "sequence", "endsequence",
    ];
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push(Tok::Str(s));
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
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            continue;
        }
        if c == '\\' {
            i += 1;
            if i < chars.len() && chars[i] == '\n' {
                i += 1;
            }
            continue;
        }
        if "@();,:+-~^|&![]'=#?*<>.{}/%".contains(c) {
            out.push(Tok::Sym(c));
            i += 1;
            continue;
        }
        if c == '`' {
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            continue;
        }
        if c == '$' || c.is_ascii_alphabetic() || c == '_' {
            let mut n = String::new();
            if c == '$' {
                n.push(c);
                i += 1;
            }
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
                while i < chars.len() {
                    let ch = chars[i];
                    let ok = ch.is_ascii_hexdigit()
                        || matches!(ch, 'x' | 'X' | 'z' | 'Z' | '?');
                    if !ok {
                        break;
                    }
                    digits.push(ch);
                    i += 1;
                }
                let mut val = 0u128;
                let mut care = 0u128;
                let mut dc = false;
                if base == 2 {
                    for (off, ch) in digits.chars().enumerate() {
                        let bit = width.saturating_sub(off + 1);
                        match ch {
                            '1' => {
                                val |= 1u128 << bit;
                                care |= 1u128 << bit;
                            }
                            '0' => {
                                care |= 1u128 << bit;
                            }
                            _ => {
                                dc = true;
                            }
                        }
                    }
                } else {
                    let parsed = u128::from_str_radix(
                        &digits
                            .chars()
                            .map(|c| if matches!(c, 'x' | 'X' | 'z' | 'Z' | '?') { '0' } else { c })
                            .collect::<String>(),
                        base,
                    )
                    .unwrap_or(0);
                    val = parsed;
                    care = if width >= 128 { u128::MAX } else { (1u128 << width.max(1)) - 1 };
                    if digits.chars().any(|c| matches!(c, 'x' | 'X' | 'z' | 'Z' | '?')) {
                        dc = true;
                    }
                }
                if dc {
                    out.push(Tok::Pat { val, care, width });
                } else {
                    out.push(Tok::Number(val, width));
                }
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
    params: HashMap<String, u128>,
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
        let msb = const_u(self)? as usize;
        if !self.eat_sym(':') {
            return Err("range :".into());
        }
        let lsb = const_u(self)? as usize;
        if !self.eat_sym(']') {
            return Err("]".into());
        }
        Ok(msb.max(lsb) - msb.min(lsb) + 1)
    }
}

fn const_atom(p: &mut P) -> Result<u128, String> {
    if p.eat_sym('(') {
        let v = const_u(p)?;
        if !p.eat_sym(')') {
            return Err("const )".into());
        }
        return Ok(v);
    }
    match p.peek() {
        Some(Tok::Number(v, _)) => {
            let n = *v;
            p.bump();
            Ok(n)
        }
        Some(Tok::Ident(s)) => {
            let name = s.clone();
            p.bump();
            p.params
                .get(&name)
                .copied()
                .ok_or_else(|| format!("unknown param {name}"))
        }
        other => Err(format!("const atom {other:?}")),
    }
}

fn const_u(p: &mut P) -> Result<u128, String> {
    let mut v = const_atom(p)?;
    loop {
        if p.eat_sym('+') {
            v = v.saturating_add(const_atom(p)?);
        } else if p.eat_sym('-') {
            v = v.saturating_sub(const_atom(p)?);
        } else if p.eat_sym('*') {
            v = v.saturating_mul(const_atom(p)?);
        } else if p.eat_sym('/') {
            let d = const_atom(p)?.max(1);
            v /= d;
        } else if p.eat_sym('%') {
            let d = const_atom(p)?.max(1);
            v %= d;
        } else {
            break;
        }
    }
    Ok(v)
}

fn const_cond(p: &mut P) -> Result<bool, String> {
    let l = const_u(p)?;
    if matches!(p.peek(), Some(Tok::Eq)) {
        p.bump();
        return Ok(l == const_u(p)?);
    }
    if matches!(p.peek(), Some(Tok::Ne)) {
        p.bump();
        return Ok(l != const_u(p)?);
    }
    if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
        return Ok(l <= const_u(p)?);
    }
    if p.eat_sym('<') {
        return Ok(l < const_u(p)?);
    }
    if p.eat_sym('>') {
        return Ok(l > const_u(p)?);
    }
    Ok(l != 0)
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
        Some(Tok::Number(v, w)) => {
            let care = if *w >= 128 { u128::MAX } else { (1u128 << (*w).max(1)) - 1 };
            Ok(RExpr::Const {
                val: *v,
                width: *w,
                care,
            })
        }
        Some(Tok::Pat { val, care, width }) => Ok(RExpr::Const {
            val: *val,
            width: *width,
            care: *care,
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
    let _ = p.eat_kw("signed");
    let _ = p.eat_kw("unsigned");
    let _ = p.eat_kw("var");
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

fn parse_for_unroll(p: &mut P) -> Result<Vec<Nba>, String> {
    if !p.eat_sym('(') {
        return Err("for (".into());
    }
    let _ = p.eat_kw("int");
    let _ = p.eat_kw("genvar");
    let var = p.ident()?;
    if !p.eat_sym('=') {
        return Err("for =".into());
    }
    let start = const_u(p)? as usize;
    if !p.eat_sym(';') {
        return Err("for ;".into());
    }
    let _ = p.ident();
    let inclusive = if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
        true
    } else if p.eat_sym('<') {
        false
    } else {
        return Err("for cmp".into());
    };
    let end = const_u(p)? as usize;
    let end = if inclusive { end + 1 } else { end };
    if !p.eat_sym(';') {
        return Err("for ;2".into());
    }
    let _ = p.ident();
    let _ = p.eat_sym('=');
    let _ = p.ident();
    let _ = p.eat_sym('+');
    let step = match p.peek() {
        Some(Tok::Number(v, _)) => {
            let n = (*v as usize).max(1);
            p.bump();
            n
        }
        _ => 1,
    };
    if !p.eat_sym(')') {
        return Err("for )".into());
    }
    let block = p.eat_kw("begin");
    let start_i = p.i;
    if block {
        let mut depth = 1i32;
        while depth > 0 {
            match p.bump() {
                Some(Tok::Kw(k)) if k == "begin" => depth += 1,
                Some(Tok::Kw(k)) if k == "end" => depth -= 1,
                None => return Err("for body".into()),
                _ => {}
            }
        }
    } else {
        while p.peek().is_some() && !matches!(p.peek(), Some(Tok::Sym(';'))) {
            p.bump();
        }
        let _ = p.eat_sym(';');
    }
    let body = p.t[start_i..p.i].to_vec();
    let mut out = Vec::new();
    let niter = end.saturating_sub(start) / step.max(1);
    if niter > 4096 {
        return Ok(Vec::new());
    }
    let mut i = start;
    while i < end {
        let toks: Vec<Tok> = body
            .iter()
            .map(|t| match t {
                Tok::Ident(s) if s == &var => Tok::Number(i as u128, 32),
                other => other.clone(),
            })
            .collect();
        let mut sp = P { t: &toks, i: 0, params: p.params.clone() };
        out.extend(parse_seq_block(&mut sp, block)?);
        i += step;
    }
    Ok(out)
}

fn parse_attr(p: &mut P) -> Option<(String, String)> {
    let save = p.i;
    if !p.eat_sym('(') {
        return None;
    }
    if !p.eat_sym('*') {
        p.i = save;
        return None;
    }
    let k = match p.bump() {
        Some(Tok::Ident(s) | Tok::Kw(s)) => s.clone(),
        _ => {
            p.i = save;
            return None;
        }
    };
    let _ = p.eat_sym('=');
    let v = match p.bump() {
        Some(Tok::Str(s) | Tok::Ident(s) | Tok::Kw(s)) => s.clone(),
        Some(Tok::Number(n, _)) => n.to_string(),
        _ => "1".into(),
    };
    let _ = p.eat_sym('*');
    let _ = p.eat_sym(')');
    Some((k, v))
}

fn parse_seq_item(p: &mut P) -> Result<Vec<Nba>, String> {
    if p.eat_kw("for") {
        return parse_for_unroll(p);
    }
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
    if p.eat_kw("case") || p.eat_kw("casez") || p.eat_kw("casex") {
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

fn skip_until_kw(p: &mut P, kw: &str) {
    while p.peek().is_some() {
        if p.eat_kw(kw) {
            return;
        }
        let _ = p.bump();
    }
}

fn parse_source(source: &str) -> Result<Vec<Rtl>, String> {
    let s = preprocess_sv(&strip_comments(source));
    let toks = tokenize(&s)?;
    let mut p = P { t: &toks, i: 0, params: HashMap::new() };
    let mut mods = Vec::new();
    while p.peek().is_some() {
        if p.eat_sym(';') {
            continue;
        }
        if p.eat_kw("import") || p.eat_kw("export") {
            let _ = skip_item_or_block(&mut p);
            continue;
        }
        if p.eat_kw("package") {
            skip_until_kw(&mut p, "endpackage");
            continue;
        }
        if p.eat_kw("typedef") {
            let _ = skip_item_or_block(&mut p);
            continue;
        }
        if p.eat_kw("interface") {
            skip_until_kw(&mut p, "endinterface");
            continue;
        }
        if p.eat_kw("function") {
            skip_until_kw(&mut p, "endfunction");
            continue;
        }
        if p.eat_kw("task") {
            skip_until_kw(&mut p, "endtask");
            continue;
        }
        match p.peek() {
            Some(Tok::Kw(k)) if k == "module" => match parse_one_module(&mut p) {
                Ok(m) => mods.push(m),
                Err(_) => skip_until_kw(&mut p, "endmodule"),
            },
            _ => {
                // Never skip_item_or_block at file scope — that can walk past `module`.
                let _ = p.bump();
            }
        }
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

fn parse_param_assigns(p: &mut P) -> Result<Vec<(String, u128)>, String> {
    let mut out = Vec::new();
    if !p.eat_sym('#') {
        return Ok(out);
    }
    if !p.eat_sym('(') {
        return Ok(out);
    }
    let mut pos = 0usize;
    loop {
        if p.eat_sym(')') {
            break;
        }
        if p.peek().is_none() {
            return Err("param list )".into());
        }
        let _ = p.eat_kw("parameter");
        let _ = p.eat_kw("localparam");
        if p.eat_sym('.') {
            let name = p.ident()?;
            if !p.eat_sym('(') {
                return Err("param .name(".into());
            }
            let val = const_u(p)?;
            if !p.eat_sym(')') {
                return Err("param .name)".into());
            }
            out.push((name, val));
        } else if matches!(p.peek(), Some(Tok::Ident(_))) {
            let name = p.ident()?;
            if !p.eat_sym('=') {
                return Err("param =".into());
            }
            let val = const_u(p)?;
            out.push((name, val));
        } else {
            let val = const_u(p)?;
            out.push((format!("#{pos}"), val));
            pos += 1;
        }
        let _ = p.eat_sym(',');
    }
    Ok(out)
}

fn apply_params(p: &mut P, assigns: Vec<(String, u128)>, ordered: &mut Vec<(String, u128)>) {
    for (k, v) in assigns {
        let mut key = k.clone();
        let mut val = v;
        if let Some(rest) = k.strip_prefix('#') {
            if let Ok(i) = rest.parse::<usize>() {
                if let Some((name, _)) = ordered.get(i) {
                    key = name.clone();
                }
            }
        }
        // Overrides already in p.params win over defaults.
        if let Some(ov) = p.params.get(&key).copied() {
            val = ov;
        } else {
            p.params.insert(key.clone(), val);
        }
        if let Some(slot) = ordered.iter_mut().find(|(n, _)| n == &key) {
            slot.1 = val;
        } else {
            ordered.push((key, val));
        }
    }
}

fn skip_begin_end(p: &mut P) -> Result<(), String> {
    let mut depth = 1i32;
    while depth > 0 {
        match p.bump() {
            Some(Tok::Kw(k)) if k == "begin" => depth += 1,
            Some(Tok::Kw(k)) if k == "end" => depth -= 1,
            None => return Err("unterminated begin".into()),
            _ => {}
        }
    }
    Ok(())
}

fn skip_item_or_block(p: &mut P) -> Result<(), String> {
    if p.eat_kw("begin") {
        if p.eat_sym(':') {
            let _ = p.ident();
        }
        return skip_begin_end(p);
    }
    let mut depth = 0i32;
    while p.peek().is_some() {
        if depth == 0 && p.eat_sym(';') {
            break;
        }
        match p.peek() {
            Some(Tok::Kw(k)) if k == "begin" => {
                p.bump();
                depth += 1;
            }
            Some(Tok::Kw(k)) if k == "end" => {
                p.bump();
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            }
            Some(Tok::Kw(k)) if k == "endgenerate" || k == "endmodule" || k == "else" => {
                if depth == 0 {
                    break;
                }
                p.bump();
            }
            None => break,
            _ => {
                p.bump();
            }
        }
    }
    Ok(())
}

fn parse_net_ref(p: &mut P) -> Result<String, String> {
    let name = p.ident()?;
    if p.eat_sym('[') {
        let idx = const_u(p)? as usize;
        if !p.eat_sym(']') {
            return Err("net ]".into());
        }
        return Ok(format!("{name}_{idx}"));
    }
    Ok(name)
}

fn parse_one_module(mut p: &mut P) -> Result<Rtl, String> {
    let tok_start = p.i;
    if !p.eat_kw("module") {
        return Err("expected module".into());
    }
    let module = p.ident()?;
    while p.eat_kw("import") {
        let _ = skip_item_or_block(&mut p);
    }
    let mut param_order: Vec<(String, u128)> = Vec::new();
    let header_params = parse_param_assigns(&mut p)?;
    apply_params(&mut p, header_params, &mut param_order);
    let mut ports = Vec::new();
    let mut signals = Vec::new();
    if p.eat_sym('(') {
        loop {
            if p.eat_sym(')') {
                break;
            }
            if p.peek().is_none() {
                break;
            }
            let dir = parse_port_dir(&mut p).unwrap_or(PortDir::In);
            skip_logic(&mut p);
            // package::typedef or typedef name before the port ident
            while matches!(p.peek(), Some(Tok::Ident(_)))
                && matches!(p.t.get(p.i + 1), Some(Tok::Sym(':')))
            {
                let _ = p.ident();
                let _ = p.eat_sym(':');
                let _ = p.eat_sym(':');
            }
            if matches!(p.peek(), Some(Tok::Ident(_)))
                && matches!(p.t.get(p.i + 1), Some(Tok::Ident(_)) | Some(Tok::Sym('[')))
            {
                let _ = p.ident();
            }
            skip_logic(&mut p);
            let w = p.width_opt().unwrap_or(1);
            match p.ident() {
                Ok(n) => {
                    ports.push((n.clone(), dir, w));
                    signals.push(Signal {
                        name: n,
                        width: w,
                        depth: 0,
                        keep: false,
                        mark_debug: false,
                    });
                }
                Err(_) => {
                    // skip this port to comma or ')'
                    let mut d = 0i32;
                    while p.peek().is_some() {
                        if d == 0 && (p.eat_sym(',') || matches!(p.peek(), Some(Tok::Sym(')')))) {
                            break;
                        }
                        match p.peek() {
                            Some(Tok::Sym('(')) => {
                                d += 1;
                                p.bump();
                            }
                            Some(Tok::Sym(')')) => {
                                if d == 0 {
                                    break;
                                }
                                d -= 1;
                                p.bump();
                            }
                            _ => {
                                p.bump();
                            }
                        }
                    }
                }
            }
            let _ = p.eat_sym(',');
        }
    }
    let _ = p.eat_sym(';');
    let mut nbas = Vec::new();
    let mut assigns = Vec::new();
    let mut insts = Vec::new();
    let mut pending_keep = false;
    let mut pending_md = false;
    let mut mem_inits: HashMap<String, BTreeMap<usize, u128>> = HashMap::new();
    if parse_module_items(
        &mut p,
        &mut ports,
        &mut signals,
        &mut nbas,
        &mut assigns,
        &mut insts,
        &mut param_order,
        &mut pending_keep,
        &mut pending_md,
        &mut mem_inits,
        "endmodule",
    )
    .is_err()
    {
        skip_until_kw(&mut p, "endmodule");
    }
    let toks = p.t[tok_start..p.i].to_vec();
    Ok(Rtl {
        module,
        ports,
        signals,
        nbas,
        assigns,
        insts,
        params: param_order,
        toks,
        mem_inits,
    })
}

fn parse_module_items(
    mut p: &mut P,
    ports: &mut Vec<(String, PortDir, usize)>,
    signals: &mut Vec<Signal>,
    nbas: &mut Vec<Nba>,
    assigns: &mut Vec<(String, Option<usize>, RExpr)>,
    insts: &mut Vec<Inst>,
    param_order: &mut Vec<(String, u128)>,
    pending_keep: &mut bool,
    pending_md: &mut bool,
    mem_inits: &mut HashMap<String, BTreeMap<usize, u128>>,
    endkw: &str,
) -> Result<(), String> {
    while !p.eat_kw(endkw) {
        if p.peek().is_none() {
            return Err(format!("unterminated until {endkw}"));
        }
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "end" || k == "endmodule" || k == "endgenerate") {
            break;
        }
        while let Some((k, v)) = parse_attr(&mut p) {
            let on = v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes");
            if k.eq_ignore_ascii_case("keep") || k.eq_ignore_ascii_case("dont_touch") {
                *pending_keep = on;
            }
            if k.eq_ignore_ascii_case("mark_debug") {
                *pending_md = on;
            }
        }
        if p.eat_kw("function") {
            skip_until_kw(p, "endfunction");
            continue;
        }
        if p.eat_kw("task") {
            skip_until_kw(p, "endtask");
            continue;
        }
        if p.eat_kw("typedef") || p.eat_kw("import") || p.eat_kw("export") {
            let _ = skip_item_or_block(p);
            continue;
        }
        if p.eat_kw("generate") {
            parse_module_items(
                p, ports, signals, nbas, assigns, insts, param_order, pending_keep, pending_md, mem_inits,
                "endgenerate",
            )?;
            continue;
        }
        if p.eat_kw("endgenerate") {
            break;
        }
        if p.eat_kw("genvar") {
            let _ = p.ident();
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("parameter") || p.eat_kw("localparam") {
            let name = p.ident()?;
            if !p.eat_sym('=') {
                return Err("parameter =".into());
            }
            let val = const_u(p)?;
            let _ = p.eat_sym(';');
            p.params.entry(name.clone()).or_insert(val);
            if !param_order.iter().any(|(n, _)| n == &name) {
                param_order.push((name, val));
            }
            continue;
        }
        if p.eat_kw("if") {
            if !p.eat_sym('(') {
                return Err("genif (".into());
            }
            let yes = const_cond(p)?;
            if !p.eat_sym(')') {
                return Err("genif )".into());
            }
            let then_begin = p.eat_kw("begin");
            if p.eat_sym(':') {
                let _ = p.ident();
            }
            if yes {
                if then_begin {
                    parse_module_items(
                        p, ports, signals, nbas, assigns, insts, param_order, pending_keep, pending_md, mem_inits,
                        "end",
                    )?;
                } else {
                    // one module item; require a following else/endgenerate/endmodule delimiter
                    parse_module_items(
                        p, ports, signals, nbas, assigns, insts, param_order, pending_keep, pending_md, mem_inits,
                        "else",
                    )?;
                    // parse_module_items consumed the else keyword — put it back
                    p.i -= 1;
                }
            } else if then_begin {
                skip_begin_end(p)?;
            } else {
                skip_item_or_block(p)?;
            }
            if p.eat_kw("else") {
                let eb = p.eat_kw("begin");
                if p.eat_sym(':') {
                    let _ = p.ident();
                }
                if yes {
                    if eb {
                        skip_begin_end(p)?;
                    } else {
                        skip_item_or_block(p)?;
                    }
                } else if eb {
                    parse_module_items(
                        p, ports, signals, nbas, assigns, insts, param_order, pending_keep, pending_md, mem_inits,
                        "end",
                    )?;
                } else {
                    skip_item_or_block(p)?;
                    p.i = p.i.saturating_sub(0);
                }
            }
            continue;
        }
        if p.eat_kw("for") {
            // Unroll: NBA body and/or instantiations.
            let save = p.i;
            match parse_for_unroll_module(p, nbas, insts, assigns, mem_inits) {
                Ok(()) => continue,
                Err(_) => {
                    p.i = save;
                    nbas.extend(parse_for_unroll(p)?);
                    continue;
                }
            }
        }
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "input" || k == "output") {
            let dir = parse_port_dir(&mut p).unwrap();
            skip_logic(&mut p);
            let w = p.width_opt()?;
            let n = p.ident()?;
            ports.push((n.clone(), dir, w));
            if !signals.iter().any(|s| s.name == n) {
                signals.push(Signal {
                    name: n,
                    width: w,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                });
            }
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("logic") || p.eat_kw("wire") || p.eat_kw("reg") {
            let w = p.width_opt()?;
            let n = p.ident()?;
            let mut depth = 0usize;
            if p.eat_sym('[') {
                let hi = const_u(p)? as usize;
                if !p.eat_sym(':') {
                    return Err("mem :".into());
                }
                let lo = const_u(p)? as usize;
                if !p.eat_sym(']') {
                    return Err("mem ]".into());
                }
                depth = hi.max(lo) - hi.min(lo) + 1;
            }
            if !signals.iter().any(|s| s.name == n) {
                signals.push(Signal {
                    name: n,
                    width: w,
                    depth,
                    keep: *pending_keep,
                    mark_debug: *pending_md,
                });
            }
            *pending_keep = false;
            *pending_md = false;
            let _ = p.eat_sym(';');
            continue;
        }
        if p.eat_kw("initial") {
            let block = p.eat_kw("begin");
            loop {
                if block && p.eat_kw("end") {
                    break;
                }
                if p.peek().is_none() {
                    break;
                }
                match parse_nba(p) {
                    Ok((lhs, bit, rhs)) => {
                        if let RExpr::Const { val, .. } = rhs {
                            mem_inits
                                .entry(lhs)
                                .or_default()
                                .insert(bit.unwrap_or(0), val);
                        }
                    }
                    Err(_) => {
                        skip_item_or_block(p)?;
                    }
                }
                if !block {
                    break;
                }
            }
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
        if p.eat_kw("function") {
            skip_until_kw(p, "endfunction");
            continue;
        }
        if p.eat_kw("task") {
            skip_until_kw(p, "endtask");
            continue;
        }
        if p.eat_kw("typedef") || p.eat_kw("import") || p.eat_kw("export") {
            let _ = skip_item_or_block(p);
            continue;
        }
        if matches!(p.peek(), Some(Tok::Ident(_))) {
            if let Ok(inst) = parse_inst(&mut p) {
                insts.push(inst);
                continue;
            }
        }
        let _ = skip_item_or_block(p);
        if p.peek().is_some() {
            let _ = p.bump();
        }
    }
    Ok(())
}

fn parse_for_unroll_module(
    p: &mut P,
    nbas: &mut Vec<Nba>,
    insts: &mut Vec<Inst>,
    assigns: &mut Vec<(String, Option<usize>, RExpr)>,
    mem_inits: &mut HashMap<String, BTreeMap<usize, u128>>,
) -> Result<(), String> {
    if !p.eat_sym('(') {
        return Err("for (".into());
    }
    let _ = p.eat_kw("int");
    let _ = p.eat_kw("genvar");
    let var = p.ident()?;
    if !p.eat_sym('=') {
        return Err("for =".into());
    }
    let start = const_u(p)? as usize;
    if !p.eat_sym(';') {
        return Err("for ;".into());
    }
    let _ = p.ident();
    let inclusive = if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
        true
    } else if p.eat_sym('<') {
        false
    } else {
        return Err("for cmp".into());
    };
    let end = const_u(p)? as usize;
    let end = if inclusive { end + 1 } else { end };
    if !p.eat_sym(';') {
        return Err("for ;2".into());
    }
    let _ = p.ident();
    let _ = p.eat_sym('=');
    let _ = p.ident();
    let _ = p.eat_sym('+');
    let step = match p.peek() {
        Some(Tok::Number(v, _)) => {
            let n = (*v as usize).max(1);
            p.bump();
            n
        }
        _ => 1,
    };
    if !p.eat_sym(')') {
        return Err("for )".into());
    }
    let block = p.eat_kw("begin");
    if p.eat_sym(':') {
        let _ = p.ident();
    }
    let start_i = p.i;
    if block {
        skip_begin_end(p)?;
    } else {
        while p.peek().is_some() && !matches!(p.peek(), Some(Tok::Sym(';'))) {
            p.bump();
        }
        let _ = p.eat_sym(';');
    }
    let body = p.t[start_i..p.i].to_vec();
    let niter = end.saturating_sub(start) / step.max(1);
    if niter > 4096 {
        return Ok(());
    }
    let mut i = start;
    while i < end {
        let toks: Vec<Tok> = body
            .iter()
            .map(|t| match t {
                Tok::Ident(s) if s == &var => Tok::Number(i as u128, 32),
                other => other.clone(),
            })
            .collect();
        let mut sp = P {
            t: &toks,
            i: 0,
            params: p.params.clone(),
        };
        let mut dummy_ports = Vec::new();
        let mut dummy_sigs = Vec::new();
        let mut dummy_params = Vec::new();
        let mut pk = false;
        let mut pmd = false;
        let mut local_nbas = Vec::new();
        let mut local_assigns = Vec::new();
        let mut local_insts = Vec::new();
        if block {
            parse_module_items(
                &mut sp,
                &mut dummy_ports,
                &mut dummy_sigs,
                &mut local_nbas,
                &mut local_assigns,
                &mut local_insts,
                &mut dummy_params,
                &mut pk,
                &mut pmd,
                mem_inits,
                "end",
            )?;
        } else if let Ok(inst) = parse_inst(&mut sp) {
            local_insts.push(inst);
        } else {
            local_nbas.extend(parse_seq_block(&mut sp, false)?);
        }
        for mut inst in local_insts {
            inst.name = format!("{}_{i}", inst.name);
            insts.push(inst);
        }
        nbas.extend(local_nbas);
        assigns.extend(local_assigns);
        i += step;
    }
    Ok(())
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
    let params = match parse_param_assigns(p) {
        Ok(v) => v,
        Err(_) => {
            p.i = start;
            return Err("inst params".into());
        }
    };
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
            let net = parse_net_ref(p).map_err(|_| {
                p.i = start;
                "inst net".to_string()
            })?;
            if !p.eat_sym(')') {
                p.i = start;
                return Err("inst .port)".into());
            }
            conns.push((port, net));
        } else {
            let net = parse_net_ref(p).map_err(|_| {
                p.i = start;
                "inst pos".to_string()
            })?;
            conns.push((format!("#{pos}"), net));
            pos += 1;
        }
        let _ = p.eat_sym(',');
    }
    let _ = p.eat_sym(';');
    Ok(Inst {
        module,
        name,
        conns,
        params,
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
        RExpr::Const { val, width, care } => {
            let _ = width;
            if (care >> bit) & 1 == 0 {
                return Ok(Expr::Const(false));
            }
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
        RExpr::Add(a, b) => adder_sum_bit(a, b, rtl, bit),
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

fn const_care_of(e: &RExpr) -> u128 {
    match e {
        RExpr::Const { care, .. } => *care,
        _ => u128::MAX,
    }
}

fn adder_sum_bit(a: &RExpr, b: &RExpr, rtl: &Rtl, bit: usize) -> Result<Expr, String> {
    let mut cin = Expr::Const(false);
    let mut sum = Expr::Const(false);
    for i in 0..=bit {
        let ai = rexpr_to_bit(a, rtl, i)?;
        let bi = rexpr_to_bit(b, rtl, i)?;
        let axb = Expr::Xor(Box::new(ai.clone()), Box::new(bi.clone()));
        sum = Expr::Xor(Box::new(axb.clone()), Box::new(cin.clone()));
        let ab = Expr::And(Box::new(ai), Box::new(bi));
        let cin_axb = Expr::And(Box::new(cin), Box::new(axb));
        cin = Expr::Or(Box::new(ab), Box::new(cin_axb));
    }
    Ok(sum)
}

fn cmp_eq_bits(a: &RExpr, b: &RExpr, rtl: &Rtl, _eq: bool) -> Result<Expr, String> {
    let wa = rexpr_width(a, rtl);
    let wb = rexpr_width(b, rtl);
    let w = wa.max(wb).max(1);
    let care = const_care_of(a) & const_care_of(b);
    let mut acc: Option<Expr> = None;
    for i in 0..w {
        if (care >> i) & 1 == 0 {
            continue;
        }
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
            } else if rexpr_is_plus_one(f)
                && rexpr_ident(f).as_deref() == Some(lhs.as_str())
                && matches!(t.as_ref(), RExpr::Ident(s) if s == lhs)
            {
                // saturating / hold: if (cond) cnt <= cnt; else cnt <= cnt+1
                for i in 0..w {
                    let inc = inc_bit_expr(lhs, w, i);
                    let c = rexpr_to_bit(cond, rtl, 0)?;
                    let hold = Expr::Var(bit_name(lhs, w, i));
                    reg_bits.push((
                        bit_name(lhs, w, i),
                        Expr::Or(
                            Box::new(Expr::And(Box::new(c.clone()), Box::new(hold))),
                            Box::new(Expr::And(Box::new(Expr::Not(Box::new(c))), Box::new(inc))),
                        ),
                    ));
                }
            } else if rexpr_is_plus_one(t)
                && rexpr_ident(t).as_deref() == Some(lhs.as_str())
                && matches!(f.as_ref(), RExpr::Ident(s) if s == lhs)
            {
                // clock enable: if (en) cnt <= cnt+1
                for i in 0..w {
                    let inc = inc_bit_expr(lhs, w, i);
                    let c = rexpr_to_bit(cond, rtl, 0)?;
                    let hold = Expr::Var(bit_name(lhs, w, i));
                    reg_bits.push((
                        bit_name(lhs, w, i),
                        Expr::Or(
                            Box::new(Expr::And(Box::new(c.clone()), Box::new(inc))),
                            Box::new(Expr::And(Box::new(Expr::Not(Box::new(c))), Box::new(hold))),
                        ),
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
    let mut mem_names: HashSet<String> = HashSet::new();
    for (lhs, _, _) in &rtl.nbas {
        if sig_depth(rtl, lhs) > 0 {
            mem_names.insert(lhs.clone());
        }
    }
    for k in rtl.mem_inits.keys() {
        mem_names.insert(k.clone());
    }
    for name in &mem_names {
        let cell = format!("u_bram{n_bram}");
        d.add_cell(&cell, CellKind::Bram18);
        let depth = sig_depth(rtl, name).max(
            rtl.mem_inits
                .get(name)
                .and_then(|m| m.keys().max().copied())
                .map(|a| a + 1)
                .unwrap_or(0),
        );
        let mut words = vec![0u64; depth.max(1).min(1024)];
        if let Some(init) = rtl.mem_inits.get(name) {
            for (addr, val) in init {
                if *addr < words.len() {
                    words[*addr] = *val as u64;
                }
            }
        }
        let hex = words
            .iter()
            .map(|w| format!("{w:x}"))
            .collect::<Vec<_>>()
            .join(",");
        let _ = d.set_cell_attr(&cell, "INIT", hex);
        n_bram += 1;
    }

    if reg_bits.is_empty() && n_mac == 0 && n_bram == 0 {
        // Unsupported items were skipped/blackboxed; still return the top with ports.
        return Ok(d);
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
        for s in &rtl.signals {
            let matches_sig = (0..s.width).any(|b| bit_name(&s.name, s.width, b) == *bitn)
                || s.name == *bitn;
            if matches_sig && s.keep {
                let _ = d.dont_touch(&ff);
            }
            if matches_sig && s.mark_debug {
                let _ = d.mark_debug(&qnet);
            }
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
        RExpr::Const { val, width, care } => RExpr::Const {
            val: *val,
            width: *width,
            care: *care,
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

fn elaborate_rtl(src: &Rtl, overrides: &HashMap<String, u128>) -> Result<Rtl, String> {
    if overrides.is_empty() {
        return Ok(src.clone());
    }
    if src.toks.is_empty() {
        return Err(format!("module {} has no tokens to re-elaborate", src.module));
    }
    let mut p = P {
        t: &src.toks,
        i: 0,
        params: overrides.clone(),
    };
    parse_one_module(&mut p)
}

fn inst_overrides(inst: &Inst, child: &Rtl) -> HashMap<String, u128> {
    let mut ov = HashMap::new();
    for (k, v) in &inst.params {
        if let Some(rest) = k.strip_prefix('#') {
            if let Ok(i) = rest.parse::<usize>() {
                if let Some((name, _)) = child.params.get(i) {
                    ov.insert(name.clone(), *v);
                    continue;
                }
            }
        }
        ov.insert(k.clone(), *v);
    }
    ov
}

fn flatten_module(mods: &HashMap<String, Rtl>, name: &str) -> Result<Rtl, String> {
    flatten_module_ov(mods, name, &HashMap::new())
}

fn flatten_module_ov(
    mods: &HashMap<String, Rtl>,
    name: &str,
    overrides: &HashMap<String, u128>,
) -> Result<Rtl, String> {
    flatten_module_ov_vis(mods, name, overrides, &mut HashSet::new())
}

fn flatten_module_ov_vis(
    mods: &HashMap<String, Rtl>,
    name: &str,
    overrides: &HashMap<String, u128>,
    visiting: &mut HashSet<String>,
) -> Result<Rtl, String> {
    if !visiting.insert(name.to_string()) {
        let proto = mods
            .get(name)
            .ok_or_else(|| format!("unknown module {name}"))?;
        let mut src = elaborate_rtl(proto, overrides)?;
        src.insts.clear();
        return Ok(src);
    }
    let proto = mods
        .get(name)
        .ok_or_else(|| format!("unknown module {name}"))?;
    let src = elaborate_rtl(proto, overrides)?;
    let mut out = Rtl {
        module: src.module.clone(),
        ports: src.ports.clone(),
        signals: src.signals.clone(),
        nbas: src.nbas.clone(),
        assigns: src.assigns.clone(),
        insts: Vec::new(),
        params: src.params.clone(),
        toks: src.toks.clone(),
        mem_inits: src.mem_inits.clone(),
    };
    for inst in &src.insts {
        let Some(child_proto) = mods.get(&inst.module) else {
            continue;
        };
        let ov = inst_overrides(inst, child_proto);
        let child = flatten_module_ov_vis(mods, &inst.module, &ov, visiting)?;
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
                    keep: s.keep,
                    mark_debug: s.mark_debug,
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
    visiting.remove(name);
    Ok(out)
}

fn synth_from_parsed(mods: Vec<Rtl>) -> Result<Design, String> {
    synth_from_parsed_top(mods, None, &HashMap::new())
}

fn synth_from_parsed_top(
    mods: Vec<Rtl>,
    top: Option<&str>,
    overrides: &HashMap<String, u128>,
) -> Result<Design, String> {
    let map: HashMap<String, Rtl> = mods.iter().map(|m| (m.module.clone(), m.clone())).collect();
    let instantiated: HashMap<String, ()> = mods
        .iter()
        .flat_map(|m| m.insts.iter().map(|i| (i.module.clone(), ())))
        .collect();
    let top_name = if let Some(name) = top {
        if !map.contains_key(name) {
            return Err(format!("unknown module {name}"));
        }
        name.to_string()
    } else {
        mods.iter()
            .rev()
            .find(|m| !instantiated.contains_key(&m.module))
            .or_else(|| mods.last())
            .ok_or_else(|| "no top module".to_string())?
            .module
            .clone()
    };
    let flat = flatten_module_ov(&map, &top_name, overrides)?;
    let mut d = synth_rtl(&flat)?;
    record_instances(&map, &top_name, &mut d, "");
    Ok(d)
}

/// Keep the pre-flatten instance tree on HNF so the Hierarchy pane is not a cell list.
fn record_instances(mods: &HashMap<String, Rtl>, module: &str, d: &mut Design, pfx: &str) {
    record_instances_vis(mods, module, d, pfx, &mut HashSet::new());
}

fn record_instances_vis(
    mods: &HashMap<String, Rtl>,
    module: &str,
    d: &mut Design,
    pfx: &str,
    visiting: &mut HashSet<String>,
) {
    if !visiting.insert(module.to_string()) {
        return;
    }
    let Some(rtl) = mods.get(module) else {
        visiting.remove(module);
        return;
    };
    for inst in &rtl.insts {
        let name = if pfx.is_empty() {
            inst.name.clone()
        } else {
            format!("{pfx}{}", inst.name)
        };
        d.instances.push(helion_ir::Instance {
            name: name.clone(),
            module: inst.module.clone(),
            conns: inst.conns.clone(),
            attrs: helion_ir::Attrs::default(),
        });
        record_instances_vis(mods, &inst.module, d, &format!("{name}_"), visiting);
    }
    visiting.remove(module);
}

pub fn synth_sv(source: &str, origin: &str) -> Result<Design, String> {
    let pre = preprocess_sv(&strip_comments(source));
    let mods = parse_source(&pre)?;
    let stem = Path::new(origin)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| mods.iter().any(|m| m.module == *s));
    synth_from_parsed_top(mods, stem, &HashMap::new())
}

/// Module names in one SV file (helion-sv parse). UG900 SIM_TOP candidates.
pub fn list_sv_modules(source: &str) -> Result<Vec<String>, String> {
    list_sv_modules_origin(source, "t.sv")
}

/// Module names in one SV file, with origin for sv-parser diagnostics.
pub fn list_sv_modules_origin(source: &str, origin: &str) -> Result<Vec<String>, String> {
    let _ = origin;
    let pre = preprocess_sv(&strip_comments(source));
    Ok(parse_source(&pre)?
        .into_iter()
        .map(|m| m.module)
        .collect())
}

/// Module names from a path on disk (helion-sv parse).
pub fn list_sv_modules_path(path: &Path) -> Result<Vec<String>, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    list_sv_modules_origin(&src, &path.display().to_string())
}

/// UG900 Compilation: sv-parser + helion-sv module list (not xvlog).
pub fn compile_sv(
    source: &str,
    origin: &str,
    opts: &SvCompileOpts,
) -> Result<Vec<String>, String> {
    let _ = (origin, opts);
    Ok(parse_source(source)?
        .into_iter()
        .map(|m| m.module)
        .collect())
}

pub fn compile_sv_path(path: &Path, opts: &SvCompileOpts) -> Result<Vec<String>, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    compile_sv(&src, &path.display().to_string(), opts)
}

fn elab_report(d: &Design) -> SvElabReport {
    SvElabReport {
        top: d.name.clone(),
        cells: d.cells.len(),
        luts: d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
            .count(),
        ffs: d
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Hff))
            .count(),
    }
}

/// UG900 Elaboration: helion-sv flatten + FlowMap snapshot (not xelab).
pub fn elaborate_sv(
    source: &str,
    origin: &str,
    top: Option<&str>,
    params: &HashMap<String, u128>,
    opts: &SvCompileOpts,
) -> Result<(Design, SvElabReport), String> {
    let _ = (origin, opts);
    let d = synth_from_parsed_top(parse_source(source)?, top, params)?;
    let report = elab_report(&d);
    Ok((d, report))
}

pub fn elaborate_sv_path(
    path: &Path,
    top: Option<&str>,
    params: &HashMap<String, u128>,
    opts: &SvCompileOpts,
) -> Result<(Design, SvElabReport), String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    elaborate_sv(&src, &path.display().to_string(), top, params, opts)
}

pub fn elaborate_sv_sources(
    files: &[(&str, &str)],
    top: Option<&str>,
    params: &HashMap<String, u128>,
    opts: &SvCompileOpts,
) -> Result<(Design, SvElabReport), String> {
    if files.is_empty() {
        return Err("no sources".into());
    }
    let mut all = String::new();
    for (origin, src) in files {
        let _ = (origin, opts);
        all.push_str(src);
        all.push('\n');
    }
    let d = synth_from_parsed_top(parse_source(&all)?, top, params)?;
    let report = elab_report(&d);
    Ok((d, report))
}

/// Elaborate many SV files together. Each file is parsed by sv-parser, then
/// modules are merged so a top in file B can instantiate a child defined in file A.
pub fn synth_sv_sources(files: &[(&str, &str)]) -> Result<Design, String> {
    if files.is_empty() {
        return Err("no sources".into());
    }
    let mut all = String::new();
    for (origin, src) in files {
        let _ = origin;
        all.push_str(src);
        all.push_str("
");
    }
    synth_from_parsed(parse_source(&all)?)
}

pub fn synth_sv_files(paths: &[&Path]) -> Result<Design, String> {
    let mut owned: Vec<(String, String)> = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
        owned.push((p.display().to_string(), text));
    }
    let refs: Vec<(&str, &str)> = owned.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    synth_sv_sources(&refs)
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
        assert_eq!(d.instances.len(), 1, "{:?}", d.instances);
        assert_eq!(d.instances[0].name, "u0");
        assert_eq!(d.instances[0].module, "tog");
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

    #[test]
    fn generate_for_unrolls_four_inverters() {
        let src = r#"
module m(input logic clk, output logic led);
  logic [3:0] q;
  always_ff @(posedge clk) begin
    for (int i = 0; i < 4; i = i + 1) begin
      q[i] <= ~q[i];
    end
  end
  assign led = q[0];
endmodule
"#;
        let d = synth_sv(src, "g.sv").unwrap();
        assert_eq!(d.lut_inits().len(), 4);
        assert!(d.lut_inits().iter().all(|&i| i == 0x5555_5555_5555_5555));
    }

    #[test]
    fn keep_attr_sets_dont_touch() {
        let src = r#"
module m(input logic clk, output logic led);
  (* keep = "true" *) logic q;
  always_ff @(posedge clk) q <= ~q;
  assign led = q;
endmodule
"#;
        let d = synth_sv(src, "k.sv").unwrap();
        assert!(
            d.cells.iter().any(|c| c.attrs.flag("DONT_TOUCH")),
            "keep attribute must land on IR"
        );
    }

    #[test]
    fn mark_debug_attr_on_q() {
        let src = r#"
module m(input logic clk, output logic led);
  (* mark_debug = "true" *) logic q;
  always_ff @(posedge clk) q <= ~q;
  assign led = q;
endmodule
"#;
        let d = synth_sv(src, "md.sv").unwrap();
        assert!(
            d.marked_debug_nets().iter().any(|n| n == "q"),
            "{:?}",
            d.marked_debug_nets()
        );
    }

    #[test]
    fn module_parameter_propagates_to_width() {
        let child = r#"
module tog #(parameter N = 1) (input logic clk, output logic led);
  logic [N-1:0] q;
  always_ff @(posedge clk) begin
    for (int i = 0; i < N; i = i + 1) begin
      q[i] <= ~q[i];
    end
  end
  assign led = q[0];
endmodule
"#;
        let d1 = synth_sv(child, "t1.sv").unwrap();
        assert_eq!(d1.lut_inits().len(), 1, "default N=1 must be one LUT {:?}", d1.lut_inits());
        let top = r#"
module tog #(parameter N = 1) (input logic clk, output logic q);
  logic [N-1:0] r;
  always_ff @(posedge clk) begin
    for (int i = 0; i < N; i = i + 1) r[i] <= ~r[i];
  end
  assign q = r[0];
endmodule
module top(input logic clk, output logic led);
  tog #(.N(4)) u0(.clk(clk), .q(led));
endmodule
"#;
        let d4 = synth_sv(top, "t4.sv").unwrap();
        assert_eq!(
            d4.lut_inits().len(),
            4,
            "N=4 must unroll four inverters, got {:?}",
            d4.lut_inits()
        );
        assert!(d4.lut_inits().iter().all(|&i| i == 0x5555_5555_5555_5555));
    }

    #[test]
    fn generate_if_selects_real_branch() {
        let src = r#"
module m #(parameter USE_INC = 0) (input logic clk, output logic led);
  logic [3:0] q;
  generate
    if (USE_INC == 1) begin
      always_ff @(posedge clk) q <= q + 1;
    end else begin
      always_ff @(posedge clk) begin
        q[0] <= ~q[0];
        q[1] <= ~q[1];
        q[2] <= ~q[2];
        q[3] <= ~q[3];
      end
    end
  endgenerate
  assign led = q[3];
endmodule
"#;
        let inv = synth_sv(src, "g0.sv").unwrap();
        assert_eq!(inv.lut_inits().len(), 4);
        assert!(inv.lut_inits().iter().all(|&i| i == 0x5555_5555_5555_5555), "else branch is four inverters {:?}", inv.lut_inits());
        let inc_src = src.replace("USE_INC = 0", "USE_INC = 1");
        let inc = synth_sv(&inc_src, "g1.sv").unwrap();
        assert_eq!(inc.lut_inits(), INC4_INIT.to_vec(), "if branch must be the incrementer");
        assert_ne!(inv.lut_inits(), inc.lut_inits());
    }

    #[test]
    fn multi_file_cross_instantiation_is_real_cells() {
        let child = r#"
module child #(parameter W = 1) (input logic clk, output logic q);
  logic [W-1:0] r;
  always_ff @(posedge clk) begin
    for (int i = 0; i < W; i = i + 1) r[i] <= ~r[i];
  end
  assign q = r[0];
endmodule
"#;
        let top = r#"
module top(input logic clk, output logic led);
  child #(.W(4)) u0(.clk(clk), .q(led));
endmodule
"#;
        let d = synth_sv_sources(&[("child.sv", child), ("top.sv", top)]).unwrap();
        assert_eq!(d.name, "top");
        assert_eq!(
            d.lut_inits().len(),
            4,
            "cross-file child #(.W(4)) must produce 4 LUT cells, got {:?}",
            d.lut_inits()
        );
        // Concatenating in the wrong order (top first) must still find child.
        let d2 = synth_sv_sources(&[("top.sv", top), ("child.sv", child)]).unwrap();
        assert_eq!(d2.lut_inits().len(), 4);
    }

    #[test]
    fn clock_enable_occupies_lut_pin() {
        let bare = wrap("~q");
        let en = r#"
module m(input logic clk, input logic en, output logic led);
  logic q;
  always_ff @(posedge clk) begin
    if (en) q <= ~q;
  end
  assign led = q;
endmodule
"#;
        let a = lut_init_of(&bare).unwrap();
        let d = synth_sv(en, "en.sv").unwrap();
        let b = d.lut_inits()[0];
        assert_eq!(a, 0x5555_5555_5555_5555);
        assert_ne!(a, b, "enable must change INIT vs bare inverter {b:#x}");
        assert!(d.ports.iter().any(|p| p.name == "en"));
    }

    #[test]
    fn saturating_add_holds_at_max_in_fabric() {
        let src = r#"
module sat(input logic clk, output logic led);
  logic [3:0] cnt;
  always_ff @(posedge clk) begin
    if (cnt == 4'hF) cnt <= cnt;
    else cnt <= cnt + 1;
  end
  assign led = cnt[3];
endmodule
"#;
        let d = synth_sv(src, "sat.sv").unwrap();
        assert_ne!(
            d.lut_inits(),
            INC4_INIT.to_vec(),
            "sat compare must occupy LUT pins so INIT != bare incrementer"
        );
        let dev = helion_device::Device::load_part("HL10T-C32-1").unwrap();
        let p = helion_pack::pack(&d, &dev).unwrap();
        let pl = helion_place::place(&p, &dev).unwrap();
        let r = helion_route::route(&pl, &dev).unwrap();
        let bits = helion_bits::bitgen(&dev, &r).unwrap();
        let mut fab = helion_fabric::Fabric::new(&dev);
        fab.program(&bits).unwrap();
        fab.finish_startup();
        let iob = r.iob_src[0].iob;
        let mut w = Vec::new();
        for _ in 0..16 {
            fab.step_user();
            w.push(fab.led_at(iob.0, iob.1));
        }
        let bits: String = w.iter().map(|b| if *b { '1' } else { '0' }).collect();
        assert_eq!(
            bits, "0000000111111111",
            "saturating counter must stick at 15 (LED=1), not wrap: {bits}"
        );
    }

    #[test]
    fn casez_dont_care_differs_from_exact_case() {
        let z = r#"
module m(input logic clk, output logic led);
  logic [1:0] s;
  always_ff @(posedge clk) begin
    casez (s)
      2'b0?: s <= 2'b11;
      default: s <= 2'b00;
    endcase
  end
  assign led = s[1];
endmodule
"#;
        let c = r#"
module m(input logic clk, output logic led);
  logic [1:0] s;
  always_ff @(posedge clk) begin
    case (s)
      2'b00: s <= 2'b11;
      default: s <= 2'b00;
    endcase
  end
  assign led = s[1];
endmodule
"#;
        let dz = synth_sv(z, "z.sv").unwrap();
        let dc = synth_sv(c, "c.sv").unwrap();
        assert_ne!(
            dz.lut_inits(),
            dc.lut_inits(),
            "casez 2'b0? must match 00 and 01, not only 00: z={:?} c={:?}",
            dz.lut_inits(),
            dc.lut_inits()
        );
    }

    #[test]
    fn bram_init_appears_in_fabric_not_only_pack() {
        let src = r#"
module rom(input logic clk, output logic led);
  logic [7:0] mem [0:3];
  logic q;
  initial begin
    mem[0] = 8'hA5;
    mem[1] = 8'h3C;
    mem[2] = 8'h00;
    mem[3] = 8'hFF;
  end
  always_ff @(posedge clk) begin
    mem[0] <= mem[0];
    q <= ~q;
  end
  assign led = q;
endmodule
"#;
        let d = synth_sv(src, "rom.sv").unwrap();
        let bram = d
            .cells
            .iter()
            .find(|c| matches!(c.kind, CellKind::Bram18))
            .expect("must infer BRAM18");
        let init = bram.attrs.get("INIT").unwrap_or("");
        assert!(init.contains("a5"), "INIT attr must carry mem[0]=A5, got {init}");
        assert!(init.contains("3c"), "INIT attr must carry mem[1]=3C, got {init}");
        let dev = helion_device::Device::load_part("HL10T-C32-1").unwrap();
        let p = helion_pack::pack(&d, &dev).unwrap();
        assert_eq!(p.brams.len(), 1);
        assert_eq!(p.brams[0].init.get(0).copied().unwrap_or(0), 0xA5);
        assert_eq!(p.brams[0].init.get(1).copied().unwrap_or(0), 0x3C);
        let pl = helion_place::place(&p, &dev).unwrap();
        let r = helion_route::route(&pl, &dev).unwrap();
        let bits = helion_bits::bitgen(&dev, &r).unwrap();
        let mut fab = helion_fabric::Fabric::new(&dev);
        fab.program(&bits).unwrap();
        assert_eq!(fab.bram_init_word(0, 0), 0xA5, "fabric must see programmed INIT[0]");
        assert_eq!(fab.bram_init_word(0, 1), 0x3C, "fabric must see programmed INIT[1]");
        assert_eq!(fab.bram_init_word(0, 3), 0xFF);
        let src2 = src.replace("8'hA5", "8'h11").replace("8'h3C", "8'h22");
        let d2 = synth_sv(&src2, "rom2.sv").unwrap();
        let p2 = helion_pack::pack(&d2, &dev).unwrap();
        let pl2 = helion_place::place(&p2, &dev).unwrap();
        let r2 = helion_route::route(&pl2, &dev).unwrap();
        let bits2 = helion_bits::bitgen(&dev, &r2).unwrap();
        fab.program(&bits2).unwrap();
        assert_eq!(fab.bram_init_word(0, 0), 0x11);
        assert_eq!(fab.bram_init_word(0, 1), 0x22);
        assert_ne!(bits.frames, bits2.frames, "different ROM contents must change bitstream");
    }

    #[test]
    fn preprocess_skips_package_and_ifdef_macros() {
        let src = r#"
`define USE_Q
package p;
  typedef struct packed { logic a; } t;
endpackage
import p::*;
module m(input logic clk, output logic led);
  `ifdef USE_Q
  logic q;
  always_ff @(posedge clk) q <= ~q;
  assign led = q;
  `else
  assign led = 1'b0;
  `endif
endmodule
"#;
        let d = synth_sv(src, "pkg.sv").unwrap();
        assert_eq!(d.name, "m");
        assert_eq!(d.lut_inits(), vec![0x5555_5555_5555_5555]);
    }

    #[test]
    fn ysyx_ibex_lists_modules_and_synths_top() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ysyx_ibex.sv");
        assert!(p.exists(), "examples/ysyx_ibex.sv must ship with the crate tests");
        let src = std::fs::read_to_string(&p).unwrap();
        let mods = list_sv_modules_origin(&src, "ysyx_ibex.sv").expect("list ibex modules");
        assert!(
            mods.iter().any(|m| m == "ysyx_ibex"),
            "top ysyx_ibex must parse: {mods:?}"
        );
        assert!(
            mods.len() > 5,
            "Ibex-scale file must yield many modules, got {}",
            mods.len()
        );
        let d = synth_sv_path(&p).expect("synth ysyx_ibex");
        assert_eq!(d.name, "ysyx_ibex");
        assert!(
            !d.ports.is_empty(),
            "ysyx_ibex ports from ANSI list"
        );
    }
}
