//! Recursive-descent parser for the VHDL simulation subset (IEEE 1076-2008
//! syntax, behavioral testbench and RTL constructs). Unsupported constructs
//! are parse errors naming the file, line, and construct.

use super::ast::*;
use super::lex::{Spanned, Tok, is_keyword};
use crate::tb::TbError;

pub struct Parser<'a> {
    file: &'a str,
    toks: Vec<Spanned>,
    pos: usize,
}

pub fn parse_design_file(file: &str, src: &str) -> Result<DesignFile, TbError> {
    let toks = super::lex::lex(file, src)?;
    Parser { file, toks, pos: 0 }.design_file()
}

const TIME_UNITS: &[(&str, i64)] = &[
    ("fs", 1),
    ("ps", 1_000),
    ("ns", 1_000_000),
    ("us", 1_000_000_000),
    ("ms", 1_000_000_000_000),
    ("sec", 1_000_000_000_000_000),
    ("min", 60_000_000_000_000_000),
    ("hr", 3_600_000_000_000_000_000),
];

pub fn time_unit(s: &str) -> Option<i64> {
    TIME_UNITS.iter().find(|(u, _)| *u == s).map(|(_, v)| *v)
}

impl<'a> Parser<'a> {
    fn line(&self) -> usize {
        self.toks
            .get(self.pos)
            .or_else(|| self.toks.last())
            .map(|t| t.line)
            .unwrap_or(1)
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, TbError> {
        Err(TbError::new(self.file, self.line(), msg))
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.tok)
    }

    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.toks.get(self.pos + k).map(|t| &t.tok)
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Id(w)) if w == kw)
    }

    fn is_kw_at(&self, k: usize, kw: &str) -> bool {
        matches!(self.peek_at(k), Some(Tok::Id(w)) if w == kw)
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if *x == s)
    }

    fn is_sym_at(&self, k: usize, s: &str) -> bool {
        matches!(self.peek_at(k), Some(Tok::Sym(x)) if *x == s)
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> Result<(), TbError> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            self.err(format!("expected `{kw}`, found {}", self.describe()))
        }
    }

    fn expect_sym(&mut self, s: &str) -> Result<(), TbError> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            self.err(format!("expected `{s}`, found {}", self.describe()))
        }
    }

    fn describe(&self) -> String {
        match self.peek() {
            None => "end of file".into(),
            Some(Tok::Id(w)) => format!("`{w}`"),
            Some(Tok::Sym(s)) => format!("`{s}`"),
            Some(Tok::Int(v)) => format!("`{v}`"),
            Some(Tok::Real(v)) => format!("`{v}`"),
            Some(Tok::Char(c)) => format!("`'{}'`", *c as char),
            Some(Tok::Str(s)) => format!("\"{s}\""),
            Some(Tok::BitStr(s)) => format!("bit string \"{s}\""),
        }
    }

    /// A non-keyword identifier.
    fn ident(&mut self) -> Result<String, TbError> {
        match self.peek() {
            Some(Tok::Id(w)) if !is_keyword(w) => {
                let w = w.clone();
                self.pos += 1;
                Ok(w)
            }
            _ => self.err(format!("expected an identifier, found {}", self.describe())),
        }
    }

    /// Skip to (and past) the next `;` at nesting depth 0.
    fn skip_to_semi(&mut self) {
        let mut depth = 0i32;
        while let Some(t) = self.peek() {
            match t {
                Tok::Sym("(") => depth += 1,
                Tok::Sym(")") => depth -= 1,
                Tok::Sym(";") if depth <= 0 => {
                    self.pos += 1;
                    return;
                }
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// `end [kw...] [name] ;`
    fn end_of(&mut self, kws: &[&str]) -> Result<(), TbError> {
        self.expect_kw("end")?;
        for kw in kws {
            self.eat_kw(kw);
        }
        if let Some(Tok::Id(w)) = self.peek() {
            if !is_keyword(w) {
                self.pos += 1;
            }
        }
        self.expect_sym(";")
    }

    // ------------------------------------------------------------------
    // Design units
    // ------------------------------------------------------------------

    fn design_file(mut self) -> Result<DesignFile, TbError> {
        let mut df = DesignFile::default();
        while self.peek().is_some() {
            if self.eat_kw("library") {
                self.skip_to_semi();
            } else if self.eat_kw("use") {
                loop {
                    let mut parts = Vec::new();
                    while let Some(Tok::Id(w)) = self.peek() {
                        parts.push(w.clone());
                        self.pos += 1;
                        if !self.eat_sym(".") {
                            break;
                        }
                    }
                    if parts.len() >= 2 && parts[0] == "work" {
                        df.uses.push(parts[1].clone());
                    }
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.expect_sym(";")?;
            } else if self.eat_kw("context") {
                self.skip_to_semi();
            } else if self.is_kw("entity") {
                df.entities.push(self.entity()?);
            } else if self.is_kw("architecture") {
                df.archs.push(self.architecture()?);
            } else if self.is_kw("package") {
                df.packages.push(self.package()?);
            } else if self.eat_kw("configuration") {
                // Skip to the matching `end configuration;`.
                let mut depth = 0;
                while self.peek().is_some() {
                    if self.is_kw("for") {
                        depth += 1;
                    }
                    if self.is_kw("end") {
                        if self.is_kw_at(1, "for") {
                            depth -= 1;
                        } else if depth <= 0 {
                            self.pos += 1;
                            self.eat_kw("configuration");
                            self.skip_to_semi();
                            break;
                        }
                    }
                    self.pos += 1;
                }
            } else {
                return self.err(format!("expected a design unit, found {}", self.describe()));
            }
        }
        Ok(df)
    }

    fn entity(&mut self) -> Result<Entity, TbError> {
        let line = self.line();
        self.expect_kw("entity")?;
        let name = self.ident()?;
        self.expect_kw("is")?;
        let mut generics = Vec::new();
        let mut ports = Vec::new();
        loop {
            if self.eat_kw("generic") {
                self.expect_sym("(")?;
                for p in self.interface_list()? {
                    generics.push(GenericDecl { name: p.name, ty: p.ty, default: p.default });
                }
                self.expect_sym(";")?;
            } else if self.eat_kw("port") {
                self.expect_sym("(")?;
                for p in self.interface_list()? {
                    ports.push(PortDecl { name: p.name, mode: p.mode, ty: p.ty, default: p.default });
                }
                self.expect_sym(";")?;
            } else {
                break;
            }
        }
        // Entity declarative part / statements are skipped.
        while self.peek().is_some() && !self.is_kw("end") {
            self.skip_to_semi();
        }
        self.end_of(&["entity"])?;
        Ok(Entity { name, generics, ports, line })
    }

    /// `name, name : [mode] subtype [:= default]; ...)` (consumes `)`).
    fn interface_list(&mut self) -> Result<Vec<Param>, TbError> {
        let mut out = Vec::new();
        loop {
            let class = if self.eat_kw("signal") {
                Some(ObjClass::Signal)
            } else if self.eat_kw("variable") {
                Some(ObjClass::Variable)
            } else if self.eat_kw("constant") {
                Some(ObjClass::Constant)
            } else if self.eat_kw("file") {
                Some(ObjClass::File)
            } else {
                None
            };
            if self.is_kw("type") || self.is_kw("package") || self.is_kw("function") || self.is_kw("procedure") {
                return self.err("generic types/packages/subprograms are not supported");
            }
            let mut names = vec![self.ident()?];
            while self.eat_sym(",") {
                names.push(self.ident()?);
            }
            self.expect_sym(":")?;
            let mode = if self.eat_kw("in") {
                Mode::In
            } else if self.eat_kw("out") || self.eat_kw("buffer") {
                Mode::Out
            } else if self.eat_kw("inout") || self.eat_kw("linkage") {
                Mode::InOut
            } else {
                Mode::In
            };
            let ty = self.subtype_ind()?;
            self.eat_kw("bus");
            let default = if self.eat_sym(":=") { Some(self.expr()?) } else { None };
            for n in names {
                out.push(Param { class: class.clone(), name: n, mode: mode.clone(), ty: ty.clone(), default: default.clone() });
            }
            if self.eat_sym(";") {
                continue;
            }
            self.expect_sym(")")?;
            return Ok(out);
        }
    }

    fn architecture(&mut self) -> Result<Architecture, TbError> {
        let line = self.line();
        self.expect_kw("architecture")?;
        let name = self.ident()?;
        self.expect_kw("of")?;
        let entity = self.ident()?;
        self.expect_kw("is")?;
        let decls = self.decls()?;
        self.expect_kw("begin")?;
        let mut stmts = Vec::new();
        while !self.is_kw("end") {
            if self.peek().is_none() {
                return self.err("unexpected end of file in architecture");
            }
            stmts.extend(self.concurrent()?);
        }
        self.end_of(&["architecture"])?;
        Ok(Architecture { name, entity, decls, stmts, line })
    }

    fn package(&mut self) -> Result<Package, TbError> {
        self.expect_kw("package")?;
        let is_body = self.eat_kw("body");
        let name = self.ident()?;
        self.expect_kw("is")?;
        if self.is_kw("new") {
            return self.err("package instantiation is not supported");
        }
        let decls = self.decls()?;
        self.end_of(&["package", "body"])?;
        Ok(Package { name, decls, is_body })
    }

    // ------------------------------------------------------------------
    // Declarations
    // ------------------------------------------------------------------

    fn decls(&mut self) -> Result<Vec<Decl>, TbError> {
        let mut out = Vec::new();
        loop {
            let line = self.line();
            if self.eat_kw("shared") {
                if !self.is_kw("variable") {
                    return self.err("expected `variable` after `shared`");
                }
            }
            if self.is_kw("signal") || self.is_kw("variable") || self.is_kw("constant") || self.is_kw("file") {
                let class = match self.peek() {
                    Some(Tok::Id(w)) if w == "signal" => ObjClass::Signal,
                    Some(Tok::Id(w)) if w == "variable" => ObjClass::Variable,
                    Some(Tok::Id(w)) if w == "constant" => ObjClass::Constant,
                    _ => ObjClass::File,
                };
                self.pos += 1;
                let mut names = vec![self.ident()?];
                while self.eat_sym(",") {
                    names.push(self.ident()?);
                }
                self.expect_sym(":")?;
                let ty = self.subtype_ind()?;
                if class == ObjClass::File {
                    self.skip_to_semi();
                    out.push(Decl::Object { class, names, ty, init: None, line });
                    continue;
                }
                self.eat_kw("register");
                self.eat_kw("bus");
                let init = if self.eat_sym(":=") { Some(self.expr()?) } else { None };
                self.expect_sym(";")?;
                out.push(Decl::Object { class, names, ty, init, line });
            } else if self.eat_kw("type") {
                let name = self.ident()?;
                if self.eat_sym(";") {
                    continue; // incomplete type declaration
                }
                self.expect_kw("is")?;
                let def = self.type_def()?;
                out.push(Decl::Type { name, def, line });
            } else if self.eat_kw("subtype") {
                let name = self.ident()?;
                self.expect_kw("is")?;
                let ty = self.subtype_ind()?;
                self.expect_sym(";")?;
                out.push(Decl::Subtype { name, ty, line });
            } else if self.is_kw("function") || self.is_kw("procedure") || self.is_kw("pure") || self.is_kw("impure") {
                out.push(Decl::Subprogram(Box::new(self.subprogram()?)));
            } else if self.eat_kw("alias") {
                let name = self.ident()?;
                if self.eat_sym(":") {
                    self.subtype_ind()?;
                }
                self.expect_kw("is")?;
                let target = self.name()?;
                self.expect_sym(";")?;
                out.push(Decl::Alias { name, target, line });
            } else if self.eat_kw("component") {
                while self.peek().is_some() && !(self.is_kw("end") && self.is_kw_at(1, "component")) {
                    self.pos += 1;
                }
                self.end_of(&["component"])?;
                out.push(Decl::Ignored);
            } else if self.eat_kw("attribute") || self.eat_kw("use") || self.eat_kw("disconnect") || self.eat_kw("group") {
                self.skip_to_semi();
                out.push(Decl::Ignored);
            } else {
                return Ok(out);
            }
        }
    }

    fn type_def(&mut self) -> Result<TypeDef, TbError> {
        if self.eat_sym("(") {
            let mut lits = Vec::new();
            loop {
                match self.peek().cloned() {
                    Some(Tok::Id(w)) => lits.push(w),
                    Some(Tok::Char(c)) => lits.push(format!("'{}'", c as char)),
                    _ => return self.err("expected an enumeration literal"),
                }
                self.pos += 1;
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym(")")?;
            self.expect_sym(";")?;
            return Ok(TypeDef::Enum(lits));
        }
        if self.eat_kw("range") {
            let lo = self.simple_expr()?;
            let asc = self.direction()?;
            let hi = self.simple_expr()?;
            if self.eat_kw("units") {
                return self.err("user-defined physical types are not supported");
            }
            self.expect_sym(";")?;
            if matches!(lo, Expr::Real(_)) || matches!(hi, Expr::Real(_)) {
                return Ok(TypeDef::Real);
            }
            return Ok(TypeDef::Int(lo, asc, hi));
        }
        if self.eat_kw("array") {
            self.expect_sym("(")?;
            let mut range = None;
            let mut index_mark = None;
            // `natural range <>` | `0 to 3` | `t_enum` | `integer range 0 to 3`
            if matches!(self.peek(), Some(Tok::Id(_))) && self.is_kw_at(1, "range") {
                let mark = self.ident()?;
                self.expect_kw("range")?;
                if self.eat_sym("<>") {
                    index_mark = Some(mark);
                } else {
                    let lo = self.simple_expr()?;
                    let asc = self.direction()?;
                    let hi = self.simple_expr()?;
                    range = Some((lo, asc, hi));
                }
            } else {
                let lo = self.simple_expr()?;
                if self.is_kw("to") || self.is_kw("downto") {
                    let asc = self.direction()?;
                    let hi = self.simple_expr()?;
                    range = Some((lo, asc, hi));
                } else if let Expr::Name(n) = lo {
                    index_mark = Some(n);
                } else {
                    return self.err("unsupported array index constraint");
                }
            }
            if self.is_sym(",") {
                return self.err("multi-dimensional arrays are not supported (use an array of arrays)");
            }
            self.expect_sym(")")?;
            self.expect_kw("of")?;
            let elem = self.subtype_ind()?;
            self.expect_sym(";")?;
            return Ok(TypeDef::Array { range, index_mark, elem });
        }
        if self.eat_kw("record") {
            let mut fields = Vec::new();
            while !self.is_kw("end") {
                let mut names = vec![self.ident()?];
                while self.eat_sym(",") {
                    names.push(self.ident()?);
                }
                self.expect_sym(":")?;
                let ty = self.subtype_ind()?;
                self.expect_sym(";")?;
                for n in names {
                    fields.push((n, ty.clone()));
                }
            }
            self.end_of(&["record"])?;
            return Ok(TypeDef::Record(fields));
        }
        if self.is_kw("access") || self.is_kw("file") {
            self.skip_to_semi();
            return Ok(TypeDef::Opaque);
        }
        if self.eat_kw("protected") {
            return self.err("protected types are not supported");
        }
        self.err(format!("unsupported type definition at {}", self.describe()))
    }

    fn direction(&mut self) -> Result<bool, TbError> {
        if self.eat_kw("to") {
            Ok(true)
        } else if self.eat_kw("downto") {
            Ok(false)
        } else {
            self.err(format!("expected `to` or `downto`, found {}", self.describe()))
        }
    }

    pub fn subtype_ind(&mut self) -> Result<SubtypeInd, TbError> {
        let line = self.line();
        let mut mark = self.ident()?;
        // Resolution function: `resolved std_ulogic`.
        if let Some(Tok::Id(w)) = self.peek() {
            if !is_keyword(w) {
                mark = self.ident()?;
            }
        }
        while self.eat_sym(".") {
            mark = self.ident()?;
        }
        let mut range = None;
        let mut elem_range = None;
        if self.eat_kw("range") {
            if self.is_sym_at(1, "'") || matches!(self.peek(), Some(Tok::Id(_))) && self.is_sym_at(1, "'") {
                let e = self.name()?;
                range = Some((e.clone(), true, e));
            } else {
                let lo = self.simple_expr()?;
                let asc = self.direction()?;
                let hi = self.simple_expr()?;
                range = Some((lo, asc, hi));
            }
        } else if self.is_sym("(") {
            self.pos += 1;
            if self.is_kw("open") {
                self.pos += 1;
            } else {
                let lo = self.simple_expr()?;
                let asc = self.direction()?;
                let hi = self.simple_expr()?;
                range = Some((lo, asc, hi));
            }
            self.expect_sym(")")?;
            if self.eat_sym("(") {
                let lo = self.simple_expr()?;
                let asc = self.direction()?;
                let hi = self.simple_expr()?;
                elem_range = Some((lo, asc, hi));
                self.expect_sym(")")?;
            }
        }
        Ok(SubtypeInd { mark, range, elem_range, line })
    }

    fn subprogram(&mut self) -> Result<Subprogram, TbError> {
        let line = self.line();
        self.eat_kw("pure");
        self.eat_kw("impure");
        let is_function = if self.eat_kw("function") {
            true
        } else {
            self.expect_kw("procedure")?;
            false
        };
        let name = match self.peek().cloned() {
            Some(Tok::Id(w)) => {
                self.pos += 1;
                w
            }
            Some(Tok::Str(s)) => {
                self.pos += 1;
                format!("\"{}\"", s.to_ascii_lowercase())
            }
            _ => return self.err("expected a subprogram name"),
        };
        let mut params = Vec::new();
        self.eat_kw("parameter");
        if self.eat_sym("(") {
            params = self.interface_list()?;
        }
        let ret = if is_function {
            self.expect_kw("return")?;
            Some(self.subtype_ind()?)
        } else {
            None
        };
        if self.eat_sym(";") {
            return Ok(Subprogram { name, is_function, params, ret, decls: Vec::new(), body: None, line });
        }
        self.expect_kw("is")?;
        let decls = self.decls()?;
        self.expect_kw("begin")?;
        let body = self.seq_stmts()?;
        self.end_of(&["function", "procedure"])?;
        Ok(Subprogram { name, is_function, params, ret, decls, body: Some(body), line })
    }

    // ------------------------------------------------------------------
    // Concurrent statements
    // ------------------------------------------------------------------

    fn concurrent(&mut self) -> Result<Vec<Concurrent>, TbError> {
        let line = self.line();
        let mut label = None;
        if matches!(self.peek(), Some(Tok::Id(w)) if !is_keyword(w)) && self.is_sym_at(1, ":") {
            label = Some(self.ident()?);
            self.pos += 1;
        }
        self.eat_kw("postponed");
        if self.eat_kw("process") {
            let mut sens = None;
            let mut sens_all = false;
            if self.eat_sym("(") {
                if self.eat_kw("all") {
                    sens_all = true;
                    sens = Some(Vec::new());
                } else {
                    let mut v = vec![self.name()?];
                    while self.eat_sym(",") {
                        v.push(self.name()?);
                    }
                    sens = Some(v);
                }
                self.expect_sym(")")?;
            }
            self.eat_kw("is");
            let decls = self.decls()?;
            self.expect_kw("begin")?;
            let body = self.seq_stmts()?;
            self.expect_kw("end")?;
            self.eat_kw("postponed");
            self.expect_kw("process")?;
            if let Some(Tok::Id(w)) = self.peek() {
                if !is_keyword(w) {
                    self.pos += 1;
                }
            }
            self.expect_sym(";")?;
            return Ok(vec![Concurrent::Process { label, sens, sens_all, decls, body, line }]);
        }
        if self.is_kw("entity") || self.is_kw("component") || self.is_kw("configuration") {
            let Some(label) = label else {
                return self.err("instantiation needs a label");
            };
            let configuration = self.is_kw("configuration");
            self.pos += 1;
            if configuration {
                return self.err("configuration instantiation is not supported");
            }
            let mut unit = self.ident()?;
            while self.eat_sym(".") {
                unit = self.ident()?;
            }
            if self.eat_sym("(") {
                self.ident()?; // architecture name
                self.expect_sym(")")?;
            }
            return self.instance_tail(label, unit, line).map(|c| vec![c]);
        }
        if let Some(lbl) = &label {
            // `label : comp_name [generic map] port map (...)`
            if matches!(self.peek(), Some(Tok::Id(w)) if !is_keyword(w)) && (self.is_kw_at(1, "port") || self.is_kw_at(1, "generic")) {
                let unit = self.ident()?;
                return self.instance_tail(lbl.clone(), unit, line).map(|c| vec![c]);
            }
        }
        if self.is_kw("for") || (self.is_kw("if") && label.is_some()) || self.is_kw("block") || self.is_kw("case") {
            return self.err("generate/block statements are not supported by the VHDL simulator");
        }
        if self.eat_kw("assert") {
            let cond = self.expr()?;
            let msg = if self.eat_kw("report") { Some(self.expr()?) } else { None };
            let severity = if self.eat_kw("severity") { Some(self.expr()?) } else { None };
            self.expect_sym(";")?;
            return Ok(vec![Concurrent::Assert { cond, msg, severity, line }]);
        }
        if self.eat_kw("with") {
            let sel = self.expr()?;
            self.expect_kw("select")?;
            self.eat_sym("?");
            let target = self.name()?;
            self.expect_sym("<=")?;
            self.eat_kw("guarded");
            self.delay_mechanism()?;
            let mut arms = Vec::new();
            loop {
                let w = self.waveform()?;
                self.expect_kw("when")?;
                let choices = self.choices()?;
                arms.push((w, choices));
                if self.eat_sym(",") {
                    continue;
                }
                break;
            }
            self.expect_sym(";")?;
            return Ok(vec![Concurrent::Select { sel, target, arms, line }]);
        }
        // Concurrent signal assignment or procedure call.
        let target = self.target()?;
        if self.eat_sym("<=") {
            self.eat_kw("guarded");
            self.delay_mechanism()?;
            let arms = self.cond_waveforms()?;
            self.expect_sym(";")?;
            return Ok(vec![Concurrent::Assign { target, arms, line }]);
        }
        if self.eat_sym(";") {
            return Ok(vec![Concurrent::Call { name: target, line }]);
        }
        self.err(format!("unsupported concurrent statement at {}", self.describe()))
    }

    fn instance_tail(&mut self, label: String, unit: String, line: usize) -> Result<Concurrent, TbError> {
        let mut generics = Vec::new();
        let mut ports = Vec::new();
        if self.eat_kw("generic") {
            self.expect_kw("map")?;
            self.expect_sym("(")?;
            generics = self.assoc_list()?;
        }
        if self.eat_kw("port") {
            self.expect_kw("map")?;
            self.expect_sym("(")?;
            ports = self.assoc_list()?;
        }
        self.expect_sym(";")?;
        Ok(Concurrent::Instance { label, unit, generics, ports, line })
    }

    /// `formal => actual, ...)` (consumes `)`).
    fn assoc_list(&mut self) -> Result<Vec<Arg>, TbError> {
        let mut out = Vec::new();
        loop {
            let formal = if matches!(self.peek(), Some(Tok::Id(_))) && self.is_sym_at(1, "=>") {
                let f = self.ident()?;
                self.pos += 1;
                Some(f)
            } else {
                None
            };
            let actual = if self.eat_kw("open") { Expr::Open } else { self.expr()? };
            out.push(Arg { formal, actual });
            if self.eat_sym(",") {
                continue;
            }
            self.expect_sym(")")?;
            return Ok(out);
        }
    }

    // ------------------------------------------------------------------
    // Sequential statements
    // ------------------------------------------------------------------

    fn seq_stmts(&mut self) -> Result<Vec<Stmt>, TbError> {
        let mut out = Vec::new();
        while !(self.is_kw("end") || self.is_kw("elsif") || self.is_kw("else") || self.is_kw("when")) {
            if self.peek().is_none() {
                return self.err("unexpected end of file in a statement list");
            }
            out.push(self.seq_stmt()?);
        }
        Ok(out)
    }

    fn seq_stmt(&mut self) -> Result<Stmt, TbError> {
        let line = self.line();
        let mut label = None;
        if matches!(self.peek(), Some(Tok::Id(w)) if !is_keyword(w)) && self.is_sym_at(1, ":") {
            label = Some(self.ident()?);
            self.pos += 1;
        }
        if self.eat_kw("if") {
            let mut arms = Vec::new();
            let c = self.expr()?;
            self.expect_kw("then")?;
            let b = self.seq_stmts()?;
            arms.push((c, b));
            let mut else_b = Vec::new();
            loop {
                if self.eat_kw("elsif") {
                    let c = self.expr()?;
                    self.expect_kw("then")?;
                    let b = self.seq_stmts()?;
                    arms.push((c, b));
                } else if self.eat_kw("else") {
                    else_b = self.seq_stmts()?;
                } else {
                    break;
                }
            }
            self.end_of(&["if"])?;
            return Ok(Stmt::If { arms, else_b, line });
        }
        if self.eat_kw("case") {
            let sel = self.expr()?;
            self.expect_kw("is")?;
            let mut arms = Vec::new();
            while self.eat_kw("when") {
                let choices = self.choices()?;
                self.expect_sym("=>")?;
                let body = self.seq_stmts()?;
                arms.push((choices, body));
            }
            self.end_of(&["case"])?;
            return Ok(Stmt::Case { sel, arms, line });
        }
        if self.is_kw("for") || self.is_kw("while") || self.is_kw("loop") {
            let kind = if self.eat_kw("for") {
                let var = self.ident()?;
                self.expect_kw("in")?;
                let start = self.simple_expr()?;
                if self.is_kw("to") || self.is_kw("downto") {
                    let asc = self.direction()?;
                    let end = self.simple_expr()?;
                    LoopKind::For(var, ForRange::Range(start, asc, end))
                } else {
                    LoopKind::For(var, ForRange::Attr(start))
                }
            } else if self.eat_kw("while") {
                LoopKind::While(self.expr()?)
            } else {
                LoopKind::Forever
            };
            self.expect_kw("loop")?;
            let body = self.seq_stmts()?;
            self.end_of(&["loop"])?;
            return Ok(Stmt::Loop { label, kind, body, line });
        }
        if self.is_kw("exit") || self.is_kw("next") {
            let exit = self.is_kw("exit");
            self.pos += 1;
            let lbl = match self.peek() {
                Some(Tok::Id(w)) if !is_keyword(w) => Some(self.ident()?),
                _ => None,
            };
            let cond = if self.eat_kw("when") { Some(self.expr()?) } else { None };
            self.expect_sym(";")?;
            return Ok(if exit {
                Stmt::Exit { label: lbl, cond, line }
            } else {
                Stmt::Next { label: lbl, cond, line }
            });
        }
        if self.eat_kw("wait") {
            let mut on = Vec::new();
            if self.eat_kw("on") {
                on.push(self.name()?);
                while self.eat_sym(",") {
                    on.push(self.name()?);
                }
            }
            let until = if self.eat_kw("until") { Some(self.expr()?) } else { None };
            let timeout = if self.eat_kw("for") { Some(self.expr()?) } else { None };
            self.expect_sym(";")?;
            return Ok(Stmt::Wait { on, until, timeout, line });
        }
        if self.eat_kw("report") {
            let msg = self.expr()?;
            let severity = if self.eat_kw("severity") { Some(self.expr()?) } else { None };
            self.expect_sym(";")?;
            return Ok(Stmt::Report { msg, severity, line });
        }
        if self.eat_kw("assert") {
            let cond = self.expr()?;
            let msg = if self.eat_kw("report") { Some(self.expr()?) } else { None };
            let severity = if self.eat_kw("severity") { Some(self.expr()?) } else { None };
            self.expect_sym(";")?;
            return Ok(Stmt::Assert { cond, msg, severity, line });
        }
        if self.eat_kw("return") {
            let value = if self.is_sym(";") { None } else { Some(self.expr()?) };
            self.expect_sym(";")?;
            return Ok(Stmt::Return { value, line });
        }
        if self.eat_kw("null") {
            self.expect_sym(";")?;
            return Ok(Stmt::Null);
        }
        if self.is_kw("with") {
            return self.err("sequential selected assignment is not supported");
        }
        let target = self.target()?;
        if self.eat_sym("<=") {
            if self.is_kw("force") || self.is_kw("release") {
                return self.err("force/release is not supported");
            }
            let transport = self.delay_mechanism()?;
            let arms = self.cond_waveforms()?;
            self.expect_sym(";")?;
            return Ok(Stmt::SigAssign { target, arms, transport, line });
        }
        if self.eat_sym(":=") {
            let mut arms = Vec::new();
            loop {
                let v = self.expr()?;
                if self.eat_kw("when") {
                    let c = self.expr()?;
                    arms.push((v, Some(c)));
                    if self.eat_kw("else") {
                        continue;
                    }
                } else {
                    arms.push((v, None));
                }
                break;
            }
            self.expect_sym(";")?;
            return Ok(Stmt::VarAssign { target, arms, line });
        }
        if self.eat_sym(";") {
            return Ok(Stmt::Call { name: target, line });
        }
        self.err(format!("expected `<=`, `:=` or `;` after a name, found {}", self.describe()))
    }

    /// `transport` / `inertial` / `reject t inertial` → true for transport.
    fn delay_mechanism(&mut self) -> Result<bool, TbError> {
        if self.eat_kw("transport") {
            return Ok(true);
        }
        if self.eat_kw("reject") {
            self.expr()?;
            self.expect_kw("inertial")?;
            return Ok(false);
        }
        self.eat_kw("inertial");
        Ok(false)
    }

    fn waveform(&mut self) -> Result<Waveform, TbError> {
        if self.eat_kw("unaffected") {
            return Ok(Waveform { elems: Vec::new() });
        }
        let mut elems = Vec::new();
        loop {
            let v = self.expr()?;
            let after = if self.eat_kw("after") { Some(self.expr()?) } else { None };
            elems.push((v, after));
            if self.is_sym(",") && !self.in_select_arm() {
                self.pos += 1;
                continue;
            }
            return Ok(Waveform { elems });
        }
    }

    /// Commas in a selected assignment separate arms, not waveform elements:
    /// a waveform element there is always followed by `when`.
    fn in_select_arm(&self) -> bool {
        false
    }

    fn cond_waveforms(&mut self) -> Result<Vec<(Waveform, Option<Expr>)>, TbError> {
        let mut arms = Vec::new();
        loop {
            let w = self.waveform()?;
            if self.eat_kw("when") {
                let c = self.expr()?;
                arms.push((w, Some(c)));
                if self.eat_kw("else") {
                    continue;
                }
            } else {
                arms.push((w, None));
            }
            return Ok(arms);
        }
    }

    fn choices(&mut self) -> Result<Vec<Choice>, TbError> {
        let mut out = Vec::new();
        loop {
            if self.eat_kw("others") {
                out.push(Choice::Others);
            } else {
                let e = self.simple_expr()?;
                if self.is_kw("to") || self.is_kw("downto") {
                    let asc = self.direction()?;
                    let hi = self.simple_expr()?;
                    out.push(Choice::Range(e, asc, hi));
                } else {
                    out.push(Choice::Expr(e));
                }
            }
            if !self.eat_sym("|") {
                return Ok(out);
            }
        }
    }

    // ------------------------------------------------------------------
    // Names and expressions
    // ------------------------------------------------------------------

    /// Assignment target: a name or an aggregate of names.
    fn target(&mut self) -> Result<Expr, TbError> {
        if self.is_sym("(") {
            return self.primary();
        }
        self.name()
    }

    pub fn name(&mut self) -> Result<Expr, TbError> {
        let mut e = match self.peek().cloned() {
            Some(Tok::Id(w)) if !is_keyword(w.as_str()) => {
                self.pos += 1;
                Expr::Name(w)
            }
            Some(Tok::Str(s)) => {
                // Operator symbol used as a function name: "and"(a, b)
                self.pos += 1;
                Expr::Name(format!("\"{}\"", s.to_ascii_lowercase()))
            }
            _ => return self.err(format!("expected a name, found {}", self.describe())),
        };
        loop {
            if self.is_sym(".") {
                self.pos += 1;
                let f = match self.peek().cloned() {
                    Some(Tok::Id(w)) => {
                        self.pos += 1;
                        w
                    }
                    _ => return self.err("expected a name after `.`"),
                };
                e = Expr::Select(Box::new(e), f);
            } else if self.is_sym("(") {
                self.pos += 1;
                let args = self.call_args()?;
                e = Expr::Call(Box::new(e), args);
            } else if self.is_sym("'") {
                // Qualified expression or attribute.
                if self.is_sym_at(1, "(") {
                    self.pos += 1;
                    let mark = match &e {
                        Expr::Name(n) => n.clone(),
                        Expr::Select(_, n) => n.clone(),
                        _ => return self.err("bad qualified expression"),
                    };
                    let inner = self.primary()?;
                    e = Expr::Qualified(mark, Box::new(inner));
                    continue;
                }
                self.pos += 1;
                let attr = match self.peek().cloned() {
                    Some(Tok::Id(w)) => {
                        self.pos += 1;
                        w
                    }
                    _ => return self.err("expected an attribute name after `'`"),
                };
                let mut args = Vec::new();
                if self.is_sym("(") && attr_takes_args(&attr) {
                    self.pos += 1;
                    args.push(self.expr()?);
                    while self.eat_sym(",") {
                        args.push(self.expr()?);
                    }
                    self.expect_sym(")")?;
                }
                e = Expr::Attr(Box::new(e), attr, args);
            } else {
                return Ok(e);
            }
        }
    }

    /// Arguments after `(` (consumes `)`): `[formal =>] expr | range`.
    fn call_args(&mut self) -> Result<Vec<Arg>, TbError> {
        let mut out = Vec::new();
        loop {
            let formal = if matches!(self.peek(), Some(Tok::Id(w)) if !is_keyword(w)) && self.is_sym_at(1, "=>") {
                let f = self.ident()?;
                self.pos += 1;
                Some(f)
            } else {
                None
            };
            let actual = if self.eat_kw("open") {
                Expr::Open
            } else {
                let e = self.expr()?;
                if self.is_kw("to") || self.is_kw("downto") {
                    let asc = self.direction()?;
                    let hi = self.expr()?;
                    Expr::Range(Box::new(e), asc, Box::new(hi))
                } else {
                    e
                }
            };
            out.push(Arg { formal, actual });
            if self.eat_sym(",") {
                continue;
            }
            self.expect_sym(")")?;
            return Ok(out);
        }
    }

    pub fn expr(&mut self) -> Result<Expr, TbError> {
        if self.eat_sym("??") {
            let e = self.relation()?;
            return Ok(Expr::Unary(UnOp::Cond, Box::new(e)));
        }
        let mut lhs = self.relation()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Id(w)) => match w.as_str() {
                    "and" => BinOp::And,
                    "or" => BinOp::Or,
                    "nand" => BinOp::Nand,
                    "nor" => BinOp::Nor,
                    "xor" => BinOp::Xor,
                    "xnor" => BinOp::Xnor,
                    _ => return Ok(lhs),
                },
                _ => return Ok(lhs),
            };
            self.pos += 1;
            let rhs = self.relation()?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
    }

    fn relation(&mut self) -> Result<Expr, TbError> {
        let lhs = self.shift_expr()?;
        let op = match self.peek() {
            Some(Tok::Sym("=")) | Some(Tok::Sym("?=")) => BinOp::Eq,
            Some(Tok::Sym("/=")) | Some(Tok::Sym("?/")) => BinOp::Ne,
            Some(Tok::Sym("<")) => BinOp::Lt,
            Some(Tok::Sym("<=")) => BinOp::Le,
            Some(Tok::Sym(">")) => BinOp::Gt,
            Some(Tok::Sym(">=")) => BinOp::Ge,
            _ => return Ok(lhs),
        };
        self.pos += 1;
        let rhs = self.shift_expr()?;
        Ok(Expr::Binary(op, Box::new(lhs), Box::new(rhs)))
    }

    fn shift_expr(&mut self) -> Result<Expr, TbError> {
        let lhs = self.simple_expr()?;
        let op = match self.peek() {
            Some(Tok::Id(w)) => match w.as_str() {
                "sll" => BinOp::Sll,
                "srl" => BinOp::Srl,
                "sla" => BinOp::Sla,
                "sra" => BinOp::Sra,
                "rol" => BinOp::Rol,
                "ror" => BinOp::Ror,
                _ => return Ok(lhs),
            },
            _ => return Ok(lhs),
        };
        self.pos += 1;
        let rhs = self.simple_expr()?;
        Ok(Expr::Binary(op, Box::new(lhs), Box::new(rhs)))
    }

    pub fn simple_expr(&mut self) -> Result<Expr, TbError> {
        let neg = if self.eat_sym("-") {
            Some(UnOp::Neg)
        } else if self.eat_sym("+") {
            Some(UnOp::Plus)
        } else {
            None
        };
        let mut lhs = self.term()?;
        if let Some(op) = neg {
            lhs = Expr::Unary(op, Box::new(lhs));
        }
        loop {
            let op = match self.peek() {
                Some(Tok::Sym("+")) => BinOp::Add,
                Some(Tok::Sym("-")) => BinOp::Sub,
                Some(Tok::Sym("&")) => BinOp::Concat,
                _ => return Ok(lhs),
            };
            self.pos += 1;
            let rhs = self.term()?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
    }

    fn term(&mut self) -> Result<Expr, TbError> {
        let mut lhs = self.factor()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Sym("*")) => BinOp::Mul,
                Some(Tok::Sym("/")) => BinOp::Div,
                Some(Tok::Id(w)) if w == "mod" => BinOp::Mod,
                Some(Tok::Id(w)) if w == "rem" => BinOp::Rem,
                _ => return Ok(lhs),
            };
            self.pos += 1;
            let rhs = self.factor()?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
    }

    fn factor(&mut self) -> Result<Expr, TbError> {
        let un = match self.peek() {
            Some(Tok::Id(w)) => match w.as_str() {
                "abs" => Some(UnOp::Abs),
                "not" => Some(UnOp::Not),
                "and" => Some(UnOp::RedAnd),
                "or" => Some(UnOp::RedOr),
                "xor" => Some(UnOp::RedXor),
                _ => None,
            },
            _ => None,
        };
        if let Some(op) = un {
            self.pos += 1;
            let e = self.primary()?;
            return Ok(Expr::Unary(op, Box::new(e)));
        }
        let base = self.primary()?;
        if self.eat_sym("**") {
            let e = self.primary()?;
            return Ok(Expr::Binary(BinOp::Pow, Box::new(base), Box::new(e)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<Expr, TbError> {
        match self.peek().cloned() {
            Some(Tok::Int(v)) => {
                self.pos += 1;
                if let Some(Tok::Id(u)) = self.peek() {
                    if let Some(m) = time_unit(u) {
                        self.pos += 1;
                        return Ok(Expr::Time(v.saturating_mul(m)));
                    }
                }
                Ok(Expr::Int(v))
            }
            Some(Tok::Real(v)) => {
                self.pos += 1;
                if let Some(Tok::Id(u)) = self.peek() {
                    if let Some(m) = time_unit(u) {
                        self.pos += 1;
                        return Ok(Expr::Time((v * m as f64).round() as i64));
                    }
                }
                Ok(Expr::Real(v))
            }
            Some(Tok::Char(c)) => {
                self.pos += 1;
                Ok(Expr::Char(c))
            }
            Some(Tok::BitStr(s)) => {
                self.pos += 1;
                Ok(Expr::BitStr(s))
            }
            Some(Tok::Str(s)) => {
                // Operator symbol call: "and"(a, b)
                if self.is_sym_at(1, "(") && is_operator_symbol(&s) {
                    return self.name();
                }
                self.pos += 1;
                Ok(Expr::Str(s))
            }
            Some(Tok::Sym("(")) => {
                self.pos += 1;
                let mut assocs = Vec::new();
                loop {
                    // Choices `=>` value, or a positional value.
                    let save = self.pos;
                    let mut choices = None;
                    if self.is_kw("others") {
                        self.pos += 1;
                        let mut ch = vec![Choice::Others];
                        while self.eat_sym("|") {
                            ch.extend(self.choices()?);
                        }
                        choices = Some(ch);
                    } else {
                        // Try `choice {| choice} =>`.
                        if let Ok(ch) = self.choices() {
                            if self.is_sym("=>") {
                                choices = Some(ch);
                            } else {
                                self.pos = save;
                            }
                        } else {
                            self.pos = save;
                        }
                    }
                    let value = if let Some(ch) = choices {
                        self.expect_sym("=>")?;
                        let v = self.expr()?;
                        assocs.push(Assoc { choices: ch, value: v });
                        None
                    } else {
                        Some(self.expr()?)
                    };
                    if let Some(v) = value {
                        assocs.push(Assoc { choices: Vec::new(), value: v });
                    }
                    if self.eat_sym(",") {
                        continue;
                    }
                    self.expect_sym(")")?;
                    break;
                }
                if assocs.len() == 1 && assocs[0].choices.is_empty() {
                    return Ok(assocs.pop().map(|a| a.value).unwrap_or(Expr::Int(0)));
                }
                Ok(Expr::Aggregate(assocs))
            }
            Some(Tok::Id(w)) if w == "null" => {
                self.pos += 1;
                Ok(Expr::Aggregate(Vec::new()))
            }
            Some(Tok::Id(w)) if w == "new" => self.err("allocators (`new`) are not supported"),
            Some(Tok::Id(w)) if !is_keyword(&w) => self.name(),
            _ => self.err(format!("expected an expression, found {}", self.describe())),
        }
    }
}

fn is_operator_symbol(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "and" | "or" | "xor" | "nand" | "nor" | "xnor" | "not" | "+" | "-" | "*" | "/" | "=" | "/=" | "<" | ">" | "<=" | ">=" | "&"
    )
}

fn attr_takes_args(attr: &str) -> bool {
    matches!(
        attr,
        "image" | "value" | "pos" | "val" | "succ" | "pred" | "leftof" | "rightof" | "length" | "range"
            | "reverse_range" | "high" | "low" | "left" | "right" | "delayed" | "stable" | "quiet"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_typical_testbench() {
        let src = r#"
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity counter_tb is
end entity;

architecture sim of counter_tb is
  constant CLK_PERIOD : time := 10 ns;
  signal clk   : std_logic := '0';
  signal rst   : std_logic := '1';
  signal q     : std_logic_vector(3 downto 0);
  type state_t is (IDLE, RUN, DONE);
  type rec_t is record a : integer; b : std_logic; end record;
begin
  clk <= not clk after CLK_PERIOD / 2;
  dut : entity work.counter port map (clk => clk, rst => rst, q => q);
  stim : process
    variable v : unsigned(3 downto 0) := (others => '0');
  begin
    wait until rising_edge(clk);
    rst <= '0';
    for i in 0 to 3 loop
      wait until rising_edge(clk);
    end loop;
    assert q = "0100" report "bad count " & integer'image(to_integer(unsigned(q))) severity error;
    std.env.stop;
    wait;
  end process;
end architecture;
"#;
        let df = parse_design_file("t.vhd", src).unwrap();
        assert_eq!(df.entities.len(), 1);
        let a = &df.archs[0];
        assert_eq!(a.decls.len(), 6);
        assert_eq!(a.stmts.len(), 3);
        match &a.stmts[1] {
            Concurrent::Instance { unit, ports, .. } => {
                assert_eq!(unit, "counter");
                assert_eq!(ports.len(), 3);
            }
            other => panic!("{other:?}"),
        }
        match &a.stmts[2] {
            Concurrent::Process { body, .. } => assert_eq!(body.len(), 6),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unsupported_constructs_name_the_line() {
        let src = "entity t is end;\narchitecture a of t is\nbegin\n  g: for i in 0 to 3 generate\n  end generate;\nend;\n";
        let e = parse_design_file("t.vhd", src).unwrap_err();
        assert_eq!(e.line, 4);
        assert!(e.msg.contains("generate"), "{}", e.msg);
    }
}
