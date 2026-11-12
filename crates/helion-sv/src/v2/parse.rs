//! Recursive-descent parser for the synthesizable SystemVerilog subset.
//!
//! Simulation-only constructs that do not change the hardware (system task
//! calls, assertions, `initial` blocks, delays, `specify` blocks) are parsed
//! and skipped. Anything else outside the subset is an error with its line.

use super::ast::*;
use super::lex::{lex, Num, Tok, T};
use std::rc::Rc;

type R<X> = Result<X, String>;

struct P {
    t: Vec<Tok>,
    i: usize,
    /// Names known to be types in the current file (typedefs, type parameters).
    types: std::collections::HashSet<String>,
}

const TYPE_KW: &[&str] = &[
    "logic", "reg", "wire", "bit", "int", "integer", "byte", "shortint", "longint", "time", "tri", "wand", "wor", "tri0", "tri1", "supply0", "supply1",
    "uwire", "var", "enum", "struct", "union", "signed", "unsigned", "string", "real", "shortreal", "realtime", "event",
];

fn int_num(v: u32) -> Expr {
    let bits: Vec<bool> = (0..32).map(|i| (v >> i) & 1 == 1).collect();
    Expr::Num(Num { bits, xz: vec![false; 32], sized: false, signed: true, fill: false })
}

impl P {
    fn peek(&self) -> &T {
        &self.t[self.i].t
    }
    fn peek_at(&self, k: usize) -> &T {
        &self.t[(self.i + k).min(self.t.len() - 1)].t
    }
    fn line(&self) -> u32 {
        self.t[self.i].line
    }
    fn bump(&mut self) -> T {
        let t = self.t[self.i].t.clone();
        if self.i + 1 < self.t.len() {
            self.i += 1;
        }
        t
    }
    fn is_op(&self, o: &str) -> bool {
        matches!(self.peek(), T::Op(x) if *x == o)
    }
    fn is_kw(&self, k: &str) -> bool {
        matches!(self.peek(), T::Id(x) if x == k)
    }
    fn kw_at(&self, n: usize, k: &str) -> bool {
        matches!(self.peek_at(n), T::Id(x) if x == k)
    }
    fn op_at(&self, n: usize, o: &str) -> bool {
        matches!(self.peek_at(n), T::Op(x) if *x == o)
    }
    fn eat_op(&mut self, o: &str) -> bool {
        if self.is_op(o) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, k: &str) -> bool {
        if self.is_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn err<X>(&self, msg: impl AsRef<str>) -> R<X> {
        let near = match self.peek() {
            T::Id(s) => format!("'{s}'"),
            T::Op(o) => format!("'{o}'"),
            T::Num(_) => "a number".to_string(),
            T::Str(_) => "a string".to_string(),
            T::Sys(s) => format!("'${s}'"),
            T::Eof => "end of file".to_string(),
        };
        Err(format!("line {}: {} (at {near})", self.line(), msg.as_ref()))
    }
    fn expect_op(&mut self, o: &str) -> R<()> {
        if self.eat_op(o) {
            Ok(())
        } else {
            self.err(format!("expected '{o}'"))
        }
    }
    fn expect_kw(&mut self, k: &str) -> R<()> {
        if self.eat_kw(k) {
            Ok(())
        } else {
            self.err(format!("expected '{k}'"))
        }
    }
    fn ident(&mut self) -> R<String> {
        match self.peek().clone() {
            T::Id(s) => {
                self.bump();
                Ok(s)
            }
            _ => self.err("expected a name"),
        }
    }
    /// Skip a balanced `( ... )` starting at the current `(`.
    fn skip_parens(&mut self) -> R<()> {
        self.expect_op("(")?;
        let mut depth = 1;
        while depth > 0 {
            match self.bump() {
                T::Op("(") => depth += 1,
                T::Op(")") => depth -= 1,
                T::Eof => return self.err("unbalanced parentheses"),
                _ => {}
            }
        }
        Ok(())
    }
    fn skip_to_semi(&mut self) -> R<()> {
        loop {
            match self.bump() {
                T::Op(";") => return Ok(()),
                T::Op("(") => {
                    self.i -= 1;
                    self.skip_parens()?;
                }
                T::Eof => return self.err("missing ';'"),
                _ => {}
            }
        }
    }
    /// Skip until the keyword `end_kw` (handles nesting of `open_kw`).
    fn skip_until_kw(&mut self, open_kw: &str, end_kw: &str) -> R<()> {
        let mut depth = 1;
        loop {
            match self.bump() {
                T::Id(s) if s == open_kw => depth += 1,
                T::Id(s) if s == end_kw => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                T::Eof => return self.err(format!("missing '{end_kw}'")),
                _ => {}
            }
        }
    }
    fn skip_label(&mut self) {
        if self.is_op(":") {
            self.bump();
            if matches!(self.peek(), T::Id(_)) {
                self.bump();
            }
        }
    }
    /// `#delay` in front of a statement or after an assignment operator.
    fn skip_delay(&mut self) -> R<()> {
        if self.eat_op("#") {
            if self.is_op("(") {
                self.skip_parens()?;
            } else {
                self.bump();
            }
        }
        Ok(())
    }

    // ----------------------------------------------------------------- types

    fn at_type(&self) -> bool {
        match self.peek() {
            T::Id(s) => {
                if TYPE_KW.contains(&s.as_str()) {
                    return true;
                }
                if self.types.contains(s) {
                    return true;
                }
                // pkg::name name  |  name name  (user type followed by a declarator)
                if self.op_at(1, "::") {
                    return matches!(self.peek_at(2), T::Id(_)) && matches!(self.peek_at(3), T::Id(_) | T::Op("["));
                }
                false
            }
            _ => false,
        }
    }

    fn dims(&mut self) -> R<Vec<Dim>> {
        let mut v = Vec::new();
        while self.is_op("[") {
            self.bump();
            let a = self.expr()?;
            self.expect_op(":")?;
            let b = self.expr()?;
            self.expect_op("]")?;
            v.push(Dim(a, b));
        }
        Ok(v)
    }

    fn unpacked_dims(&mut self) -> R<Vec<(Expr, Option<Expr>)>> {
        let mut v = Vec::new();
        while self.is_op("[") {
            self.bump();
            let a = self.expr()?;
            let b = if self.eat_op(":") { Some(self.expr()?) } else { None };
            self.expect_op("]")?;
            v.push((a, b));
        }
        Ok(v)
    }

    fn signing(&mut self) -> Option<bool> {
        if self.eat_kw("signed") {
            Some(true)
        } else if self.eat_kw("unsigned") {
            Some(false)
        } else {
            None
        }
    }

    fn data_type(&mut self) -> R<DataType> {
        // Net/variable keywords in front of the real type.
        loop {
            if self.is_kw("var") || self.is_kw("const") || self.is_kw("static") || self.is_kw("automatic") {
                self.bump();
                continue;
            }
            if ["wire", "tri", "wand", "wor", "tri0", "tri1", "uwire", "supply0", "supply1"].iter().any(|k| self.is_kw(k)) {
                // `wire logic [..] x` or plain `wire [..] x`
                self.bump();
                if self.is_kw("logic") || self.is_kw("reg") || self.is_kw("bit") {
                    continue;
                }
                let signed = self.signing().unwrap_or(false);
                let dims = self.dims()?;
                return Ok(DataType::Vec { signed, dims });
            }
            break;
        }
        let Some(T::Id(k)) = Some(self.peek().clone()) else {
            if self.is_op("[") {
                let dims = self.dims()?;
                return Ok(DataType::Vec { signed: false, dims });
            }
            return self.err("expected a data type");
        };
        match k.as_str() {
            "logic" | "reg" | "bit" => {
                self.bump();
                let signed = self.signing().unwrap_or(false);
                let dims = self.dims()?;
                Ok(DataType::Vec { signed, dims })
            }
            "signed" | "unsigned" => {
                let signed = self.signing().unwrap_or(false);
                let dims = self.dims()?;
                Ok(DataType::Vec { signed, dims })
            }
            "int" | "integer" | "byte" | "shortint" | "longint" | "time" => {
                self.bump();
                let (width, dflt) = match k.as_str() {
                    "byte" => (8, true),
                    "shortint" => (16, true),
                    "longint" => (64, true),
                    "time" => (64, false),
                    _ => (32, true),
                };
                let signed = self.signing().unwrap_or(dflt);
                Ok(DataType::Int { width, signed })
            }
            "enum" => {
                self.bump();
                let base = if self.is_op("{") { None } else { Some(Box::new(self.data_type()?)) };
                self.expect_op("{")?;
                let mut items = Vec::new();
                loop {
                    let name = self.ident()?;
                    if self.is_op("[") {
                        return self.err("enum name ranges are not supported");
                    }
                    let val = if self.eat_op("=") { Some(self.expr()?) } else { None };
                    items.push((name, val));
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op("}")?;
                let dims = self.dims()?;
                Ok(DataType::Enum { base, items, dims })
            }
            "struct" | "union" => {
                self.bump();
                let union = k == "union";
                if !self.eat_kw("packed") {
                    return self.err("only packed structs and unions are synthesizable here");
                }
                let signed = self.signing().unwrap_or(false);
                self.expect_op("{")?;
                let mut fields = Vec::new();
                while !self.is_op("}") {
                    let ty = self.data_type()?;
                    loop {
                        let name = self.ident()?;
                        if self.is_op("[") {
                            return self.err("unpacked array inside a packed struct");
                        }
                        fields.push((ty.clone(), name));
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    self.expect_op(";")?;
                }
                self.expect_op("}")?;
                let dims = self.dims()?;
                Ok(DataType::Struct { fields, dims, signed, union })
            }
            "string" | "real" | "shortreal" | "realtime" | "event" => self.err(format!("type '{k}' is not synthesizable")),
            _ => {
                let mut path = vec![self.ident()?];
                while self.eat_op("::") {
                    path.push(self.ident()?);
                }
                let dims = self.dims()?;
                Ok(DataType::Named { path, dims })
            }
        }
    }

    /// A data type, or an implicit one when the next token is already the
    /// declarator name.
    fn opt_data_type(&mut self) -> R<DataType> {
        if matches!(self.peek(), T::Id(_)) && !self.at_type() {
            // `name` alone: implicit 1-bit logic.
            let next_is_decl = matches!(self.peek_at(1), T::Op(",") | T::Op(";") | T::Op("=") | T::Op(")") | T::Op("["));
            if next_is_decl {
                return Ok(DataType::Vec { signed: false, dims: vec![] });
            }
        }
        self.data_type()
    }

    fn declarator(&mut self) -> R<Declarator> {
        let name = self.ident()?;
        let unpacked = self.unpacked_dims()?;
        let init = if self.eat_op("=") { Some(self.expr()?) } else { None };
        Ok(Declarator { name, unpacked, init })
    }

    // ----------------------------------------------------------- expressions

    fn expr(&mut self) -> R<Expr> {
        let c = self.binary(0)?;
        if self.eat_op("?") {
            let a = self.expr()?;
            self.expect_op(":")?;
            let b = self.expr()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    fn bin_prec(&self) -> Option<(u8, &'static str)> {
        let T::Op(o) = self.peek() else {
            if self.is_kw("inside") {
                return Some((7, "inside"));
            }
            return None;
        };
        let p = match *o {
            "||" => 1,
            "&&" => 2,
            "|" => 3,
            "^" | "^~" | "~^" => 4,
            "&" => 5,
            "==" | "!=" | "===" | "!==" | "==?" | "!=?" => 6,
            "<" | "<=" | ">" | ">=" => 7,
            "<<" | ">>" | "<<<" | ">>>" => 8,
            "+" | "-" => 9,
            "*" | "/" | "%" => 10,
            "**" => 11,
            _ => return None,
        };
        Some((p, o))
    }

    fn binary(&mut self, min: u8) -> R<Expr> {
        let mut lhs = self.unary()?;
        while let Some((p, op)) = self.bin_prec() {
            if p < min {
                break;
            }
            self.bump();
            if op == "inside" {
                self.expect_op("{")?;
                let items = self.inside_items("}")?;
                lhs = Expr::Inside(Box::new(lhs), items);
                continue;
            }
            let rhs = self.binary(p + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn inside_items(&mut self, close: &str) -> R<Vec<InsideItem>> {
        let mut items = Vec::new();
        loop {
            if self.is_op("[") {
                self.bump();
                let a = self.expr()?;
                self.expect_op(":")?;
                let b = self.expr()?;
                self.expect_op("]")?;
                items.push(InsideItem::Range(a, b));
            } else {
                items.push(InsideItem::Value(self.expr()?));
            }
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(close)?;
        Ok(items)
    }

    fn unary(&mut self) -> R<Expr> {
        if let T::Op(o) = self.peek() {
            let o = *o;
            if matches!(o, "+" | "-" | "!" | "~" | "&" | "|" | "^" | "~&" | "~|" | "~^" | "^~") {
                self.bump();
                let e = self.unary()?;
                return Ok(Expr::Unary(o, Box::new(e)));
            }
            if o == "++" || o == "--" {
                return self.err("increment inside an expression is not supported");
            }
        }
        self.postfix()
    }

    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.is_op("[") {
                self.bump();
                let a = self.expr()?;
                if self.eat_op(":") {
                    let b = self.expr()?;
                    self.expect_op("]")?;
                    e = Expr::Range(Box::new(e), Box::new(a), Box::new(b));
                } else if self.eat_op("+:") {
                    let b = self.expr()?;
                    self.expect_op("]")?;
                    e = Expr::PlusRange(Box::new(e), Box::new(a), Box::new(b), true);
                } else if self.eat_op("-:") {
                    let b = self.expr()?;
                    self.expect_op("]")?;
                    e = Expr::PlusRange(Box::new(e), Box::new(a), Box::new(b), false);
                } else {
                    self.expect_op("]")?;
                    e = Expr::Index(Box::new(e), Box::new(a));
                }
            } else if self.is_op(".") && matches!(self.peek_at(1), T::Id(_)) {
                self.bump();
                let f = self.ident()?;
                e = Expr::Member(Box::new(e), f);
            } else if self.is_op("'") && self.op_at(1, "(") {
                // cast: expr'(value) with expr a width or a type name
                self.bump();
                self.bump();
                let v = self.expr()?;
                self.expect_op(")")?;
                let to = match &e {
                    Expr::Id(path) if path.len() > 1 || self.types.contains(&path[0]) => CastTo::Type(DataType::Named { path: path.clone(), dims: vec![] }),
                    _ => CastTo::Width(e),
                };
                e = Expr::Cast(Box::new(to), Box::new(v));
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn call_args(&mut self) -> R<Vec<Expr>> {
        let mut args = Vec::new();
        self.expect_op("(")?;
        if self.eat_op(")") {
            return Ok(args);
        }
        loop {
            if self.is_op(".") {
                return self.err("named function arguments are not supported");
            }
            args.push(self.expr()?);
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(")")?;
        Ok(args)
    }

    fn primary(&mut self) -> R<Expr> {
        match self.peek().clone() {
            T::Num(n) => {
                self.bump();
                Ok(Expr::Num(n))
            }
            T::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            T::Sys(name) => {
                self.bump();
                let args = if self.is_op("(") {
                    // `$bits(type)` takes a type.
                    if name == "bits" && self.type_ahead_in_parens() {
                        self.bump();
                        let ty = self.data_type()?;
                        self.expect_op(")")?;
                        vec![Expr::Type(Box::new(ty))]
                    } else {
                        self.call_args()?
                    }
                } else {
                    vec![]
                };
                Ok(Expr::SysCall(name, args))
            }
            T::Op("(") => {
                self.bump();
                let e = self.expr()?;
                self.expect_op(")")?;
                Ok(e)
            }
            T::Op("{") => {
                self.bump();
                if self.is_op("<<") || self.is_op(">>") {
                    return self.err("streaming operators are not supported");
                }
                let first = self.expr()?;
                if self.is_op("{") {
                    // replication {n{a, b}}
                    self.bump();
                    let mut items = vec![self.expr()?];
                    while self.eat_op(",") {
                        items.push(self.expr()?);
                    }
                    self.expect_op("}")?;
                    self.expect_op("}")?;
                    return Ok(Expr::Repl(Box::new(first), items));
                }
                let mut items = vec![first];
                while self.eat_op(",") {
                    items.push(self.expr()?);
                }
                self.expect_op("}")?;
                Ok(Expr::Concat(items))
            }
            T::Op("'{") => {
                self.bump();
                let mut items = Vec::new();
                loop {
                    if self.is_kw("default") && self.op_at(1, ":") {
                        self.bump();
                        self.bump();
                        items.push((PatKey::Default, self.expr()?));
                    } else {
                        let e = self.expr()?;
                        if self.eat_op(":") {
                            items.push((PatKey::Key(e), self.expr()?));
                        } else if self.is_op("{") {
                            // '{n{value}}
                            self.bump();
                            let v = self.expr()?;
                            self.expect_op("}")?;
                            items.push((PatKey::Repl(e), v));
                        } else {
                            items.push((PatKey::None, e));
                        }
                    }
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op("}")?;
                Ok(Expr::Pattern(items))
            }
            T::Id(k) => {
                // casts with keyword types: signed'(x), unsigned'(x), int'(x), logic'(x)
                if self.op_at(1, "'") && self.op_at(2, "(") {
                    let to = match k.as_str() {
                        "signed" => Some(CastTo::Signed),
                        "unsigned" => Some(CastTo::Unsigned),
                        "int" | "integer" | "byte" | "shortint" | "longint" | "logic" | "bit" | "reg" => {
                            let ty = self.data_type()?;
                            self.i -= 0;
                            Some(CastTo::Type(ty))
                        }
                        _ => None,
                    };
                    if let Some(to) = to {
                        if matches!(to, CastTo::Signed | CastTo::Unsigned) {
                            self.bump();
                        }
                        self.expect_op("'")?;
                        self.expect_op("(")?;
                        let v = self.expr()?;
                        self.expect_op(")")?;
                        return Ok(Expr::Cast(Box::new(to), Box::new(v)));
                    }
                }
                if TYPE_KW.contains(&k.as_str()) {
                    // A type used as an expression, e.g. a `parameter type` argument.
                    let ty = self.data_type()?;
                    return Ok(Expr::Type(Box::new(ty)));
                }
                let mut path = vec![self.ident()?];
                while self.is_op("::") {
                    self.bump();
                    path.push(self.ident()?);
                }
                if self.is_op("(") {
                    let args = self.call_args()?;
                    return Ok(Expr::Call(path, args));
                }
                Ok(Expr::Id(path))
            }
            _ => self.err("expected an expression"),
        }
    }

    fn type_ahead_in_parens(&self) -> bool {
        match self.peek_at(1) {
            T::Id(s) => TYPE_KW.contains(&s.as_str()),
            _ => false,
        }
    }

    // ------------------------------------------------------------ statements

    fn stmt(&mut self) -> R<Stmt> {
        let line = self.line();
        self.skip_delay()?;
        // label: statement
        if matches!(self.peek(), T::Id(_)) && self.op_at(1, ":") && !self.is_kw("default") {
            self.bump();
            self.bump();
        }
        loop {
            if self.is_kw("unique") || self.is_kw("unique0") || self.is_kw("priority") {
                self.bump();
            } else {
                break;
            }
        }
        if self.eat_op(";") {
            return Ok(Stmt::Null);
        }
        if self.eat_kw("begin") {
            self.skip_label();
            let mut decls = Vec::new();
            let mut stmts = Vec::new();
            while !self.is_kw("end") {
                if matches!(self.peek(), T::Eof) {
                    return self.err("missing 'end'");
                }
                if self.at_decl() {
                    decls.push(self.decl_item()?);
                } else {
                    stmts.push(self.stmt()?);
                }
            }
            self.bump();
            self.skip_label();
            return Ok(Stmt::Block { decls, stmts });
        }
        if self.eat_kw("if") {
            self.expect_op("(")?;
            let cond = self.expr()?;
            self.expect_op(")")?;
            let then = Box::new(self.stmt()?);
            let els = if self.eat_kw("else") { Some(Box::new(self.stmt()?)) } else { None };
            return Ok(Stmt::If { cond, then, els, line });
        }
        if self.is_kw("case") || self.is_kw("casez") || self.is_kw("casex") {
            let mut kind = match self.ident()?.as_str() {
                "casez" => CaseKind::Casez,
                "casex" => CaseKind::Casex,
                _ => CaseKind::Case,
            };
            self.expect_op("(")?;
            let expr = self.expr()?;
            self.expect_op(")")?;
            if self.eat_kw("inside") {
                kind = CaseKind::Inside;
            }
            let mut arms = Vec::new();
            while !self.eat_kw("endcase") {
                if matches!(self.peek(), T::Eof) {
                    return self.err("missing 'endcase'");
                }
                if self.eat_kw("default") {
                    self.eat_op(":");
                    arms.push((Vec::new(), self.stmt()?));
                    continue;
                }
                let items = self.inside_items(":")?;
                arms.push((items, self.stmt()?));
            }
            return Ok(Stmt::Case { kind, expr, arms, line });
        }
        if self.eat_kw("for") {
            self.expect_op("(")?;
            let mut init = Vec::new();
            let mut decls = Vec::new();
            if !self.is_op(";") {
                if self.at_type() {
                    let ty = self.data_type()?;
                    let mut names = Vec::new();
                    loop {
                        names.push(self.declarator()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    decls.push(Item::Decl { ty, names, line });
                } else {
                    loop {
                        init.push(self.simple_assign()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                }
            }
            self.expect_op(";")?;
            let cond = self.expr()?;
            self.expect_op(";")?;
            let mut step = Vec::new();
            if !self.is_op(")") {
                loop {
                    step.push(self.simple_assign()?);
                    if !self.eat_op(",") {
                        break;
                    }
                }
            }
            self.expect_op(")")?;
            let body = Box::new(self.stmt()?);
            return Ok(Stmt::For { init, decls, cond, step, body, line });
        }
        if self.eat_kw("while") {
            self.expect_op("(")?;
            let cond = self.expr()?;
            self.expect_op(")")?;
            let body = Box::new(self.stmt()?);
            return Ok(Stmt::While { cond, body, line });
        }
        if self.eat_kw("do") {
            let body = Box::new(self.stmt()?);
            self.expect_kw("while")?;
            self.expect_op("(")?;
            let cond = self.expr()?;
            self.expect_op(")")?;
            self.expect_op(";")?;
            return Ok(Stmt::DoWhile { cond, body, line });
        }
        if self.eat_kw("repeat") {
            self.expect_op("(")?;
            let count = self.expr()?;
            self.expect_op(")")?;
            let body = Box::new(self.stmt()?);
            return Ok(Stmt::Repeat { count, body, line });
        }
        if self.eat_kw("return") {
            let e = if self.is_op(";") { None } else { Some(self.expr()?) };
            self.expect_op(";")?;
            return Ok(Stmt::Return(e, line));
        }
        if self.eat_kw("break") {
            self.expect_op(";")?;
            return Ok(Stmt::Break(line));
        }
        if self.eat_kw("continue") {
            self.expect_op(";")?;
            return Ok(Stmt::Continue(line));
        }
        for k in ["forever", "fork", "wait", "disable", "force", "release", "deassign"] {
            if self.is_kw(k) {
                return self.err(format!("'{k}' is not synthesizable"));
            }
        }
        if self.is_kw("assert") || self.is_kw("assume") || self.is_kw("cover") {
            // Immediate assertion: no hardware.
            self.bump();
            self.eat_kw("property");
            self.eat_kw("final");
            if self.eat_op("#") {
                self.bump();
            }
            self.skip_parens()?;
            if self.is_kw("else") {
                self.bump();
                self.stmt()?;
            } else if self.is_op(";") {
                self.bump();
            } else {
                self.stmt()?;
                if self.eat_kw("else") {
                    self.stmt()?;
                }
            }
            return Ok(Stmt::Null);
        }
        if let T::Sys(_) = self.peek() {
            // System task ($display, $error, $fatal ...): no hardware.
            self.skip_to_semi()?;
            return Ok(Stmt::Null);
        }
        if self.is_op("@") {
            return self.err("event control inside a procedure is not synthesizable");
        }
        let s = self.simple_assign()?;
        self.expect_op(";")?;
        Ok(s)
    }

    /// `lhs = rhs`, `lhs <= rhs`, `lhs op= rhs`, `lhs++`, `++lhs`, or a call.
    fn simple_assign(&mut self) -> R<Stmt> {
        let line = self.line();
        if self.is_op("++") || self.is_op("--") {
            let op = if self.is_op("++") { "+" } else { "-" };
            self.bump();
            let lhs = self.postfix()?;
            return Ok(Stmt::Assign { rhs: Expr::Binary(op, Box::new(lhs.clone()), Box::new(int_num(1))), lhs, nonblocking: false, line });
        }
        let lhs = self.postfix()?;
        if self.eat_op("++") {
            return Ok(Stmt::Assign { rhs: Expr::Binary("+", Box::new(lhs.clone()), Box::new(int_num(1))), lhs, nonblocking: false, line });
        }
        if self.eat_op("--") {
            return Ok(Stmt::Assign { rhs: Expr::Binary("-", Box::new(lhs.clone()), Box::new(int_num(1))), lhs, nonblocking: false, line });
        }
        if self.eat_op("=") {
            self.skip_delay()?;
            let rhs = self.expr()?;
            return Ok(Stmt::Assign { lhs, rhs, nonblocking: false, line });
        }
        if self.eat_op("<=") {
            self.skip_delay()?;
            let rhs = self.expr()?;
            return Ok(Stmt::Assign { lhs, rhs, nonblocking: true, line });
        }
        for (tok, op) in [
            ("+=", "+"),
            ("-=", "-"),
            ("*=", "*"),
            ("/=", "/"),
            ("%=", "%"),
            ("&=", "&"),
            ("|=", "|"),
            ("^=", "^"),
            ("<<=", "<<"),
            (">>=", ">>"),
            ("<<<=", "<<<"),
            (">>>=", ">>>"),
        ] {
            if self.eat_op(tok) {
                let rhs = self.expr()?;
                return Ok(Stmt::Assign { rhs: Expr::Binary(op, Box::new(lhs.clone()), Box::new(rhs)), lhs, nonblocking: false, line });
            }
        }
        if matches!(lhs, Expr::Call(..)) {
            return Ok(Stmt::Call(lhs, line));
        }
        if let Expr::Id(path) = &lhs {
            // A task enable without parentheses.
            return Ok(Stmt::Call(Expr::Call(path.clone(), vec![]), line));
        }
        self.err("expected an assignment")
    }

    /// At a declaration inside a procedural block or module body.
    fn at_decl(&self) -> bool {
        if self.is_kw("localparam") || self.is_kw("parameter") || self.is_kw("typedef") {
            return true;
        }
        if self.is_kw("static") || self.is_kw("automatic") || self.is_kw("const") || self.is_kw("var") {
            return true;
        }
        match self.peek() {
            T::Id(s) if TYPE_KW.contains(&s.as_str()) => true,
            T::Id(s) => {
                if self.types.contains(s) {
                    // `t x;` but not `t'(...)` or an expression starting with a type-named signal
                    return matches!(self.peek_at(1), T::Id(_) | T::Op("["));
                }
                if self.op_at(1, "::") {
                    return matches!(self.peek_at(2), T::Id(_)) && matches!(self.peek_at(3), T::Id(_));
                }
                // `name name ;|=|,|[`
                matches!(self.peek_at(1), T::Id(_)) && matches!(self.peek_at(2), T::Op(";") | T::Op("=") | T::Op(",") | T::Op("["))
            }
            _ => false,
        }
    }

    fn param_decl(&mut self, local: bool, in_header: bool) -> R<Vec<ParamDecl>> {
        let line = self.line();
        let mut out = Vec::new();
        if self.eat_kw("type") {
            loop {
                let name = self.ident()?;
                self.types.insert(name.clone());
                let type_value = if self.eat_op("=") { Some(self.data_type()?) } else { None };
                out.push(ParamDecl { local, ty: None, name, unpacked: vec![], value: None, type_value, is_type: true, line });
                if in_header || !self.eat_op(",") {
                    break;
                }
            }
            return Ok(out);
        }
        // Optional type: present when two names follow or a type keyword is next.
        let ty = if self.at_type() || self.is_op("[") {
            Some(self.data_type()?)
        } else if matches!(self.peek(), T::Id(_)) && matches!(self.peek_at(1), T::Id(_)) {
            Some(self.data_type()?)
        } else {
            None
        };
        loop {
            let name = self.ident()?;
            let unpacked = self.unpacked_dims()?;
            let value = if self.eat_op("=") { Some(self.expr()?) } else { None };
            out.push(ParamDecl { local, ty: ty.clone(), name, unpacked, value, type_value: None, is_type: false, line });
            if in_header {
                // In a header the comma may start a new `parameter` keyword or type.
                if self.is_op(",") && matches!(self.peek_at(1), T::Id(_)) && self.op_at(2, "=") && !self.kw_at(1, "parameter") && !self.kw_at(1, "localparam") {
                    self.bump();
                    continue;
                }
                break;
            }
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(out)
    }

    /// A declaration item: parameter, typedef, or variables.
    fn decl_item(&mut self) -> R<Item> {
        let line = self.line();
        if self.is_kw("localparam") || self.is_kw("parameter") {
            let local = self.is_kw("localparam");
            self.bump();
            let mut ps = self.param_decl(local, false)?;
            self.expect_op(";")?;
            if ps.len() == 1 {
                return Ok(Item::Param(ps.pop().unwrap()));
            }
            return Ok(Item::Block(GenBlock { label: None, items: ps.into_iter().map(Item::Param).collect() }));
        }
        if self.eat_kw("typedef") {
            let ty = self.data_type()?;
            let name = self.ident()?;
            let unpacked = self.unpacked_dims()?;
            self.expect_op(";")?;
            self.types.insert(name.clone());
            return Ok(Item::Typedef { name, ty, unpacked, line });
        }
        // The initializer of a net is a continuous assignment; that of a
        // variable is its power-up value.
        let is_net = ["wire", "tri", "wand", "wor", "tri0", "tri1", "uwire"].iter().any(|k| self.is_kw(k));
        let ty = self.data_type()?;
        let mut names = Vec::new();
        let mut assigns = Vec::new();
        loop {
            let mut d = self.declarator()?;
            if is_net {
                if let Some(rhs) = d.init.take() {
                    assigns.push(Item::Assign { lhs: Expr::Id(vec![d.name.clone()]), rhs, line });
                }
            }
            names.push(d);
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";")?;
        if assigns.is_empty() {
            return Ok(Item::Decl { ty, names, line });
        }
        let mut items = vec![Item::Decl { ty, names, line }];
        items.extend(assigns);
        Ok(Item::Block(GenBlock { label: None, items }))
    }

    // -------------------------------------------------------------- modules

    fn sens(&mut self) -> R<Sens> {
        self.expect_op("@")?;
        if self.eat_op("*") {
            return Ok(Sens::Star);
        }
        self.expect_op("(")?;
        if self.eat_op("*") {
            self.expect_op(")")?;
            return Ok(Sens::Star);
        }
        let mut edges = Vec::new();
        let mut levels = false;
        loop {
            if self.eat_kw("posedge") {
                edges.push((true, self.expr()?));
            } else if self.eat_kw("negedge") {
                edges.push((false, self.expr()?));
            } else {
                self.expr()?;
                levels = true;
            }
            if !(self.eat_kw("or") || self.eat_op(",")) {
                break;
            }
        }
        self.expect_op(")")?;
        if levels && !edges.is_empty() {
            return self.err("sensitivity list mixes edges and levels");
        }
        Ok(if levels { Sens::Levels } else { Sens::Edges(edges) })
    }

    fn gen_block(&mut self) -> R<GenBlock> {
        if self.eat_kw("begin") {
            let mut label = None;
            if self.eat_op(":") {
                label = Some(self.ident()?);
            }
            let mut items = Vec::new();
            while !self.is_kw("end") {
                if matches!(self.peek(), T::Eof) {
                    return self.err("missing 'end'");
                }
                self.item(&mut items)?;
            }
            self.bump();
            self.skip_label();
            return Ok(GenBlock { label, items });
        }
        let mut items = Vec::new();
        self.item(&mut items)?;
        Ok(GenBlock { label: None, items })
    }

    fn function(&mut self, is_task: bool) -> R<FuncDecl> {
        let line = self.line();
        while self.is_kw("automatic") || self.is_kw("static") {
            self.bump();
        }
        let mut ret = None;
        if !is_task {
            if self.eat_kw("void") {
            } else if matches!(self.peek(), T::Id(_)) && (self.op_at(1, "(") || self.op_at(1, ";")) {
                // implicit 1-bit return
                ret = Some(DataType::Vec { signed: false, dims: vec![] });
            } else {
                ret = Some(self.data_type()?);
            }
        }
        let mut name = self.ident()?;
        while self.eat_op("::") {
            name = self.ident()?;
        }
        let mut args = Vec::new();
        if self.eat_op("(") {
            let mut dir = Dir::In;
            let mut ty = DataType::Vec { signed: false, dims: vec![] };
            while !self.is_op(")") {
                let l = self.line();
                let mut explicit = false;
                if self.eat_kw("input") {
                    dir = Dir::In;
                    explicit = true;
                } else if self.eat_kw("output") {
                    dir = Dir::Out;
                    explicit = true;
                } else if self.eat_kw("inout") || self.eat_kw("ref") {
                    return self.err("inout and ref arguments are not supported");
                }
                let named_next = matches!(self.peek(), T::Id(_)) && matches!(self.peek_at(1), T::Op(",") | T::Op(")") | T::Op("=") | T::Op("["));
                if !named_next || self.at_type() {
                    ty = self.data_type()?;
                } else if explicit {
                    ty = DataType::Vec { signed: false, dims: vec![] };
                }
                let decl = self.declarator()?;
                args.push(PortDecl { dir, ty: ty.clone(), decl, line: l });
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(")")?;
        }
        self.expect_op(";")?;
        let end = if is_task { "endtask" } else { "endfunction" };
        let mut decls = Vec::new();
        let mut body = Vec::new();
        while !self.is_kw(end) {
            if matches!(self.peek(), T::Eof) {
                return self.err(format!("missing '{end}'"));
            }
            if self.is_kw("input") || self.is_kw("output") {
                // old-style argument declarations
                let dir = if self.is_kw("input") { Dir::In } else { Dir::Out };
                self.bump();
                let l = self.line();
                let ty = self.opt_data_type()?;
                loop {
                    let decl = self.declarator()?;
                    args.push(PortDecl { dir, ty: ty.clone(), decl, line: l });
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op(";")?;
            } else if self.at_decl() {
                decls.push(self.decl_item()?);
            } else {
                body.push(self.stmt()?);
            }
        }
        self.bump();
        self.skip_label();
        Ok(FuncDecl { name, ret, args, decls, body, is_task, line })
    }

    fn instance(&mut self, out: &mut Vec<Item>) -> R<()> {
        let line = self.line();
        let module = self.ident()?;
        let mut params = Vec::new();
        if self.eat_op("#") {
            self.expect_op("(")?;
            while !self.is_op(")") {
                let name = if self.eat_op(".") {
                    let n = self.ident()?;
                    self.expect_op("(")?;
                    Some(n)
                } else {
                    None
                };
                let arg = if name.is_some() && self.is_op(")") {
                    None
                } else if self.at_type() && !matches!(self.peek(), T::Id(s) if !TYPE_KW.contains(&s.as_str()) && !self.types.contains(s)) {
                    Some(ParamArg::Type(self.data_type()?))
                } else {
                    Some(ParamArg::Expr(self.expr()?))
                };
                if name.is_some() {
                    self.expect_op(")")?;
                }
                if let Some(a) = arg {
                    params.push((name, a));
                }
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(")")?;
        }
        loop {
            let name = self.ident()?;
            if self.is_op("[") {
                return self.err("arrays of instances are not supported");
            }
            self.expect_op("(")?;
            let mut conns = Vec::new();
            let mut wildcard = false;
            while !self.is_op(")") {
                if self.eat_op(".") {
                    if self.eat_op("*") {
                        wildcard = true;
                    } else {
                        let port = self.ident()?;
                        if self.eat_op("(") {
                            let expr = if self.is_op(")") { None } else { Some(self.expr()?) };
                            self.expect_op(")")?;
                            conns.push(Conn { port: Some(port), expr });
                        } else {
                            conns.push(Conn { port: Some(port.clone()), expr: Some(Expr::Id(vec![port])) });
                        }
                    }
                } else if self.is_op(",") {
                    conns.push(Conn { port: None, expr: None });
                } else {
                    conns.push(Conn { port: None, expr: Some(self.expr()?) });
                }
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(")")?;
            out.push(Item::Inst(InstDecl { module: module.clone(), params: params.clone(), name, conns, wildcard, line }));
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";")
    }

    /// One module-level item (may add several entries to `out`).
    fn item(&mut self, out: &mut Vec<Item>) -> R<()> {
        let line = self.line();
        if self.eat_op(";") {
            return Ok(());
        }
        let T::Id(k) = self.peek().clone() else {
            return self.err("expected a module item");
        };
        match k.as_str() {
            "generate" | "endgenerate" => {
                self.bump();
            }
            "assign" => {
                self.bump();
                self.skip_delay()?;
                loop {
                    let lhs = self.postfix()?;
                    self.expect_op("=")?;
                    let rhs = self.expr()?;
                    out.push(Item::Assign { lhs, rhs, line });
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op(";")?;
            }
            "always_ff" | "always_comb" | "always_latch" | "always" => {
                self.bump();
                let kind = match k.as_str() {
                    "always_ff" => AlwaysKind::Ff,
                    "always_comb" => AlwaysKind::Comb,
                    "always_latch" => AlwaysKind::Latch,
                    _ => AlwaysKind::Plain,
                };
                let sens = if self.is_op("@") { self.sens()? } else { Sens::Star };
                if kind == AlwaysKind::Plain && !self.op_at(0, "@") && matches!(sens, Sens::Star) && self.t[self.i - 1].t == T::Id("always".into()) {
                    return self.err("'always' without a sensitivity list is not synthesizable");
                }
                let body = self.stmt()?;
                out.push(Item::Always { kind, sens, body, line });
            }
            "initial" | "final" => {
                // Simulation only: parsed and dropped.
                self.bump();
                self.stmt()?;
            }
            "genvar" => {
                self.bump();
                let mut names = vec![self.ident()?];
                while self.eat_op(",") {
                    names.push(self.ident()?);
                }
                self.expect_op(";")?;
                out.push(Item::Genvar(names));
            }
            "for" => {
                self.bump();
                self.expect_op("(")?;
                self.eat_kw("genvar");
                let var = self.ident()?;
                self.expect_op("=")?;
                let init = self.expr()?;
                self.expect_op(";")?;
                let cond = self.expr()?;
                self.expect_op(";")?;
                let step = self.simple_assign()?;
                self.expect_op(")")?;
                let body = self.gen_block()?;
                out.push(Item::GenFor { var, init, cond, step, body, line });
            }
            "if" => {
                self.bump();
                self.expect_op("(")?;
                let cond = self.expr()?;
                self.expect_op(")")?;
                let then = self.gen_block()?;
                let els = if self.eat_kw("else") { Some(self.gen_block()?) } else { None };
                out.push(Item::GenIf { cond, then, els, line });
            }
            "case" => {
                self.bump();
                self.expect_op("(")?;
                let expr = self.expr()?;
                self.expect_op(")")?;
                let mut arms = Vec::new();
                while !self.eat_kw("endcase") {
                    if self.eat_kw("default") {
                        self.eat_op(":");
                        arms.push((Vec::new(), self.gen_block()?));
                        continue;
                    }
                    let mut vals = vec![self.expr()?];
                    while self.eat_op(",") {
                        vals.push(self.expr()?);
                    }
                    self.expect_op(":")?;
                    arms.push((vals, self.gen_block()?));
                }
                out.push(Item::GenCase { expr, arms, line });
            }
            "begin" => {
                let b = self.gen_block()?;
                out.push(Item::Block(b));
            }
            "function" => {
                self.bump();
                out.push(Item::Func(Rc::new(self.function(false)?)));
            }
            "task" => {
                self.bump();
                out.push(Item::Func(Rc::new(self.function(true)?)));
            }
            "import" => {
                self.bump();
                loop {
                    let pkg = self.ident()?;
                    self.expect_op("::")?;
                    let name = if self.eat_op("*") { None } else { Some(self.ident()?) };
                    out.push(Item::Import { pkg, name });
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op(";")?;
            }
            "export" | "timeunit" | "timeprecision" | "default" | "clocking" | "bind" | "alias" => {
                if k == "bind" || k == "alias" {
                    return self.err(format!("'{k}' is not supported"));
                }
                self.skip_to_semi()?;
            }
            "specify" => {
                self.bump();
                self.skip_until_kw("specify", "endspecify")?;
            }
            "property" => {
                self.bump();
                self.skip_until_kw("property", "endproperty")?;
                self.skip_label();
            }
            "sequence" => {
                self.bump();
                self.skip_until_kw("sequence", "endsequence")?;
                self.skip_label();
            }
            "covergroup" => {
                self.bump();
                self.skip_until_kw("covergroup", "endgroup")?;
                self.skip_label();
            }
            "assert" | "assume" | "cover" | "restrict" => {
                // Concurrent or immediate assertion at module level: no hardware.
                self.bump();
                self.eat_kw("property");
                self.eat_kw("final");
                self.skip_parens()?;
                if self.is_kw("else") {
                    self.bump();
                    self.stmt()?;
                } else if !self.eat_op(";") {
                    self.stmt()?;
                    if self.eat_kw("else") {
                        self.stmt()?;
                    }
                }
            }
            "defparam" => return self.err("'defparam' is not supported"),
            "modport" => self.skip_to_semi()?,
            "input" | "output" | "inout" => {
                let dir = match k.as_str() {
                    "input" => Dir::In,
                    "output" => Dir::Out,
                    _ => Dir::Inout,
                };
                self.bump();
                let ty = self.opt_data_type()?;
                loop {
                    let decl = self.declarator()?;
                    out.push(Item::PortDir(PortDecl { dir, ty: ty.clone(), decl, line }));
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op(";")?;
            }
            "and" | "or" | "xor" | "nand" | "nor" | "xnor" | "not" | "buf" => {
                // Gate primitive: `and g1 (y, a, b);`
                self.bump();
                self.skip_delay()?;
                loop {
                    if matches!(self.peek(), T::Id(_)) {
                        self.bump();
                    }
                    self.expect_op("(")?;
                    let y = self.postfix()?;
                    let mut ins = Vec::new();
                    while self.eat_op(",") {
                        ins.push(self.expr()?);
                    }
                    self.expect_op(")")?;
                    if ins.is_empty() {
                        return self.err("gate without inputs");
                    }
                    let (op, inv) = match k.as_str() {
                        "and" => ("&", false),
                        "nand" => ("&", true),
                        "or" => ("|", false),
                        "nor" => ("|", true),
                        "xor" => ("^", false),
                        "xnor" => ("^", true),
                        "not" => ("&", true),
                        _ => ("&", false),
                    };
                    let mut it = ins.into_iter();
                    let mut e = it.next().unwrap();
                    for x in it {
                        e = Expr::Binary(op, Box::new(e), Box::new(x));
                    }
                    if inv {
                        e = Expr::Unary("~", Box::new(e));
                    }
                    out.push(Item::Assign { lhs: y, rhs: e, line });
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op(";")?;
            }
            _ => {
                if self.at_decl() {
                    out.push(self.decl_item()?);
                } else if matches!(self.peek_at(1), T::Id(_)) || self.op_at(1, "#") {
                    self.instance(out)?;
                } else if self.op_at(1, ":") {
                    // label: assert ...
                    self.bump();
                    self.bump();
                    self.item(out)?;
                } else {
                    return self.err("this module item is not supported");
                }
            }
        }
        Ok(())
    }

    fn header_params(&mut self) -> R<Vec<ParamDecl>> {
        let mut params = Vec::new();
        if !self.eat_op("#") {
            return Ok(params);
        }
        self.expect_op("(")?;
        let mut local = false;
        while !self.is_op(")") {
            if self.eat_kw("parameter") {
                local = false;
            } else if self.eat_kw("localparam") {
                local = true;
            }
            params.extend(self.param_decl(local, true)?);
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(")")?;
        Ok(params)
    }

    fn module(&mut self, is_interface: bool) -> R<Module> {
        let line = self.line();
        while self.is_kw("automatic") || self.is_kw("static") {
            self.bump();
        }
        let name = self.ident()?;
        let mut items = Vec::new();
        while self.is_kw("import") {
            self.item(&mut items)?;
        }
        let params = self.header_params()?;
        let mut ports = Vec::new();
        let mut port_names = Vec::new();
        if self.eat_op("(") {
            let mut dir = Dir::In;
            let mut ty = DataType::Vec { signed: false, dims: vec![] };
            let mut ansi = false;
            while !self.is_op(")") {
                let l = self.line();
                if self.eat_op(".") {
                    return self.err("explicit port expressions are not supported");
                }
                let mut explicit_dir = false;
                if self.is_kw("input") || self.is_kw("output") || self.is_kw("inout") {
                    dir = match self.ident()?.as_str() {
                        "input" => Dir::In,
                        "output" => Dir::Out,
                        _ => Dir::Inout,
                    };
                    explicit_dir = true;
                    ansi = true;
                }
                // `name ,|)|[|=` : a new declarator of the previous type (or a non-ANSI name).
                let plain_name = matches!(self.peek(), T::Id(s) if !TYPE_KW.contains(&s.as_str()))
                    && matches!(self.peek_at(1), T::Op(",") | T::Op(")") | T::Op("[") | T::Op("="));
                if plain_name && !explicit_dir && !ansi {
                    port_names.push(self.ident()?);
                } else {
                    if !plain_name {
                        if matches!(self.peek(), T::Id(_)) && self.op_at(1, ".") {
                            return self.err("interface ports are not supported");
                        }
                        ty = self.data_type()?;
                        ansi = true;
                    } else if explicit_dir {
                        ty = DataType::Vec { signed: false, dims: vec![] };
                    }
                    let decl = self.declarator()?;
                    ports.push(PortDecl { dir, ty: ty.clone(), decl, line: l });
                }
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(")")?;
        }
        self.expect_op(";")?;
        let end = if is_interface { "endinterface" } else { "endmodule" };
        while !self.is_kw(end) {
            if matches!(self.peek(), T::Eof) {
                return self.err(format!("missing '{end}'"));
            }
            self.item(&mut items)?;
        }
        self.bump();
        self.skip_label();
        Ok(Module { name, params, ports, port_names, items, is_interface, line })
    }

    fn source(&mut self) -> R<Source> {
        let mut src = Source::default();
        loop {
            match self.peek().clone() {
                T::Eof => break,
                T::Op(";") => {
                    self.bump();
                }
                T::Id(k) => match k.as_str() {
                    "module" | "macromodule" => {
                        self.bump();
                        src.modules.push(self.module(false)?);
                    }
                    "interface" => {
                        self.bump();
                        src.modules.push(self.module(true)?);
                    }
                    "package" => {
                        self.bump();
                        while self.is_kw("automatic") || self.is_kw("static") {
                            self.bump();
                        }
                        let name = self.ident()?;
                        self.expect_op(";")?;
                        let mut items = Vec::new();
                        while !self.is_kw("endpackage") {
                            if matches!(self.peek(), T::Eof) {
                                return self.err("missing 'endpackage'");
                            }
                            self.item(&mut items)?;
                        }
                        self.bump();
                        self.skip_label();
                        src.packages.push(Package { name, items });
                    }
                    "program" | "class" | "primitive" | "checker" | "config" => return self.err(format!("'{k}' is not synthesizable")),
                    _ => self.item(&mut src.unit)?,
                },
                _ => return self.err("expected a module, interface or package"),
            }
        }
        Ok(src)
    }
}

pub fn parse(text: &str) -> R<Source> {
    let t = lex(text)?;
    // Type names are needed to tell `t x;` from `m u (...)`: collect them first.
    let mut types = std::collections::HashSet::new();
    for i in 0..t.len() {
        if matches!(&t[i].t, T::Id(k) if k == "typedef") {
            // typedef ... name ;   (the name is the identifier before the terminating ';' at depth 0)
            let mut depth = 0i32;
            let mut last: Option<&str> = None;
            let mut j = i + 1;
            while j < t.len() {
                match &t[j].t {
                    T::Op("{") | T::Op("(") | T::Op("[") => depth += 1,
                    T::Op("}") | T::Op(")") | T::Op("]") => depth -= 1,
                    T::Op(";") if depth == 0 => break,
                    T::Id(s) if depth == 0 => last = Some(s),
                    _ => {}
                }
                j += 1;
            }
            if let Some(n) = last {
                types.insert(n.to_string());
            }
        }
        if matches!(&t[i].t, T::Id(k) if k == "parameter" || k == "localparam") && matches!(&t[i + 1].t, T::Id(k) if k == "type") {
            if let T::Id(n) = &t[i + 2].t {
                types.insert(n.clone());
            }
        }
    }
    let mut p = P { t, i: 0, types };
    p.source()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_small_design() {
        let src = r#"
package p; typedef enum logic [1:0] {A, B, C} st_t; localparam int W = 4; endpackage
module child #(parameter int W = 8, parameter logic [3:0] K = 4'h3) (input logic clk, input logic [W-1:0] d, output logic [W-1:0] q);
  always_ff @(posedge clk) q <= d + W'(K);
endmodule
module top import p::*; (input clk, input [3:0] a, b, output reg [3:0] y, output logic z);
  st_t s;
  logic [3:0] m [0:3];
  wire [3:0] w = a & b;
  child #(.W(4)) u (.clk, .d(w), .q(y));
  always_comb begin
    z = 1'b0;
    unique case (s)
      A: z = a[0];
      B, C: z = |b;
      default: ;
    endcase
    for (int i = 0; i < 4; i++) if (m[i] == a) z = ~z;
  end
  generate
    for (genvar g = 0; g < 4; g++) begin : gen
      always_ff @(posedge clk or negedge a[0]) if (!a[0]) m[g] <= '0; else m[g] <= {a[g], 3'b0};
    end
  endgenerate
  function automatic logic [3:0] f(input logic [3:0] x);
    return x ^ 4'hf;
  endfunction
  initial $display("hi %d", 3);
  assert property (@(posedge clk) a |-> b);
endmodule
"#;
        let s = parse(src).unwrap();
        assert_eq!(s.packages.len(), 1);
        assert_eq!(s.modules.len(), 2);
        let top = &s.modules[1];
        assert_eq!(top.ports.len(), 5);
        assert!(top.items.iter().any(|i| matches!(i, Item::Inst(d) if d.module == "child" && d.conns.len() == 3)));
        assert!(top.items.iter().any(|i| matches!(i, Item::GenFor { .. })));
        assert!(top.items.iter().any(|i| matches!(i, Item::Func(_))));
    }

    #[test]
    fn errors_name_the_line() {
        let e = parse("module m;\n  real x;\nendmodule").unwrap_err();
        assert!(e.starts_with("line 2:"), "{e}");
    }
}
