//! Expressions, constraint blocks, and event controls.

use super::{BUILTIN_TYPES, Parser, is_reserved};
use crate::SvError;
use crate::ast::*;
use crate::lex::Tok;

impl Parser<'_> {
    pub(crate) fn parse_expr(&mut self) -> Result<Expr, SvError> {
        self.parse_cond()
    }

    fn parse_cond(&mut self) -> Result<Expr, SvError> {
        let c = self.parse_bin(0)?;
        if self.eat_op("?") {
            let a = self.parse_cond()?;
            self.expect_op(":")?;
            let b = self.parse_cond()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    fn bin_op(&self, level: usize) -> Option<BinOp> {
        let o = match self.tok() {
            Some(Tok::Op(o)) => *o,
            Some(Tok::Id(s)) if level == 7 && s == "inside" => return Some(BinOp::Eq), // marker, handled below
            _ => return None,
        };
        let op = match (level, o) {
            (0, "||") => BinOp::LOr,
            (1, "&&") => BinOp::LAnd,
            (2, "|") => BinOp::Or,
            (3, "^") => BinOp::Xor,
            (3, "~^") | (3, "^~") => BinOp::Xnor,
            (4, "&") => BinOp::And,
            (5, "==") => BinOp::Eq,
            (5, "!=") => BinOp::Ne,
            (5, "===") => BinOp::CaseEq,
            (5, "!==") => BinOp::CaseNe,
            (5, "==?") => BinOp::WildEq,
            (5, "!=?") => BinOp::WildNe,
            (7, "<") => BinOp::Lt,
            (7, "<=") => BinOp::Le,
            (7, ">") => BinOp::Gt,
            (7, ">=") => BinOp::Ge,
            (8, "<<") => BinOp::Shl,
            (8, ">>") => BinOp::Shr,
            (8, "<<<") => BinOp::AShl,
            (8, ">>>") => BinOp::AShr,
            (9, "+") => BinOp::Add,
            (9, "-") => BinOp::Sub,
            (10, "*") => BinOp::Mul,
            (10, "/") => BinOp::Div,
            (10, "%") => BinOp::Mod,
            (11, "**") => BinOp::Pow,
            _ => return None,
        };
        Some(op)
    }

    fn parse_bin(&mut self, level: usize) -> Result<Expr, SvError> {
        if level > 11 {
            return self.parse_unary();
        }
        if level == 6 {
            return self.parse_bin(7);
        }
        let mut l = self.parse_bin(level + 1)?;
        loop {
            if level == 7 && self.is_kw("inside") {
                self.p += 1;
                let items = self.parse_inside_set()?;
                l = Expr::Inside(Box::new(l), items);
                continue;
            }
            let Some(op) = self.bin_op(level) else { break };
            self.p += 1;
            let r = if op == BinOp::Pow { self.parse_bin(level)? } else { self.parse_bin(level + 1)? };
            l = Expr::Binary(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    pub(crate) fn parse_inside_set(&mut self) -> Result<Vec<InsideItem>, SvError> {
        self.expect_op("{")?;
        let mut items = Vec::new();
        loop {
            if self.eat_op("[") {
                let a = self.parse_expr()?;
                self.expect_op(":")?;
                let b = if self.eat_op("$") { Expr::Dollar } else { self.parse_expr()? };
                self.expect_op("]")?;
                items.push(InsideItem::Range(a, b));
            } else {
                items.push(InsideItem::One(self.parse_expr()?));
            }
            if self.eat_op("}") {
                break;
            }
            self.expect_op(",")?;
        }
        Ok(items)
    }

    fn parse_unary(&mut self) -> Result<Expr, SvError> {
        let op = match self.tok() {
            Some(Tok::Op("-")) => Some(UnOp::Neg),
            Some(Tok::Op("+")) => Some(UnOp::Plus),
            Some(Tok::Op("!")) => Some(UnOp::Not),
            Some(Tok::Op("~")) => Some(UnOp::BitNot),
            Some(Tok::Op("&")) => Some(UnOp::RedAnd),
            Some(Tok::Op("|")) => Some(UnOp::RedOr),
            Some(Tok::Op("^")) => Some(UnOp::RedXor),
            Some(Tok::Op("~&")) => Some(UnOp::RedNand),
            Some(Tok::Op("~|")) => Some(UnOp::RedNor),
            Some(Tok::Op("~^")) | Some(Tok::Op("^~")) => Some(UnOp::RedXnor),
            _ => None,
        };
        if let Some(op) = op {
            self.p += 1;
            let e = self.parse_unary()?;
            return Ok(Expr::Unary(op, Box::new(e)));
        }
        if self.is_op("++") || self.is_op("--") {
            let inc = self.is_op("++");
            self.p += 1;
            let e = self.parse_unary()?;
            return Ok(Expr::IncDec { e: Box::new(e), inc, prefix: true });
        }
        let e = self.parse_postfix()?;
        if self.is_op("++") || self.is_op("--") {
            let inc = self.is_op("++");
            self.p += 1;
            return Ok(Expr::IncDec { e: Box::new(e), inc, prefix: false });
        }
        Ok(e)
    }

    /// Primary with postfix selects, members, and calls (no binary operators):
    /// also the left-hand side of assignments.
    pub(crate) fn parse_postfix(&mut self) -> Result<Expr, SvError> {
        let mut e = self.parse_primary()?;
        loop {
            if self.is_op(".") && matches!(self.tok_at(1), Some(Tok::Id(_))) {
                self.p += 1;
                let n = match self.tok() {
                    Some(Tok::Id(s)) => s.clone(),
                    _ => unreachable!(),
                };
                self.p += 1;
                e = Expr::Member(Box::new(e), n);
                if self.is_kw("with") && !self.is_op_at(1, "{") {
                    // `q.find with (item > 1)`
                    let loc = self.loc();
                    let with = self.parse_with()?;
                    e = Expr::Call { f: Box::new(e), args: vec![], named: vec![], with: Some(with), loc };
                }
                continue;
            }
            if self.is_op("[") {
                self.p += 1;
                let a = self.parse_expr()?;
                if self.eat_op(":") {
                    let b = self.parse_expr()?;
                    self.expect_op("]")?;
                    e = Expr::Slice(Box::new(e), Box::new(a), Box::new(b), SliceKind::Fixed);
                } else if self.eat_op("+:") {
                    let b = self.parse_expr()?;
                    self.expect_op("]")?;
                    e = Expr::Slice(Box::new(e), Box::new(a), Box::new(b), SliceKind::Up);
                } else if self.eat_op("-:") {
                    let b = self.parse_expr()?;
                    self.expect_op("]")?;
                    e = Expr::Slice(Box::new(e), Box::new(a), Box::new(b), SliceKind::Down);
                } else {
                    self.expect_op("]")?;
                    e = Expr::Index(Box::new(e), Box::new(a));
                }
                continue;
            }
            if self.is_op("(") && matches!(e, Expr::Ident(_) | Expr::Member(..) | Expr::Scoped(_) | Expr::Super) {
                let loc = self.loc();
                let (args, named) = self.parse_call_args()?;
                let with = if self.is_kw("with") { Some(self.parse_with()?) } else { None };
                e = Expr::Call { f: Box::new(e), args, named, with, loc };
                continue;
            }
            if self.is_kw("with") && matches!(e, Expr::Call { with: None, .. }) {
                let w = self.parse_with()?;
                if let Expr::Call { with, .. } = &mut e {
                    *with = Some(w);
                }
                continue;
            }
            break;
        }
        Ok(e)
    }

    fn parse_with(&mut self) -> Result<WithClause, SvError> {
        self.expect_kw("with")?;
        if self.is_op("{") {
            return Ok(WithClause::Constraints(self.parse_constraint_block()?));
        }
        if self.eat_op("(") {
            // `randomize() with (a, b) {…}` restricts names: accept and ignore the list.
            let e = self.parse_expr()?;
            if self.eat_op(",") || (self.is_op(")") && self.is_op_at(1, "{")) {
                while !self.eat_op(")") {
                    self.p += 1;
                }
                return Ok(WithClause::Constraints(self.parse_constraint_block()?));
            }
            self.expect_op(")")?;
            if self.is_op("{") {
                return Ok(WithClause::Constraints(self.parse_constraint_block()?));
            }
            return Ok(WithClause::Expr(Box::new(e)));
        }
        Err(self.err("expected `{` or `(` after `with`"))
    }

    pub(crate) fn parse_call_args(&mut self) -> Result<(Vec<Option<Expr>>, Vec<(String, Option<Expr>)>), SvError> {
        self.expect_op("(")?;
        let mut args = Vec::new();
        let mut named = Vec::new();
        if self.eat_op(")") {
            return Ok((args, named));
        }
        loop {
            if self.is_op(".") && matches!(self.tok_at(1), Some(Tok::Id(_))) && self.is_op_at(2, "(") {
                self.p += 1;
                let n = self.ident()?;
                self.expect_op("(")?;
                let e = if self.is_op(")") { None } else { Some(self.parse_expr()?) };
                self.expect_op(")")?;
                named.push((n, e));
            } else if self.is_op(",") || self.is_op(")") {
                args.push(None);
            } else {
                args.push(Some(self.parse_expr()?));
            }
            if self.eat_op(")") {
                break;
            }
            self.expect_op(",")?;
        }
        Ok((args, named))
    }

    fn parse_primary(&mut self) -> Result<Expr, SvError> {
        let loc = self.loc();
        let t = self.tok().cloned().ok_or_else(|| self.err("expected an expression"))?;
        match t {
            Tok::Num { val, width, signed, .. } => {
                self.p += 1;
                if self.is_op("'") && self.is_op_at(1, "(") {
                    self.p += 2;
                    let e = self.parse_expr()?;
                    self.expect_op(")")?;
                    return Ok(Expr::Cast(CastTo::Width(Box::new(Expr::Num { val, width, signed })), Box::new(e)));
                }
                Ok(Expr::Num { val, width, signed })
            }
            Tok::Fill(b) => {
                self.p += 1;
                Ok(Expr::Fill(b))
            }
            Tok::Real(v, _) => {
                self.p += 1;
                Ok(Expr::Real(v))
            }
            Tok::Time(ps, _) => {
                self.p += 1;
                Ok(Expr::Time(ps))
            }
            Tok::Str(s) => {
                self.p += 1;
                Ok(Expr::Str(s))
            }
            Tok::Sys(name) => {
                self.p += 1;
                let args = if self.is_op("(") {
                    // `$bits(type)` / `$typename(type)` take a type.
                    if matches!(name.as_str(), "$bits" | "$typename") && matches!(self.tok_at(1), Some(Tok::Id(s)) if BUILTIN_TYPES.contains(&s.as_str())) {
                        self.p += 1;
                        let ty = self.parse_data_type()?;
                        self.expect_op(")")?;
                        vec![Some(Expr::Type(ty))]
                    } else {
                        self.parse_call_args()?.0
                    }
                } else {
                    vec![]
                };
                Ok(Expr::Sys { name, args, loc })
            }
            Tok::Op("(") => {
                self.p += 1;
                let e = self.parse_expr()?;
                if self.eat_op("=") {
                    let r = self.parse_expr()?;
                    self.expect_op(")")?;
                    return Ok(Expr::AssignExpr(Box::new(e), Box::new(r)));
                }
                self.expect_op(")")?;
                Ok(e)
            }
            Tok::Op("{") => {
                self.p += 1;
                if self.eat_op("}") {
                    return Ok(Expr::Concat(vec![]));
                }
                let first = self.parse_expr()?;
                if self.is_op("{") {
                    // Replication `{n{a, b}}`.
                    self.p += 1;
                    let mut parts = vec![self.parse_expr()?];
                    while self.eat_op(",") {
                        parts.push(self.parse_expr()?);
                    }
                    self.expect_op("}")?;
                    self.expect_op("}")?;
                    return Ok(Expr::Repl(Box::new(first), parts));
                }
                let mut parts = vec![first];
                while self.eat_op(",") {
                    parts.push(self.parse_expr()?);
                }
                self.expect_op("}")?;
                Ok(Expr::Concat(parts))
            }
            Tok::Op("'") if self.is_op_at(1, "{") => {
                self.p += 2;
                let mut parts = Vec::new();
                let mut default = None;
                if self.eat_op("}") {
                    return Ok(Expr::Pattern(parts, None));
                }
                loop {
                    if self.is_kw("default") && self.is_op_at(1, ":") {
                        self.p += 2;
                        default = Some(Box::new(self.parse_expr()?));
                    } else {
                        let e = self.parse_expr()?;
                        if self.is_op("{") {
                            self.p += 1;
                            let mut inner = vec![self.parse_expr()?];
                            while self.eat_op(",") {
                                inner.push(self.parse_expr()?);
                            }
                            self.expect_op("}")?;
                            parts.push(Expr::Repl(Box::new(e), inner));
                        } else if self.eat_op(":") {
                            // `'{name: value}` / `'{0: v}`: positional value only.
                            parts.push(self.parse_expr()?);
                        } else {
                            parts.push(e);
                        }
                    }
                    if self.eat_op("}") {
                        break;
                    }
                    self.expect_op(",")?;
                }
                Ok(Expr::Pattern(parts, default))
            }
            Tok::Op("$") => {
                self.p += 1;
                Ok(Expr::Dollar)
            }
            Tok::Id(s) => self.parse_ident_primary(s, loc),
            _ => Err(self.err("expected an expression")),
        }
    }

    fn parse_ident_primary(&mut self, s: String, loc: Loc) -> Result<Expr, SvError> {
        match s.as_str() {
            "null" => {
                self.p += 1;
                return Ok(Expr::Null);
            }
            "this" => {
                self.p += 1;
                return Ok(Expr::This);
            }
            "super" => {
                self.p += 1;
                return Ok(Expr::Super);
            }
            "new" => {
                self.p += 1;
                if self.eat_op("[") {
                    let n = self.parse_expr()?;
                    self.expect_op("]")?;
                    let src = if self.eat_op("(") {
                        let e = self.parse_expr()?;
                        self.expect_op(")")?;
                        Some(Box::new(e))
                    } else {
                        None
                    };
                    return Ok(Expr::New { args: vec![], size: Some(Box::new(n)), src, copy: None, loc });
                }
                if self.is_op("(") {
                    let (args, _) = self.parse_call_args()?;
                    return Ok(Expr::New { args, size: None, src: None, copy: None, loc });
                }
                if !self.is_op(";") && !self.is_op(",") && !self.is_op(")") {
                    let e = self.parse_postfix()?;
                    return Ok(Expr::New { args: vec![], size: None, src: None, copy: Some(Box::new(e)), loc });
                }
                return Ok(Expr::New { args: vec![], size: None, src: None, copy: None, loc });
            }
            "signed" | "unsigned" if self.is_op_at(1, "'") => {
                self.p += 3;
                let e = self.parse_expr()?;
                self.expect_op(")")?;
                return Ok(Expr::Cast(CastTo::Signed(s == "signed"), Box::new(e)));
            }
            "std" if self.is_op_at(1, "::") && self.is_kw_at(2, "randomize") => {
                self.p += 2;
                return self.parse_ident_primary("randomize".into(), loc).map(|e| match e {
                    Expr::Ident(_) => Expr::Ident("std::randomize".into()),
                    other => other,
                });
            }
            "randomize" => {
                self.p += 1;
                let f = Expr::Ident("randomize".into());
                if self.is_op("(") {
                    let (args, named) = self.parse_call_args()?;
                    let with = if self.is_kw("with") { Some(self.parse_with()?) } else { None };
                    return Ok(Expr::Call { f: Box::new(f), args, named, with, loc });
                }
                return Ok(f);
            }
            _ => {}
        }
        if BUILTIN_TYPES.contains(&s.as_str()) {
            // Cast `int'(x)` or a type used as an expression.
            let ty = self.parse_data_type()?;
            if self.eat_op("'") {
                self.expect_op("(")?;
                let e = self.parse_expr()?;
                self.expect_op(")")?;
                return Ok(Expr::Cast(CastTo::Type(ty), Box::new(e)));
            }
            return Ok(Expr::Type(ty));
        }
        if is_reserved(&s) {
            return Err(self.err(format!("unexpected keyword `{s}`")));
        }
        // Scoped names / parameterized class references.
        if self.is_op_at(1, "::") || (self.is_op_at(1, "#") && self.is_op_at(2, "(")) {
            let path = self.parse_type_path()?;
            if self.eat_op("'") {
                self.expect_op("(")?;
                let e = self.parse_expr()?;
                self.expect_op(")")?;
                return Ok(Expr::Cast(CastTo::Type(DataType::Named { path }), Box::new(e)));
            }
            return Ok(Expr::Scoped(path));
        }
        self.p += 1;
        if self.is_op("'") && self.is_op_at(1, "(") {
            self.p += 2;
            let e = self.parse_expr()?;
            self.expect_op(")")?;
            return Ok(Expr::Cast(CastTo::Type(DataType::Named { path: vec![Seg { name: s, params: None }] }), Box::new(e)));
        }
        Ok(Expr::Ident(s))
    }

    // ---- constraints ------------------------------------------------------------

    pub(crate) fn parse_constraint_block(&mut self) -> Result<Vec<CItem>, SvError> {
        self.expect_op("{")?;
        let mut items = Vec::new();
        while !self.eat_op("}") {
            if self.at_end() {
                return Err(self.err("unterminated constraint block"));
            }
            if self.eat_op(";") {
                continue;
            }
            items.push(self.parse_citem()?);
        }
        Ok(items)
    }

    fn parse_citem_or_block(&mut self) -> Result<Vec<CItem>, SvError> {
        if self.is_op("{") {
            self.parse_constraint_block()
        } else {
            Ok(vec![self.parse_citem()?])
        }
    }

    fn parse_citem(&mut self) -> Result<CItem, SvError> {
        if self.eat_kw("if") {
            self.expect_op("(")?;
            let c = self.parse_expr()?;
            self.expect_op(")")?;
            let t = self.parse_citem_or_block()?;
            let e = if self.eat_kw("else") { self.parse_citem_or_block()? } else { vec![] };
            return Ok(CItem::IfElse(c, t, e));
        }
        if self.eat_kw("foreach") {
            let (arr, vars) = self.parse_foreach_head()?;
            let body = self.parse_citem_or_block()?;
            return Ok(CItem::Foreach { arr, vars, body });
        }
        if self.eat_kw("solve") {
            let mut a = vec![self.parse_postfix()?];
            while self.eat_op(",") {
                a.push(self.parse_postfix()?);
            }
            self.expect_kw("before")?;
            let mut b = vec![self.parse_postfix()?];
            while self.eat_op(",") {
                b.push(self.parse_postfix()?);
            }
            self.expect_op(";")?;
            return Ok(CItem::SolveBefore(a, b));
        }
        if self.eat_kw("soft") {
            let i = self.parse_citem()?;
            return Ok(CItem::Soft(Box::new(i)));
        }
        if self.eat_kw("unique") {
            self.expect_op("{")?;
            let mut v = vec![self.parse_expr()?];
            while self.eat_op(",") {
                v.push(self.parse_expr()?);
            }
            self.expect_op("}")?;
            self.expect_op(";")?;
            return Ok(CItem::Unique(v));
        }
        let e = self.parse_expr()?;
        if self.eat_op("->") {
            let body = self.parse_citem_or_block()?;
            return Ok(CItem::Implies(e, body));
        }
        if self.eat_kw("dist") {
            self.expect_op("{")?;
            let mut items = Vec::new();
            loop {
                let (lo, hi) = if self.eat_op("[") {
                    let a = self.parse_expr()?;
                    self.expect_op(":")?;
                    let b = self.parse_expr()?;
                    self.expect_op("]")?;
                    (a, Some(b))
                } else {
                    (self.parse_expr()?, None)
                };
                let (per_range, weight) = if self.eat_op(":=") {
                    (false, self.parse_expr()?)
                } else if self.eat_op(":/") {
                    (true, self.parse_expr()?)
                } else {
                    (false, Expr::num(1))
                };
                items.push(DistItem { lo, hi, per_range, weight });
                if self.eat_op("}") {
                    break;
                }
                self.expect_op(",")?;
            }
            self.expect_op(";")?;
            return Ok(CItem::Dist { e, items });
        }
        self.expect_op(";")?;
        Ok(match e {
            Expr::Inside(x, items) => CItem::Inside { e: *x, items, neg: false },
            Expr::Unary(UnOp::Not, inner) if matches!(*inner, Expr::Inside(..)) => match *inner {
                Expr::Inside(x, items) => CItem::Inside { e: *x, items, neg: true },
                _ => unreachable!(),
            },
            other => CItem::Expr(other),
        })
    }

    /// `( arr [ i, j ] )` → (arr, [i, j]).
    pub(crate) fn parse_foreach_head(&mut self) -> Result<(Expr, Vec<Option<String>>), SvError> {
        if !self.is_op("(") {
            return Err(self.err("expected `(` after foreach"));
        }
        // Find the last top-level `[` before the closing `)`.
        let close = self.skip_balanced_from(0).map(|n| self.p + n - 2).ok_or_else(|| self.err("unbalanced foreach ("))?;
        self.p += 1;
        // `close` indexes the `]` before `)`.
        if !matches!(self.t.get(close).map(|t| &t.tok), Some(Tok::Op("]"))) {
            return Err(self.err("foreach needs `array[index, …]`"));
        }
        let mut depth = 0i32;
        let mut open = None;
        let mut k = close;
        while k > self.p {
            match &self.t[k].tok {
                Tok::Op("]") | Tok::Op(")") | Tok::Op("}") => depth += 1,
                Tok::Op("[") | Tok::Op("(") | Tok::Op("{") => {
                    depth -= 1;
                    if depth == 0 {
                        open = Some(k);
                        break;
                    }
                }
                _ => {}
            }
            k -= 1;
        }
        let open = open.ok_or_else(|| self.err("foreach needs `array[index, …]`"))?;
        // Parse the array expression from the token range.
        let sub: Vec<_> = self.t[self.p..open].to_vec();
        let mut uid = *self.uid;
        let mut sp = Parser::new(sub, self.map, &mut uid);
        let arr = sp.parse_postfix()?;
        if !sp.at_end() {
            return Err(self.err("unsupported foreach array expression"));
        }
        self.p = open + 1;
        let mut vars = Vec::new();
        loop {
            if self.is_op(",") || self.is_op("]") {
                vars.push(None);
            } else {
                vars.push(Some(self.ident()?));
            }
            if self.eat_op("]") {
                break;
            }
            self.expect_op(",")?;
        }
        self.expect_op(")")?;
        Ok((arr, vars))
    }

    // ---- event controls -----------------------------------------------------------

    /// After `@`: `(posedge a or b, negedge c)`, `ev`, `(vif.cb)`.
    pub(crate) fn parse_event_terms(&mut self) -> Result<Vec<EvTerm>, SvError> {
        if self.eat_op("*") || (self.is_op("(") && self.is_op_at(1, "*") && self.is_op_at(2, ")")) {
            return Err(self.err("`@*` is not supported in testbench code; list the signals"));
        }
        if !self.eat_op("(") {
            let e = self.parse_postfix()?;
            return Ok(vec![EvTerm { edge: None, e }]);
        }
        let mut v = Vec::new();
        loop {
            let edge = if self.eat_kw("posedge") {
                Some(Edge::Pos)
            } else if self.eat_kw("negedge") {
                Some(Edge::Neg)
            } else if self.eat_kw("edge") {
                Some(Edge::Any)
            } else {
                None
            };
            let e = self.parse_postfix()?;
            if self.eat_kw("iff") {
                return Err(self.err("`iff` in event controls is not supported"));
            }
            v.push(EvTerm { edge, e });
            if self.eat_kw("or") || self.eat_op(",") {
                continue;
            }
            self.expect_op(")")?;
            break;
        }
        Ok(v)
    }
}
