//! Procedural statements.

use std::rc::Rc;

use super::{BUILTIN_TYPES, Parser, is_reserved};
use crate::SvError;
use crate::ast::*;
use crate::lex::Tok;

impl Parser<'_> {
    /// Does a declaration start here (`int x`, `my_t x`, `pkg::t#(p) x`)?
    pub(crate) fn looks_like_decl(&self) -> bool {
        match self.tok() {
            Some(Tok::Id(s)) => {
                if BUILTIN_TYPES.contains(&s.as_str()) && s != "void" {
                    return true;
                }
                if matches!(s.as_str(), "automatic" | "static" | "const" | "var" | "enum" | "struct") {
                    return true;
                }
                if s == "virtual" {
                    return true;
                }
                if is_reserved(s) {
                    return false;
                }
            }
            _ => return false,
        }
        let mut k = 1;
        loop {
            if self.is_op_at(k, "#") && self.is_op_at(k + 1, "(") {
                match self.skip_balanced_from(k + 1) {
                    Some(n) => k = n,
                    None => return false,
                }
            }
            if self.is_op_at(k, "::") && matches!(self.tok_at(k + 1), Some(Tok::Id(_))) {
                k += 2;
                continue;
            }
            break;
        }
        matches!(self.tok_at(k), Some(Tok::Id(s)) if !is_reserved(s))
            && (self.is_op_at(k + 1, ";") || self.is_op_at(k + 1, "=") || self.is_op_at(k + 1, ",") || self.is_op_at(k + 1, "["))
    }

    fn body_of(&mut self) -> Result<Body, SvError> {
        let s = self.parse_stmt()?;
        Ok(Rc::from(vec![s]))
    }

    /// Statement or `;` (null).
    fn stmt_or_null(&mut self) -> Result<Body, SvError> {
        if self.eat_op(";") {
            return Ok(Rc::from(vec![]));
        }
        self.body_of()
    }

    pub(crate) fn parse_stmt(&mut self) -> Result<S, SvError> {
        let loc = self.loc();
        let k = self.parse_stmt_kind()?;
        Ok(S { loc, k })
    }

    fn parse_block_items(&mut self, end: &str) -> Result<Vec<S>, SvError> {
        let mut v = Vec::new();
        loop {
            if self.at_end() {
                return Err(self.err(format!("missing `{end}`")));
            }
            if self.is_kw(end) {
                break;
            }
            if end == "end" && (self.is_kw("join") || self.is_kw("join_any") || self.is_kw("join_none")) {
                return Err(self.err("expected `end`"));
            }
            if self.looks_like_decl() {
                let loc = self.loc();
                let q = self.parse_var_quals();
                let ty = self.parse_data_type()?;
                for d in self.parse_declarators(ty, q)? {
                    v.push(S { loc: d.loc, k: SK::Decl(d) });
                }
                let _ = loc;
                continue;
            }
            if self.is_kw("typedef") {
                return Err(self.err("typedef inside a procedural block is not supported"));
            }
            v.push(self.parse_stmt()?);
        }
        Ok(v)
    }

    fn parse_stmt_kind(&mut self) -> Result<SK, SvError> {
        // Labels: `name: begin`.
        if self.peek_ident().is_some() && self.is_op_at(1, ":") && (self.is_kw_at(2, "begin") || self.is_kw_at(2, "fork")) {
            self.p += 2;
        }
        if self.eat_op(";") {
            return Ok(SK::Null);
        }
        if self.eat_kw("begin") {
            self.end_label();
            let v = self.parse_block_items("end")?;
            self.expect_kw("end")?;
            self.end_label();
            return Ok(SK::Block(Rc::from(v)));
        }
        if self.eat_kw("fork") {
            self.end_label();
            // Leading declarations run in the parent when the fork starts.
            let mut decls = Vec::new();
            while self.looks_like_decl() {
                let q = self.parse_var_quals();
                let ty = self.parse_data_type()?;
                for d in self.parse_declarators(ty, q)? {
                    decls.push(S { loc: d.loc, k: SK::Decl(d) });
                }
            }
            let mut branches = Vec::new();
            let kind = loop {
                if self.eat_kw("join") {
                    break JoinKind::All;
                }
                if self.eat_kw("join_any") {
                    break JoinKind::Any;
                }
                if self.eat_kw("join_none") {
                    break JoinKind::None;
                }
                if self.at_end() {
                    return Err(self.err("missing join"));
                }
                branches.push(self.body_of()?);
            };
            self.end_label();
            return Ok(SK::Fork { kind, decls: Rc::from(decls), branches });
        }
        if self.is_kw("unique") || self.is_kw("unique0") || self.is_kw("priority") {
            if self.is_kw_at(1, "if") || self.is_kw_at(1, "case") || self.is_kw_at(1, "casez") || self.is_kw_at(1, "casex") {
                self.p += 1;
            }
        }
        if self.eat_kw("if") {
            self.expect_op("(")?;
            let cond = self.parse_expr()?;
            self.expect_op(")")?;
            let then = self.stmt_or_null()?;
            let els = if self.eat_kw("else") { Some(self.stmt_or_null()?) } else { None };
            return Ok(SK::If { cond, then, els });
        }
        for (kw, kind) in [("case", CaseKind::Case), ("casez", CaseKind::Casez), ("casex", CaseKind::Casex)] {
            if self.eat_kw(kw) {
                self.expect_op("(")?;
                let e = self.parse_expr()?;
                self.expect_op(")")?;
                let inside = self.eat_kw("inside");
                let mut items = Vec::new();
                let mut default = None;
                while !self.eat_kw("endcase") {
                    if self.at_end() {
                        return Err(self.err("missing endcase"));
                    }
                    if self.eat_kw("default") {
                        let _ = self.eat_op(":");
                        default = Some(self.stmt_or_null()?);
                        continue;
                    }
                    let mut labels = Vec::new();
                    loop {
                        if inside && self.eat_op("[") {
                            let a = self.parse_expr()?;
                            self.expect_op(":")?;
                            let b = self.parse_expr()?;
                            self.expect_op("]")?;
                            labels.push(InsideItem::Range(a, b));
                        } else {
                            labels.push(InsideItem::One(self.parse_expr()?));
                        }
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    self.expect_op(":")?;
                    let body = self.stmt_or_null()?;
                    items.push((labels, body));
                }
                return Ok(SK::Case { kind, e, inside, items, default });
            }
        }
        if self.eat_kw("for") {
            self.expect_op("(")?;
            let mut init = Vec::new();
            if !self.is_op(";") {
                loop {
                    let loc = self.loc();
                    if self.looks_like_decl() || self.is_kw("int") {
                        let _ = self.eat_kw("automatic") || self.eat_kw("var");
                        let ty = self.parse_data_type()?;
                        let name = self.ident()?;
                        self.expect_op("=")?;
                        let e = self.parse_expr()?;
                        init.push(S {
                            loc,
                            k: SK::Decl(VarDecl { ty, name, udims: vec![], init: Some(e), rand: RandKind::None, is_static: false, is_const: false, loc }),
                        });
                    } else {
                        let l = self.parse_postfix()?;
                        self.expect_op("=")?;
                        let r = self.parse_expr()?;
                        init.push(S { loc, k: SK::Assign { lhs: l, op: "=", rhs: r, nb: false } });
                    }
                    if !self.eat_op(",") {
                        break;
                    }
                }
            }
            self.expect_op(";")?;
            let cond = if self.is_op(";") { None } else { Some(self.parse_expr()?) };
            self.expect_op(";")?;
            let mut step = Vec::new();
            if !self.is_op(")") {
                loop {
                    let loc = self.loc();
                    step.push(S { loc, k: self.parse_simple_assign()? });
                    if !self.eat_op(",") {
                        break;
                    }
                }
            }
            self.expect_op(")")?;
            let body = self.stmt_or_null()?;
            return Ok(SK::For { init: Rc::from(init), cond, step: Rc::from(step), body });
        }
        if self.eat_kw("foreach") {
            let (arr, vars) = self.parse_foreach_head()?;
            let body = self.stmt_or_null()?;
            return Ok(SK::Foreach { arr, vars, body });
        }
        if self.eat_kw("while") {
            self.expect_op("(")?;
            let c = self.parse_expr()?;
            self.expect_op(")")?;
            let b = self.stmt_or_null()?;
            return Ok(SK::While(c, b));
        }
        if self.eat_kw("do") {
            let b = self.stmt_or_null()?;
            self.expect_kw("while")?;
            self.expect_op("(")?;
            let c = self.parse_expr()?;
            self.expect_op(")")?;
            self.expect_op(";")?;
            return Ok(SK::DoWhile(b, c));
        }
        if self.eat_kw("repeat") {
            self.expect_op("(")?;
            let c = self.parse_expr()?;
            self.expect_op(")")?;
            let b = self.stmt_or_null()?;
            return Ok(SK::Repeat(c, b));
        }
        if self.eat_kw("forever") {
            let b = self.stmt_or_null()?;
            return Ok(SK::Forever(b));
        }
        if self.eat_op("#") {
            let d = if self.eat_op("(") {
                let e = self.parse_expr()?;
                self.expect_op(")")?;
                e
            } else {
                match self.tok().cloned() {
                    Some(Tok::Num { val, width, signed, .. }) => {
                        self.p += 1;
                        Expr::Num { val, width, signed }
                    }
                    Some(Tok::Real(v, _)) => {
                        self.p += 1;
                        Expr::Real(v)
                    }
                    Some(Tok::Time(t, _)) => {
                        self.p += 1;
                        Expr::Time(t)
                    }
                    Some(Tok::Id(_)) => self.parse_postfix()?,
                    _ => return Err(self.err("expected a delay value after `#`")),
                }
            };
            let b = self.stmt_or_null()?;
            return Ok(SK::Delay(d, b));
        }
        if self.is_op("##") {
            return Err(self.err("cycle delays `##n` need a default clocking block; use @(vif.cb) instead"));
        }
        if self.eat_op("@") {
            let ev = self.parse_event_terms()?;
            let b = self.stmt_or_null()?;
            return Ok(SK::Event(ev, b));
        }
        if self.eat_kw("wait") {
            if self.eat_kw("fork") {
                self.expect_op(";")?;
                return Ok(SK::WaitFork);
            }
            self.expect_op("(")?;
            let c = self.parse_expr()?;
            self.expect_op(")")?;
            let b = self.stmt_or_null()?;
            return Ok(SK::Wait(c, b));
        }
        if self.eat_kw("disable") {
            if self.eat_kw("fork") {
                self.expect_op(";")?;
                return Ok(SK::DisableFork);
            }
            let n = self.ident()?;
            self.expect_op(";")?;
            return Ok(SK::Disable(n));
        }
        if self.eat_op("->") || self.eat_op("->>") {
            let e = self.parse_postfix()?;
            self.expect_op(";")?;
            return Ok(SK::Trigger(e));
        }
        if self.eat_kw("return") {
            let e = if self.is_op(";") { None } else { Some(self.parse_expr()?) };
            self.expect_op(";")?;
            return Ok(SK::Return(e));
        }
        if self.eat_kw("break") {
            self.expect_op(";")?;
            return Ok(SK::Break);
        }
        if self.eat_kw("continue") {
            self.expect_op(";")?;
            return Ok(SK::Continue);
        }
        if self.is_kw("assert") || self.is_kw("assume") {
            self.p += 1;
            if self.is_kw("property") || self.is_kw("final") {
                return Err(self.err("concurrent/deferred assertions are not supported in class-based testbenches"));
            }
            let _ = self.eat_op("#") && { self.p += 1; true };
            self.expect_op("(")?;
            let cond = self.parse_expr()?;
            self.expect_op(")")?;
            let pass = if self.is_kw("else") || self.is_op(";") { None } else { Some(self.body_of()?) };
            let fail = if self.eat_kw("else") {
                Some(self.body_of()?)
            } else {
                let _ = self.eat_op(";");
                None
            };
            return Ok(SK::Assert { cond, pass, fail });
        }
        if self.eat_kw("randcase") {
            let mut arms = Vec::new();
            while !self.eat_kw("endcase") {
                if self.at_end() {
                    return Err(self.err("missing endcase"));
                }
                let w = self.parse_expr()?;
                self.expect_op(":")?;
                arms.push((w, self.stmt_or_null()?));
            }
            return Ok(SK::RandCase(arms));
        }
        if self.looks_like_decl() {
            return Err(self.err("declarations must come before statements in a block"));
        }
        let k = self.parse_simple_assign()?;
        self.expect_op(";")?;
        Ok(k)
    }

    /// Assignment / increment / call (no trailing `;`).
    fn parse_simple_assign(&mut self) -> Result<SK, SvError> {
        if self.is_op("++") || self.is_op("--") {
            let inc = self.is_op("++");
            self.p += 1;
            let e = self.parse_postfix()?;
            return Ok(SK::IncDec { e, inc });
        }
        let lhs = self.parse_postfix()?;
        if self.is_op("++") || self.is_op("--") {
            let inc = self.is_op("++");
            self.p += 1;
            return Ok(SK::IncDec { e: lhs, inc });
        }
        let op = match self.tok() {
            Some(Tok::Op(o)) if matches!(*o, "=" | "<=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" | "<<<=" | ">>>=") => *o,
            _ => {
                return match lhs {
                    Expr::Call { .. } | Expr::Sys { .. } | Expr::Ident(_) | Expr::Member(..) | Expr::Scoped(_) | Expr::Cast(..) => Ok(SK::Expr(lhs)),
                    _ => Err(self.err("expected an assignment or a call")),
                };
            }
        };
        self.p += 1;
        if self.is_op("#") || self.is_op("@") {
            return Err(self.err("intra-assignment delays are not supported; use `#d x = y;`"));
        }
        let rhs = self.parse_expr()?;
        if op == "<=" {
            return Ok(SK::Assign { lhs, op: "=", rhs, nb: true });
        }
        Ok(SK::Assign { lhs, op, rhs, nb: false })
    }
}
