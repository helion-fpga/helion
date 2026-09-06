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
        if w >= 64 {
            return pat;
        }
        let mask = (1u64 << w) - 1;
        let mut sh = 0;
        while sh < 64 {
            acc |= (pat & mask) << sh;
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
    /// Part-select `sig[hi:lo]` (inclusive). Bit i of the slice is `sig[lo+i]`.
    Range(String, usize, usize),
    /// Indexed part-select `sig[base +: width]` (ascending) or `sig[base -: width]`.
    IndexPart {
        name: String,
        base: Box<RExpr>,
        width: usize,
        ascending: bool,
    },
    /// Concatenation `{a,b,...}` (left = MSB). Replication `{N{e}}` may fold to Const.
    Concat(Vec<RExpr>),
    /// Logical right shift `a >> sh` (const shift amount in rexpr_to_bit; zero-fill).
    Shr(Box<RExpr>, Box<RExpr>),
    /// Arithmetic right shift `a >>> sh` (const shift; sign-fill from MSB).
    Ashr(Box<RExpr>, Box<RExpr>),
    /// Reduction XOR `^in` (1-bit result).
    RedXor(Box<RExpr>),
    /// Reduction AND `&in` (1-bit). `~&in` is Not(RedAnd(...)).
    RedAnd(Box<RExpr>),
    /// Reduction OR `|in` (1-bit). `~|in` is Not(RedOr(...)).
    RedOr(Box<RExpr>),
    Not(Box<RExpr>),
    And(Box<RExpr>, Box<RExpr>),
    Or(Box<RExpr>, Box<RExpr>),
    Xor(Box<RExpr>, Box<RExpr>),
    Add(Box<RExpr>, Box<RExpr>),
    Sub(Box<RExpr>, Box<RExpr>),
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
    Ge, // >=
    Eq, // ==
    Ne, // !=
    Lor, // ||
    Land, // &&
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
        if c == '>' && chars.get(i + 1) == Some(&'=') {
            out.push(Tok::Ge);
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
        if c == '|' && chars.get(i + 1) == Some(&'|') {
            out.push(Tok::Lor);
            i += 2;
            continue;
        }
        if c == '&' && chars.get(i + 1) == Some(&'&') {
            out.push(Tok::Land);
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
        if c == '\'' {
            i += 1;
            let mut base = 2;
            let mut saw_base = false;
            if i < chars.len() {
                match chars[i].to_ascii_lowercase() {
                    'b' => {
                        base = 2;
                        saw_base = true;
                        i += 1;
                    }
                    'h' => {
                        base = 16;
                        saw_base = true;
                        i += 1;
                    }
                    'd' => {
                        base = 10;
                        saw_base = true;
                        i += 1;
                    }
                    'o' => {
                        base = 8;
                        saw_base = true;
                        i += 1;
                    }
                    '0' | '1' => {}
                    _ => {
                        out.push(Tok::Sym('\''));
                        continue;
                    }
                }
            }
            let mut digits = String::new();
            while i < chars.len() {
                let ch = chars[i];
                if ch == '_' {
                    i += 1;
                    continue;
                }
                let ok = ch.is_ascii_hexdigit() || matches!(ch, 'x' | 'X' | 'z' | 'Z' | '?');
                if !ok {
                    break;
                }
                digits.push(ch);
                i += 1;
            }
            if digits.is_empty() {
                out.push(Tok::Sym('\''));
                continue;
            }
            let width = if saw_base { digits.len().max(1) } else { 1 };
            let val = u128::from_str_radix(
                &digits
                    .chars()
                    .map(|c| if matches!(c, 'x' | 'X' | 'z' | 'Z' | '?') { '0' } else { c })
                    .collect::<String>(),
                base,
            )
            .unwrap_or(0);
            out.push(Tok::Number(val, width));
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
                    if ch == '_' {
                        i += 1;
                        continue;
                    }
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

/// Ibex `ibex_pkg::regfile_e` literals — packages are skipped, so seed defaults
/// so `RegFile == RegFileFF` links the FF register file (~992 sequential bits).
fn enum_const_default(name: &str) -> Option<u128> {
    match name {
        "RegFileFF" => Some(0),
        "RegFileFPGA" => Some(1),
        "RegFileLatch" => Some(2),
        // Harmless Ibex param enums (packages skipped).
        "RV32MNone" => Some(0),
        "RV32MSlow" => Some(1),
        "RV32MFast" => Some(2),
        "RV32MSingleCycle" => Some(3),
        "RV32BNone" => Some(0),
        "RV32BBalanced" => Some(1),
        "RV32BOTEarlGrey" => Some(2),
        "RV32BFull" => Some(3),
        _ => None,
    }
}

fn clog2_u(n: u128) -> u128 {
    if n <= 1 {
        0
    } else {
        (u128::BITS - (n - 1).leading_zeros()) as u128
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
        Some(Tok::Ident(s)) if s == "$clog2" => {
            p.bump();
            if !p.eat_sym('(') {
                return Err("$clog2 (".into());
            }
            let arg = const_u(p)?;
            if !p.eat_sym(')') {
                return Err("$clog2 )".into());
            }
            Ok(clog2_u(arg))
        }
        Some(Tok::Ident(s)) => {
            let mut name = s.clone();
            p.bump();
            // `pkg::EnumLit` after skipped packages — resolve the member name.
            if matches!(p.peek(), Some(Tok::Sym(':')))
                && matches!(p.t.get(p.i + 1), Some(Tok::Sym(':')))
                && matches!(p.t.get(p.i + 2), Some(Tok::Ident(_)))
            {
                p.bump();
                p.bump();
                name = p.ident()?;
            }
            if let Some(v) = p.params.get(&name).copied() {
                return Ok(v);
            }
            enum_const_default(&name).ok_or_else(|| format!("unknown param {name}"))
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
            if p.eat_sym('*') {
                let e = const_atom(p)? as u32;
                v = v.saturating_pow(e.min(63));
            } else {
                v = v.saturating_mul(const_atom(p)?);
            }
        } else if p.eat_sym('/') {
            let d = const_atom(p)?.max(1);
            v /= d;
        } else if p.eat_sym('%') {
            let d = const_atom(p)?.max(1);
            v %= d;
        } else if p.eat_sym('<') {
            // `<<` or arithmetic `<<<` (both logical shifts on const u128).
            if !p.eat_sym('<') {
                // Lone '<' is relational — put it back for const_rel.
                p.i -= 1;
                break;
            }
            let _ = p.eat_sym('<'); // optional third < for <<<
            let sh = const_atom(p)? as u32;
            v = v.checked_shl(sh.min(63)).unwrap_or(0);
        } else {
            break;
        }
    }
    if p.eat_sym('?') {
        let t = const_u(p)?;
        if !p.eat_sym(':') {
            return Err("const ternary :".into());
        }
        let f = const_u(p)?;
        return Ok(if v != 0 { t } else { f });
    }
    Ok(v)
}

fn const_rel(p: &mut P) -> Result<bool, String> {
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
    if matches!(p.peek(), Some(Tok::Ge)) {
        p.bump();
        return Ok(l >= const_u(p)?);
    }
    if p.eat_sym('<') {
        return Ok(l < const_u(p)?);
    }
    if p.eat_sym('>') {
        return Ok(l > const_u(p)?);
    }
    Ok(l != 0)
}

/// Generate/parameter const condition: relational, then left-assoc `||` / `&&`.
fn const_cond(p: &mut P) -> Result<bool, String> {
    let mut v = const_rel(p)?;
    loop {
        match p.peek() {
            Some(Tok::Land) => {
                p.bump();
                let r = const_rel(p)?;
                v = v && r;
            }
            Some(Tok::Lor) => {
                p.bump();
                let r = const_rel(p)?;
                v = v || r;
            }
            _ => break,
        }
    }
    Ok(v)
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
    let e = parse_shift(p)?;
    if matches!(p.peek(), Some(Tok::Eq)) {
        p.bump();
        return Ok(RExpr::Eq(Box::new(e), Box::new(parse_shift(p)?)));
    }
    if matches!(p.peek(), Some(Tok::Ne)) {
        p.bump();
        return Ok(RExpr::Ne(Box::new(e), Box::new(parse_shift(p)?)));
    }
    // a <= b  ≡  !(b < a)
    if matches!(p.peek(), Some(Tok::Le)) {
        p.bump();
        let r = parse_shift(p)?;
        return Ok(RExpr::Not(Box::new(RExpr::Lt(Box::new(r), Box::new(e)))));
    }
    // a >= b  ≡  !(a < b)
    if matches!(p.peek(), Some(Tok::Ge)) {
        p.bump();
        let r = parse_shift(p)?;
        return Ok(RExpr::Not(Box::new(RExpr::Lt(Box::new(e), Box::new(r)))));
    }
    if p.eat_sym('<') {
        // Do not steal `<<` (left shift handled in parse_shift).
        if matches!(p.peek(), Some(Tok::Sym('<'))) {
            p.i -= 1;
            return Ok(e);
        }
        return Ok(RExpr::Lt(Box::new(e), Box::new(parse_shift(p)?)));
    }
    if matches!(p.peek(), Some(Tok::Sym('>')))
        && matches!(p.t.get(p.i + 1), Some(Tok::Sym('>')))
    {
        // `>>` / `>>>` belong to parse_shift (already consumed there); leave alone.
        return Ok(e);
    }
    if p.eat_sym('>') {
        // a > b  ≡  b < a
        return Ok(RExpr::Lt(Box::new(parse_shift(p)?), Box::new(e)));
    }
    Ok(e)
}

fn parse_shift(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_add(p)?;
    loop {
        if matches!(p.peek(), Some(Tok::Sym('>')))
            && matches!(p.t.get(p.i + 1), Some(Tok::Sym('>')))
        {
            p.bump();
            p.bump();
            // `>>>` = arithmetic right shift; `>>` = logical.
            let arith = p.eat_sym('>');
            let sh = parse_add(p)?;
            e = if arith {
                RExpr::Ashr(Box::new(e), Box::new(sh))
            } else {
                RExpr::Shr(Box::new(e), Box::new(sh))
            };
            continue;
        }
        // `<<` / `<<<` — const fold when both sides const; else leave as identity*shift via mul-of-power2 not needed for corpus.
        if matches!(p.peek(), Some(Tok::Sym('<')))
            && matches!(p.t.get(p.i + 1), Some(Tok::Sym('<')))
        {
            p.bump();
            p.bump();
            let _ = p.eat_sym('<'); // <<<
            let sh = parse_add(p)?;
            e = match (&e, &sh) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, .. },
                ) => {
                    let shv = (*b).min(63) as u32;
                    RExpr::Const {
                        val: a.checked_shl(shv).unwrap_or(0),
                        width: *wa,
                        care: *ca,
                    }
                }
                _ => e, // non-const shl: keep LHS (rare); prefer not to drop assign
            };
            continue;
        }
        break;
    }
    Ok(e)
}

fn parse_add(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_mul(p)?;
    loop {
        // Do not consume the '+'/'-' of indexed part-select `+:` / `-:`.
        if matches!(p.peek(), Some(Tok::Sym('+')))
            && matches!(p.t.get(p.i + 1), Some(Tok::Sym(':')))
        {
            break;
        }
        if matches!(p.peek(), Some(Tok::Sym('-')))
            && matches!(p.t.get(p.i + 1), Some(Tok::Sym(':')))
        {
            break;
        }
        if p.eat_sym('+') {
            let r = parse_mul(p)?;
            e = match (&e, &r) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, width: wb, care: cb },
                ) => RExpr::Const {
                    val: a.wrapping_add(*b),
                    width: (*wa).max(*wb),
                    care: *ca & *cb,
                },
                _ => RExpr::Add(Box::new(e), Box::new(r)),
            };
            continue;
        }
        if p.eat_sym('-') {
            let r = parse_mul(p)?;
            e = match (&e, &r) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, width: wb, care: cb },
                ) => RExpr::Const {
                    val: a.wrapping_sub(*b),
                    width: (*wa).max(*wb),
                    care: *ca & *cb,
                },
                _ => RExpr::Sub(Box::new(e), Box::new(r)),
            };
            continue;
        }
        break;
    }
    Ok(e)
}

fn parse_mul(p: &mut P) -> Result<RExpr, String> {
    let mut e = parse_or_r(p)?;
    loop {
        if p.eat_sym('*') {
            let r = parse_or_r(p)?;
            e = match (&e, &r) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, width: wb, care: cb },
                ) => RExpr::Const {
                    val: a.wrapping_mul(*b),
                    width: (*wa).max(*wb),
                    care: *ca & *cb,
                },
                _ => RExpr::Mul(Box::new(e), Box::new(r)),
            };
            continue;
        }
        if p.eat_sym('/') {
            let r = parse_or_r(p)?;
            e = match (&e, &r) {
                (
                    RExpr::Const { val: a, width: wa, care: ca },
                    RExpr::Const { val: b, width: wb, care: cb },
                ) if *b != 0 => RExpr::Const {
                    val: a / *b,
                    width: (*wa).max(*wb),
                    care: *ca & *cb,
                },
                _ => return Err("div needs const operands".into()),
            };
            continue;
        }
        break;
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
    // Unary minus: -a  ≡  0 - a
    if p.eat_sym('-') {
        let x = parse_un_r(p)?;
        return Ok(RExpr::Sub(
            Box::new(RExpr::Const {
                val: 0,
                width: 32,
                care: u128::MAX,
            }),
            Box::new(x),
        ));
    }
    // Unary reductions (distinct from binary &/|/ ^ in parse_*_r).
    if p.eat_sym('^') {
        return Ok(RExpr::RedXor(Box::new(parse_un_r(p)?)));
    }
    if p.eat_sym('&') {
        return Ok(RExpr::RedAnd(Box::new(parse_un_r(p)?)));
    }
    if p.eat_sym('|') {
        return Ok(RExpr::RedOr(Box::new(parse_un_r(p)?)));
    }
    parse_atom_r(p)
}

fn care_mask(width: usize) -> u128 {
    if width >= 128 {
        u128::MAX
    } else {
        (1u128 << width.max(1)) - 1
    }
}

fn parse_atom_r(p: &mut P) -> Result<RExpr, String> {
    if p.eat_sym('(') {
        let e = parse_rexpr(p)?;
        if !p.eat_sym(')') {
            return Err(")".into());
        }
        return Ok(e);
    }
    // Concat `{a,b}` or replication `{N{expr}}` (fold `{N{1'b0}}` to Const zero).
    if p.eat_sym('{') {
        let first = parse_rexpr(p)?;
        if p.eat_sym('{') {
            let inner = parse_rexpr(p)?;
            if !p.eat_sym('}') {
                return Err("replic }".into());
            }
            if !p.eat_sym('}') {
                return Err("replic }}".into());
            }
            let n = match first {
                RExpr::Const { val, .. } => val as usize,
                _ => return Err("replic count not const".into()),
            };
            let n = n.min(256);
            if matches!(inner, RExpr::Const { val: 0, .. }) {
                return Ok(RExpr::Const {
                    val: 0,
                    width: n.max(1),
                    care: care_mask(n.max(1)),
                });
            }
            if let RExpr::Const { val, width, care } = inner {
                let w = width.max(1);
                let mut acc = 0u128;
                let mut tw = 0usize;
                for _ in 0..n {
                    acc |= (val & care_mask(w)) << tw;
                    tw = tw.saturating_add(w);
                    if tw >= 128 {
                        break;
                    }
                }
                return Ok(RExpr::Const {
                    val: acc,
                    width: (n * w).max(1).min(128),
                    care: care_mask((n * w).max(1).min(128)) & if care == 0 { 0 } else { u128::MAX },
                });
            }
            let mut parts = Vec::with_capacity(n.max(1));
            for _ in 0..n.max(1) {
                parts.push(inner.clone());
            }
            return Ok(RExpr::Concat(parts));
        }
        let mut parts = vec![first];
        while p.eat_sym(',') {
            parts.push(parse_rexpr(p)?);
        }
        if !p.eat_sym('}') {
            return Err("concat }".into());
        }
        if parts.len() == 1 {
            return Ok(parts.pop().unwrap());
        }
        return Ok(RExpr::Concat(parts));
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
                // Indexed part-select: sig[base +: W] / sig[base -: W]
                // or range/bit. Prefer +: / -: before treating ':' as range.
                let base = parse_add(p)?;
                if p.eat_sym('+') && p.eat_sym(':') {
                    let w = const_u(p)? as usize;
                    if !p.eat_sym(']') {
                        return Err("]".into());
                    }
                    Ok(RExpr::IndexPart {
                        name,
                        base: Box::new(base),
                        width: w.max(1),
                        ascending: true,
                    })
                } else if p.eat_sym('-') && p.eat_sym(':') {
                    let w = const_u(p)? as usize;
                    if !p.eat_sym(']') {
                        return Err("]".into());
                    }
                    Ok(RExpr::IndexPart {
                        name,
                        base: Box::new(base),
                        width: w.max(1),
                        ascending: false,
                    })
                } else if p.eat_sym(':') {
                    let a = match base {
                        RExpr::Const { val, .. } => val as usize,
                        _ => return Err("range msb not const".into()),
                    };
                    let b = const_u(p)? as usize;
                    if !p.eat_sym(']') {
                        return Err("]".into());
                    }
                    let hi = a.max(b);
                    let lo = a.min(b);
                    // Parameter part-select e.g. SEED[STATE_WIDTH-1:0]
                    if let Some(v) = p.params.get(&name).copied() {
                        let w = (hi - lo + 1).max(1);
                        let mask = care_mask(w.min(128));
                        let val = (v >> lo) & mask;
                        return Ok(RExpr::Const {
                            val,
                            width: w,
                            care: mask,
                        });
                    }
                    Ok(RExpr::Range(name, lo, hi))
                } else {
                    if !p.eat_sym(']') {
                        return Err("]".into());
                    }
                    match base {
                        RExpr::Const { val, .. } => {
                            if let Some(v) = p.params.get(&name).copied() {
                                let bit = val as usize;
                                return Ok(RExpr::Const {
                                    val: (v >> bit) & 1,
                                    width: 1,
                                    care: 1,
                                });
                            }
                            Ok(RExpr::Bit(name, val as usize))
                        }
                        other => Ok(RExpr::IndexPart {
                            name,
                            base: Box::new(other),
                            width: 1,
                            ascending: true,
                        }),
                    }
                }
            } else if let Some(v) = p.params.get(&name).copied() {
                // Parameter used in an expression (e.g. sel*DW +: DW).
                Ok(RExpr::Const {
                    val: v,
                    width: 32,
                    care: u128::MAX,
                })
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
        // Bit, or const range name[hi:lo] (treated as full-vector assign).
        let save = p.i;
        if let Ok(hi) = const_u(p) {
            if p.eat_sym(':') {
                let _lo = const_u(p)?;
                if !p.eat_sym(']') {
                    return Err("]".into());
                }
                let _ = hi;
                return Ok((name, None));
            }
            if p.eat_sym(']') {
                return Ok((name, Some(hi as usize)));
            }
        }
        p.i = save;
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
        if p.peek().is_none() {
            break;
        }
        match parse_seq_item(p) {
            Ok(s) => v.extend(s),
            Err(_) => {
                let _ = skip_item_or_block(p);
            }
        }
        if !block {
            break;
        }
    }
    Ok(v)
}


/// Parse for-loop step: `i++`, `i += N`, or `i = i + N` (default step 1).
fn parse_for_step(p: &mut P) -> Result<usize, String> {
    let _ = p.ident(); // loop variable
    // i++
    if p.eat_sym('+') {
        if p.eat_sym('+') {
            return Ok(1);
        }
        // i += N
        if p.eat_sym('=') {
            let n = const_u(p)? as usize;
            return Ok(n.max(1));
        }
        return Err("for step +".into());
    }
    // i = i + N
    if p.eat_sym('=') {
        let _ = p.ident();
        if !p.eat_sym('+') {
            return Err("for step =+".into());
        }
        let n = match p.peek() {
            Some(Tok::Number(v, _)) => {
                let n = (*v as usize).max(1);
                p.bump();
                n
            }
            _ => 1,
        };
        return Ok(n);
    }
    Err("for step".into())
}

fn parse_for_unroll(p: &mut P) -> Result<Vec<Nba>, String> {
    if !p.eat_sym('(') {
        return Err("for (".into());
    }
    let _ = p.eat_kw("int");
    let _ = p.eat_kw("genvar");
    let _ = p.eat_kw("unsigned");
    let _ = p.eat_kw("signed");
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
    let step = parse_for_step(p)?;
    if !p.eat_sym(')') {
        return Err("for )".into());
    }
    let block = p.eat_kw("begin");
    if p.eat_sym(':') {
        let _ = p.ident();
    }
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
    let _ = p.eat_kw("unique");
    let _ = p.eat_kw("priority");
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
        // Never walk past a module boundary looking for endfunction/endtask.
        if kw != "endmodule"
            && matches!(
                p.peek(),
                Some(Tok::Kw(k)) if k == "endmodule" || k == "endpackage" || k == "module"
            )
        {
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
        let i0 = p.i;
        if p.eat_sym(')') {
            break;
        }
        if p.peek().is_none() {
            break;
        }
        let _ = p.eat_kw("parameter");
        let _ = p.eat_kw("localparam");
        skip_sv_type(p);
        if p.eat_sym('.') {
            let name = match p.ident() {
                Ok(n) => n,
                Err(_) => {
                    skip_until_arg_end(p);
                    let _ = p.eat_sym(',');
                    continue;
                }
            };
            if p.eat_sym('(') {
                match const_u(p) {
                    Ok(val) => {
                        if p.eat_sym(')') {
                            out.push((name, val));
                        } else {
                            skip_until_arg_end(p);
                            let _ = p.eat_sym(')');
                        }
                    }
                    Err(_) => {
                        skip_until_arg_end(p);
                        let _ = p.eat_sym(')');
                    }
                }
            }
        } else if matches!(p.peek(), Some(Tok::Ident(_))) {
            let name = p.ident().unwrap();
            skip_sv_type(p);
            if p.eat_sym('=') {
                match const_u(p) {
                    Ok(val) => {
                        // Register immediately so later defaults can ref earlier ones
                        // (`OW = 2 * DW`). Existing overrides in p.params win.
                        let effective = p.params.get(&name).copied().unwrap_or(val);
                        p.params.insert(name.clone(), effective);
                        out.push((name, effective));
                    }
                    Err(_) => skip_until_arg_end(p),
                }
            } else {
                skip_until_arg_end(p);
            }
        } else {
            match const_u(p) {
                Ok(val) => {
                    out.push((format!("#{pos}"), val));
                    pos += 1;
                }
                Err(_) => skip_until_arg_end(p),
            }
        }
        let _ = p.eat_sym(',');
        if p.i == i0 {
            p.bump();
        }
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
        match p.peek() {
            Some(Tok::Kw(k)) if k == "endmodule" || k == "endgenerate" || k == "endpackage" => {
                return Err("unterminated begin".into());
            }
            Some(Tok::Kw(k)) if k == "begin" => {
                p.bump();
                depth += 1;
            }
            Some(Tok::Kw(k)) if k == "end" => {
                p.bump();
                depth -= 1;
            }
            None => return Err("unterminated begin".into()),
            _ => {
                p.bump();
            }
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
                if depth == 0 {
                    break;
                }
                p.bump();
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            }
            Some(Tok::Kw(k)) if k == "endcase" || k == "endgenerate" || k == "endmodule" || k == "else" => {
                if depth == 0 || k == "endmodule" || k == "endgenerate" {
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

fn skip_to_semi(p: &mut P) {
    let mut d = 0i32;
    while p.peek().is_some() {
        if d == 0 && p.eat_sym(';') {
            return;
        }
        match p.peek() {
            Some(Tok::Sym('(' | '[' | '{')) => {
                d += 1;
                p.bump();
            }
            Some(Tok::Sym(')' | ']' | '}')) => {
                // Stray closers (e.g. after a failed part-select parse) must not
                // abort before ';' — that desyncs the module scanner and drops
                // later always_ff / instances (Ibex timer, etc.).
                if d == 0 {
                    p.bump();
                    continue;
                }
                d -= 1;
                p.bump();
            }
            Some(Tok::Kw(k)) if d == 0
                && matches!(k.as_str(), "end" | "endmodule" | "endgenerate" | "endcase" | "else") =>
            {
                return;
            }
            None => return,
            _ => {
                p.bump();
            }
        }
    }
}

fn skip_brackets(p: &mut P) {
    if !p.eat_sym('[') {
        return;
    }
    let mut d = 1i32;
    while d > 0 && p.peek().is_some() {
        if p.eat_sym('[') {
            d += 1;
        } else if p.eat_sym(']') {
            d -= 1;
        } else {
            p.bump();
        }
    }
}

fn skip_until_arg_end(p: &mut P) {
    let mut d = 0i32;
    while p.peek().is_some() {
        if d == 0
            && matches!(
                p.peek(),
                Some(Tok::Sym(',')) | Some(Tok::Sym(')')) | Some(Tok::Sym(';'))
            )
        {
            return;
        }
        match p.peek() {
            Some(Tok::Sym('(' | '[' | '{')) => {
                d += 1;
                p.bump();
            }
            Some(Tok::Sym(')' | ']' | '}')) => {
                if d == 0 {
                    return;
                }
                d -= 1;
                p.bump();
            }
            None => return,
            _ => {
                p.bump();
            }
        }
    }
}

/// Skip `@...` sensitivity. Returns true for combo `@*` / `@(*)` (no edge).
/// Async `posedge clk or negedge nreset` is treated like sync (edges ignored).
fn skip_event_control(p: &mut P) -> bool {
    let _ = p.eat_sym('@');
    if p.eat_sym('*') {
        return true;
    }
    if p.eat_sym('(') {
        let start = p.i;
        if p.eat_sym('*') && matches!(p.peek(), Some(Tok::Sym(')'))) {
            p.bump();
            return true;
        }
        p.i = start;
        let mut d = 1i32;
        let mut has_edge = false;
        while d > 0 && p.peek().is_some() {
            if p.eat_kw("posedge") || p.eat_kw("negedge") {
                has_edge = true;
            } else if p.eat_sym('(') {
                d += 1;
            } else if p.eat_sym(')') {
                d -= 1;
            } else {
                p.bump();
            }
        }
        return !has_edge;
    }
    false
}

fn skip_for_rest(p: &mut P) {
    if p.eat_sym('(') {
        let mut d = 1i32;
        while d > 0 && p.peek().is_some() {
            if p.eat_sym('(') {
                d += 1;
            } else if p.eat_sym(')') {
                d -= 1;
            } else {
                p.bump();
            }
        }
    }
    if p.eat_kw("begin") {
        if p.eat_sym(':') {
            let _ = p.ident();
        }
        let _ = skip_begin_end(p);
    } else {
        let _ = skip_item_or_block(p);
    }
}

fn skip_sv_type(p: &mut P) {
    loop {
        let start = p.i;
        if p.eat_kw("bit")
            || p.eat_kw("int")
            || p.eat_kw("logic")
            || p.eat_kw("wire")
            || p.eat_kw("reg")
            || p.eat_kw("signed")
            || p.eat_kw("unsigned")
            || p.eat_kw("byte")
            || p.eat_kw("integer")
            || p.eat_kw("automatic")
            || p.eat_kw("const")
            || p.eat_kw("var")
            || p.eat_kw("packed")
            || p.eat_kw("struct")
            || p.eat_kw("enum")
        {
            continue;
        }
        if matches!(p.peek(), Some(Tok::Ident(_))) {
            let next = p.t.get(p.i + 1);
            if matches!(next, Some(Tok::Sym(':'))) {
                let _ = p.ident();
                let _ = p.eat_sym(':');
                let _ = p.eat_sym(':');
                if matches!(p.peek(), Some(Tok::Ident(_)) | Some(Tok::Kw(_))) {
                    p.bump();
                }
                continue;
            }
            if matches!(next, Some(Tok::Ident(_)) | Some(Tok::Kw(_)) | Some(Tok::Sym('['))) {
                p.bump();
                continue;
            }
        }
        if matches!(p.peek(), Some(Tok::Sym('['))) {
            skip_brackets(p);
            continue;
        }
        if p.i == start {
            break;
        }
    }
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
                    while p.eat_sym('[') {
                        let mut d = 1i32;
                        while d > 0 && p.peek().is_some() {
                            if p.eat_sym('[') {
                                d += 1;
                            } else if p.eat_sym(']') {
                                d -= 1;
                            } else {
                                p.bump();
                            }
                        }
                    }
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
        if matches!(p.peek(), Some(Tok::Kw(k)) if k == "end" || k == "endmodule" || k == "endgenerate" || k == "module") {
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
            skip_sv_type(p);
            match p.ident() {
                Ok(name) => {
                    if p.eat_sym('=') {
                        match const_u(p) {
                            Ok(val) => {
                                p.params.entry(name.clone()).or_insert(val);
                                if !param_order.iter().any(|(n, _)| n == &name) {
                                    param_order.push((name, val));
                                }
                            }
                            Err(_) => skip_until_arg_end(p),
                        }
                    }
                    skip_to_semi(p);
                }
                Err(_) => skip_to_semi(p),
            }
            continue;
        }
        if p.eat_kw("if") {
            // Generate-if / else-if / else chain. Unknown const_cond → prefer else.
            let mut taken = false;
            loop {
                if !p.eat_sym('(') {
                    let _ = skip_item_or_block(p);
                    break;
                }
                // Unknown generate-if (e.g. package enums): prefer else/generic branch
                // so prim_flop → prim_generic_flop still maps real FFs.
                let yes = match const_cond(p) {
                    Ok(v) => v,
                    Err(_) => {
                        skip_until_arg_end(p);
                        false
                    }
                };
                if !p.eat_sym(')') {
                    skip_for_rest(p);
                    if p.eat_kw("else") {
                        skip_for_rest(p);
                    }
                    break;
                }
                let then_begin = p.eat_kw("begin");
                if p.eat_sym(':') {
                    let _ = p.ident();
                }
                if yes && !taken {
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
                    taken = true;
                } else if then_begin {
                    skip_begin_end(p)?;
                } else {
                    skip_item_or_block(p)?;
                }
                if !p.eat_kw("else") {
                    break;
                }
                // `else if (...)` — recurse as next arm of the chain.
                if p.eat_kw("if") {
                    continue;
                }
                let eb = p.eat_kw("begin");
                if p.eat_sym(':') {
                    let _ = p.ident();
                }
                if !taken {
                    if eb {
                        parse_module_items(
                            p, ports, signals, nbas, assigns, insts, param_order, pending_keep, pending_md, mem_inits,
                            "end",
                        )?;
                    } else {
                        skip_item_or_block(p)?;
                    }
                } else if eb {
                    skip_begin_end(p)?;
                } else {
                    skip_item_or_block(p)?;
                }
                break;
            }
            continue;
        }
        if p.eat_kw("for") {
            // Unroll: NBA body and/or instantiations. Skip still-unsupported bounds.
            let save = p.i;
            match parse_for_unroll_module(p, signals, nbas, insts, assigns, mem_inits) {
                Ok(()) => continue,
                Err(_) => {
                    p.i = save;
                    match parse_for_unroll(p) {
                        Ok(v) => {
                            nbas.extend(v);
                            continue;
                        }
                        Err(_) => {
                            p.i = save;
                            skip_for_rest(p);
                            continue;
                        }
                    }
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
            let w = match p.width_opt() {
                Ok(w) => w,
                Err(_) => {
                    skip_to_semi(p);
                    continue;
                }
            };
            let n = match p.ident() {
                Ok(n) => n,
                Err(_) => {
                    skip_to_semi(p);
                    continue;
                }
            };
            let mut depth = 0usize;
            if matches!(p.peek(), Some(Tok::Sym('['))) {
                let save = p.i;
                let _ = p.eat_sym('[');
                if let (Ok(hi), true, Ok(lo), true) =
                    (const_u(p), p.eat_sym(':'), const_u(p), p.eat_sym(']'))
                {
                    let hi = hi as usize;
                    let lo = lo as usize;
                    depth = hi.max(lo) - hi.min(lo) + 1;
                } else {
                    p.i = save;
                    skip_brackets(p);
                }
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
            if p.eat_sym('=') {
                skip_until_arg_end(p);
            }
            while p.eat_sym(',') {
                if let Ok(n2) = p.ident() {
                    if !signals.iter().any(|s| s.name == n2) {
                        signals.push(Signal {
                            name: n2,
                            width: w,
                            depth: 0,
                            keep: *pending_keep,
                            mark_debug: *pending_md,
                        });
                    }
                    if matches!(p.peek(), Some(Tok::Sym('['))) {
                        skip_brackets(p);
                    }
                    if p.eat_sym('=') {
                        skip_until_arg_end(p);
                    }
                } else {
                    break;
                }
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
            if p.eat_sym('{') {
                let mut names: Vec<String> = Vec::new();
                loop {
                    if p.eat_sym('}') {
                        break;
                    }
                    match p.ident() {
                        Ok(n) => names.push(n),
                        Err(_) => break,
                    }
                    let _ = p.eat_sym(',');
                }
                if p.eat_sym('=') {
                    match parse_rexpr(p) {
                        Ok(rhs) => {
                            let _ = p.eat_sym(';');
                            let widths: Vec<usize> = names
                                .iter()
                                .map(|n| {
                                    signals
                                        .iter()
                                        .find(|s| s.name == *n)
                                        .map(|s| s.width)
                                        .or_else(|| {
                                            ports
                                                .iter()
                                                .find(|(pn, _, _)| pn == n)
                                                .map(|(_, _, w)| *w)
                                        })
                                        .unwrap_or(1)
                                })
                                .collect();
                            let total: usize = widths.iter().sum();
                            let mut bit_hi = total;
                            for (n, w) in names.iter().zip(widths.iter()) {
                                bit_hi = bit_hi.saturating_sub(*w);
                                for i in 0..*w {
                                    let src_bit = bit_hi + i;
                                    let piece = if src_bit == 0 {
                                        rhs.clone()
                                    } else {
                                        RExpr::Shr(
                                            Box::new(rhs.clone()),
                                            Box::new(RExpr::Const {
                                                val: src_bit as u128,
                                                width: 32,
                                                care: u128::MAX,
                                            }),
                                        )
                                    };
                                    assigns.push((
                                        n.clone(),
                                        if *w == 1 { None } else { Some(i) },
                                        piece,
                                    ));
                                }
                            }
                        }
                        Err(_) => skip_to_semi(p),
                    }
                } else {
                    skip_to_semi(p);
                }
            } else {
                match (parse_lhs(&mut p), p.eat_sym('='), parse_rexpr(&mut p)) {
                    (Ok((lhs, bit)), true, Ok(rhs)) => {
                        let _ = p.eat_sym(';');
                        assigns.push((lhs, bit, rhs));
                    }
                    _ => skip_to_semi(p),
                }
            }
            continue;
        }
        if p.eat_kw("always_comb") {
            let block = p.eat_kw("begin");
            if p.eat_sym(':') {
                let _ = p.ident();
            }
            match parse_seq_block(&mut p, block) {
                Ok(stmts) => assigns.extend(stmts),
                Err(_) => {
                    let _ = skip_item_or_block(p);
                }
            }
            continue;
        }
        if p.eat_kw("always_ff") || p.eat_kw("always") || p.eat_kw("always_latch") {
            let combo = skip_event_control(p);
            let block = p.eat_kw("begin");
            if p.eat_sym(':') {
                let _ = p.ident();
            }
            match parse_seq_block(&mut p, block) {
                Ok(stmts) => {
                    if combo {
                        // `always @(*)` next-state etc. → comb assigns, not FFs.
                        assigns.extend(stmts);
                    } else {
                        // Includes async `posedge clk or negedge rst` (sync-reset mux).
                        nbas.extend(stmts);
                    }
                }
                Err(_) => {
                    let _ = skip_item_or_block(p);
                }
            }
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
        let before = p.i;
        let _ = skip_item_or_block(p);
        if p.i == before && p.peek().is_some() {
            let _ = p.bump();
        }
    }
    Ok(())
}

fn parse_for_unroll_module(
    p: &mut P,
    signals: &mut Vec<Signal>,
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
    let _ = p.eat_kw("unsigned");
    let _ = p.eat_kw("signed");
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
    let step = parse_for_step(p)?;
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
        // Local decls inside the generate-for must be uniquified per iteration
        // so vector widths survive and each copy maps to its own FF bits.
        let mut subst: HashMap<String, String> = HashMap::new();
        for mut sig in dummy_sigs {
            let fresh = format!("{}_{i}", sig.name);
            subst.insert(sig.name.clone(), fresh.clone());
            sig.name = fresh;
            if !signals.iter().any(|s| s.name == sig.name) {
                signals.push(sig);
            }
        }
        for mut inst in local_insts {
            inst.name = format!("{}_{i}", inst.name);
            for (port, net) in inst.conns.iter_mut() {
                let _ = port;
                if let Some(n) = subst.get(net) {
                    *net = n.clone();
                }
            }
            insts.push(inst);
        }
        for (lhs, bit, rhs) in local_nbas {
            let lhs = subst.get(&lhs).cloned().unwrap_or(lhs);
            nbas.push((lhs, bit, rewrite_rexpr(&rhs, &subst)));
        }
        for (lhs, bit, rhs) in local_assigns {
            let lhs = subst.get(&lhs).cloned().unwrap_or(lhs);
            assigns.push((lhs, bit, rewrite_rexpr(&rhs, &subst)));
        }
        i += step;
    }
    Ok(())
}

fn parse_inst_net(p: &mut P) -> Option<String> {
    while p.eat_sym('!') || p.eat_sym('~') {}
    if matches!(p.peek(), Some(Tok::Sym(')'))) {
        return None;
    }
    if p.eat_sym('{') {
        let mut d = 1i32;
        while d > 0 && p.peek().is_some() {
            if p.eat_sym('{') {
                d += 1;
            } else if p.eat_sym('}') {
                d -= 1;
            } else {
                p.bump();
            }
        }
        return None;
    }
    if matches!(p.peek(), Some(Tok::Number(_, _)) | Some(Tok::Pat { .. })) {
        p.bump();
        skip_until_arg_end(p);
        return None;
    }
    if matches!(p.peek(), Some(Tok::Ident(_))) {
        let mut name = p.ident().ok()?;
        if p.eat_sym(':') {
            let _ = p.eat_sym(':');
            if let Ok(n) = p.ident() {
                name = n;
            }
        }
        if p.eat_sym('[') {
            let mut d = 1i32;
            while d > 0 && p.peek().is_some() {
                if p.eat_sym('[') {
                    d += 1;
                } else if p.eat_sym(']') {
                    d -= 1;
                } else {
                    p.bump();
                }
            }
        }
        skip_until_arg_end(p);
        return Some(name);
    }
    skip_until_arg_end(p);
    None
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
        let i0 = p.i;
        if p.eat_sym(')') {
            break;
        }
        if p.peek().is_none() {
            break;
        }
        if p.eat_sym('.') {
            let port = match p.ident() {
                Ok(n) => n,
                Err(_) => {
                    skip_until_arg_end(p);
                    let _ = p.eat_sym(',');
                    continue;
                }
            };
            if p.eat_sym('(') {
                if let Some(net) = parse_inst_net(p) {
                    conns.push((port, net));
                }
                let _ = p.eat_sym(')');
            } else {
                conns.push((port.clone(), port));
            }
        } else if let Some(net) = parse_inst_net(p) {
            conns.push((format!("#{pos}"), net));
            pos += 1;
        }
        let _ = p.eat_sym(',');
        if p.i == i0 {
            p.bump();
        }
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

fn index_part_bit(
    name: &str,
    base: &RExpr,
    part_w: usize,
    ascending: bool,
    rtl: &Rtl,
    bit: usize,
) -> Result<Expr, String> {
    if bit >= part_w {
        return Ok(Expr::Const(false));
    }
    let wsrc = sig_width(rtl, name);
    if let RExpr::Const { val, .. } = base {
        let idx = if ascending {
            (*val as usize).saturating_add(bit)
        } else {
            (*val as usize).saturating_sub(bit)
        };
        return Ok(Expr::Var(bit_name(
            name,
            wsrc,
            idx.min(wsrc.saturating_sub(1)),
        )));
    }
    // sel * stride  → N-way mux of data[k*stride + bit]
    let (sel, stride) = match base {
        RExpr::Mul(a, b) => match (a.as_ref(), b.as_ref()) {
            (RExpr::Const { val, .. }, other) => (other, (*val as usize).max(1)),
            (other, RExpr::Const { val, .. }) => (other, (*val as usize).max(1)),
            _ => return Err("index mul needs const stride".into()),
        },
        other => {
            // width-1 dynamic bit: mux over possible indices 0..min(wsrc,16)
            let n = wsrc.min(16).max(1);
            let mut acc: Option<Expr> = None;
            for k in 0..n {
                let eq = cmp_eq_bits(
                    other,
                    &RExpr::Const {
                        val: k as u128,
                        width: 32,
                        care: u128::MAX,
                    },
                    rtl,
                    true,
                )?;
                let src = Expr::Var(bit_name(name, wsrc, k));
                let term = Expr::And(Box::new(eq), Box::new(src));
                acc = Some(match acc {
                    None => term,
                    Some(a) => Expr::Or(Box::new(a), Box::new(term)),
                });
            }
            return Ok(acc.unwrap_or(Expr::Const(false)));
        }
    };
    let n = (wsrc / stride).max(1).min(64);
    let mut acc: Option<Expr> = None;
    for k in 0..n {
        let eq = cmp_eq_bits(
            sel,
            &RExpr::Const {
                val: k as u128,
                width: 32,
                care: u128::MAX,
            },
            rtl,
            true,
        )?;
        let src_idx = if ascending {
            k * stride + bit
        } else {
            k * stride + (part_w - 1 - bit)
        };
        let src = Expr::Var(bit_name(name, wsrc, src_idx.min(wsrc.saturating_sub(1))));
        let term = Expr::And(Box::new(eq), Box::new(src));
        acc = Some(match acc {
            None => term,
            Some(a) => Expr::Or(Box::new(a), Box::new(term)),
        });
    }
    Ok(acc.unwrap_or(Expr::Const(false)))
}

fn rexpr_to_bit(e: &RExpr, rtl: &Rtl, bit: usize) -> Result<Expr, String> {
    match e {
        RExpr::Const { val, width, care } => {
            let _ = width;
            if bit >= 128 {
                return Ok(Expr::Const(false));
            }
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
        RExpr::Range(s, lo, hi) => {
            let w = sig_width(rtl, s);
            let idx = (*lo + bit).min(*hi).min(w.saturating_sub(1));
            Ok(Expr::Var(bit_name(s, w, idx)))
        }
        RExpr::IndexPart {
            name,
            base,
            width,
            ascending,
        } => index_part_bit(name, base, *width, *ascending, rtl, bit),
        RExpr::Concat(parts) => {
            let mut offset = 0usize;
            for part in parts.iter().rev() {
                let w = rexpr_width(part, rtl).max(1);
                if bit < offset + w {
                    return rexpr_to_bit(part, rtl, bit - offset);
                }
                offset = offset.saturating_add(w);
            }
            Ok(Expr::Const(false))
        }
        RExpr::Shr(a, sh) => {
            let shift = match sh.as_ref() {
                RExpr::Const { val, .. } => *val as usize,
                _ => return Err("shift amount not const".into()),
            };
            let wa = rexpr_width(a, rtl);
            if bit.saturating_add(shift) >= wa {
                Ok(Expr::Const(false))
            } else {
                rexpr_to_bit(a, rtl, bit + shift)
            }
        }
        RExpr::Ashr(a, sh) => {
            let shift = match sh.as_ref() {
                RExpr::Const { val, .. } => *val as usize,
                _ => return Err("ashr amount not const".into()),
            };
            let wa = rexpr_width(a, rtl).max(1);
            let src = bit.saturating_add(shift);
            if src >= wa {
                // Sign-fill from MSB.
                rexpr_to_bit(a, rtl, wa - 1)
            } else {
                rexpr_to_bit(a, rtl, src)
            }
        }
        RExpr::RedXor(x) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            let w = rexpr_width(x, rtl).max(1);
            let mut acc = rexpr_to_bit(x, rtl, 0)?;
            for i in 1..w {
                acc = Expr::Xor(Box::new(acc), Box::new(rexpr_to_bit(x, rtl, i)?));
            }
            Ok(acc)
        }
        RExpr::RedAnd(x) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            let w = rexpr_width(x, rtl).max(1);
            let mut acc = rexpr_to_bit(x, rtl, 0)?;
            for i in 1..w {
                acc = Expr::And(Box::new(acc), Box::new(rexpr_to_bit(x, rtl, i)?));
            }
            Ok(acc)
        }
        RExpr::RedOr(x) => {
            if bit != 0 {
                return Ok(Expr::Const(false));
            }
            let w = rexpr_width(x, rtl).max(1);
            let mut acc = rexpr_to_bit(x, rtl, 0)?;
            for i in 1..w {
                acc = Expr::Or(Box::new(acc), Box::new(rexpr_to_bit(x, rtl, i)?));
            }
            Ok(acc)
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
        RExpr::Sub(a, b) => sub_diff_bit(a, b, rtl, bit),
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
    // Bound nested Add expansion: naive per-bit re-entry is exponential in nesting depth
    // (FM-HEL-HANG-1539: Ibex probe hung after flatten in rexpr_to_bit over Add trees).
    if bit > 128 {
        return Err("adder bit too wide".into());
    }
    if rexpr_add_depth(a).saturating_add(rexpr_add_depth(b)) > 8 {
        return Err("adder nesting too deep".into());
    }
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

fn sub_diff_bit(a: &RExpr, b: &RExpr, rtl: &Rtl, bit: usize) -> Result<Expr, String> {
    // Bound like adder_sum_bit (Ibex hang caps).
    if bit > 128 {
        return Err("sub bit too wide".into());
    }
    if rexpr_add_depth(a).saturating_add(rexpr_add_depth(b)) > 8 {
        return Err("sub nesting too deep".into());
    }
    let mut borrow = Expr::Const(false);
    let mut diff = Expr::Const(false);
    for i in 0..=bit {
        let ai = rexpr_to_bit(a, rtl, i)?;
        let bi = rexpr_to_bit(b, rtl, i)?;
        let axb = Expr::Xor(Box::new(ai.clone()), Box::new(bi.clone()));
        diff = Expr::Xor(Box::new(axb.clone()), Box::new(borrow.clone()));
        // borrow_out = (~a & b) | (~(a^b) & borrow)
        let nota_b = Expr::And(Box::new(Expr::Not(Box::new(ai))), Box::new(bi));
        let eq_ab = Expr::Not(Box::new(axb));
        let eq_bor = Expr::And(Box::new(eq_ab), Box::new(borrow));
        borrow = Expr::Or(Box::new(nota_b), Box::new(eq_bor));
    }
    Ok(diff)
}

fn rexpr_add_depth(e: &RExpr) -> usize {
    match e {
        RExpr::Add(a, b) | RExpr::Sub(a, b) => 1 + rexpr_add_depth(a).max(rexpr_add_depth(b)),
        RExpr::Not(x) | RExpr::RedXor(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) => {
            rexpr_add_depth(x)
        }
        RExpr::Shr(a, b) | RExpr::Ashr(a, b) => rexpr_add_depth(a).max(rexpr_add_depth(b)),
        RExpr::Concat(parts) => parts.iter().map(rexpr_add_depth).max().unwrap_or(0),
        RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Mul(a, b)
        | RExpr::Eq(a, b)
        | RExpr::Ne(a, b)
        | RExpr::Lt(a, b) => rexpr_add_depth(a).max(rexpr_add_depth(b)),
        RExpr::Mux(c, t, f) => rexpr_add_depth(c)
            .max(rexpr_add_depth(t))
            .max(rexpr_add_depth(f)),
        RExpr::IndexPart { base, .. } => rexpr_add_depth(base),
        RExpr::Range(_, _, _) | RExpr::Bit(_, _) | RExpr::Ident(_) | RExpr::Const { .. } => 0,
    }
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
        RExpr::Range(_, lo, hi) => hi - lo + 1,
        RExpr::IndexPart { width, .. } => (*width).max(1),
        RExpr::Const { width, .. } => (*width).max(1),
        RExpr::Concat(parts) => parts.iter().map(|p| rexpr_width(p, rtl).max(1)).sum(),
        RExpr::Shr(a, _) | RExpr::Ashr(a, _) => rexpr_width(a, rtl),
        RExpr::RedXor(_)
        | RExpr::RedAnd(_)
        | RExpr::RedOr(_)
        | RExpr::Eq(_, _)
        | RExpr::Ne(_, _)
        | RExpr::Lt(_, _) => 1,
        RExpr::Not(x) => rexpr_width(x, rtl).min(1).max(1),
        RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Add(a, b)
        | RExpr::Sub(a, b)
        | RExpr::Mul(a, b) => rexpr_width(a, rtl).max(rexpr_width(b, rtl)),
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


/// Collapse `sig[hi:lo]` to Ident when used as a Mul operand (full-vector slice).
fn strip_range_to_ident(e: &RExpr) -> RExpr {
    match e {
        RExpr::Range(name, _, _) => RExpr::Ident(name.clone()),
        other => other.clone(),
    }
}

fn normalize_mul_operands(e: &RExpr) -> RExpr {
    match e {
        RExpr::Mul(a, b) => RExpr::Mul(
            Box::new(strip_range_to_ident(a)),
            Box::new(strip_range_to_ident(b)),
        ),
        RExpr::Add(a, b) => RExpr::Add(
            Box::new(normalize_mul_operands(a)),
            Box::new(normalize_mul_operands(b)),
        ),
        RExpr::Mux(c, t, f) => RExpr::Mux(
            Box::new((**c).clone()),
            Box::new(normalize_mul_operands(t)),
            Box::new(normalize_mul_operands(f)),
        ),
        other => other.clone(),
    }
}

fn expr_contains_mul(e: &RExpr) -> bool {
    match e {
        RExpr::Mul(_, _) => true,
        RExpr::Not(x) | RExpr::RedAnd(x) | RExpr::RedOr(x) | RExpr::RedXor(x) => expr_contains_mul(x),
        RExpr::And(a, b)
        | RExpr::Or(a, b)
        | RExpr::Xor(a, b)
        | RExpr::Add(a, b)
        | RExpr::Sub(a, b)
        | RExpr::Eq(a, b)
        | RExpr::Ne(a, b)
        | RExpr::Lt(a, b)
        | RExpr::Shr(a, b)
        | RExpr::Ashr(a, b) => expr_contains_mul(a) || expr_contains_mul(b),
        RExpr::Mux(c, t, f) => {
            expr_contains_mul(c) || expr_contains_mul(t) || expr_contains_mul(f)
        }
        RExpr::Concat(parts) => parts.iter().any(expr_contains_mul),
        // Address math in sig[base*+:W] is not a DSP multiply.
        RExpr::IndexPart { .. } => false,
        _ => false,
    }
}


/// Pull (A, B, optional C-accum) port/signal names from a MAC/mul RHS tree.
fn mac_ab_c_from_rhs(e: &RExpr) -> (Option<String>, Option<String>, Option<String>) {
    let e = normalize_mul_operands(e);
    fn mul_ab(e: &RExpr) -> Option<(String, String)> {
        match e {
            RExpr::Mul(a, b) => Some((
                rexpr_ident(a).or_else(|| match a.as_ref() {
                    RExpr::Range(n, _, _) | RExpr::Bit(n, _) => Some(n.clone()),
                    _ => None,
                })?,
                rexpr_ident(b).or_else(|| match b.as_ref() {
                    RExpr::Range(n, _, _) | RExpr::Bit(n, _) => Some(n.clone()),
                    _ => None,
                })?,
            )),
            RExpr::Add(l, r) => mul_ab(l).or_else(|| mul_ab(r)),
            RExpr::Mux(_, t, f) => mul_ab(t).or_else(|| mul_ab(f)),
            _ => None,
        }
    }
    fn accum_c(e: &RExpr, a: &str, b: &str) -> Option<String> {
        match e {
            RExpr::Add(l, r) => {
                let li = rexpr_ident(l).or_else(|| match l.as_ref() {
                    RExpr::Range(n, _, _) | RExpr::Bit(n, _) => Some(n.clone()),
                    _ => None,
                });
                let ri = rexpr_ident(r).or_else(|| match r.as_ref() {
                    RExpr::Range(n, _, _) | RExpr::Bit(n, _) => Some(n.clone()),
                    _ => None,
                });
                if matches!(r.as_ref(), RExpr::Mul(_, _)) {
                    return li.filter(|n| n != a && n != b);
                }
                if matches!(l.as_ref(), RExpr::Mul(_, _)) {
                    return ri.filter(|n| n != a && n != b);
                }
                accum_c(l, a, b).or_else(|| accum_c(r, a, b))
            }
            RExpr::Mux(_, t, f) => accum_c(t, a, b).or_else(|| accum_c(f, a, b)),
            _ => None,
        }
    }
    match mul_ab(&e) {
        Some((a, b)) => {
            let c = accum_c(&e, &a, &b);
            (Some(a), Some(b), c)
        }
        None => (None, None, None),
    }
}

/// Infer Mac27 and wire A/B/(C)/P so schematic/HNF are not pin-NC islands.
fn emit_mac27(d: &mut Design, cell: String, rhs: &RExpr, lhs: &str) {
    d.add_cell(&cell, CellKind::Mac27);
    let (a, b, c) = mac_ab_c_from_rhs(rhs);
    if let Some(a) = a {
        d.connect(&a, &cell, "A");
    }
    if let Some(b) = b {
        d.connect(&b, &cell, "B");
    }
    if let Some(c) = c {
        d.connect(&c, &cell, "C");
    }
    d.connect(lhs, &cell, "P");
}

fn is_mac_rhs(e: &RExpr) -> bool {
    let e = normalize_mul_operands(e);
    match &e {
        RExpr::Mul(_, _) => true,
        // `a*b`, `a*b+c`, `c+a*b`, or enable/clear mux wrapping a MAC.
        RExpr::Add(l, r) => {
            matches!(l.as_ref(), RExpr::Mul(_, _))
                || matches!(r.as_ref(), RExpr::Mul(_, _))
                || expr_contains_mul(l)
                || expr_contains_mul(r)
        }
        RExpr::Mux(_, t, f) => expr_contains_mul(t) || expr_contains_mul(f),
        _ => expr_contains_mul(&e),
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


/// LUT6 INIT for a 2-input AND of (I0^a_inv) & (I1^b_inv), then XOR o_inv.
/// 4-bit pattern is replicated across 64 bits (unused I2..I5).
fn lut6_and2(a_inv: bool, b_inv: bool, o_inv: bool) -> u64 {
    let mut pat = 0u64;
    for addr in 0..4u32 {
        let i0 = addr & 1 == 1;
        let i1 = addr & 2 == 2;
        let v = ((i0 ^ a_inv) && (i1 ^ b_inv)) ^ o_inv;
        if v {
            pat |= 1u64 << addr;
        }
    }
    let mut acc = 0u64;
    let mut sh = 0;
    while sh < 64 {
        acc |= (pat & 0xf) << sh;
        sh += 4;
    }
    acc
}

fn lut6_inv() -> u64 {
    0x5555_5555_5555_5555
}

fn lut6_const(one: bool) -> u64 {
    if one {
        u64::MAX
    } else {
        0
    }
}

/// Map a >6-PI cone to a LUT2 tree. Output net is the function of `aig`.
/// PI<=6 stays on the single-LUT6 path so gold INIT patterns do not move.
fn map_wide_cone(d: &mut Design, aig: &Aig, prefix: &str) -> String {
    use std::collections::HashMap;
    let mut node_net: HashMap<u32, String> = HashMap::new();
    let mut n_lut = 0usize;
    fn emit_lut(d: &mut Design, prefix: &str, n_lut: &mut usize, init: u64, ins: &[(&str, &str)]) -> String {
        let cell = format!("{prefix}l{n_lut}");
        let out = format!("{prefix}n{n_lut}");
        *n_lut += 1;
        d.add_cell(&cell, CellKind::Lut6 { init });
        d.connect(&out, &cell, "O");
        for (net, pin) in ins {
            d.connect(*net, &cell, *pin);
        }
        out
    }
    fn lit_net(
        d: &mut Design,
        aig: &Aig,
        lit: Lit,
        prefix: &str,
        n_lut: &mut usize,
        node_net: &mut HashMap<u32, String>,
    ) -> String {
        let n = node_true(d, aig, lit.node, prefix, n_lut, node_net);
        if !lit.inv {
            return n;
        }
        emit_lut(d, prefix, n_lut, lut6_inv(), &[(&n, "I0")])
    }
    fn node_true(
        d: &mut Design,
        aig: &Aig,
        node: u32,
        prefix: &str,
        n_lut: &mut usize,
        node_net: &mut HashMap<u32, String>,
    ) -> String {
        if node == 0 {
            return emit_lut(d, prefix, n_lut, lut6_const(false), &[]);
        }
        if (node as usize) <= aig.pis.len() {
            return aig.pis[(node as usize) - 1].clone();
        }
        if let Some(n) = node_net.get(&node) {
            return n.clone();
        }
        let ai = (node as usize) - 1 - aig.pis.len();
        let (a, b) = aig.ands[ai];
        let na = lit_net(d, aig, a, prefix, n_lut, node_net);
        let nb = lit_net(d, aig, b, prefix, n_lut, node_net);
        let out = emit_lut(
            d,
            prefix,
            n_lut,
            lut6_and2(false, false, false),
            &[(&na, "I0"), (&nb, "I1")],
        );
        node_net.insert(node, out.clone());
        out
    }
    lit_net(d, aig, aig.output, prefix, &mut n_lut, &mut node_net)
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
            emit_mac27(&mut d, format!("u_mac{n_mac}"), rhs, lhs);
            n_mac += 1;
            continue;
        }
        let w = sig_width(rtl, lhs);
        if let Some(b) = bit {
            let e = if rexpr_is_plus_one(rhs) {
                Some(inc_bit_expr(lhs, w, *b))
            } else {
                rexpr_to_bit(rhs, rtl, 0).ok()
            };
            if let Some(e) = e {
                reg_bits.push((bit_name(lhs, w, *b), e));
            }
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
                    let Ok(c) = rexpr_to_bit(cond, rtl, 0) else { break; };
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
                    let Ok(c) = rexpr_to_bit(cond, rtl, 0) else { break; };
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
                    let Ok(c) = rexpr_to_bit(cond, rtl, 0) else { break; };
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
                    if let Ok(e) = rexpr_to_bit(rhs, rtl, i) {
                        reg_bits.push((bit_name(lhs, w, i), e));
                    }
                }
            }
        } else {
            for i in 0..w {
                if let Ok(e) = rexpr_to_bit(rhs, rtl, i) {
                    reg_bits.push((bit_name(lhs, w, i), e));
                }
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

    // Continuous assigns → comb LUT cones (pure-comb modules e.g. mux).
    // Skip simple Ident/Bit/Range drives — those stay on the IOB passthrough path
    // so sequential timing (WNS) is not broken by orphan comb LUTs.
    let mut comb_bits: Vec<(String, Expr)> = Vec::new();
    for (lhs, bit, rhs) in &rtl.assigns {
        match rhs {
            RExpr::Ident(_) | RExpr::Bit(_, _) | RExpr::Range(_, _, _) => continue,
            _ => {}
        }
        // Comb multiply → DSP MAC (do not bitblast).
        if expr_contains_mul(rhs) {
            emit_mac27(&mut d, format!("u_mac{n_mac}"), rhs, lhs);
            n_mac += 1;
            continue;
        }
        let w = sig_width(rtl, lhs);
        if let Some(b) = bit {
            if let Ok(e) = rexpr_to_bit(rhs, rtl, 0) {
                comb_bits.push((bit_name(lhs, w, *b), e));
            }
        } else {
            let rw = rexpr_width(rhs, rtl).min(w).max(1).min(256);
            for i in 0..rw.min(w) {
                if let Ok(e) = rexpr_to_bit(rhs, rtl, i) {
                    comb_bits.push((bit_name(lhs, w, i), e));
                }
            }
        }
    }

    if reg_bits.is_empty() && comb_bits.is_empty() && n_mac == 0 && n_bram == 0 {
        // Unsupported items were skipped/blackboxed; still return the top with ports.
        return Ok(d);
    }

    let single_q = reg_bits.len() == 1 && reg_bits[0].0 == "q";
    for (i, (bitn, expr)) in reg_bits.iter().enumerate() {
        let aig = Aig::from_expr(expr);
        if aig.pis.len() > 6 {
            let (ff, qnet) = if single_q {
                ("u_ff".to_string(), "q".to_string())
            } else {
                (format!("u_ff{i}"), bitn.clone())
            };
            let wide = map_wide_cone(&mut d, &aig, &format!("u_w{i}_"));
            d.add_cell(&ff, CellKind::Hff);
            d.connect(clk, &ff, "CLK");
            d.connect(&wide, &ff, "D");
            d.connect(&qnet, &ff, "Q");
            continue;
        }
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

    for (i, (bitn, expr)) in comb_bits.iter().enumerate() {
        let aig = Aig::from_expr(expr);
        if aig.pis.len() > 6 {
            let wide = map_wide_cone(&mut d, &aig, &format!("u_cw{i}_"));
            // Alias wide cone output onto the assign net name.
            d.add_cell(format!("u_cbuf{i}"), CellKind::Lut6 { init: 0x2 });
            d.connect(&wide, format!("u_cbuf{i}"), "I0");
            d.connect(bitn, format!("u_cbuf{i}"), "O");
            continue;
        }
        let init = aig.flowmap_lut6();
        let lut = format!("u_clut{i}");
        d.add_cell(&lut, CellKind::Lut6 { init });
        d.connect(bitn, &lut, "O");
        for (pin, pi) in aig.pis.iter().enumerate() {
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
        let w = sig_width(rtl, lhs);
        if bit.is_none() && w > 1 {
            for i in 0..w.min(256) {
                let qnet = bit_name(lhs, w, i);
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
            continue;
        }
        let (qnet, _) = if let Some(b) = bit {
            (bit_name(lhs, w, *b), *b)
        } else {
            match drive_target(rhs, rtl) {
                Ok(x) => x,
                Err(_) => continue,
            }
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
        RExpr::Range(s, lo, hi) => RExpr::Range(id(s), *lo, *hi),
        RExpr::IndexPart {
            name,
            base,
            width,
            ascending,
        } => RExpr::IndexPart {
            name: id(name),
            base: Box::new(rewrite_rexpr(base, subst)),
            width: *width,
            ascending: *ascending,
        },
        RExpr::Concat(parts) => RExpr::Concat(
            parts.iter().map(|p| rewrite_rexpr(p, subst)).collect(),
        ),
        RExpr::Shr(a, b) => RExpr::Shr(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::Ashr(a, b) => RExpr::Ashr(
            Box::new(rewrite_rexpr(a, subst)),
            Box::new(rewrite_rexpr(b, subst)),
        ),
        RExpr::RedXor(x) => RExpr::RedXor(Box::new(rewrite_rexpr(x, subst))),
        RExpr::RedAnd(x) => RExpr::RedAnd(Box::new(rewrite_rexpr(x, subst))),
        RExpr::RedOr(x) => RExpr::RedOr(Box::new(rewrite_rexpr(x, subst))),
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
        RExpr::Sub(a, b) => RExpr::Sub(
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
    fn logikbench_mux_maps_luts() {
        let src = r#"
module mux #(parameter DW = 8, parameter N = 4)
(
    input [$clog2(N)-1:0] sel,
    input [N*DW-1:0] data,
    output [DW-1:0] out
);
   assign out[DW-1:0] = data[sel*DW +: DW];
endmodule
"#;
        let d = synth_sv(src, "mux").expect("synth");
        let cells = d.cells.len();
        assert!(cells > 0, "expected comb LUTs for mux, cells={cells}");
    }

    #[test]
    fn clog2_const_works() {
        assert_eq!(clog2_u(1), 0);
        assert_eq!(clog2_u(2), 1);
        assert_eq!(clog2_u(16), 4);
        assert_eq!(clog2_u(17), 5);
    }

    #[test]
    fn fsm_tiny_maps_state_and_out_regs() {
        // Minimal LogikBench parametric FSM shape: async reset, replication
        // concat, >> shift, reduction XOR, $clog2 localparam.
        let src = r#"
module fsm_tiny #(parameter STATES = 4, parameter DW = 4, parameter [31:0] SEED = 32'hA5A5A5A5)
(
    input clk,
    input nreset,
    input [DW-1:0] in,
    output reg [DW-1:0] out
);
   localparam STATE_WIDTH = $clog2(STATES);
   reg [STATE_WIDTH-1:0] current_state;
   reg [STATE_WIDTH-1:0] next_state;
   always @(posedge clk or negedge nreset)
     if (!nreset)
       current_state <= {STATE_WIDTH{1'b0}};
     else
       current_state <= next_state;
   always @(*)
     case (current_state)
       {STATE_WIDTH{1'b0}}: next_state = in[0] ? (STATES - 1) : 1'b1;
       ((STATES/2) - 1): next_state = (^in) ? {STATE_WIDTH{1'b1}} : {STATE_WIDTH{1'b0}};
       (STATES - 1): next_state = in ^ (SEED[STATE_WIDTH-1:0] >> 1);
       default: next_state = (current_state ^ SEED[STATE_WIDTH-1:0]) + in;
     endcase
   always @(posedge clk or negedge nreset)
     if (!nreset)
       out <= {DW{1'b0}};
     else
       out <= (current_state ^ (current_state >> 2)) + in;
endmodule
"#;
        let d = synth_sv(src, "fsm_tiny.sv").expect("fsm_tiny synth");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        eprintln!("fsm_tiny cells={} luts={luts} ffs={ffs}", d.cells.len());
        assert!(ffs >= 6, "expect state+out regs, ffs={ffs}");
        assert!(luts >= 4, "expect next-state/out LUTs, luts={luts}");
        assert!(d.cells.len() > 17, "must exceed pre-fix ~17 cell baseline, got {}", d.cells.len());
    }

    #[test]
    fn unary_reductions_band_bxor_bnand() {
        for (name, body) in [
            ("band", "assign out = &in;"),
            ("bxor", "assign out = ^in;"),
            ("bnand", "assign out = ~&in;"),
        ] {
            let src = format!(
                "module {name} #(parameter DW = 64) (input [DW-1:0] in, output out);\n  {body}\nendmodule\n"
            );
            let d = synth_sv(&src, &format!("{name}.sv")).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(
                !d.cells.is_empty(),
                "{name} must map cells>0, got {}",
                d.cells.len()
            );
        }
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
    fn logikbench_mac_with_clear_en_infers_mac27() {
        let src = r#"
module mac #(parameter DW = 16, parameter OW = 40) (
  input clk, input clear, input en,
  input [DW-1:0] a, input [DW-1:0] b,
  output reg [OW-1:0] c
);
  always @(posedge clk) begin
    if (clear) c <= 0;
    else if (en) c <= c + a * b;
  end
endmodule
"#;
        let d = synth_sv(src, "mac.sv").expect("synth");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "clear/en MAC must map Mac27, cells={}",
            d.cells.len()
        );
        assert_eq!(d.net_on("u_mac0", "A"), Some("a"));
        assert_eq!(d.net_on("u_mac0", "B"), Some("b"));
        assert_eq!(d.net_on("u_mac0", "C"), Some("c"));
        assert_eq!(d.net_on("u_mac0", "P"), Some("c"));
    }

    
    #[test]
    fn dependent_param_ow_equals_two_times_dw() {
        let src = r#"
module mul #(parameter DW = 8, parameter OW = 2 * DW) (
  input [DW-1:0] a, input [DW-1:0] b, output [OW-1:0] out
);
  assign out = a * b;
endmodule
"#;
        let d = synth_sv(src, "mul.sv").expect("synth");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "OW=2*DW mul must map Mac27, cells={}",
            d.cells.len()
        );
    }


#[test]
    fn comb_mul_assign_infers_mac27() {
        let src = r#"
module mul #(parameter DW = 8, parameter OW = 2 * DW) (
  input [DW-1:0] a, input [DW-1:0] b, output [OW-1:0] out
);
  assign out[OW-1:0] = a[DW-1:0] * b[DW-1:0];
endmodule
"#;
        let d = synth_sv(src, "mul.sv").expect("synth");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "comb a*b must map Mac27, cells={}",
            d.cells.len()
        );
        assert_eq!(d.net_on("u_mac0", "A"), Some("a"));
        assert_eq!(d.net_on("u_mac0", "B"), Some("b"));
        assert_eq!(d.net_on("u_mac0", "P"), Some("out"));
    }

    
    #[test]
    fn logikbench_mul_ranged_assign_infers_mac27() {
        let src = r#"
module mul #(parameter DW = 16, parameter OW = 2 * DW)
   (
    input [DW-1:0]  a,
    input [DW-1:0]  b,
    output [OW-1:0] out
    );
	assign out[OW-1:0] = a[DW-1:0] * b[DW-1:0];
endmodule
"#;
        let d = synth_sv(src, "mul.sv").expect("synth");
        assert!(
            d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "ranged comb mul must map Mac27, cells={}",
            d.cells.len()
        );
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
    fn const_cond_or_and_left_assoc() {
        // Ibex-style: RV32M == RV32MFast || RV32M == RV32MSingleCycle
        let toks = tokenize("RV32M == RV32MFast || RV32M == RV32MSingleCycle").unwrap();
        let mut params = HashMap::new();
        params.insert("RV32M".into(), 2u128); // RV32MFast
        let mut p = P {
            t: &toks,
            i: 0,
            params,
        };
        assert_eq!(
            const_cond(&mut p).unwrap(),
            true,
            "RV32MFast must match Fast||SingleCycle"
        );
        assert!(p.peek().is_none(), "must consume full || expr");

        let toks = tokenize("RV32M == RV32MFast || RV32M == RV32MSingleCycle").unwrap();
        let mut params = HashMap::new();
        params.insert("RV32M".into(), 0u128); // RV32MNone
        let mut p = P {
            t: &toks,
            i: 0,
            params,
        };
        assert_eq!(
            const_cond(&mut p).unwrap(),
            false,
            "RV32MNone must not match Fast||SingleCycle"
        );

        let toks = tokenize("A==1 && A==2").unwrap();
        let mut params = HashMap::new();
        params.insert("A".into(), 1u128);
        let mut p = P {
            t: &toks,
            i: 0,
            params,
        };
        assert_eq!(const_cond(&mut p).unwrap(), false, "A==1 && A==2 with A=1");

        let toks = tokenize("A==2 || A==3 && A==2").unwrap();
        let mut params = HashMap::new();
        params.insert("A".into(), 2u128);
        let mut p = P {
            t: &toks,
            i: 0,
            params,
        };
        // left-assoc: (A==2 || A==3) && A==2 → true
        assert_eq!(const_cond(&mut p).unwrap(), true);
    }

    #[test]
    fn generate_else_if_selects_branch() {
        // Maps FFs only on the Fast||SingleCycle arm (Ibex multdiv generate shape).
        let src = r#"
module m #(parameter int RV32M = RV32MFast) (
  input logic clk, output logic [3:0] q
);
  if (RV32M == RV32MSlow) begin
    assign q = 4'h0;
  end else if (RV32M == RV32MFast || RV32M == RV32MSingleCycle) begin
    logic [3:0] r;
    always_ff @(posedge clk) r <= ~r;
    assign q = r;
  end else begin
    assign q = 4'hF;
  end
endmodule
"#;
        let d = synth_sv(src, "elif.sv").unwrap();
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs, 4, "Fast||SingleCycle else-if must map 4 FFs, got {ffs}");
        assert_eq!(d.lut_inits().len(), 4);
        assert!(
            d.lut_inits().iter().all(|&i| i == 0x5555_5555_5555_5555),
            "Fast branch must be invertors {:?}",
            d.lut_inits()
        );

        let none = src.replace("RV32M = RV32MFast", "RV32M = RV32MNone");
        let d0 = synth_sv(&none, "elif_none.sv").unwrap();
        let ffs0 = d0.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs0, 0, "RV32MNone must take final else assigns only, got {ffs0}");
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
    fn timer_full_maps_ffs() {
        // Slice the in-tree Ibex timer module (mtime is 64b).
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ysyx_ibex.sv");
        let all = std::fs::read_to_string(&p).expect("ysyx_ibex.sv");
        let start = all.find("module timer #(").expect("timer module");
        let rest = &all[start..];
        let end = rest.find("\nendmodule").expect("timer endmodule") + "\nendmodule".len();
        let src = &rest[..end];
        let d = synth_sv(src, "timer.sv").expect("timer");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        eprintln!("timer_full ffs={ffs} cells={}", d.cells.len());
        assert!(ffs >= 64, "timer must map mtime FFs, got {ffs}");
    }

    #[test]
    fn nested_add_rexpr_is_bounded_not_hang() {
        // Pathological nesting used to explode adder_sum_bit (HANG-1539).
        let mut e = RExpr::Ident("a".into());
        for _ in 0..12 {
            e = RExpr::Add(Box::new(e), Box::new(RExpr::Ident("b".into())));
        }
        let rtl = Rtl {
            module: "t".into(),
            ports: vec![],
            signals: vec![
                Signal {
                    name: "a".into(),
                    width: 8,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                },
                Signal {
                    name: "b".into(),
                    width: 8,
                    depth: 0,
                    keep: false,
                    mark_debug: false,
                },
            ],
            nbas: vec![],
            assigns: vec![],
            insts: vec![],
            params: vec![],
            toks: vec![],
            mem_inits: Default::default(),
        };
        let r = rexpr_to_bit(&e, &rtl, 7);
        assert!(r.is_err(), "deep Add must Err, not hang: {r:?}");
    }

    #[test]
    fn genvar_plusplus_unrolls_flops() {
        let src = r#"
module rf #(parameter int NUM_WORDS = 4, parameter int W = 8) (
  input logic clk_i, input logic rst_ni,
  input logic [W-1:0] wdata_i, input logic [NUM_WORDS-1:0] we_i,
  output logic [W-1:0] r0
);
  for (genvar i = 1; i < NUM_WORDS; i++) begin : g_rf
    logic [W-1:0] q;
    always_ff @(posedge clk_i) begin
      if (we_i[i]) q <= wdata_i;
    end
  end
  assign r0 = 8'h0;
endmodule
"#;
        let d = synth_sv(src, "rf.sv").expect("synth rf");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert!(ffs >= 24, "i++ genvar must map 3×8 FFs, got {ffs}");
    }


    #[test]
    fn regfile_enum_literal_selects_generate_ff_branch() {
        // Packages/enums are skipped; RegFileFF must still const-fold so the
        // generate-if then branch (always_ff bank) is kept, not the else.
        let src = r#"
module rf_sel #(parameter regfile_e RegFile = RegFileFF) (
  input logic clk,
  output logic [31:0] q
);
  logic [31:0] r;
  if (RegFile == RegFileFF) begin
    always_ff @(posedge clk) r <= ~r;
  end else begin
    assign r = 32'h0;
  end
  assign q = r;
endmodule
"#;
        let d = synth_sv(src, "rf_sel.sv").expect("synth rf_sel");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs, 32, "RegFile==RegFileFF must take then always_ff, got {ffs}");

        let pkg = src.replace("RegFile = RegFileFF", "RegFile = ibex_pkg::RegFileFF")
            .replace("RegFile == RegFileFF", "RegFile == ibex_pkg::RegFileFF");
        let d2 = synth_sv(&pkg, "rf_sel_pkg.sv").expect("synth pkg::RegFileFF");
        let ffs2 = d2.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs2, 32, "pkg::RegFileFF must const-fold, got {ffs2}");
    }

    #[test]
    fn unknown_generate_if_takes_else_flops() {
        let src = r#"
module wrap (input logic clk_i, input logic rst_ni, input logic d_i, output logic q_o);
  parameter int Impl = 99;
  if (Impl == MissingPkg::Xilinx) begin : gen_x
    assign q_o = d_i;
  end else begin : gen_g
    always_ff @(posedge clk_i) q_o <= d_i;
  end
endmodule
"#;
        let d = synth_sv(src, "wrap.sv").expect("synth wrap");
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        assert_eq!(ffs, 1, "unknown generate-if should take else always_ff");
    }

    #[test]
    fn ysyx_ibex_lists_modules_and_synths_top() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ysyx_ibex.sv");
        assert!(p.exists(), "examples/ysyx_ibex.sv must ship with the crate tests");
        let src = std::fs::read_to_string(&p).unwrap();
        let mods = list_sv_modules_origin(&src, "ysyx_ibex.sv").expect("list ibex modules");
        eprintln!("ibex_modules={} has_top={}", mods.len(), mods.iter().any(|m| m == "ysyx_ibex"));
        assert!(
            mods.iter().any(|m| m == "ysyx_ibex"),
            "top ysyx_ibex must parse: {mods:?}"
        );
        assert!(
            mods.len() > 5,
            "Ibex-scale file must yield many modules, got {}",
            mods.len()
        );
        {
            let pre = preprocess_sv(&strip_comments(&src));
            let mods = parse_source(&pre).expect("parse ibex stats");
            let n_inst: usize = mods.iter().map(|m| m.insts.len()).sum();
            let n_nba: usize = mods.iter().map(|m| m.nbas.len()).sum();
            let n_as: usize = mods.iter().map(|m| m.assigns.len()).sum();
            let with_nba = mods.iter().filter(|m| !m.nbas.is_empty()).count();
            let with_inst = mods.iter().filter(|m| !m.insts.is_empty()).count();
            eprintln!(
                "parse modules={} insts={n_inst} nbas={n_nba} assigns={n_as} mods_with_nba={with_nba} mods_with_inst={with_inst}",
                mods.len()
            );
            let map: std::collections::HashMap<String, Rtl> =
                mods.iter().map(|m| (m.module.clone(), m.clone())).collect();
            let flat = flatten_module_ov(&map, "ysyx_ibex", &std::collections::HashMap::new())
                .expect("flatten ibex");
            eprintln!(
                "flat nbas={} assigns={} signals={}",
                flat.nbas.len(),
                flat.assigns.len(),
                flat.signals.len()
            );
            // Cap the diagnostic bit-probe (FM-HEL-HANG-1539): full per-bit walk can
            // hang on nested Add trees. Bound time-ish work; synth_sv_path is the real gate.
            let mut bit_ok = 0usize;
            let mut bit_fail = 0usize;
            let mut probed = 0usize;
            const PROBE_CAP: usize = 4096;
            'probe: for (lhs, bit, rhs) in &flat.nbas {
                let w = sig_width(&flat, lhs).min(64);
                if let Some(b) = bit {
                    match rexpr_to_bit(rhs, &flat, (*b).min(63)) {
                        Ok(_) => bit_ok += 1,
                        Err(_) => bit_fail += 1,
                    }
                    probed += 1;
                } else {
                    for i in 0..w.max(1) {
                        match rexpr_to_bit(rhs, &flat, i) {
                            Ok(_) => bit_ok += 1,
                            Err(_) => bit_fail += 1,
                        }
                        probed += 1;
                        if probed >= PROBE_CAP {
                            break 'probe;
                        }
                    }
                }
                if probed >= PROBE_CAP {
                    break;
                }
            }
            eprintln!(
                "rexpr_to_bit ok={bit_ok} fail={bit_fail} probed={probed} (cap={PROBE_CAP})"
            );
        }
        let t0 = std::time::Instant::now();
        let d = synth_sv_path(&p).expect("synth ysyx_ibex");
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        let ffs = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Hff)).count();
        eprintln!(
            "synth_sv_path Ok name={} ports={} cells={} luts={} ffs={} elapsed_ms={}",
            d.name,
            d.ports.len(),
            d.cells.len(),
            luts,
            ffs,
            t0.elapsed().as_millis()
        );
        assert_eq!(d.name, "ysyx_ibex");
        assert!(
            !d.ports.is_empty(),
            "ysyx_ibex ports from ANSI list"
        );
        assert!(
            !d.cells.is_empty(),
            "ysyx_ibex must map at least one LUT/FF from always_ff/assigns, cells=0"
        );
    }

    #[test]
    fn logikbench_lrelu_ge_ashr_maps_cells() {
        let src = r#"
module lrelu #(parameter DW = 16, parameter ASHIFT = 7)
   (
    input signed [DW-1:0]  in,
    output signed [DW-1:0] out
    );
   assign out = (in >= 0) ? in : (in >>> ASHIFT);
endmodule
"#;
        let d = synth_sv(src, "lrelu.sv").expect("synth lrelu");
        assert!(
            !d.cells.is_empty(),
            "lrelu >= / >>> must map cells, got 0"
        );
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        assert!(luts > 0, "lrelu must produce LUTs for mux/ashr, luts={luts}");
    }

    #[test]
    fn logikbench_hswish_region_le_ge_maps_cells() {
        let src = r#"
module hswish #(parameter DW = 16, parameter QW = 8)
   (
    input signed [DW-1:0]  x,
    output signed [DW-1:0] out
    );
   localparam signed [DW:0] THREE = 3 <<< QW;
   localparam signed [DW:0] SIX  = 6 <<< QW;
   wire signed [2*DW:0]     prod;
   wire signed [2*DW:0]     mid;
   assign prod = x * (x + THREE);
   assign mid  = prod / SIX;
   assign out = (x <= -THREE) ? {DW{1'b0}} :
                (x >=  THREE) ? x :
                mid[DW-1:0];
endmodule
"#;
        let d = synth_sv(src, "hswish.sv").expect("synth hswish");
        assert!(
            !d.cells.is_empty(),
            "hswish region <=/>= must map cells, got 0"
        );
        // Region mux + compares should produce LUTs even if /const is skipped.
        let luts = d.cells.iter().filter(|c| matches!(c.kind, CellKind::Lut6 { .. })).count();
        assert!(
            luts > 0 || d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)),
            "hswish must map LUTs or Mac27, cells={}",
            d.cells.len()
        );
    }

    #[test]
    fn cmp_le_ge_and_ashr_smoke() {
        let src = r#"
module t(input [7:0] a, b, input signed [15:0] s, output o, output signed [15:0] y);
  assign o = (a <= b) & (a >= b);
  assign y = s >>> 3;
endmodule
"#;
        let d = synth_sv(src, "cmp.sv").expect("synth cmp");
        assert!(!d.cells.is_empty(), "le/ge/ashr smoke must map cells");
    }
}
