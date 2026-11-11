//! Recursive-descent parser for the VHDL synthesis subset.

use super::ast::*;
use super::lex::{T, Tok, is_reserved};

pub struct P {
    t: Vec<Tok>,
    i: usize,
}

type R<X> = Result<X, String>;

const TIME_UNITS: &[&str] = &["fs", "ps", "ns", "us", "ms", "sec", "min", "hr"];

impl P {
    pub fn new(t: Vec<Tok>) -> Self {
        P { t, i: 0 }
    }

    fn peek(&self) -> Option<&T> {
        self.t.get(self.i).map(|t| &t.t)
    }
    fn peek_at(&self, k: usize) -> Option<&T> {
        self.t.get(self.i + k).map(|t| &t.t)
    }
    fn line(&self) -> Line {
        self.t
            .get(self.i)
            .or(self.t.last())
            .map(|t| t.line)
            .unwrap_or(0)
    }
    fn bump(&mut self) {
        self.i += 1;
    }
    fn at_end(&self) -> bool {
        self.i >= self.t.len()
    }
    fn kw(&self, s: &str) -> bool {
        matches!(self.peek(), Some(T::Id(x)) if x == s)
    }
    fn kw_at(&self, k: usize, s: &str) -> bool {
        matches!(self.peek_at(k), Some(T::Id(x)) if x == s)
    }
    fn eat_kw(&mut self, s: &str) -> bool {
        if self.kw(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(T::Sym(x)) if *x == s)
    }
    fn sym_at(&self, k: usize, s: &str) -> bool {
        matches!(self.peek_at(k), Some(T::Sym(x)) if *x == s)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if self.sym(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn err<X>(&self, msg: &str) -> R<X> {
        let near = self
            .t
            .get(self.i)
            .map(|t| t.orig.clone())
            .unwrap_or_else(|| "end of file".into());
        Err(format!("line {}: {msg} (near `{near}`)", self.line()))
    }
    fn expect_kw(&mut self, s: &str) -> R<()> {
        if self.eat_kw(s) {
            Ok(())
        } else {
            self.err(&format!("expected `{s}`"))
        }
    }
    fn expect_sym(&mut self, s: &str) -> R<()> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            self.err(&format!("expected `{s}`"))
        }
    }
    fn is_ident(&self) -> bool {
        matches!(self.peek(), Some(T::Id(x)) if !is_reserved(x))
    }
    fn ident(&mut self) -> R<String> {
        Ok(self.ident_orig()?.0)
    }
    fn ident_orig(&mut self) -> R<(String, String)> {
        match self.t.get(self.i) {
            Some(Tok {
                t: T::Id(x), orig, ..
            }) if !is_reserved(x) => {
                let r = (x.clone(), orig.clone());
                self.i += 1;
                Ok(r)
            }
            _ => self.err("expected an identifier"),
        }
    }
    fn opt_ident(&mut self) -> Option<String> {
        if self.is_ident() {
            self.ident().ok()
        } else {
            None
        }
    }
    /// Skip to (and past) the next `;` at parenthesis depth 0.
    fn skip_semi(&mut self) {
        let mut d = 0i32;
        while let Some(t) = self.peek() {
            match t {
                T::Sym("(") => d += 1,
                T::Sym(")") => d -= 1,
                T::Sym(";") if d <= 0 => {
                    self.bump();
                    return;
                }
                _ => {}
            }
            self.bump();
        }
    }
    /// `end [kw...] [name] ;`
    fn end_of(&mut self, kws: &[&str]) -> R<()> {
        self.expect_kw("end")?;
        for k in kws {
            let _ = self.eat_kw(k);
        }
        if let Some(T::Str(_)) = self.peek() {
            self.bump();
        }
        let _ = self.opt_ident();
        self.expect_sym(";")
    }

    // ------------------------------------------------------------------
    // Design file

    pub fn design_file(&mut self) -> R<Vec<DesignUnit>> {
        let mut out = Vec::new();
        let mut uses: Vec<String> = Vec::new();
        while !self.at_end() {
            if self.eat_kw("library") {
                self.skip_semi();
                continue;
            }
            if self.eat_kw("use") {
                loop {
                    let mut parts = Vec::new();
                    loop {
                        match self.peek() {
                            Some(T::Id(x)) => {
                                parts.push(x.clone());
                                self.bump();
                            }
                            Some(T::Str(s)) => {
                                parts.push(s.clone());
                                self.bump();
                            }
                            _ => return self.err("bad use clause"),
                        }
                        if !self.eat_sym(".") {
                            break;
                        }
                    }
                    if parts.last().map(|s| s == "all").unwrap_or(false) {
                        parts.pop();
                    }
                    uses.push(parts.join("."));
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                self.expect_sym(";")?;
                continue;
            }
            if self.kw("context") {
                if self.kw_at(2, "is") {
                    self.bump();
                    while !self.at_end() && !self.kw("end") {
                        self.bump();
                    }
                    self.end_of(&["context"])?;
                } else {
                    self.bump();
                    self.skip_semi();
                }
                continue;
            }
            let unit = if self.eat_kw("entity") {
                Unit::Entity(self.entity()?)
            } else if self.eat_kw("architecture") {
                Unit::Arch(self.architecture()?)
            } else if self.eat_kw("package") {
                if self.eat_kw("body") {
                    let name = self.ident()?;
                    self.expect_kw("is")?;
                    let decls = self.decls()?;
                    self.end_of(&["package", "body"])?;
                    Unit::PackageBody { name, decls }
                } else {
                    let name = self.ident()?;
                    self.expect_kw("is")?;
                    if self.kw("new") {
                        return self.err("package instantiation is not supported");
                    }
                    let decls = self.decls()?;
                    self.end_of(&["package"])?;
                    Unit::Package { name, decls }
                }
            } else if self.eat_kw("configuration") {
                // Configurations select architectures for testbenches; the
                // default binding (last analysed architecture) is used.
                while !self.at_end() {
                    if self.kw("end") && !self.kw_at(1, "for") {
                        break;
                    }
                    self.bump();
                }
                self.end_of(&["configuration"])?;
                uses.clear();
                continue;
            } else {
                return self.err("expected a design unit (entity, architecture, package)");
            };
            out.push(DesignUnit {
                uses: std::mem::take(&mut uses),
                unit,
            });
        }
        Ok(out)
    }

    fn entity(&mut self) -> R<Entity> {
        let line = self.line();
        let (name, orig) = self.ident_orig()?;
        self.expect_kw("is")?;
        let mut generics = Vec::new();
        let mut ports = Vec::new();
        if self.eat_kw("generic") {
            generics = self.interface_list()?;
            self.expect_sym(";")?;
        }
        if self.eat_kw("port") {
            ports = self.interface_list()?;
            self.expect_sym(";")?;
        }
        let decls = self.decls()?;
        if self.eat_kw("begin") {
            // Passive statements (asserts); ignored.
            while !self.at_end() && !self.kw("end") {
                self.bump();
            }
        }
        self.end_of(&["entity"])?;
        Ok(Entity {
            name,
            orig,
            generics,
            ports,
            decls,
            line,
        })
    }

    fn architecture(&mut self) -> R<Arch> {
        let line = self.line();
        let name = self.ident()?;
        self.expect_kw("of")?;
        let entity = self.ident()?;
        self.expect_kw("is")?;
        let decls = self.decls()?;
        self.expect_kw("begin")?;
        let body = self.conc_list()?;
        self.end_of(&["architecture"])?;
        Ok(Arch {
            name,
            entity,
            decls,
            body,
            line,
        })
    }

    // ------------------------------------------------------------------
    // Interfaces

    fn interface_list(&mut self) -> R<Vec<IfDecl>> {
        self.expect_sym("(")?;
        let mut out = Vec::new();
        loop {
            if self.sym(")") {
                break;
            }
            let line = self.line();
            let mut class = None;
            for c in ["signal", "constant", "variable", "file"] {
                if self.eat_kw(c) {
                    class = Some(c.to_string());
                }
            }
            if self.kw("type")
                || self.kw("package")
                || self.kw("function")
                || self.kw("procedure")
                || self.kw("impure")
                || self.kw("pure")
            {
                return self.err("generic types, packages and subprograms are not supported");
            }
            let mut names = vec![self.ident_orig()?];
            while self.eat_sym(",") {
                names.push(self.ident_orig()?);
            }
            self.expect_sym(":")?;
            let mode = if self.eat_kw("in") {
                Mode::In
            } else if self.eat_kw("out") {
                Mode::Out
            } else if self.eat_kw("inout") {
                Mode::InOut
            } else if self.eat_kw("buffer") {
                Mode::Buffer
            } else if self.eat_kw("linkage") {
                Mode::Linkage
            } else {
                Mode::In
            };
            let ty = self.subind()?;
            let _ = self.eat_kw("bus");
            let default = if self.eat_sym(":=") {
                Some(self.expr()?)
            } else {
                None
            };
            out.push(IfDecl {
                names,
                class,
                mode,
                ty,
                default,
                line,
            });
            if !self.eat_sym(";") {
                break;
            }
        }
        self.expect_sym(")")?;
        Ok(out)
    }

    // ------------------------------------------------------------------
    // Types

    fn type_mark(&mut self) -> R<Name> {
        let line = self.line();
        let mut n = Name::Id(self.ident()?, line);
        while self.sym(".") {
            self.bump();
            let f = match self.peek() {
                Some(T::Id(x)) => x.clone(),
                _ => return self.err("expected a name after `.`"),
            };
            self.bump();
            n = Name::Sel(Box::new(n), f);
        }
        // `t'subtype`, `x'element`, `t'base`
        if self.sym("'") {
            if let Some(T::Id(a)) = self.peek_at(1) {
                if matches!(a.as_str(), "subtype" | "element" | "base") {
                    let a = a.clone();
                    self.bump();
                    self.bump();
                    n = Name::Attr(Box::new(n), a, None);
                }
            }
        }
        Ok(n)
    }

    pub fn subind(&mut self) -> R<SubInd> {
        let line = self.line();
        if self.sym("(") {
            return self.err("element resolution indications are not supported");
        }
        let mut mark = self.type_mark()?;
        // `resolved std_logic`: a resolution function name precedes the mark.
        if self.is_ident() {
            mark = self.type_mark()?;
        }
        let cons = if self.eat_kw("range") {
            Some(Constraint::Range(self.range()?))
        } else if self.sym("(") {
            Some(self.index_constraint()?)
        } else {
            None
        };
        Ok(SubInd { mark, cons, line })
    }

    fn index_constraint(&mut self) -> R<Constraint> {
        let first = self.index_list()?;
        let mut elems = Vec::new();
        while self.sym("(") {
            elems.push(self.index_list()?);
        }
        Ok(Constraint::Index(first, elems))
    }

    fn index_list(&mut self) -> R<Vec<Option<DiscRange>>> {
        self.expect_sym("(")?;
        let mut v = Vec::new();
        loop {
            if self.eat_kw("open") {
                v.push(None);
            } else {
                v.push(Some(self.disc_range()?));
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(")")?;
        Ok(v)
    }

    pub fn disc_range(&mut self) -> R<DiscRange> {
        let line = self.line();
        let e = self.simple_expr()?;
        if self.kw("to") || self.kw("downto") {
            let down = self.kw("downto");
            self.bump();
            let r = self.simple_expr()?;
            return Ok(DiscRange::Range(Range::Dir(Box::new(e), down, Box::new(r))));
        }
        if self.eat_kw("range") {
            let Expr::Name(n) = e else {
                return self.err("expected a type mark before `range`");
            };
            let r = self.range()?;
            return Ok(DiscRange::Sub(SubInd {
                mark: *n,
                cons: Some(Constraint::Range(r)),
                line,
            }));
        }
        match e {
            Expr::Name(n) => Ok(DiscRange::Range(Range::Name(n))),
            _ => self.err("expected a discrete range"),
        }
    }

    pub fn range(&mut self) -> R<Range> {
        match self.disc_range()? {
            DiscRange::Range(r) => Ok(r),
            DiscRange::Sub(_) => self.err("expected a range"),
        }
    }

    fn type_def(&mut self, name: &str) -> R<TypeDef> {
        if self.sym("(") {
            self.bump();
            let mut lits = Vec::new();
            loop {
                match self.peek() {
                    Some(T::Id(x)) => {
                        lits.push(EnumLit::Id(x.clone()));
                        self.bump();
                    }
                    Some(T::Char(c)) => {
                        lits.push(EnumLit::Char(*c));
                        self.bump();
                    }
                    _ => return self.err("expected an enumeration literal"),
                }
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym(")")?;
            return Ok(TypeDef::Enum(lits));
        }
        if self.eat_kw("range") {
            let r = self.range()?;
            if self.eat_kw("units") {
                while !self.at_end() && !(self.kw("end") && self.kw_at(1, "units")) {
                    self.bump();
                }
                self.bump();
                self.bump();
                let _ = self.opt_ident();
                return Ok(TypeDef::Unsupported(format!("physical type {name}")));
            }
            let is_real = match &r {
                Range::Dir(a, _, _) => matches!(**a, Expr::Real(_)),
                _ => false,
            };
            return Ok(if is_real {
                TypeDef::Real(r)
            } else {
                TypeDef::Int(r)
            });
        }
        if self.eat_kw("array") {
            self.expect_sym("(")?;
            let mut idx = Vec::new();
            loop {
                // `natural range <>` or a discrete range
                let save = self.i;
                if self.is_ident() {
                    let m = self.type_mark()?;
                    if self.eat_kw("range") && self.eat_sym("<>") {
                        idx.push(IdxDef::Unbounded(m));
                        if !self.eat_sym(",") {
                            break;
                        }
                        continue;
                    }
                    self.i = save;
                }
                idx.push(IdxDef::Cons(self.disc_range()?));
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym(")")?;
            self.expect_kw("of")?;
            let elem = self.subind()?;
            return Ok(TypeDef::Array { idx, elem });
        }
        if self.eat_kw("record") {
            let mut fields = Vec::new();
            while !self.kw("end") {
                let mut names = vec![self.ident()?];
                while self.eat_sym(",") {
                    names.push(self.ident()?);
                }
                self.expect_sym(":")?;
                let ty = self.subind()?;
                self.expect_sym(";")?;
                fields.push((names, ty));
            }
            self.expect_kw("end")?;
            self.expect_kw("record")?;
            let _ = self.opt_ident();
            return Ok(TypeDef::Record(fields));
        }
        if self.eat_kw("protected") {
            while !self.at_end() && !(self.kw("end") && self.kw_at(1, "protected")) {
                self.bump();
            }
            self.bump();
            self.bump();
            let _ = self.opt_ident();
            return Ok(TypeDef::Unsupported(format!("protected type {name}")));
        }
        // access, file, etc.
        let what = match self.peek() {
            Some(T::Id(x)) => x.clone(),
            _ => "unknown".into(),
        };
        while !self.at_end() && !self.sym(";") {
            self.bump();
        }
        Ok(TypeDef::Unsupported(format!("{what} type {name}")))
    }

    // ------------------------------------------------------------------
    // Declarations

    fn decl_start(&self) -> bool {
        matches!(self.peek(), Some(T::Id(x)) if matches!(x.as_str(),
            "signal" | "constant" | "variable" | "shared" | "type" | "subtype" | "component" | "function"
            | "procedure" | "pure" | "impure" | "alias" | "attribute" | "use" | "file" | "group" | "disconnect"))
    }

    pub fn decls(&mut self) -> R<Vec<Decl>> {
        let mut out = Vec::new();
        while self.decl_start() {
            out.push(self.decl()?);
        }
        Ok(out)
    }

    fn names_orig(&mut self) -> R<Vec<(String, String)>> {
        let mut names = vec![self.ident_orig()?];
        while self.eat_sym(",") {
            names.push(self.ident_orig()?);
        }
        Ok(names)
    }

    fn decl(&mut self) -> R<Decl> {
        let line = self.line();
        if self.eat_kw("signal") {
            let names = self.names_orig()?;
            self.expect_sym(":")?;
            let ty = self.subind()?;
            let _ = self.eat_kw("register") || self.eat_kw("bus");
            let init = if self.eat_sym(":=") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect_sym(";")?;
            return Ok(Decl::Signal {
                names,
                ty,
                init,
                line,
            });
        }
        if self.eat_kw("constant") {
            let names = self.names_orig()?;
            self.expect_sym(":")?;
            let ty = self.subind()?;
            let val = if self.eat_sym(":=") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect_sym(";")?;
            return Ok(Decl::Constant {
                names,
                ty,
                val,
                line,
            });
        }
        if self.kw("variable") || self.kw("shared") {
            let shared = self.eat_kw("shared");
            self.expect_kw("variable")?;
            let names = self.names_orig()?;
            self.expect_sym(":")?;
            let ty = self.subind()?;
            let init = if self.eat_sym(":=") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect_sym(";")?;
            return Ok(Decl::Variable {
                names,
                ty,
                init,
                shared,
                line,
            });
        }
        if self.eat_kw("type") {
            let name = self.ident()?;
            if self.eat_sym(";") {
                return Ok(Decl::Ignored {
                    what: format!("incomplete type {name}"),
                    line,
                });
            }
            self.expect_kw("is")?;
            let def = self.type_def(&name)?;
            self.expect_sym(";")?;
            return Ok(Decl::Type { name, def, line });
        }
        if self.eat_kw("subtype") {
            let name = self.ident()?;
            self.expect_kw("is")?;
            let ind = self.subind()?;
            self.expect_sym(";")?;
            return Ok(Decl::Subtype { name, ind, line });
        }
        if self.eat_kw("component") {
            let name = self.ident()?;
            let _ = self.eat_kw("is");
            let mut generics = Vec::new();
            let mut ports = Vec::new();
            if self.eat_kw("generic") {
                generics = self.interface_list()?;
                self.expect_sym(";")?;
            }
            if self.eat_kw("port") {
                ports = self.interface_list()?;
                self.expect_sym(";")?;
            }
            self.end_of(&["component"])?;
            return Ok(Decl::Component {
                name,
                generics,
                ports,
                line,
            });
        }
        if self.kw("function") || self.kw("procedure") || self.kw("pure") || self.kw("impure") {
            return Ok(Decl::Subprog(self.subprogram()?));
        }
        if self.eat_kw("alias") {
            let name = match self.peek() {
                Some(T::Id(x)) => x.clone(),
                Some(T::Char(c)) => c.to_string(),
                Some(T::Str(s)) => s.clone(),
                _ => return self.err("expected alias name"),
            };
            self.bump();
            let ty = if self.eat_sym(":") {
                Some(self.subind()?)
            } else {
                None
            };
            self.expect_kw("is")?;
            let target = self.name()?;
            if self.sym("[") {
                while !self.at_end() && !self.sym("]") {
                    self.bump();
                }
                self.bump();
                self.skip_semi();
                return Ok(Decl::Ignored {
                    what: format!("subprogram alias {name}"),
                    line,
                });
            }
            self.expect_sym(";")?;
            return Ok(Decl::Alias {
                name,
                ty,
                target,
                line,
            });
        }
        if self.eat_kw("file") {
            let name = self.ident().ok();
            self.skip_semi();
            return Ok(Decl::Unsupported {
                name,
                what: "file declaration".into(),
                line,
            });
        }
        // attribute / use / group / disconnect
        let what = match self.peek() {
            Some(T::Id(x)) => x.clone(),
            _ => String::new(),
        };
        self.skip_semi();
        Ok(Decl::Ignored { what, line })
    }

    fn subprogram(&mut self) -> R<Subprog> {
        let line = self.line();
        let _ = self.eat_kw("pure") || self.eat_kw("impure");
        let is_func = if self.eat_kw("function") {
            true
        } else {
            self.expect_kw("procedure")?;
            false
        };
        let name = match self.peek() {
            Some(T::Id(x)) => x.clone(),
            Some(T::Str(s)) => format!("\"{}\"", s.to_ascii_lowercase()),
            _ => return self.err("expected subprogram name"),
        };
        self.bump();
        let _ = self.eat_kw("parameter");
        let params = if self.sym("(") {
            self.interface_list()?
        } else {
            Vec::new()
        };
        let ret = if is_func {
            self.expect_kw("return")?;
            Some(self.type_mark()?)
        } else {
            None
        };
        if self.eat_sym(";") {
            return Ok(Subprog {
                name,
                is_func,
                params,
                ret,
                decls: Vec::new(),
                body: None,
                line,
            });
        }
        self.expect_kw("is")?;
        let decls = self.decls()?;
        self.expect_kw("begin")?;
        let body = self.seq_list()?;
        self.end_of(&["function", "procedure"])?;
        Ok(Subprog {
            name,
            is_func,
            params,
            ret,
            decls,
            body: Some(body),
            line,
        })
    }

    // ------------------------------------------------------------------
    // Expressions

    pub fn expr(&mut self) -> R<Expr> {
        if self.eat_sym("??") {
            let p = self.primary()?;
            return Ok(Expr::Un(UnOp::Cond, Box::new(p)));
        }
        let first = self.relation()?;
        let op = match self.peek() {
            Some(T::Id(x)) => match x.as_str() {
                "and" => Some(BinOp::And),
                "or" => Some(BinOp::Or),
                "xor" => Some(BinOp::Xor),
                "xnor" => Some(BinOp::Xnor),
                "nand" => Some(BinOp::Nand),
                "nor" => Some(BinOp::Nor),
                _ => None,
            },
            _ => None,
        };
        let Some(op) = op else { return Ok(first) };
        let mut acc = first;
        while let Some(T::Id(x)) = self.peek() {
            let o = match x.as_str() {
                "and" => BinOp::And,
                "or" => BinOp::Or,
                "xor" => BinOp::Xor,
                "xnor" => BinOp::Xnor,
                "nand" => BinOp::Nand,
                "nor" => BinOp::Nor,
                _ => break,
            };
            if o != op {
                return self.err("mixing logical operators requires parentheses");
            }
            self.bump();
            let r = self.relation()?;
            acc = Expr::Bin(o, Box::new(acc), Box::new(r));
            if matches!(o, BinOp::Nand | BinOp::Nor) {
                break;
            }
        }
        Ok(acc)
    }

    fn relation(&mut self) -> R<Expr> {
        let a = self.shift_expr()?;
        let op = match self.peek() {
            Some(T::Sym(s)) => match *s {
                "=" => Some(BinOp::Eq),
                "/=" => Some(BinOp::Ne),
                "<" => Some(BinOp::Lt),
                "<=" => Some(BinOp::Le),
                ">" => Some(BinOp::Gt),
                ">=" => Some(BinOp::Ge),
                "?=" => Some(BinOp::MEq),
                "?/=" => Some(BinOp::MNe),
                "?<" => Some(BinOp::MLt),
                "?<=" => Some(BinOp::MLe),
                "?>" => Some(BinOp::MGt),
                "?>=" => Some(BinOp::MGe),
                _ => None,
            },
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let b = self.shift_expr()?;
            return Ok(Expr::Bin(op, Box::new(a), Box::new(b)));
        }
        Ok(a)
    }

    fn shift_expr(&mut self) -> R<Expr> {
        let a = self.simple_expr()?;
        let op = match self.peek() {
            Some(T::Id(x)) => match x.as_str() {
                "sll" => Some(BinOp::Sll),
                "srl" => Some(BinOp::Srl),
                "sla" => Some(BinOp::Sla),
                "sra" => Some(BinOp::Sra),
                "rol" => Some(BinOp::Rol),
                "ror" => Some(BinOp::Ror),
                _ => None,
            },
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let b = self.simple_expr()?;
            return Ok(Expr::Bin(op, Box::new(a), Box::new(b)));
        }
        Ok(a)
    }

    pub fn simple_expr(&mut self) -> R<Expr> {
        let neg = if self.eat_sym("-") {
            Some(UnOp::Neg)
        } else if self.eat_sym("+") {
            Some(UnOp::Pos)
        } else {
            None
        };
        let mut acc = self.term()?;
        if let Some(u) = neg {
            acc = Expr::Un(u, Box::new(acc));
        }
        loop {
            let op = if self.sym("+") {
                BinOp::Add
            } else if self.sym("-") {
                BinOp::Sub
            } else if self.sym("&") {
                BinOp::Cat
            } else {
                break;
            };
            self.bump();
            let r = self.term()?;
            acc = Expr::Bin(op, Box::new(acc), Box::new(r));
        }
        Ok(acc)
    }

    fn term(&mut self) -> R<Expr> {
        let mut acc = self.factor()?;
        loop {
            let op = if self.sym("*") {
                BinOp::Mul
            } else if self.sym("/") {
                BinOp::Div
            } else if self.kw("mod") {
                BinOp::Mod
            } else if self.kw("rem") {
                BinOp::Rem
            } else {
                break;
            };
            self.bump();
            let r = self.factor()?;
            acc = Expr::Bin(op, Box::new(acc), Box::new(r));
        }
        Ok(acc)
    }

    fn factor(&mut self) -> R<Expr> {
        let un = match self.peek() {
            Some(T::Id(x)) => match x.as_str() {
                "abs" => Some(UnOp::Abs),
                "not" => Some(UnOp::Not),
                "and" => Some(UnOp::RAnd),
                "or" => Some(UnOp::ROr),
                "xor" => Some(UnOp::RXor),
                "nand" => Some(UnOp::RNand),
                "nor" => Some(UnOp::RNor),
                "xnor" => Some(UnOp::RXnor),
                _ => None,
            },
            _ => None,
        };
        if let Some(u) = un {
            self.bump();
            let p = self.primary()?;
            return Ok(Expr::Un(u, Box::new(p)));
        }
        let p = self.primary()?;
        if self.eat_sym("**") {
            let e = self.primary()?;
            return Ok(Expr::Bin(BinOp::Pow, Box::new(p), Box::new(e)));
        }
        Ok(p)
    }

    pub fn primary(&mut self) -> R<Expr> {
        match self.peek().cloned() {
            Some(T::Int(v)) => {
                self.bump();
                if let Some(T::Id(u)) = self.peek() {
                    if TIME_UNITS.contains(&u.as_str()) {
                        let u = u.clone();
                        self.bump();
                        return Ok(Expr::Phys(v as f64, u));
                    }
                }
                Ok(Expr::Int(v))
            }
            Some(T::Real(v)) => {
                self.bump();
                if let Some(T::Id(u)) = self.peek() {
                    if TIME_UNITS.contains(&u.as_str()) {
                        let u = u.clone();
                        self.bump();
                        return Ok(Expr::Phys(v, u));
                    }
                }
                Ok(Expr::Real(v))
            }
            Some(T::Char(c)) => {
                self.bump();
                Ok(Expr::Char(c))
            }
            Some(T::Str(s)) => {
                if self.sym_at(1, "(") {
                    // Operator symbol call: "and"(a, b)
                    let n = self.name()?;
                    return Ok(Expr::Name(Box::new(n)));
                }
                self.bump();
                Ok(Expr::Str(s))
            }
            Some(T::BitStr(s)) => {
                self.bump();
                Ok(Expr::BitStr(s))
            }
            Some(T::Sym("(")) => self.paren_or_aggregate(),
            Some(T::Id(x)) if x == "null" => {
                self.bump();
                Ok(Expr::Null)
            }
            Some(T::Id(x)) if x == "new" => self.err("allocators are not synthesizable"),
            Some(T::Id(x)) if !is_reserved(&x) => {
                let n = self.name()?;
                if self.sym("'") && self.sym_at(1, "(") {
                    self.bump();
                    let e = self.paren_or_aggregate()?;
                    return Ok(Expr::Qual(Box::new(n), Box::new(e)));
                }
                Ok(Expr::Name(Box::new(n)))
            }
            Some(T::Sym("<<")) => self.err("external names are not supported"),
            _ => self.err("expected an expression"),
        }
    }

    fn paren_or_aggregate(&mut self) -> R<Expr> {
        self.expect_sym("(")?;
        let mut elems: Vec<AggElem> = Vec::new();
        let mut is_agg = false;
        loop {
            if self.eat_kw("others") {
                self.expect_sym("=>")?;
                let v = self.expr()?;
                elems.push(AggElem {
                    choices: vec![Choice::Others],
                    val: v,
                });
                is_agg = true;
            } else {
                let first = self.expr()?;
                if self.kw("to") || self.kw("downto") || self.sym("|") || self.sym("=>") {
                    let mut choices = Vec::new();
                    let mut cur = first;
                    loop {
                        if self.kw("to") || self.kw("downto") {
                            let down = self.kw("downto");
                            self.bump();
                            let r = self.simple_expr()?;
                            choices.push(Choice::Range(Range::Dir(
                                Box::new(cur),
                                down,
                                Box::new(r),
                            )));
                        } else {
                            choices.push(Choice::Expr(cur));
                        }
                        if self.eat_sym("|") {
                            if self.eat_kw("others") {
                                choices.push(Choice::Others);
                                break;
                            }
                            cur = self.expr()?;
                            continue;
                        }
                        break;
                    }
                    self.expect_sym("=>")?;
                    let v = self.expr()?;
                    elems.push(AggElem { choices, val: v });
                    is_agg = true;
                } else {
                    elems.push(AggElem {
                        choices: Vec::new(),
                        val: first,
                    });
                }
            }
            if self.eat_sym(",") {
                is_agg = true;
                continue;
            }
            break;
        }
        self.expect_sym(")")?;
        if !is_agg && elems.len() == 1 {
            return Ok(elems.pop().unwrap().val);
        }
        Ok(Expr::Agg(elems))
    }

    pub fn name(&mut self) -> R<Name> {
        let line = self.line();
        let mut n = match self.peek().cloned() {
            Some(T::Id(x)) => {
                self.bump();
                Name::Id(x, line)
            }
            Some(T::Str(s)) => {
                self.bump();
                Name::OpSym(format!("\"{}\"", s.to_ascii_lowercase()))
            }
            _ => return self.err("expected a name"),
        };
        loop {
            if self.sym(".") {
                self.bump();
                let f = match self.peek().cloned() {
                    Some(T::Id(x)) => x,
                    Some(T::Char(c)) => c.to_string(),
                    Some(T::Str(s)) => format!("\"{}\"", s.to_ascii_lowercase()),
                    _ => return self.err("expected a name after `.`"),
                };
                self.bump();
                n = Name::Sel(Box::new(n), f);
                continue;
            }
            if self.sym("(") {
                self.bump();
                // Slice or association list.
                let first_formal_or_expr = self.expr_or_open()?;
                if self.kw("to") || self.kw("downto") {
                    let down = self.kw("downto");
                    self.bump();
                    let r = self.simple_expr()?;
                    self.expect_sym(")")?;
                    n = Name::Slice(
                        Box::new(n),
                        Range::Dir(Box::new(first_formal_or_expr), down, Box::new(r)),
                    );
                    continue;
                }
                let mut args = Vec::new();
                let mut cur = first_formal_or_expr;
                loop {
                    if self.eat_sym("=>") {
                        let formal = match cur {
                            Expr::Name(f) => *f,
                            _ => return self.err("bad formal in association"),
                        };
                        let actual = self.expr_or_open()?;
                        args.push(Assoc {
                            formal: Some(formal),
                            actual,
                        });
                    } else {
                        args.push(Assoc {
                            formal: None,
                            actual: cur,
                        });
                    }
                    if self.eat_sym(",") {
                        cur = self.expr_or_open()?;
                        continue;
                    }
                    break;
                }
                self.expect_sym(")")?;
                n = Name::Call(Box::new(n), args);
                continue;
            }
            if self.sym("'") && !self.sym_at(1, "(") {
                self.bump();
                let a = match self.peek().cloned() {
                    Some(T::Id(x)) => x,
                    _ => return self.err("expected an attribute name"),
                };
                self.bump();
                let arg = if self.sym("(")
                    && !matches!(
                        a.as_str(),
                        "range"
                            | "reverse_range"
                            | "length"
                            | "high"
                            | "low"
                            | "left"
                            | "right"
                            | "event"
                    ) {
                    self.bump();
                    let e = self.expr()?;
                    self.expect_sym(")")?;
                    Some(Box::new(e))
                } else if self.sym("(")
                    && matches!(
                        a.as_str(),
                        "range" | "reverse_range" | "length" | "high" | "low" | "left" | "right"
                    )
                {
                    // `x'range(2)` dimension argument
                    if matches!(self.peek_at(1), Some(T::Int(_))) && self.sym_at(2, ")") {
                        self.bump();
                        let e = self.expr()?;
                        self.expect_sym(")")?;
                        Some(Box::new(e))
                    } else {
                        None
                    }
                } else {
                    None
                };
                n = Name::Attr(Box::new(n), a, arg);
                continue;
            }
            break;
        }
        Ok(n)
    }

    fn expr_or_open(&mut self) -> R<Expr> {
        if self.eat_kw("open") {
            return Ok(Expr::Open);
        }
        if self.eat_kw("others") {
            // `others => x` inside a call-like aggregate is not valid here.
            return self.err("unexpected `others`");
        }
        self.expr()
    }

    // ------------------------------------------------------------------
    // Sequential statements

    pub fn seq_list(&mut self) -> R<Vec<Seq>> {
        let mut out = Vec::new();
        while !self.at_end()
            && !(self.kw("end") || self.kw("elsif") || self.kw("else") || self.kw("when"))
        {
            out.push(self.seq()?);
        }
        Ok(out)
    }

    fn choices(&mut self) -> R<Vec<Choice>> {
        let mut v = Vec::new();
        loop {
            if self.eat_kw("others") {
                v.push(Choice::Others);
            } else {
                let e = self.simple_expr()?;
                if self.kw("to") || self.kw("downto") {
                    let down = self.kw("downto");
                    self.bump();
                    let r = self.simple_expr()?;
                    v.push(Choice::Range(Range::Dir(Box::new(e), down, Box::new(r))));
                } else if self.eat_kw("range") {
                    let r = self.range()?;
                    v.push(Choice::Range(r));
                } else {
                    v.push(Choice::Expr(e));
                }
            }
            if !self.eat_sym("|") {
                break;
            }
        }
        Ok(v)
    }

    /// Waveform: first element's value; `after` clauses and later elements
    /// are ignored. `unaffected` yields `Expr::Open`.
    fn waveform(&mut self) -> R<Expr> {
        if self.eat_kw("unaffected") {
            return Ok(Expr::Open);
        }
        let v = self.expr()?;
        if self.eat_kw("after") {
            let _ = self.expr()?;
        }
        while self.sym(",") {
            self.bump();
            let _ = self.expr()?;
            if self.eat_kw("after") {
                let _ = self.expr()?;
            }
        }
        Ok(v)
    }

    fn delay_mech(&mut self) -> R<()> {
        if self.eat_kw("transport") {
            return Ok(());
        }
        if self.eat_kw("reject") {
            let _ = self.expr()?;
            self.expect_kw("inertial")?;
            return Ok(());
        }
        let _ = self.eat_kw("inertial");
        Ok(())
    }

    fn cond_waveforms(&mut self, var: bool) -> R<Vec<CondVal>> {
        let mut out = Vec::new();
        loop {
            let val = if var { self.expr()? } else { self.waveform()? };
            if self.eat_kw("when") {
                let c = self.expr()?;
                out.push(CondVal { val, cond: Some(c) });
                if self.eat_kw("else") {
                    continue;
                }
                break;
            }
            out.push(CondVal { val, cond: None });
            break;
        }
        Ok(out)
    }

    fn seq(&mut self) -> R<Seq> {
        let line = self.line();
        let mut label = None;
        if self.is_ident() && self.sym_at(1, ":") {
            label = Some(self.ident()?);
            self.bump();
        }
        if self.eat_kw("null") {
            self.expect_sym(";")?;
            return Ok(Seq::Null);
        }
        if self.eat_kw("assert") || self.kw("report") {
            if self.kw("report") {
                self.bump();
            }
            self.skip_semi();
            return Ok(Seq::Null);
        }
        if self.eat_kw("wait") {
            let mut has_on = false;
            let mut has_for = false;
            let mut until = None;
            if self.eat_kw("on") {
                has_on = true;
                loop {
                    let _ = self.name()?;
                    if !self.eat_sym(",") {
                        break;
                    }
                }
            }
            if self.eat_kw("until") {
                until = Some(self.expr()?);
            }
            if self.eat_kw("for") {
                has_for = true;
                let _ = self.expr()?;
            }
            self.expect_sym(";")?;
            return Ok(Seq::Wait {
                until,
                has_for,
                has_on,
                line,
            });
        }
        if self.eat_kw("if") {
            let mut arms = Vec::new();
            let c = self.expr()?;
            self.expect_kw("then")?;
            let b = self.seq_list()?;
            arms.push((c, b));
            let mut els = None;
            loop {
                if self.eat_kw("elsif") {
                    let c = self.expr()?;
                    self.expect_kw("then")?;
                    let b = self.seq_list()?;
                    arms.push((c, b));
                    continue;
                }
                if self.eat_kw("else") {
                    els = Some(self.seq_list()?);
                }
                break;
            }
            self.end_of(&["if"])?;
            return Ok(Seq::If { arms, els, line });
        }
        if self.eat_kw("case") {
            let matching = self.eat_sym("?");
            let sel = self.expr()?;
            self.expect_kw("is")?;
            let mut arms = Vec::new();
            while self.eat_kw("when") {
                let ch = self.choices()?;
                self.expect_sym("=>")?;
                let b = self.seq_list()?;
                arms.push((ch, b));
            }
            self.expect_kw("end")?;
            self.expect_kw("case")?;
            let _ = self.eat_sym("?");
            let _ = self.opt_ident();
            self.expect_sym(";")?;
            return Ok(Seq::Case {
                sel,
                arms,
                matching,
                line,
            });
        }
        if self.kw("for") || self.kw("while") || self.kw("loop") {
            if self.eat_kw("for") {
                let var = self.ident()?;
                self.expect_kw("in")?;
                let range = self.disc_range()?;
                self.expect_kw("loop")?;
                let body = self.seq_list()?;
                self.end_of(&["loop"])?;
                return Ok(Seq::For {
                    label,
                    var,
                    range,
                    body,
                    line,
                });
            }
            let cond = if self.eat_kw("while") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect_kw("loop")?;
            let body = self.seq_list()?;
            self.end_of(&["loop"])?;
            return Ok(Seq::While {
                label,
                cond,
                body,
                line,
            });
        }
        if self.kw("next") || self.kw("exit") {
            let is_next = self.kw("next");
            self.bump();
            let lbl = self.opt_ident();
            let cond = if self.eat_kw("when") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect_sym(";")?;
            return Ok(if is_next {
                Seq::Next {
                    label: lbl,
                    cond,
                    line,
                }
            } else {
                Seq::Exit {
                    label: lbl,
                    cond,
                    line,
                }
            });
        }
        if self.eat_kw("return") {
            let val = if self.sym(";") {
                None
            } else {
                Some(self.expr()?)
            };
            self.expect_sym(";")?;
            return Ok(Seq::Return { val, line });
        }
        if self.eat_kw("with") {
            let sel = self.expr()?;
            self.expect_kw("select")?;
            let _ = self.eat_sym("?");
            let target = self.target()?;
            let var = if self.eat_sym(":=") {
                true
            } else {
                self.expect_sym("<=")?;
                self.delay_mech()?;
                false
            };
            let arms = self.sel_arms(var)?;
            return Ok(Seq::SelAsg {
                sel,
                target,
                var,
                arms,
                line,
            });
        }
        if self.kw("force") || self.kw("release") {
            return self.err("force/release is not synthesizable");
        }
        let target = self.target()?;
        if self.eat_sym("<=") {
            if self.kw("force") || self.kw("release") {
                return self.err("force/release is not synthesizable");
            }
            self.delay_mech()?;
            let rhs = self.cond_waveforms(false)?;
            self.expect_sym(";")?;
            return Ok(Seq::SigAsg { target, rhs, line });
        }
        if self.eat_sym(":=") {
            let rhs = self.cond_waveforms(true)?;
            self.expect_sym(";")?;
            return Ok(Seq::VarAsg { target, rhs, line });
        }
        if self.eat_sym(";") {
            if let Expr::Name(n) = target {
                return Ok(Seq::Call { name: *n, line });
            }
        }
        self.err("expected `<=`, `:=` or `;`")
    }

    fn target(&mut self) -> R<Expr> {
        if self.sym("(") {
            return self.paren_or_aggregate();
        }
        Ok(Expr::Name(Box::new(self.name()?)))
    }

    fn sel_arms(&mut self, var: bool) -> R<Vec<(Expr, Vec<Choice>)>> {
        let mut arms = Vec::new();
        loop {
            let v = if var { self.expr()? } else { self.waveform()? };
            self.expect_kw("when")?;
            let ch = self.choices()?;
            arms.push((v, ch));
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(";")?;
        Ok(arms)
    }

    // ------------------------------------------------------------------
    // Concurrent statements

    pub fn conc_list(&mut self) -> R<Vec<Conc>> {
        let mut out = Vec::new();
        while !self.at_end()
            && !(self.kw("end") || self.kw("elsif") || self.kw("else") || self.kw("when"))
        {
            out.push(self.conc()?);
        }
        Ok(out)
    }

    fn gen_body(&mut self) -> R<(Vec<Decl>, Vec<Conc>)> {
        let decls = self.decls()?;
        let _ = self.eat_kw("begin");
        let body = self.conc_list()?;
        // VHDL-2008 alternative end: `end [alt_label];` before elsif/else/end generate
        if self.kw("end") && !self.kw_at(1, "generate") {
            let save = self.i;
            self.bump();
            let _ = self.opt_ident();
            if self.eat_sym(";")
                && (self.kw("elsif") || self.kw("else") || self.kw("when") || self.kw("end"))
            {
                return Ok((decls, body));
            }
            self.i = save;
        }
        Ok((decls, body))
    }

    fn conc(&mut self) -> R<Conc> {
        let line = self.line();
        let mut label = None;
        if self.is_ident() && self.sym_at(1, ":") {
            label = Some(self.ident()?);
            self.bump();
        }
        let _ = self.eat_kw("postponed");
        if self.eat_kw("process") {
            let sens = if self.eat_sym("(") {
                if self.eat_kw("all") {
                    self.expect_sym(")")?;
                    Sens::All
                } else {
                    let mut v = Vec::new();
                    loop {
                        v.push(self.name()?);
                        if !self.eat_sym(",") {
                            break;
                        }
                    }
                    self.expect_sym(")")?;
                    Sens::List(v)
                }
            } else {
                Sens::None
            };
            let _ = self.eat_kw("is");
            let decls = self.decls()?;
            self.expect_kw("begin")?;
            let body = self.seq_list()?;
            self.expect_kw("end")?;
            let _ = self.eat_kw("postponed");
            self.expect_kw("process")?;
            let _ = self.opt_ident();
            self.expect_sym(";")?;
            return Ok(Conc::Process {
                label,
                sens,
                decls,
                body,
                line,
            });
        }
        if self.eat_kw("assert") {
            self.skip_semi();
            return Ok(Conc::Ignored);
        }
        if self.eat_kw("with") {
            let sel = self.expr()?;
            self.expect_kw("select")?;
            let _ = self.eat_sym("?");
            let target = self.target()?;
            self.expect_sym("<=")?;
            let _ = self.eat_kw("guarded");
            self.delay_mech()?;
            let arms = self.sel_arms(false)?;
            return Ok(Conc::SelAsg {
                sel,
                target,
                arms,
                line,
            });
        }
        if let Some(lbl) = label.clone() {
            if self.eat_kw("block") {
                if self.sym("(") {
                    return self.err("guarded blocks are not supported");
                }
                let _ = self.eat_kw("is");
                if self.kw("generic") || self.kw("port") {
                    return self.err("block generics/ports are not supported");
                }
                let decls = self.decls()?;
                self.expect_kw("begin")?;
                let body = self.conc_list()?;
                self.end_of(&["block"])?;
                return Ok(Conc::Block {
                    label: lbl,
                    decls,
                    body,
                    line,
                });
            }
            if self.eat_kw("for") {
                let var = self.ident()?;
                self.expect_kw("in")?;
                let range = self.disc_range()?;
                self.expect_kw("generate")?;
                let (decls, body) = self.gen_body()?;
                self.end_of(&["generate"])?;
                return Ok(Conc::ForGen {
                    label: lbl,
                    var,
                    range,
                    decls,
                    body,
                    line,
                });
            }
            if self.eat_kw("if") {
                let mut arms = Vec::new();
                let mut els = None;
                if self.is_ident() && self.sym_at(1, ":") {
                    self.bump();
                    self.bump();
                }
                let c = self.expr()?;
                self.expect_kw("generate")?;
                let (d, b) = self.gen_body()?;
                arms.push((c, d, b));
                loop {
                    if self.eat_kw("elsif") {
                        if self.is_ident() && self.sym_at(1, ":") {
                            self.bump();
                            self.bump();
                        }
                        let c = self.expr()?;
                        self.expect_kw("generate")?;
                        let (d, b) = self.gen_body()?;
                        arms.push((c, d, b));
                        continue;
                    }
                    if self.eat_kw("else") {
                        if self.is_ident() && self.sym_at(1, ":") {
                            self.bump();
                            self.bump();
                        }
                        self.expect_kw("generate")?;
                        els = Some(self.gen_body()?);
                    }
                    break;
                }
                self.end_of(&["generate"])?;
                return Ok(Conc::IfGen {
                    label: lbl,
                    arms,
                    els,
                    line,
                });
            }
            if self.eat_kw("case") {
                let sel = self.expr()?;
                self.expect_kw("generate")?;
                let mut arms = Vec::new();
                while self.eat_kw("when") {
                    if self.is_ident() && self.sym_at(1, ":") {
                        self.bump();
                        self.bump();
                    }
                    let ch = self.choices()?;
                    self.expect_sym("=>")?;
                    let (d, b) = self.gen_body()?;
                    arms.push((ch, d, b));
                }
                self.end_of(&["generate"])?;
                return Ok(Conc::CaseGen {
                    label: lbl,
                    sel,
                    arms,
                    line,
                });
            }
            // Instances.
            if self.kw("entity") || self.kw("component") || self.kw("configuration") {
                let kind = if let Some(T::Id(k)) = self.peek() {
                    k.clone()
                } else {
                    unreachable!()
                };
                self.bump();
                let mut name = self.ident()?;
                while self.eat_sym(".") {
                    name = self.ident()?;
                }
                let unit = match kind.as_str() {
                    "entity" => {
                        let arch = if self.eat_sym("(") {
                            let a = self.ident()?;
                            self.expect_sym(")")?;
                            Some(a)
                        } else {
                            None
                        };
                        InstUnit::Entity { name, arch }
                    }
                    "component" => InstUnit::Comp(name),
                    _ => InstUnit::Config(name),
                };
                return self.inst_tail(lbl, unit, line);
            }
            if self.is_ident()
                && (self.kw_at(1, "generic") || self.kw_at(1, "port") || self.sym_at(1, ";"))
            {
                let name = self.ident()?;
                return self.inst_tail(lbl, InstUnit::Comp(name), line);
            }
            if self.is_ident() && self.sym_at(1, ".") {
                // lib.comp port map (...)
                let save = self.i;
                let mut name = self.ident()?;
                while self.eat_sym(".") {
                    name = self.ident()?;
                }
                if self.kw("generic") || self.kw("port") {
                    return self.inst_tail(lbl, InstUnit::Comp(name), line);
                }
                self.i = save;
            }
        }
        // Concurrent signal assignment or procedure call.
        let target = self.target()?;
        if self.eat_sym("<=") {
            let _ = self.eat_kw("guarded");
            self.delay_mech()?;
            let arms = self.cond_waveforms(false)?;
            self.expect_sym(";")?;
            return Ok(Conc::CondAsg { target, arms, line });
        }
        if self.eat_sym(";") {
            if let Expr::Name(n) = target {
                return Ok(Conc::Call { name: *n, line });
            }
        }
        self.err("expected a concurrent statement")
    }

    fn inst_tail(&mut self, label: String, unit: InstUnit, line: Line) -> R<Conc> {
        let mut gmap = Vec::new();
        let mut pmap = Vec::new();
        if self.eat_kw("generic") {
            self.expect_kw("map")?;
            gmap = self.assoc_list()?;
        }
        if self.eat_kw("port") {
            self.expect_kw("map")?;
            pmap = self.assoc_list()?;
        }
        self.expect_sym(";")?;
        Ok(Conc::Inst {
            label,
            unit,
            gmap,
            pmap,
            line,
        })
    }

    fn assoc_list(&mut self) -> R<Vec<Assoc>> {
        self.expect_sym("(")?;
        let mut v = Vec::new();
        loop {
            let first = self.expr_or_open()?;
            if self.eat_sym("=>") {
                let formal = match first {
                    Expr::Name(n) => *n,
                    _ => return self.err("bad formal in map"),
                };
                let actual = self.expr_or_open()?;
                v.push(Assoc {
                    formal: Some(formal),
                    actual,
                });
            } else {
                v.push(Assoc {
                    formal: None,
                    actual: first,
                });
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        self.expect_sym(")")?;
        Ok(v)
    }
}

pub fn parse(src: &str) -> Result<Vec<DesignUnit>, String> {
    let toks = super::lex::lex(src)?;
    P::new(toks).design_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_counter() {
        let src = r#"
library ieee; use ieee.std_logic_1164.all; use ieee.numeric_std.all;
entity c is port (clk : in std_logic; q : out std_logic_vector(3 downto 0)); end entity;
architecture rtl of c is
  signal r : unsigned(3 downto 0) := (others => '0');
  type st is (a, b, c);
begin
  process(clk) begin
    if rising_edge(clk) then
      case r is when "0000" | "0001" => r <= r + 1; when others => r <= (others => '0'); end case;
    end if;
  end process;
  q <= std_logic_vector(r) when r(0) = '1' else x"F";
  u1 : entity work.foo(rtl) generic map (N => 4) port map (a => q(1 downto 0), b => open);
  g : for i in 0 to 3 generate begin end generate;
end architecture;
"#;
        let u = parse(src).unwrap();
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].uses, vec!["ieee.std_logic_1164", "ieee.numeric_std"]);
        let Unit::Arch(a) = &u[1].unit else { panic!() };
        assert_eq!(a.body.len(), 4);
    }
}
