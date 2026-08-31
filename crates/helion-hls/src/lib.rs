//! C subset → schedule/bind onto Helion LUT/FF/DSP (not string-match).
//!
//! Static registers, `!`/`~`, `+`, `*`, constant `for` unroll. Original HLS.

use helion_ir::Design;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Res {
    Lut,
    Ff,
    Dsp,
}

#[derive(Clone, Debug)]
pub struct BindOp {
    pub name: String,
    pub res: Res,
    pub cycle: u32,
}

#[derive(Clone, Debug)]
pub struct HlsResult {
    pub design: Design,
    pub ops: Vec<BindOp>,
    pub sv: String,
}

fn strip(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        let l = line.split("//").next().unwrap_or("");
        out.push_str(l);
        out.push('\n');
    }
    out
}

#[derive(Clone, Debug)]
enum Tok {
    Ident(String),
    Num(u128),
    Sym(String),
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
    fn eat(&mut self, s: &str) -> bool {
        match self.peek() {
            Some(Tok::Ident(k)) if k == s => {
                self.i += 1;
                true
            }
            Some(Tok::Sym(k)) if k == s => {
                self.i += 1;
                true
            }
            _ => false,
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        match self.bump() {
            Some(Tok::Ident(s)) => Ok(s.clone()),
            other => Err(format!("ident {other:?}")),
        }
    }
    fn num(&mut self) -> Option<u128> {
        match self.peek() {
            Some(Tok::Num(n)) => {
                let v = *n;
                self.i += 1;
                Some(v)
            }
            _ => None,
        }
    }
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = strip(src).chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '+' && chars.get(i + 1) == Some(&'+') {
            out.push(Tok::Sym("++".into()));
            i += 2;
            continue;
        }
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            out.push(Tok::Sym("--".into()));
            i += 2;
            continue;
        }
        if c == '&' && chars.get(i + 1) == Some(&'&') {
            out.push(Tok::Sym("&&".into()));
            i += 2;
            continue;
        }
        if c == '|' && chars.get(i + 1) == Some(&'|') {
            out.push(Tok::Sym("||".into()));
            i += 2;
            continue;
        }
        if "(){};,=+-*/~!<>[]&|^".contains(c) {
            out.push(Tok::Sym(c.to_string()));
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let mut n = String::new();
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                n.push(chars[i]);
                i += 1;
            }
            out.push(Tok::Ident(n));
            continue;
        }
        if c.is_ascii_digit() {
            let mut n = String::new();
            while i < chars.len() && chars[i].is_ascii_digit() {
                n.push(chars[i]);
                i += 1;
            }
            out.push(Tok::Num(n.parse().unwrap_or(0)));
            continue;
        }
        return Err(format!("hls char {c:?}"));
    }
    Ok(out)
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct Arg {
    name: String,
    ptr: bool,
    width: usize,
    output: bool,
}

#[derive(Clone, Debug)]
struct Var {
    name: String,
    width: usize,
    is_static: bool,
}

#[derive(Clone, Debug)]
enum CExpr {
    Num(u128),
    Var(String),
    Bit(String, usize),
    Not(Box<CExpr>),
    Add(Box<CExpr>, Box<CExpr>),
    Mul(Box<CExpr>, Box<CExpr>),
    And(Box<CExpr>, Box<CExpr>),
    Xor(Box<CExpr>, Box<CExpr>),
    Or(Box<CExpr>, Box<CExpr>),
    Deref(String),
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
enum Stmt {
    Assign { lhs: String, deref: bool, bit: Option<usize>, rhs: CExpr },
    For { var: String, start: usize, end: usize, body: Vec<Stmt> }, // var kept for debug
    Return(CExpr),
}

fn parse_type(p: &mut P) -> Result<(usize, bool), String> {
    let _ = p.eat("const");
    let _ = p.eat("unsigned");
    let _ = p.eat("signed");
    if p.eat("void") {
        return Ok((0, false));
    }
    if p.eat("bool") {
        return Ok((1, false));
    }
    if p.eat("char") {
        return Ok((8, false));
    }
    if p.eat("int") {
        return Ok((32, false));
    }
    if p.eat("ap_uint") || p.eat("ap_int") {
        if !p.eat("<") {
            return Err("ap_int <".into());
        }
        let w = p.num().ok_or("ap_int width")? as usize;
        if !p.eat(">") {
            return Err("ap_int >".into());
        }
        return Ok((w.max(1), false));
    }
    Err("hls type".into())
}

fn parse_expr(p: &mut P) -> Result<CExpr, String> {
    parse_or(p)
}

fn parse_or(p: &mut P) -> Result<CExpr, String> {
    let mut e = parse_xor(p)?;
    while p.eat("|") {
        let r = parse_xor(p)?;
        e = CExpr::Or(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_xor(p: &mut P) -> Result<CExpr, String> {
    let mut e = parse_and(p)?;
    while p.eat("^") {
        let r = parse_and(p)?;
        e = CExpr::Xor(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_and(p: &mut P) -> Result<CExpr, String> {
    let mut e = parse_add(p)?;
    while p.eat("&") {
        let r = parse_add(p)?;
        e = CExpr::And(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_add(p: &mut P) -> Result<CExpr, String> {
    let mut e = parse_mul(p)?;
    while p.eat("+") || p.eat("-") {
        let r = parse_mul(p)?;
        e = CExpr::Add(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_mul(p: &mut P) -> Result<CExpr, String> {
    let mut e = parse_un(p)?;
    while p.eat("*") {
        let r = parse_un(p)?;
        e = CExpr::Mul(Box::new(e), Box::new(r));
    }
    Ok(e)
}

fn parse_un(p: &mut P) -> Result<CExpr, String> {
    if p.eat("!") || p.eat("~") {
        return Ok(CExpr::Not(Box::new(parse_un(p)?)));
    }
    if p.eat("*") {
        let n = p.ident()?;
        return Ok(CExpr::Deref(n));
    }
    if p.eat("(") {
        let e = parse_expr(p)?;
        if !p.eat(")") {
            return Err(")".into());
        }
        return Ok(e);
    }
    if let Some(n) = p.num() {
        return Ok(CExpr::Num(n));
    }
    let n = p.ident()?;
    if p.eat("[") {
        let idx = p.num().ok_or("bit idx")? as usize;
        if !p.eat("]") {
            return Err("]".into());
        }
        return Ok(CExpr::Bit(n, idx));
    }
    Ok(CExpr::Var(n))
}

fn parse_stmt(p: &mut P) -> Result<Stmt, String> {
    if p.eat("return") {
        let e = parse_expr(p)?;
        let _ = p.eat(";");
        return Ok(Stmt::Return(e));
    }
    if p.eat("for") {
        if !p.eat("(") {
            return Err("for (".into());
        }
        let _ = p.eat("int");
        let var = p.ident()?;
        if !p.eat("=") {
            return Err("for =".into());
        }
        let start = p.num().ok_or("for start")? as usize;
        if !p.eat(";") {
            return Err("for ;".into());
        }
        let _ = p.ident();
        if !p.eat("<") {
            return Err("for <".into());
        }
        let end = p.num().ok_or("for end")? as usize;
        if !p.eat(";") {
            return Err("for ;2".into());
        }
        // i++  |  ++i  |  i = i + 1
        let _ = p.eat("++");
        let _ = p.eat("--");
        if matches!(p.peek(), Some(Tok::Ident(_))) {
            let _ = p.ident();
            let _ = p.eat("++");
            let _ = p.eat("--");
            if p.eat("=") {
                let _ = p.ident();
                let _ = p.eat("+");
                let _ = p.num();
            }
        }
        if !p.eat(")") {
            return Err("for )".into());
        }
        let body = if p.eat("{") {
            let mut v = Vec::new();
            while !p.eat("}") {
                if p.peek().is_none() {
                    return Err("for body".into());
                }
                v.push(parse_stmt(p)?);
            }
            v
        } else {
            vec![parse_stmt(p)?]
        };
        return Ok(Stmt::For {
            var,
            start,
            end,
            body,
        });
    }
    // ++name / name++
    if p.eat("++") {
        let n = p.ident()?;
        let _ = p.eat(";");
        return Ok(Stmt::Assign {
            lhs: n.clone(),
            deref: false,
            bit: None,
            rhs: CExpr::Add(Box::new(CExpr::Var(n)), Box::new(CExpr::Num(1))),
        });
    }
    let deref = p.eat("*");
    let lhs = p.ident()?;
    if p.eat("++") {
        let _ = p.eat(";");
        return Ok(Stmt::Assign {
            lhs: lhs.clone(),
            deref: false,
            bit: None,
            rhs: CExpr::Add(Box::new(CExpr::Var(lhs)), Box::new(CExpr::Num(1))),
        });
    }
    let bit = if p.eat("[") {
        let idx = p.num().ok_or("lhs bit")? as usize;
        if !p.eat("]") {
            return Err("]".into());
        }
        Some(idx)
    } else {
        None
    };
    if !p.eat("=") {
        return Err(format!("assign {lhs}"));
    }
    let rhs = parse_expr(p)?;
    let _ = p.eat(";");
    Ok(Stmt::Assign {
        lhs,
        deref,
        bit,
        rhs,
    })
}

fn parse_unit(p: &mut P) -> Result<(String, Vec<Arg>, Vec<Var>, Vec<Stmt>), String> {
    let (_w, _) = parse_type(p)?;
    let fname = p.ident()?;
    if !p.eat("(") {
        return Err("fn (".into());
    }
    let mut args = Vec::new();
    while !p.eat(")") {
        if p.peek().is_none() {
            return Err("fn )".into());
        }
        let (w, _) = parse_type(p)?;
        let ptr = p.eat("*");
        let n = p.ident()?;
        args.push(Arg {
            name: n,
            ptr,
            width: if ptr { 1 } else { w.max(1) },
            output: ptr,
        });
        let _ = p.eat(",");
    }
    if !p.eat("{") {
        return Err("fn {".into());
    }
    let mut vars = Vec::new();
    let mut stmts = Vec::new();
    while !p.eat("}") {
        if p.peek().is_none() {
            return Err("unterminated fn".into());
        }
        let is_static = p.eat("static");
        let start = p.i;
        if parse_type(p).is_ok() {
            // might be decl
            let ptr = p.eat("*");
            if let Ok(_n) = p.ident() {
                if p.eat("=") || p.eat(";") {
                    // rewind equals
                    // we already consumed = or ;
                    let mut init = CExpr::Num(0);
                    // If we ate '=', parse expr. Recover by checking last tok is messy;
                    // re-parse from start.
                    p.i = start;
                    let _ = p.eat("static");
                    let (w, _) = parse_type(p)?;
                    let _ = p.eat("*");
                    let n = p.ident()?;
                    if p.eat("=") {
                        init = parse_expr(p)?;
                    }
                    let _ = p.eat(";");
                    let _ = (ptr, init);
                    vars.push(Var {
                        name: n,
                        width: w.max(1),
                        is_static,
                    });
                    continue;
                }
            }
        }
        p.i = start;
        let _ = p.eat("static");
        stmts.push(parse_stmt(p)?);
    }
    Ok((fname, args, vars, stmts))
}

fn unroll(stmts: &[Stmt], cycle: u32) -> Vec<(u32, Stmt)> {
    let mut out = Vec::new();
    for s in stmts {
        match s {
            Stmt::For {
                var: _,
                start,
                end,
                body,
            } => {
                for it in *start..*end {
                    let inner = unroll(body, cycle + it as u32);
                    out.extend(inner);
                }
            }
            other => out.push((cycle, other.clone())),
        }
    }
    out
}

fn expr_sv(e: &CExpr) -> String {
    match e {
        CExpr::Num(n) => n.to_string(),
        CExpr::Var(s) => s.clone(),
        CExpr::Bit(s, i) => format!("{s}[{i}]"),
        CExpr::Not(x) => format!("~{}", expr_sv(x)),
        CExpr::Add(a, b) => format!("{} + {}", expr_sv(a), expr_sv(b)),
        CExpr::Mul(a, b) => format!("{} * {}", expr_sv(a), expr_sv(b)),
        CExpr::And(a, b) => format!("{} & {}", expr_sv(a), expr_sv(b)),
        CExpr::Xor(a, b) => format!("{} ^ {}", expr_sv(a), expr_sv(b)),
        CExpr::Or(a, b) => format!("{} | {}", expr_sv(a), expr_sv(b)),
        CExpr::Deref(s) => s.clone(),
    }
}

fn walk_ops(e: &CExpr, cycle: u32, ops: &mut Vec<BindOp>) {
    match e {
        CExpr::Not(x) => {
            ops.push(BindOp {
                name: format!("not@{}", ops.len()),
                res: Res::Lut,
                cycle,
            });
            walk_ops(x, cycle, ops);
        }
        CExpr::Add(a, b) => {
            ops.push(BindOp {
                name: format!("add@{}", ops.len()),
                res: Res::Lut,
                cycle,
            });
            walk_ops(a, cycle, ops);
            walk_ops(b, cycle, ops);
        }
        CExpr::Mul(a, b) => {
            ops.push(BindOp {
                name: format!("mul@{}", ops.len()),
                res: Res::Dsp,
                cycle,
            });
            walk_ops(a, cycle, ops);
            walk_ops(b, cycle, ops);
        }
        CExpr::And(a, b) | CExpr::Xor(a, b) | CExpr::Or(a, b) => {
            ops.push(BindOp {
                name: format!("bit@{}", ops.len()),
                res: Res::Lut,
                cycle,
            });
            walk_ops(a, cycle, ops);
            walk_ops(b, cycle, ops);
        }
        _ => {}
    }
}

fn bind_muls(
    e: &CExpr,
    extra_regs: &mut Vec<(String, usize)>,
    nbas: &mut Vec<String>,
    mul_i: &mut usize,
) -> CExpr {
    match e {
        CExpr::Mul(a, b) => {
            let la = bind_muls(a, extra_regs, nbas, mul_i);
            let lb = bind_muls(b, extra_regs, nbas, mul_i);
            let t = format!("tmul{mul_i}");
            extra_regs.push((t.clone(), 27));
            nbas.push(format!("{t} <= {} * {};", expr_sv(&la), expr_sv(&lb)));
            *mul_i += 1;
            CExpr::Var(t)
        }
        CExpr::Not(x) => CExpr::Not(Box::new(bind_muls(x, extra_regs, nbas, mul_i))),
        CExpr::Add(a, b) => CExpr::Add(
            Box::new(bind_muls(a, extra_regs, nbas, mul_i)),
            Box::new(bind_muls(b, extra_regs, nbas, mul_i)),
        ),
        CExpr::And(a, b) => CExpr::And(
            Box::new(bind_muls(a, extra_regs, nbas, mul_i)),
            Box::new(bind_muls(b, extra_regs, nbas, mul_i)),
        ),
        CExpr::Xor(a, b) => CExpr::Xor(
            Box::new(bind_muls(a, extra_regs, nbas, mul_i)),
            Box::new(bind_muls(b, extra_regs, nbas, mul_i)),
        ),
        CExpr::Or(a, b) => CExpr::Or(
            Box::new(bind_muls(a, extra_regs, nbas, mul_i)),
            Box::new(bind_muls(b, extra_regs, nbas, mul_i)),
        ),
        other => other.clone(),
    }
}

fn emit_sv(
    fname: &str,
    args: &[Arg],
    vars: &[Var],
    flat: &[(u32, Stmt)],
) -> Result<(String, Vec<BindOp>), String> {
    let mut ops = Vec::new();
    let mut ports: Vec<String> = vec!["input logic clk".into()];
    for a in args {
        if a.name == "clk" {
            continue;
        }
        let w = a.width;
        let dir = if a.output { "output" } else { "input" };
        if w == 1 {
            ports.push(format!("{dir} logic {}", a.name));
        } else {
            ports.push(format!("{dir} logic [{}:0] {}", w - 1, a.name));
        }
    }
    let mut has_y = args.iter().any(|a| a.name == "y" && a.output);
    let mut nbas = Vec::new();
    let mut assigns = Vec::new();
    let mut extra_regs: Vec<(String, usize)> = Vec::new();
    let mut mul_i = 0usize;
    for (cyc, st) in flat {
        match st {
            Stmt::Assign {
                lhs,
                deref,
                bit,
                rhs,
            } => {
                walk_ops(rhs, *cyc, &mut ops);
                let rhs = bind_muls(rhs, &mut extra_regs, &mut nbas, &mut mul_i);
                if *deref {
                    assigns.push(format!("{} = {};", lhs, expr_sv(&rhs)));
                } else {
                    let lhs_sv = if let Some(b) = bit {
                        format!("{lhs}[{b}]")
                    } else {
                        lhs.clone()
                    };
                    nbas.push(format!("{lhs_sv} <= {};", expr_sv(&rhs)));
                }
                if vars.iter().any(|v| v.name == *lhs && v.is_static) {
                    ops.push(BindOp {
                        name: format!("ff_{lhs}"),
                        res: Res::Ff,
                        cycle: *cyc,
                    });
                }
            }
            Stmt::Return(e) => {
                walk_ops(e, *cyc, &mut ops);
                if !has_y {
                    ports.push("output logic [26:0] y".into());
                    has_y = true;
                }
                let e = bind_muls(e, &mut extra_regs, &mut nbas, &mut mul_i);
                nbas.push(format!("y <= {};", expr_sv(&e)));
            }
            Stmt::For { .. } => {}
        }
    }
    let mut sv = format!("module {fname}({});\n", ports.join(", "));
    for v in vars {
        if v.width == 1 {
            sv.push_str(&format!("  logic {};\n", v.name));
        } else {
            sv.push_str(&format!("  logic [{}:0] {};\n", v.width - 1, v.name));
        }
    }
    for (n, w) in &extra_regs {
        sv.push_str(&format!("  logic [{}:0] {n};\n", w - 1));
    }
    if !nbas.is_empty() {
        sv.push_str("  always_ff @(posedge clk) begin\n");
        for l in &nbas {
            sv.push_str(&format!("    {l}\n"));
        }
        sv.push_str("  end\n");
    }
    for l in &assigns {
        sv.push_str(&format!("  assign {l}\n"));
    }
    sv.push_str("endmodule\n");
    if nbas.is_empty() && assigns.is_empty() {
        return Err("hls: empty body".into());
    }
    Ok((sv, ops))
}

pub fn hls_compile(source: &str) -> Result<HlsResult, String> {
    let t = tokenize(source)?;
    if t.is_empty() {
        return Err("hls: empty".into());
    }
    let mut p = P { t: &t, i: 0 };
    let (fname, args, vars, stmts) = parse_unit(&mut p).map_err(|e| format!("hls: {e}"))?;
    let flat = unroll(&stmts, 0);
    if flat.is_empty() && vars.is_empty() {
        return Err("hls: unsupported body".into());
    }
    let (sv, ops) = emit_sv(&fname, &args, &vars, &flat)?;
    let design = helion_sv::synth_sv(&sv, "hls.sv").map_err(|e| format!("sv synth: {e} :: {sv}"))?;
    Ok(HlsResult { design, ops, sv })
}

pub fn synth_c(source: &str) -> Result<Design, String> {
    Ok(hls_compile(source)?.design)
}

pub fn synth_c_path(path: &Path) -> Result<Design, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    synth_c(&src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, INC4_INIT};

    #[test]
    fn hls_blinky_static_invert() {
        let src = r#"
void blinky(bool *led) {
  static bool q = 0;
  q = !q;
  *led = q;
}
"#;
        let d = synth_c(src).unwrap();
        assert_eq!(d.lut_inits(), vec![0x5555_5555_5555_5555]);
    }

    #[test]
    fn hls_mul_is_mac27() {
        let src = r#"
int mac(int a, int b) { return a * b; }
"#;
        let d = synth_c(src).unwrap();
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
    }

    #[test]
    fn hls_garbage_fails() {
        assert!(synth_c("lorem ipsum").is_err());
    }

    #[test]
    fn hls_schedule_bind_is_not_pattern_match() {
        let inc3 = r#"
void counter(bool *led) {
  static ap_uint<3> cnt = 0;
  cnt = cnt + 1;
  *led = cnt[2];
}
"#;
        let inc4 = r#"
void counter(bool *led) {
  static ap_uint<4> cnt = 0;
  cnt = cnt + 1;
  *led = cnt[3];
}
"#;
        let r3 = hls_compile(inc3).unwrap();
        let r4 = hls_compile(inc4).unwrap();
        assert_eq!(r3.design.lut_inits().len(), 3, "3-bit HLS incrementer {:?}", r3.sv);
        assert_eq!(r4.design.lut_inits(), INC4_INIT.to_vec(), "4-bit must match gold {}", r4.sv);
        assert_ne!(r3.design.lut_inits().len(), r4.design.lut_inits().len());
        assert!(r3.ops.iter().any(|o| o.res == Res::Lut));
        assert!(r3.ops.iter().any(|o| o.res == Res::Ff));

        let one = r#"
int mac1(int a, int b) { return a * b; }
"#;
        let two = r#"
int mac2(int a, int b) {
  int t = 0;
  for (int i = 0; i < 2; i++) {
    t = a * b;
  }
  return t;
}
"#;
        let m1 = hls_compile(one).unwrap();
        let m2 = hls_compile(two).unwrap();
        let n1 = m1.design.cells.iter().filter(|c| matches!(c.kind, CellKind::Mac27)).count();
        let n2 = m2.design.cells.iter().filter(|c| matches!(c.kind, CellKind::Mac27)).count();
        assert_eq!(n1, 1, "one mul → one DSP {}", m1.sv);
        assert_eq!(n2, 2, "unrolled 2-iter mul loop must bind 2 DSP, not 1: ops={:?} sv={}", m2.ops, m2.sv);
        assert!(m2.ops.iter().filter(|o| o.res == Res::Dsp).count() >= 2);
        assert!(
            m2.ops.iter().any(|o| o.cycle >= 1),
            "loop iterations must occupy distinct schedule cycles {:?}",
            m2.ops
        );
    }
}
