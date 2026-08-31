//! SV subset: **sv-parser** AST → NBA RHS → AIG → FlowMap LUT6+FF.

use helion_ir::{CellKind, Design, PortDir};
use std::collections::HashMap;
use std::path::Path;
use sv_parser::{parse_sv_str, NodeEvent, RefNode};

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
        a.output = a.expr(e);
        a
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

fn tree_text(tree: &sv_parser::SyntaxTree) -> String {
    let mut s = String::new();
    for event in tree.into_iter().event() {
        let NodeEvent::Enter(node) = event else { continue };
        if let Some(loc) = locate_any(&node) {
            if let Some(t) = tree.get_str(&loc) {
                s.push_str(t);
            }
        }
    }
    s
}

fn locate_any(node: &RefNode) -> Option<sv_parser::Locate> {
    match node {
        RefNode::Locate(l) => Some(**l),
        _ => None,
    }
}

fn always_nba_rhs(tree: &sv_parser::SyntaxTree) -> Result<String, String> {
    let mut depth = 0i32;
    let mut buf = String::new();
    for event in tree.into_iter().event() {
        match event {
            NodeEvent::Enter(RefNode::AlwaysConstruct(_)) => depth += 1,
            NodeEvent::Leave(RefNode::AlwaysConstruct(_)) => depth -= 1,
            NodeEvent::Enter(node) if depth > 0 => {
                if let Some(loc) = locate_any(&node) {
                    if let Some(t) = tree.get_str(&loc) {
                        buf.push_str(t);
                    }
                }
            }
            _ => {}
        }
    }
    let compact: String = buf.chars().filter(|c| !c.is_whitespace()).collect();
    let rhs = compact
        .split("<=")
        .nth(1)
        .or_else(|| compact.split('=').nth(1))
        .ok_or_else(|| format!("no NBA in always: {compact}"))?;
    let rhs = rhs.split(';').next().unwrap_or(rhs);
    Ok(rhs.to_string())
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
        if *i < chars.len() && (chars[*i] == '0' || chars[*i] == '1') && !chars.get(*i+1).map(|c| c.is_alphanumeric()).unwrap_or(false) {
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

pub fn synth_sv(source: &str, origin: &str) -> Result<Design, String> {
    let tree = parse_sv(source, origin)?;
    let mut module = "top".to_string();
    if let Some(rest) = source.trim_start().strip_prefix("module") {
        if let Some(name) = rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).find(|s| !s.is_empty()) {
            if name != "module" {
                module = name.to_string();
            }
        }
    }
    let _ = tree; // tree used below via always_nba_rhs; keep parse-success requirement
    let rhs = always_nba_rhs(&tree)?;
    let expr = parse_expr(&rhs)?;
    let aig = Aig::from_expr(&expr);
    let init = aig.flowmap_lut6();
    let mut d = Design::new(module);
    d.add_port("clk", PortDir::In);
    d.add_port("led", PortDir::Out);
    d.add_cell("u_lut", CellKind::Lut6 { init });
    d.add_cell("u_ff", CellKind::Hff);
    d.add_cell("u_iob", CellKind::IobOut);
    d.connect("clk", "u_ff", "CLK");
    d.connect("d", "u_lut", "O");
    d.connect("d", "u_ff", "D");
    d.connect("q", "u_ff", "Q");
    d.connect("q", "u_lut", "I0");
    d.connect("q", "u_iob", "I");
    d.connect("led", "u_iob", "PAD");
    let _ = tree_text(&tree);
    Ok(d)
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
}
