//! C/C++ HLS kernel lexer, preprocessor, and AST parser.

use crate::pragma::{ parse_pragma_line, HlsDirective };
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Num(i128),
    Sym(String),
    Pragma(HlsDirective),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Not,
    BitNot,
    Neg,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    LogAnd,
    LogOr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CExpr {
    Num(i128),
    Var(String),
    Bit(String, Box<CExpr>),
    Slice(String, usize, usize),
    Index {
        array: String,
        indices: Vec<CExpr>,
    },
    Deref(String),
    Unary {
        op: UnaryOp,
        expr: Box<CExpr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<CExpr>,
        rhs: Box<CExpr>,
    },
    Ternary {
        cond: Box<CExpr>,
        then_e: Box<CExpr>,
        else_e: Box<CExpr>,
    },
    Call {
        func: String,
        args: Vec<CExpr>,
    },
    StreamRead(String),
    StreamEmpty(String),
    StreamFull(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LValue {
    Var(String),
    Deref(String),
    Bit(String, CExpr),
    Slice(String, usize, usize),
    ArrayElem {
        array: String,
        indices: Vec<CExpr>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CType {
    pub width: usize,
    pub signed: bool,
    pub is_void: bool,
    pub is_stream: bool,
    pub is_axis_pkt: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CArg {
    pub name: String,
    pub width: usize,
    pub signed: bool,
    pub is_ptr: bool,
    pub is_ref: bool,
    pub is_const: bool,
    pub is_stream: bool,
    pub is_axis_pkt: bool,
    pub dims: Vec<usize>,
    pub output: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CVar {
    pub name: String,
    pub width: usize,
    pub signed: bool,
    pub is_static: bool,
    pub is_const: bool,
    pub is_stream: bool,
    pub dims: Vec<usize>,
    pub init: Option<CExpr>,
    pub init_list: Vec<i128>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CStmt {
    Pragma(HlsDirective),
    Decl(CVar),
    Assign {
        lhs: LValue,
        rhs: CExpr,
    },
    For {
        label: Option<String>,
        var: String,
        start: i64,
        end: i64,
        step: i64,
        inclusive: bool,
        pragmas: Vec<HlsDirective>,
        body: Vec<CStmt>,
    },
    While {
        label: Option<String>,
        cond: CExpr,
        trip_bound: usize,
        pragmas: Vec<HlsDirective>,
        body: Vec<CStmt>,
    },
    If {
        cond: CExpr,
        then_body: Vec<CStmt>,
        else_body: Vec<CStmt>,
    },
    Call {
        func: String,
        args: Vec<CExpr>,
    },
    StreamWrite {
        stream: String,
        val: CExpr,
    },
    Return(Option<CExpr>),
}

#[derive(Clone, Debug)]
pub struct CFunction {
    pub name: String,
    pub ret_width: usize,
    pub ret_signed: bool,
    pub args: Vec<CArg>,
    pub vars: Vec<CVar>,
    pub body: Vec<CStmt>,
    pub pragmas: Vec<HlsDirective>,
    pub is_inline: bool,
}

#[derive(Clone, Debug)]
pub struct TranslationUnit {
    pub functions: Vec<CFunction>,
    pub global_vars: Vec<CVar>,
    pub top_pragmas: Vec<HlsDirective>,
}

/// Strip `/* ... */` block comments while preserving line breaks.
fn strip_block_comments(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                if chars[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            if i < chars.len() {
                i += 2;
            }
            out.push(' ');
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Tokenize C/C++ HLS source, preserving `#pragma HLS ...` as `Tok::Pragma`
/// and expanding object-like `#define NAME VAL` macros.
pub fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let no_block = strip_block_comments(src);
    // Join backslash-continued lines first.
    let mut lines: Vec<String> = Vec::new();
    let mut cont = String::new();
    for raw in no_block.lines() {
        let l = raw.split("//").next().unwrap_or("").trim_end();
        if let Some(prefix) = l.strip_suffix('\\') {
            cont.push_str(prefix);
            cont.push(' ');
        } else if !cont.is_empty() {
            cont.push_str(l);
            lines.push(std::mem::take(&mut cont));
        } else {
            lines.push(l.to_string());
        }
    }
    if !cont.is_empty() {
        lines.push(cont);
    }

    let mut defines: HashMap<String, Vec<Tok>> = HashMap::new();
    let mut out = Vec::new();

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            if let Some(dir) = parse_pragma_line(trimmed)? {
                out.push(Tok::Pragma(dir));
                continue;
            }
            let after_hash = trimmed[1..].trim_start();
            let lower = after_hash.to_ascii_lowercase();
            if lower.starts_with("include")
                || lower.starts_with("ifdef")
                || lower.starts_with("ifndef")
                || lower.starts_with("endif")
                || lower.starts_with("else")
                || lower.starts_with("elif")
                || lower.starts_with("pragma")
            {
                continue;
            }
            if lower.starts_with("define") {
                let rest = after_hash["define".len()..].trim_start();
                let mut parts = rest.splitn(2, char::is_whitespace);
                if let Some(name) = parts.next() {
                    let val_str = parts.next().unwrap_or("").trim();
                    if !name.is_empty() && !name.contains('(') && !val_str.is_empty() {
                        if let Ok(toks) = lex_line(val_str, &defines) {
                            defines.insert(name.to_string(), toks);
                        }
                    }
                }
                continue;
            }
            return Err(format!("unsupported preprocessor directive: {trimmed}"));
        }
        let line_toks = lex_line(trimmed, &defines)?;
        out.extend(line_toks);
    }
    Ok(out)
}

fn lex_line(line: &str, defines: &HashMap<String, Vec<Tok>>) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // 3-char operators
        if i + 2 < chars.len() {
            let s3: String = chars[i..i + 3].iter().collect();
            if s3 == "<<=" || s3 == ">>=" {
                out.push(Tok::Sym(s3));
                i += 3;
                continue;
            }
        }
        // 2-char operators
        if i + 1 < chars.len() {
            let s2: String = chars[i..i + 2].iter().collect();
            if matches!(
                s2.as_str(),
                "++" | "--"
                    | "+="
                    | "-="
                    | "*="
                    | "/="
                    | "%="
                    | "&="
                    | "|="
                    | "^="
                    | "<<"
                    | ">>"
                    | "<="
                    | ">="
                    | "=="
                    | "!="
                    | "&&"
                    | "||"
                    | "::"
                    | "->"
            ) {
                out.push(Tok::Sym(s2));
                i += 2;
                continue;
            }
        }
        if "(){};,=+-*/%~!<>[]:?&|^.".contains(c) {
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
            if let Some(expanded) = defines.get(&n) {
                out.extend(expanded.iter().cloned());
            } else if n == "true" {
                out.push(Tok::Num(1));
            } else if n == "false" {
                out.push(Tok::Num(0));
            } else {
                out.push(Tok::Ident(n));
            }
            continue;
        }
        if c.is_ascii_digit() {
            if c == '0' && chars.get(i + 1).is_some_and(|&x| x == 'x' || x == 'X') {
                i += 2;
                let mut hex = String::new();
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    hex.push(chars[i]);
                    i += 1;
                }
                while i < chars.len() && matches!(chars[i], 'u' | 'U' | 'l' | 'L') {
                    i += 1;
                }
                let val = i128::from_str_radix(&hex, 16).unwrap_or(0);
                out.push(Tok::Num(val));
                continue;
            }
            if c == '0' && chars.get(i + 1).is_some_and(|&x| x == 'b' || x == 'B') {
                i += 2;
                let mut bin = String::new();
                while i < chars.len() && (chars[i] == '0' || chars[i] == '1' || chars[i] == '_') {
                    if chars[i] != '_' {
                        bin.push(chars[i]);
                    }
                    i += 1;
                }
                while i < chars.len() && matches!(chars[i], 'u' | 'U' | 'l' | 'L') {
                    i += 1;
                }
                let val = i128::from_str_radix(&bin, 2).unwrap_or(0);
                out.push(Tok::Num(val));
                continue;
            }
            let mut n = String::new();
            while i < chars.len() && chars[i].is_ascii_digit() {
                n.push(chars[i]);
                i += 1;
            }
            // Skip float suffix or integer suffix if present
            if i < chars.len() && chars[i] == '.' {
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == 'f' || chars[i] == 'F') {
                    i += 1;
                }
            }
            while i < chars.len() && matches!(chars[i], 'u' | 'U' | 'l' | 'L') {
                i += 1;
            }
            out.push(Tok::Num(n.parse().unwrap_or(0)));
            continue;
        }
        return Err(format!("hls char {c:?}"));
    }
    Ok(out)
}

pub struct Parser<'a> {
    pub t: &'a [Tok],
    pub i: usize,
    pub typedefs: HashMap<String, CType>,
    pub const_vals: HashMap<String, i128>,
}

impl<'a> Parser<'a> {
    pub fn new(t: &'a [Tok]) -> Self {
        Self {
            t,
            i: 0,
            typedefs: HashMap::new(),
            const_vals: HashMap::new(),
        }
    }

    fn peek(&self) -> Option<&'a Tok> {
        self.t.get(self.i)
    }

    fn peek_n(&self, offset: usize) -> Option<&'a Tok> {
        self.t.get(self.i + offset)
    }

    fn bump(&mut self) -> Option<&'a Tok> {
        let tok = self.t.get(self.i)?;
        self.i += 1;
        Some(tok)
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
            other => Err(format!("expected identifier, got {other:?}")),
        }
    }

    fn num(&mut self) -> Option<i128> {
        match self.peek() {
            Some(Tok::Num(n)) => {
                let v = *n;
                self.i += 1;
                Some(v)
            }
            Some(Tok::Ident(name)) => {
                if let Some(&v) = self.const_vals.get(name) {
                    self.i += 1;
                    Some(v)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn eval_const_expr(&mut self) -> Result<i128, String> {
        let e = self.parse_expr()?;
        self.eval_cexpr_const(&e)
            .ok_or_else(|| format!("expected constant integer expression, got {e:?}"))
    }

    pub fn eval_cexpr_const(&self, e: &CExpr) -> Option<i128> {
        match e {
            CExpr::Num(n) => Some(*n),
            CExpr::Var(s) => self.const_vals.get(s).copied(),
            CExpr::Unary { op, expr } => {
                let v = self.eval_cexpr_const(expr)?;
                match op {
                    UnaryOp::Neg => Some(-v),
                    UnaryOp::Not => Some(i128::from(v == 0)),
                    UnaryOp::BitNot => Some(!v),
                }
            }
            CExpr::Binary { op, lhs, rhs } => {
                let l = self.eval_cexpr_const(lhs)?;
                let r = self.eval_cexpr_const(rhs)?;
                match op {
                    BinOp::Add => Some(l + r),
                    BinOp::Sub => Some(l - r),
                    BinOp::Mul => Some(l * r),
                    BinOp::Div => (r != 0).then(|| l / r),
                    BinOp::Mod => (r != 0).then(|| l % r),
                    BinOp::Shl => Some(l << r),
                    BinOp::Shr => Some(l >> r),
                    BinOp::And => Some(l & r),
                    BinOp::Or => Some(l | r),
                    BinOp::Xor => Some(l ^ r),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn parse_type(&mut self) -> Result<(CType, bool), String> {
        let mut is_const = false;
        let mut is_unsigned = false;
        let mut is_signed = false;
        loop {
            if self.eat("const") || self.eat("volatile") || self.eat("register") {
                is_const = true;
            } else if self.eat("unsigned") {
                is_unsigned = true;
            } else if self.eat("signed") {
                is_signed = true;
            } else if self.eat("hls") && self.eat("::") {
                // namespace prefix `hls::`
            } else if self.eat("std") && self.eat("::") {
                // namespace prefix `std::`
            } else {
                break;
            }
        }

        if self.eat("void") {
            return Ok((
                CType {
                    width: 0,
                    signed: false,
                    is_void: true,
                    is_stream: false,
                    is_axis_pkt: false,
                },
                is_const,
            ));
        }
        if self.eat("bool") {
            return Ok((
                CType {
                    width: 1,
                    signed: false,
                    is_void: false,
                    is_stream: false,
                    is_axis_pkt: false,
                },
                is_const,
            ));
        }
        if self.eat("char") || self.eat("int8_t") || self.eat("uint8_t") {
            return Ok((
                CType {
                    width: 8,
                    signed: !is_unsigned,
                    is_void: false,
                    is_stream: false,
                    is_axis_pkt: false,
                },
                is_const,
            ));
        }
        if self.eat("short") || self.eat("int16_t") || self.eat("uint16_t") {
            let _ = self.eat("int");
            return Ok((
                CType {
                    width: 16,
                    signed: !is_unsigned,
                    is_void: false,
                    is_stream: false,
                    is_axis_pkt: false,
                },
                is_const,
            ));
        }
        if self.eat("long") {
            let _ = self.eat("long");
            let _ = self.eat("int");
            return Ok((
                CType {
                    width: 64,
                    signed: !is_unsigned,
                    is_void: false,
                    is_stream: false,
                    is_axis_pkt: false,
                },
                is_const,
            ));
        }
        if self.eat("int")
            || self.eat("int32_t")
            || self.eat("uint32_t")
            || self.eat("size_t")
            || self.eat("float")
        {
            return Ok((
                CType {
                    width: 32,
                    signed: !is_unsigned,
                    is_void: false,
                    is_stream: false,
                    is_axis_pkt: false,
                },
                is_const,
            ));
        }
        if self.eat("int64_t") || self.eat("uint64_t") {
            return Ok((
                CType {
                    width: 64,
                    signed: !is_unsigned,
                    is_void: false,
                    is_stream: false,
                    is_axis_pkt: false,
                },
                is_const,
            ));
        }
        if self.eat("ap_uint") || self.eat("ap_int") || self.eat("ap_fixed") || self.eat("ap_ufixed") {
            if !self.eat("<") {
                return Err("expected '<' after ap_int/ap_uint".into());
            }
            let w = self.num().ok_or("ap_int width")? as usize;
            while !self.eat(">") {
                if self.peek() == Some(&Tok::Sym(">>".into())) {
                    break;
                }
                if self.peek().is_none() {
                    return Err("unterminated ap_int<...>".into());
                }
                self.bump();
            }
            return Ok((
                CType {
                    width: w.max(1),
                    signed: is_signed,
                    is_void: false,
                    is_stream: false,
                    is_axis_pkt: false,
                },
                is_const,
            ));
        }
        if self.eat("ap_axis") || self.eat("ap_axiu") {
            let mut w = 32usize;
            if self.eat("<") {
                if let Some(n) = self.num() {
                    w = (n as usize).max(1);
                }
                while !self.eat(">") {
                    if self.peek() == Some(&Tok::Sym(">>".into())) {
                        break;
                    }
                    if self.peek().is_none() {
                        return Err("unterminated ap_axis<...>".into());
                    }
                    self.bump();
                }
            }
            return Ok((
                CType {
                    width: w,
                    signed: false,
                    is_void: false,
                    is_stream: false,
                    is_axis_pkt: true,
                },
                is_const,
            ));
        }
        if self.eat("stream") {
            if !self.eat("<") {
                return Err("expected '<' after stream".into());
            }
            let (inner, _) = self.parse_type()?;
            if !self.eat(">") && !self.eat(">>") {
                return Err("expected '>' after stream<T>".into());
            }
            return Ok((
                CType {
                    width: inner.width.max(1),
                    signed: inner.signed,
                    is_void: false,
                    is_stream: true,
                    is_axis_pkt: inner.is_axis_pkt,
                },
                is_const,
            ));
        }
        if is_unsigned || is_signed {
            return Ok((
                CType {
                    width: 32,
                    signed: !is_unsigned,
                    is_void: false,
                    is_stream: false,
                    is_axis_pkt: false,
                },
                is_const,
            ));
        }
        if let Some(Tok::Ident(name)) = self.peek() {
            if let Some(ct) = self.typedefs.get(name).cloned() {
                self.i += 1;
                return Ok((ct, is_const));
            }
        }
        Err(format!("unknown C/HLS type at {:?}", self.peek()))
    }

    pub fn parse_expr(&mut self) -> Result<CExpr, String> {
        self.parse_ternary()
    }

    fn parse_ternary(&mut self) -> Result<CExpr, String> {
        let cond = self.parse_log_or()?;
        if self.eat("?") {
            let then_e = self.parse_expr()?;
            if !self.eat(":") {
                return Err("expected ':' in ternary expression".into());
            }
            let else_e = self.parse_ternary()?;
            return Ok(CExpr::Ternary {
                cond: Box::new(cond),
                then_e: Box::new(then_e),
                else_e: Box::new(else_e),
            });
        }
        Ok(cond)
    }

    fn parse_log_or(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_log_and()?;
        while self.eat("||") {
            let r = self.parse_log_and()?;
            e = CExpr::Binary {
                op: BinOp::LogOr,
                lhs: Box::new(e),
                rhs: Box::new(r),
            };
        }
        Ok(e)
    }

    fn parse_log_and(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_bit_or()?;
        while self.eat("&&") {
            let r = self.parse_bit_or()?;
            e = CExpr::Binary {
                op: BinOp::LogAnd,
                lhs: Box::new(e),
                rhs: Box::new(r),
            };
        }
        Ok(e)
    }

    fn parse_bit_or(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_bit_xor()?;
        while self.eat("|") {
            let r = self.parse_bit_xor()?;
            e = CExpr::Binary {
                op: BinOp::Or,
                lhs: Box::new(e),
                rhs: Box::new(r),
            };
        }
        Ok(e)
    }

    fn parse_bit_xor(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_bit_and()?;
        while self.eat("^") {
            let r = self.parse_bit_and()?;
            e = CExpr::Binary {
                op: BinOp::Xor,
                lhs: Box::new(e),
                rhs: Box::new(r),
            };
        }
        Ok(e)
    }

    fn parse_bit_and(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_eq()?;
        while self.eat("&") {
            let r = self.parse_eq()?;
            e = CExpr::Binary {
                op: BinOp::And,
                lhs: Box::new(e),
                rhs: Box::new(r),
            };
        }
        Ok(e)
    }

    fn parse_eq(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_rel()?;
        loop {
            if self.eat("==") {
                let r = self.parse_rel()?;
                e = CExpr::Binary {
                    op: BinOp::Eq,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else if self.eat("!=") {
                let r = self.parse_rel()?;
                e = CExpr::Binary {
                    op: BinOp::Ne,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_rel(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_shift()?;
        loop {
            if self.eat("<=") {
                let r = self.parse_shift()?;
                e = CExpr::Binary {
                    op: BinOp::Le,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else if self.eat(">=") {
                let r = self.parse_shift()?;
                e = CExpr::Binary {
                    op: BinOp::Ge,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else if self.eat("<") {
                let r = self.parse_shift()?;
                e = CExpr::Binary {
                    op: BinOp::Lt,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else if self.eat(">") {
                let r = self.parse_shift()?;
                e = CExpr::Binary {
                    op: BinOp::Gt,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_shift(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_add()?;
        loop {
            if self.eat("<<") {
                let r = self.parse_add()?;
                e = CExpr::Binary {
                    op: BinOp::Shl,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else if self.eat(">>") {
                let r = self.parse_add()?;
                e = CExpr::Binary {
                    op: BinOp::Shr,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_add(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_mul()?;
        loop {
            if self.eat("+") {
                let r = self.parse_mul()?;
                e = CExpr::Binary {
                    op: BinOp::Add,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else if self.eat("-") {
                let r = self.parse_mul()?;
                e = CExpr::Binary {
                    op: BinOp::Sub,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_mul(&mut self) -> Result<CExpr, String> {
        let mut e = self.parse_unary()?;
        loop {
            if self.eat("*") {
                let r = self.parse_unary()?;
                e = CExpr::Binary {
                    op: BinOp::Mul,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else if self.eat("/") {
                let r = self.parse_unary()?;
                e = CExpr::Binary {
                    op: BinOp::Div,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else if self.eat("%") {
                let r = self.parse_unary()?;
                e = CExpr::Binary {
                    op: BinOp::Mod,
                    lhs: Box::new(e),
                    rhs: Box::new(r),
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_unary(&mut self) -> Result<CExpr, String> {
        if self.eat("!") {
            return Ok(CExpr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.eat("~") {
            return Ok(CExpr::Unary {
                op: UnaryOp::BitNot,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.eat("-") {
            let inner = self.parse_unary()?;
            if let CExpr::Num(n) = inner {
                return Ok(CExpr::Num(-n));
            }
            return Ok(CExpr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(inner),
            });
        }
        if self.eat("+") {
            return self.parse_unary();
        }
        if self.eat("*") {
            let n = self.ident()?;
            return Ok(CExpr::Deref(n));
        }
        if self.eat("(") {
            // Check for C-style cast `(int)expr` or `(ap_uint<8>)expr`
            let save = self.i;
            if self.parse_type().is_ok() && self.eat(")") {
                return self.parse_unary();
            }
            self.i = save;
            let e = self.parse_expr()?;
            if !self.eat(")") {
                return Err("expected ')'".into());
            }
            return Ok(e);
        }
        if let Some(Tok::Num(n)) = self.peek() {
            let v = *n;
            self.i += 1;
            return Ok(CExpr::Num(v));
        }
        let mut n = self.ident()?;
        while self.eat("::") {
            n = self.ident()?;
        }
        if let Some(&val) = self.const_vals.get(&n) {
            return Ok(CExpr::Num(val));
        }
        // Member access: `s.read()`, `s.empty()`, `s.full()`, `pkt.data`, `x.range(hi, lo)`, `x.bit(i)`
        if self.eat(".") || self.eat("->") {
            let member = self.ident()?;
            if self.eat("(") {
                let mut args = Vec::new();
                while !self.eat(")") {
                    args.push(self.parse_expr()?);
                    let _ = self.eat(",");
                }
                return match member.as_str() {
                    "read" | "read_nb" => Ok(CExpr::StreamRead(n)),
                    "empty" => Ok(CExpr::StreamEmpty(n)),
                    "full" => Ok(CExpr::StreamFull(n)),
                    "bit" if args.len() == 1 => Ok(CExpr::Bit(n, Box::new(args[0].clone()))),
                    "range" if args.len() == 2 => {
                        let hi = self.eval_cexpr_const(&args[0]).unwrap_or(0) as usize;
                        let lo = self.eval_cexpr_const(&args[1]).unwrap_or(0) as usize;
                        Ok(CExpr::Slice(n, hi, lo))
                    }
                    _ => Ok(CExpr::Var(n)),
                };
            }
            // Struct field e.g. `pkt.data` -> treat as `n`
            return Ok(CExpr::Var(n));
        }
        // Function call or `ap_int` range select `x(hi, lo)`
        if self.eat("(") {
            let mut args = Vec::new();
            while !self.eat(")") {
                if self.peek().is_none() {
                    return Err("unterminated call arguments".into());
                }
                args.push(self.parse_expr()?);
                let _ = self.eat(",");
            }
            return Ok(CExpr::Call { func: n, args });
        }
        // Indexing `a[i]` or `A[i][j]`
        if self.eat("[") {
            let mut indices = Vec::new();
            let first = self.parse_expr()?;
            if !self.eat("]") {
                return Err("expected ']'".into());
            }
            indices.push(first);
            while self.eat("[") {
                let idx = self.parse_expr()?;
                if !self.eat("]") {
                    return Err("expected ']'".into());
                }
                indices.push(idx);
            }
            return Ok(CExpr::Index { array: n, indices });
        }
        Ok(CExpr::Var(n))
    }

    fn parse_block_or_stmt(&mut self) -> Result<Vec<CStmt>, String> {
        if self.eat("{") {
            let mut stmts = Vec::new();
            while !self.eat("}") {
                if self.peek().is_none() {
                    return Err("unterminated block".into());
                }
                self.parse_stmts_into(&mut stmts)?;
            }
            Ok(stmts)
        } else {
            let mut stmts = Vec::new();
            self.parse_stmts_into(&mut stmts)?;
            Ok(stmts)
        }
    }

    fn parse_for_loop(&mut self, label: Option<String>, pre_pragmas: Vec<HlsDirective>) -> Result<CStmt, String> {
        if !self.eat("(") {
            return Err("expected '(' after for".into());
        }
        let _ = self.parse_type();
        let var = self.ident()?;
        if !self.eat("=") {
            return Err("expected '=' in for init".into());
        }
        let start = self.eval_const_expr()? as i64;
        if !self.eat(";") {
            return Err("expected ';' after for init".into());
        }
        let _cond_var = self.ident()?;
        let mut inclusive = false;
        let cmp_down = if self.eat("<=") {
            inclusive = true;
            false
        } else if self.eat("<") || self.eat("!=") {
            false
        } else if self.eat(">=") {
            inclusive = true;
            true
        } else if self.eat(">") {
            true
        } else {
            return Err("unsupported for loop condition operator".into());
        };
        let end = self.eval_const_expr()? as i64;
        if !self.eat(";") {
            return Err("expected ';' after for condition".into());
        }
        // Step: i++, ++i, i--, --i, i += k, i -= k, i = i + k, i = i - k
        let mut step: i64 = if cmp_down { -1 } else { 1 };
        if self.eat("++") {
            let _ = self.ident();
            step = 1;
        } else if self.eat("--") {
            let _ = self.ident();
            step = -1;
        } else if matches!(self.peek(), Some(Tok::Ident(_))) {
            let _ = self.ident();
            if self.eat("++") {
                step = 1;
            } else if self.eat("--") {
                step = -1;
            } else if self.eat("+=") {
                step = self.eval_const_expr()? as i64;
            } else if self.eat("-=") {
                step = -(self.eval_const_expr()? as i64);
            } else if self.eat("=") {
                let _ = self.ident();
                if self.eat("+") {
                    step = self.eval_const_expr()? as i64;
                } else if self.eat("-") {
                    step = -(self.eval_const_expr()? as i64);
                }
            }
        }
        if !self.eat(")") {
            return Err("expected ')' after for step".into());
        }
        let raw_body = self.parse_block_or_stmt()?;
        let mut pragmas = pre_pragmas;
        let mut body = Vec::new();
        for s in raw_body {
            if let CStmt::Pragma(p) = s {
                pragmas.push(p);
            } else {
                body.push(s);
            }
        }
        Ok(CStmt::For {
            label,
            var,
            start,
            end,
            step,
            inclusive,
            pragmas,
            body,
        })
    }

    fn parse_stmts_into(&mut self, out: &mut Vec<CStmt>) -> Result<(), String> {
        while self.eat(";") {}
        let Some(pk) = self.peek().cloned() else {
            return Ok(());
        };
        if let Tok::Pragma(dir) = pk {
            self.i += 1;
            // If immediately followed by a `for` or labeled `for`, attach to that loop!
            if self.peek() == Some(&Tok::Ident("for".into())) {
                self.i += 1;
                let st = self.parse_for_loop(None, vec![dir])?;
                out.push(st);
                return Ok(());
            }
            if let (Some(Tok::Ident(_)), Some(Tok::Sym(colon)), Some(Tok::Ident(kw))) =
                (self.peek(), self.peek_n(1), self.peek_n(2))
            {
                if colon == ":" && kw == "for" {
                    let lbl = self.ident()?;
                    let _ = self.eat(":");
                    let _ = self.eat("for");
                    let st = self.parse_for_loop(Some(lbl), vec![dir])?;
                    out.push(st);
                    return Ok(());
                }
            }
            out.push(CStmt::Pragma(dir));
            return Ok(());
        }

        // Labeled statement: `LABEL: for (...)` or `LABEL: while (...)` or `LABEL: { ... }`
        if let (Some(Tok::Ident(_)), Some(Tok::Sym(colon))) = (self.peek(), self.peek_n(1)) {
            if colon == ":" {
                let lbl = self.ident()?;
                let _ = self.eat(":");
                let mut pre_pragmas = Vec::new();
                while let Some(Tok::Pragma(dir)) = self.peek().cloned() {
                    self.i += 1;
                    pre_pragmas.push(dir);
                }
                if self.eat("for") {
                    let st = self.parse_for_loop(Some(lbl), pre_pragmas)?;
                    out.push(st);
                    return Ok(());
                }
                if self.eat("while") {
                    if !self.eat("(") {
                        return Err("expected '(' after while".into());
                    }
                    let cond = self.parse_expr()?;
                    if !self.eat(")") {
                        return Err("expected ')' after while condition".into());
                    }
                    let raw_body = self.parse_block_or_stmt()?;
                    let mut pragmas = pre_pragmas;
                    let mut body = Vec::new();
                    for s in raw_body {
                        if let CStmt::Pragma(p) = s {
                            pragmas.push(p);
                        } else {
                            body.push(s);
                        }
                    }
                    out.push(CStmt::While {
                        label: Some(lbl),
                        cond,
                        trip_bound: 16,
                        pragmas,
                        body,
                    });
                    return Ok(());
                }
            }
        }

        if self.eat("return") {
            if self.eat(";") {
                out.push(CStmt::Return(None));
                return Ok(());
            }
            let e = self.parse_expr()?;
            let _ = self.eat(";");
            out.push(CStmt::Return(Some(e)));
            return Ok(());
        }

        if self.eat("for") {
            let st = self.parse_for_loop(None, Vec::new())?;
            out.push(st);
            return Ok(());
        }

        if self.eat("while") {
            if !self.eat("(") {
                return Err("expected '(' after while".into());
            }
            let cond = self.parse_expr()?;
            if !self.eat(")") {
                return Err("expected ')' after while condition".into());
            }
            let raw_body = self.parse_block_or_stmt()?;
            let mut pragmas = Vec::new();
            let mut body = Vec::new();
            for s in raw_body {
                if let CStmt::Pragma(p) = s {
                    pragmas.push(p);
                } else {
                    body.push(s);
                }
            }
            out.push(CStmt::While {
                label: None,
                cond,
                trip_bound: 16,
                pragmas,
                body,
            });
            return Ok(());
        }

        if self.eat("if") {
            if !self.eat("(") {
                return Err("expected '(' after if".into());
            }
            let cond = self.parse_expr()?;
            if !self.eat(")") {
                return Err("expected ')' after if condition".into());
            }
            let then_body = self.parse_block_or_stmt()?;
            let else_body = if self.eat("else") {
                self.parse_block_or_stmt()?
            } else {
                Vec::new()
            };
            out.push(CStmt::If {
                cond,
                then_body,
                else_body,
            });
            return Ok(());
        }

        // Check for variable declaration (scalar or array, possibly comma-separated)
        let save = self.i;
        let is_static = self.eat("static");
        if let Ok((ctype, is_const)) = self.parse_type() {
            let _ = self.eat("*");
            let _ = self.eat("&");
            if matches!(self.peek(), Some(Tok::Ident(_))) {
                loop {
                    let _ = self.eat("*");
                    let _ = self.eat("&");
                    let name = self.ident()?;
                    let mut dims = Vec::new();
                    while self.eat("[") {
                        if self.eat("]") {
                            dims.push(0);
                        } else {
                            let d = self.eval_const_expr()? as usize;
                            if !self.eat("]") {
                                return Err("expected ']' in array declaration".into());
                            }
                            dims.push(d.max(1));
                        }
                    }
                    let mut init = None;
                    let mut init_list = Vec::new();
                    if self.eat("=") {
                        if self.eat("{") {
                            while !self.eat("}") {
                                if self.eat("{") {
                                    while !self.eat("}") {
                                        let v = self.eval_const_expr()?;
                                        init_list.push(v);
                                        let _ = self.eat(",");
                                    }
                                } else {
                                    let v = self.eval_const_expr()?;
                                    init_list.push(v);
                                }
                                let _ = self.eat(",");
                            }
                            if dims.len() == 1 && dims[0] == 0 {
                                dims[0] = init_list.len().max(1);
                            }
                        } else {
                            let e = self.parse_expr()?;
                            if is_const && dims.is_empty() {
                                if let Some(cv) = self.eval_cexpr_const(&e) {
                                    self.const_vals.insert(name.clone(), cv);
                                }
                            }
                            init = Some(e);
                        }
                    }
                    out.push(CStmt::Decl(CVar {
                        name: name.clone(),
                        width: ctype.width.max(1),
                        signed: ctype.signed,
                        is_static,
                        is_const,
                        is_stream: ctype.is_stream,
                        dims: dims.clone(),
                        init: init.clone(),
                        init_list,
                    }));
                    // If a non-static local scalar has a runtime initializer, also emit an Assign
                    if !is_static && dims.is_empty() {
                        if let Some(init_e) = init {
                            if !matches!(init_e, CExpr::Num(0)) {
                                out.push(CStmt::Assign {
                                    lhs: LValue::Var(name),
                                    rhs: init_e,
                                });
                            }
                        }
                    }
                    if !self.eat(",") {
                        break;
                    }
                }
                let _ = self.eat(";");
                return Ok(());
            }
        }
        self.i = save;

        // Prefix ++name / --name
        if self.eat("++") {
            let n = self.ident()?;
            let _ = self.eat(";");
            out.push(CStmt::Assign {
                lhs: LValue::Var(n.clone()),
                rhs: CExpr::Binary {
                    op: BinOp::Add,
                    lhs: Box::new(CExpr::Var(n)),
                    rhs: Box::new(CExpr::Num(1)),
                },
            });
            return Ok(());
        }
        if self.eat("--") {
            let n = self.ident()?;
            let _ = self.eat(";");
            out.push(CStmt::Assign {
                lhs: LValue::Var(n.clone()),
                rhs: CExpr::Binary {
                    op: BinOp::Sub,
                    lhs: Box::new(CExpr::Var(n)),
                    rhs: Box::new(CExpr::Num(1)),
                },
            });
            return Ok(());
        }

        let deref = self.eat("*");
        let name = self.ident()?;

        // Stream method call `s.write(val);` or `s.read(var);`
        if self.eat(".") || self.eat("->") {
            let method = self.ident()?;
            if self.eat("(") {
                let mut args = Vec::new();
                while !self.eat(")") {
                    args.push(self.parse_expr()?);
                    let _ = self.eat(",");
                }
                let _ = self.eat(";");
                if method == "write" || method == "write_nb" {
                    if let Some(v) = args.into_iter().next() {
                        out.push(CStmt::StreamWrite { stream: name, val: v });
                        return Ok(());
                    }
                } else if method == "read" || method == "read_nb" {
                    if let Some(CExpr::Var(dst)) = args.into_iter().next() {
                        out.push(CStmt::Assign {
                            lhs: LValue::Var(dst),
                            rhs: CExpr::StreamRead(name),
                        });
                        return Ok(());
                    }
                } else if method == "range" && args.len() == 2 && self.eat("=") {
                    let hi = self.eval_cexpr_const(&args[0]).unwrap_or(0) as usize;
                    let lo = self.eval_cexpr_const(&args[1]).unwrap_or(0) as usize;
                    let rhs = self.parse_expr()?;
                    let _ = self.eat(";");
                    out.push(CStmt::Assign {
                        lhs: LValue::Slice(name, hi, lo),
                        rhs,
                    });
                    return Ok(());
                }
                return Ok(());
            }
            // Struct field assignment `pkt.data = val;`
            let _ = self.eat("=");
            let rhs = self.parse_expr()?;
            let _ = self.eat(";");
            out.push(CStmt::Assign {
                lhs: LValue::Var(name),
                rhs,
            });
            return Ok(());
        }

        // Stream operator `s << val;` or `s >> var;`
        if self.eat("<<") {
            let val = self.parse_expr()?;
            let _ = self.eat(";");
            out.push(CStmt::StreamWrite { stream: name, val });
            return Ok(());
        }
        if self.eat(">>") {
            let dst = self.ident()?;
            let _ = self.eat(";");
            out.push(CStmt::Assign {
                lhs: LValue::Var(dst),
                rhs: CExpr::StreamRead(name),
            });
            return Ok(());
        }

        // Standalone function call `func(a, b, c);`
        if self.eat("(") {
            let mut args = Vec::new();
            while !self.eat(")") {
                if self.peek().is_none() {
                    return Err("unterminated function call".into());
                }
                args.push(self.parse_expr()?);
                let _ = self.eat(",");
            }
            let _ = self.eat(";");
            out.push(CStmt::Call { func: name, args });
            return Ok(());
        }

        // Postfix `name++;` or `name--;`
        if self.eat("++") {
            let _ = self.eat(";");
            out.push(CStmt::Assign {
                lhs: LValue::Var(name.clone()),
                rhs: CExpr::Binary {
                    op: BinOp::Add,
                    lhs: Box::new(CExpr::Var(name)),
                    rhs: Box::new(CExpr::Num(1)),
                },
            });
            return Ok(());
        }
        if self.eat("--") {
            let _ = self.eat(";");
            out.push(CStmt::Assign {
                lhs: LValue::Var(name.clone()),
                rhs: CExpr::Binary {
                    op: BinOp::Sub,
                    lhs: Box::new(CExpr::Var(name)),
                    rhs: Box::new(CExpr::Num(1)),
                },
            });
            return Ok(());
        }

        // Array indexing or bit indexing on LHS
        let mut indices = Vec::new();
        while self.eat("[") {
            let idx = self.parse_expr()?;
            if !self.eat("]") {
                return Err("expected ']' on LHS".into());
            }
            indices.push(idx);
        }

        let lhs = if deref {
            LValue::Deref(name.clone())
        } else if !indices.is_empty() {
            LValue::ArrayElem {
                array: name.clone(),
                indices: indices.clone(),
            }
        } else {
            LValue::Var(name.clone())
        };

        let lhs_as_expr = || {
            if deref {
                CExpr::Deref(name.clone())
            } else if !indices.is_empty() {
                CExpr::Index {
                    array: name.clone(),
                    indices: indices.clone(),
                }
            } else {
                CExpr::Var(name.clone())
            }
        };

        let compound_op = if self.eat("+=") {
            Some(BinOp::Add)
        } else if self.eat("-=") {
            Some(BinOp::Sub)
        } else if self.eat("*=") {
            Some(BinOp::Mul)
        } else if self.eat("&=") {
            Some(BinOp::And)
        } else if self.eat("|=") {
            Some(BinOp::Or)
        } else if self.eat("^=") {
            Some(BinOp::Xor)
        } else if self.eat("<<=") {
            Some(BinOp::Shl)
        } else if self.eat(">>=") {
            Some(BinOp::Shr)
        } else {
            None
        };

        if let Some(op) = compound_op {
            let rhs_part = self.parse_expr()?;
            let _ = self.eat(";");
            out.push(CStmt::Assign {
                lhs,
                rhs: CExpr::Binary {
                    op,
                    lhs: Box::new(lhs_as_expr()),
                    rhs: Box::new(rhs_part),
                },
            });
            return Ok(());
        }

        if !self.eat("=") {
            return Err(format!("expected assignment operator after '{name}'"));
        }
        let rhs = self.parse_expr()?;
        let _ = self.eat(";");
        out.push(CStmt::Assign { lhs, rhs });
        Ok(())
    }

    pub fn parse_translation_unit(&mut self) -> Result<TranslationUnit, String> {
        let mut functions = Vec::new();
        let mut global_vars = Vec::new();
        let mut top_pragmas = Vec::new();

        while self.peek().is_some() {
            while self.eat(";") {}
            if self.peek().is_none() {
                break;
            }
            if let Some(Tok::Pragma(dir)) = self.peek().cloned() {
                self.i += 1;
                top_pragmas.push(dir);
                continue;
            }
            // `typedef <type> <name>;`
            if self.eat("typedef") {
                let (ct, _) = self.parse_type()?;
                let alias = self.ident()?;
                let _ = self.eat(";");
                self.typedefs.insert(alias, ct);
                continue;
            }
            // `using <name> = <type>;`
            if self.eat("using") {
                if self.eat("namespace") {
                    let _ = self.ident();
                    let _ = self.eat(";");
                    continue;
                }
                let alias = self.ident()?;
                if self.eat("=") {
                    let (ct, _) = self.parse_type()?;
                    self.typedefs.insert(alias, ct);
                }
                let _ = self.eat(";");
                continue;
            }
            // `extern "C" { ... }`
            if self.eat("extern") {
                if self.eat("{") {
                    continue;
                }
            }
            if self.eat("}") {
                continue;
            }

            let is_inline = self.eat("inline") || self.eat("static");
            let _ = self.eat("inline");
            let (ret_type, is_const_top) = self.parse_type()?;
            let _ = self.eat("*");
            let name = self.ident()?;

            // Check if this is a global variable / constant declaration rather than a function
            if !self.eat("(") {
                let mut dims = Vec::new();
                while self.eat("[") {
                    let d = self.eval_const_expr()? as usize;
                    if !self.eat("]") {
                        return Err("expected ']' in global array".into());
                    }
                    dims.push(d.max(1));
                }
                let mut init = None;
                let mut init_list = Vec::new();
                if self.eat("=") {
                    if self.eat("{") {
                        while !self.eat("}") {
                            let v = self.eval_const_expr()?;
                            init_list.push(v);
                            let _ = self.eat(",");
                        }
                    } else {
                        let e = self.parse_expr()?;
                        if is_const_top && dims.is_empty() {
                            if let Some(cv) = self.eval_cexpr_const(&e) {
                                self.const_vals.insert(name.clone(), cv);
                            }
                        }
                        init = Some(e);
                    }
                }
                let _ = self.eat(";");
                global_vars.push(CVar {
                    name,
                    width: ret_type.width.max(1),
                    signed: ret_type.signed,
                    is_static: true,
                    is_const: is_const_top,
                    is_stream: ret_type.is_stream,
                    dims,
                    init,
                    init_list,
                });
                continue;
            }

            // Function parameters
            let mut args = Vec::new();
            while !self.eat(")") {
                if self.peek().is_none() {
                    return Err("unterminated function parameter list".into());
                }
                if self.eat("void") && self.peek() == Some(&Tok::Sym(")".into())) {
                    break;
                }
                let (arg_t, is_const) = self.parse_type()?;
                let is_ptr = self.eat("*");
                let is_ref = self.eat("&");
                let arg_name = self.ident()?;
                let mut dims = Vec::new();
                while self.eat("[") {
                    if self.eat("]") {
                        dims.push(16);
                    } else {
                        let d = self.eval_const_expr()? as usize;
                        if !self.eat("]") {
                            return Err("expected ']' in parameter array".into());
                        }
                        dims.push(d.max(1));
                    }
                }
                args.push(CArg {
                    name: arg_name,
                    width: arg_t.width.max(1),
                    signed: arg_t.signed,
                    is_ptr,
                    is_ref,
                    is_const,
                    is_stream: arg_t.is_stream,
                    is_axis_pkt: arg_t.is_axis_pkt,
                    output: (is_ptr || is_ref) && !is_const,
                    dims,
                });
                let _ = self.eat(",");
            }

            // Skip forward declaration `void foo(...);`
            if self.eat(";") {
                continue;
            }
            if !self.eat("{") {
                return Err(format!("expected '{{' for function '{name}'"));
            }

            let mut body_raw = Vec::new();
            while !self.eat("}") {
                if self.peek().is_none() {
                    return Err(format!("unterminated function '{name}'"));
                }
                self.parse_stmts_into(&mut body_raw)?;
            }

            // Determine which array/stream parameters are written vs read in the body
            let mut vars = global_vars.clone();
            let mut pragmas = std::mem::take(&mut top_pragmas);
            let mut body = Vec::new();
            for st in body_raw {
                match st {
                    CStmt::Pragma(p) => pragmas.push(p),
                    CStmt::Decl(v) => vars.push(v),
                    other => body.push(other),
                }
            }

            // Refine `output` flag on array and stream parameters by inspecting writes
            for arg in &mut args {
                if !arg.dims.is_empty() || arg.is_stream || arg.is_ptr || arg.is_ref {
                    if stmts_write_to(&body, &arg.name) {
                        arg.output = true;
                    } else if !arg.dims.is_empty() || arg.is_stream || arg.is_const {
                        arg.output = false;
                    }
                }
            }

            functions.push(CFunction {
                name,
                ret_width: ret_type.width,
                ret_signed: ret_type.signed,
                args,
                vars,
                body,
                pragmas,
                is_inline,
            });
        }

        if functions.is_empty() {
            return Err("hls: no function definitions found".into());
        }

        Ok(TranslationUnit {
            functions,
            global_vars,
            top_pragmas,
        })
    }
}

fn stmts_write_to(stmts: &[CStmt], target: &str) -> bool {
    for st in stmts {
        match st {
            CStmt::Assign { lhs, .. } => match lhs {
                LValue::Var(n) | LValue::Deref(n) | LValue::Bit(n, _) | LValue::Slice(n, _, _) => {
                    if n == target {
                        return true;
                    }
                }
                LValue::ArrayElem { array, .. } => {
                    if array == target {
                        return true;
                    }
                }
            },
            CStmt::StreamWrite { stream, .. } if stream == target => return true,
            CStmt::For { body, .. } | CStmt::While { body, .. } => {
                if stmts_write_to(body, target) {
                    return true;
                }
            }
            CStmt::If {
                then_body,
                else_body,
                ..
            } => {
                if stmts_write_to(then_body, target) || stmts_write_to(else_body, target) {
                    return true;
                }
            }
            CStmt::Call { args, .. } => {
                // Typically the last argument(s) in a dataflow stage call are outputs
                if let Some(CExpr::Var(last)) = args.last() {
                    if last == target {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}
