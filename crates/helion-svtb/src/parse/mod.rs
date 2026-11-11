//! Recursive-descent parser: design units, classes, and declarations.
//! Statements live in [`stmt`], expressions and constraints in [`expr`].

mod expr;
mod stmt;

use std::rc::Rc;

use crate::ast::*;
use crate::lex::{Tok, Token};
use crate::pp::SourceMap;
use crate::SvError;

pub struct Parser<'a> {
    pub(crate) t: Vec<Token>,
    pub(crate) p: usize,
    pub(crate) map: &'a SourceMap,
    pub(crate) uid: &'a mut u32,
}

pub(crate) const BUILTIN_TYPES: &[&str] = &[
    "bit", "logic", "reg", "int", "integer", "byte", "shortint", "longint", "time", "real", "realtime",
    "shortreal", "string", "event", "chandle", "void",
];

/// Reserved words that can never start a user-type declaration.
pub(crate) fn is_reserved(s: &str) -> bool {
    matches!(
        s,
        "begin" | "end" | "if" | "else" | "for" | "foreach" | "while" | "do" | "repeat" | "forever" | "fork"
            | "join" | "join_any" | "join_none" | "case" | "casez" | "casex" | "endcase" | "default" | "return"
            | "break" | "continue" | "wait" | "disable" | "assert" | "new" | "null" | "this" | "super"
            | "function" | "endfunction" | "task" | "endtask" | "class" | "endclass" | "module" | "endmodule"
            | "initial" | "always" | "always_comb" | "always_ff" | "always_latch" | "assign" | "posedge"
            | "negedge" | "edge" | "or" | "and" | "inside" | "dist" | "with" | "randcase" | "unique"
            | "priority" | "constraint" | "typedef" | "package" | "endpackage" | "import" | "export"
            | "interface" | "endinterface" | "virtual" | "extends" | "static" | "automatic" | "local"
            | "protected" | "rand" | "randc" | "const" | "input" | "output" | "inout" | "ref" | "parameter"
            | "localparam" | "final" | "solve" | "before" | "soft" | "clocking" | "endclocking" | "modport"
            | "program" | "endprogram" | "pure" | "extern" | "type" | "enum" | "struct" | "union" | "signed"
            | "unsigned" | "var" | "wire" | "iff" | "void"
    )
}

impl<'a> Parser<'a> {
    pub fn new(t: Vec<Token>, map: &'a SourceMap, uid: &'a mut u32) -> Self {
        Self { t, p: 0, map, uid }
    }

    // ---- token helpers ----------------------------------------------------

    pub(crate) fn tok(&self) -> Option<&Tok> {
        self.t.get(self.p).map(|t| &t.tok)
    }
    pub(crate) fn tok_at(&self, n: usize) -> Option<&Tok> {
        self.t.get(self.p + n).map(|t| &t.tok)
    }
    pub(crate) fn loc(&self) -> Loc {
        match self.t.get(self.p).or_else(|| self.t.last()) {
            Some(t) => Loc { file: t.file, line: t.line },
            None => Loc::default(),
        }
    }
    pub(crate) fn err(&self, m: impl Into<String>) -> SvError {
        let l = self.loc();
        let near = self.t.get(self.p).map(|t| t.src()).unwrap_or_else(|| "end of file".into());
        SvError::new(&self.map.label(l.file), l.line as usize, format!("{} (near `{near}`)", m.into()))
    }
    pub(crate) fn is_op(&self, o: &str) -> bool {
        matches!(self.tok(), Some(Tok::Op(x)) if *x == o)
    }
    pub(crate) fn is_op_at(&self, n: usize, o: &str) -> bool {
        matches!(self.tok_at(n), Some(Tok::Op(x)) if *x == o)
    }
    pub(crate) fn eat_op(&mut self, o: &str) -> bool {
        if self.is_op(o) {
            self.p += 1;
            true
        } else {
            false
        }
    }
    pub(crate) fn expect_op(&mut self, o: &str) -> Result<(), SvError> {
        if self.eat_op(o) { Ok(()) } else { Err(self.err(format!("expected `{o}`"))) }
    }
    pub(crate) fn is_kw(&self, k: &str) -> bool {
        matches!(self.tok(), Some(Tok::Id(x)) if x == k)
    }
    pub(crate) fn is_kw_at(&self, n: usize, k: &str) -> bool {
        matches!(self.tok_at(n), Some(Tok::Id(x)) if x == k)
    }
    pub(crate) fn eat_kw(&mut self, k: &str) -> bool {
        if self.is_kw(k) {
            self.p += 1;
            true
        } else {
            false
        }
    }
    pub(crate) fn expect_kw(&mut self, k: &str) -> Result<(), SvError> {
        if self.eat_kw(k) { Ok(()) } else { Err(self.err(format!("expected `{k}`"))) }
    }
    pub(crate) fn ident(&mut self) -> Result<String, SvError> {
        match self.tok() {
            Some(Tok::Id(s)) if !is_reserved(s) || matches!(s.as_str(), "new") => {
                let s = s.clone();
                self.p += 1;
                Ok(s)
            }
            _ => Err(self.err("expected an identifier")),
        }
    }
    pub(crate) fn peek_ident(&self) -> Option<&str> {
        match self.tok() {
            Some(Tok::Id(s)) if !is_reserved(s) => Some(s.as_str()),
            _ => None,
        }
    }
    pub(crate) fn at_end(&self) -> bool {
        self.p >= self.t.len()
    }
    /// Skip an optional `: label` after an end keyword.
    pub(crate) fn end_label(&mut self) {
        if self.is_op(":") && matches!(self.tok_at(1), Some(Tok::Id(_))) {
            self.p += 2;
        }
    }
    /// Index of the token after the balanced group starting at `self.p + n`
    /// (which must be an opening bracket).
    pub(crate) fn skip_balanced_from(&self, n: usize) -> Option<usize> {
        let mut depth = 0i32;
        let mut k = self.p + n;
        while k < self.t.len() {
            match &self.t[k].tok {
                Tok::Op("(") | Tok::Op("[") | Tok::Op("{") => depth += 1,
                Tok::Op(")") | Tok::Op("]") | Tok::Op("}") => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(k + 1 - self.p);
                    }
                }
                _ => {}
            }
            k += 1;
        }
        None
    }

    // ---- source unit -------------------------------------------------------

    pub fn parse_unit(&mut self, unit: &mut SourceUnit) -> Result<(), SvError> {
        while !self.at_end() {
            if self.eat_op(";") {
                continue;
            }
            if self.is_kw("module") || self.is_kw("interface") || self.is_kw("program") {
                let m = self.parse_module()?;
                unit.modules.push(Rc::new(m));
                continue;
            }
            if self.is_kw("package") {
                let p = self.parse_package()?;
                unit.packages.push(Rc::new(p));
                continue;
            }
            let it = self.parse_pkg_item()?;
            unit.unit.extend(it);
        }
        Ok(())
    }

    fn parse_package(&mut self) -> Result<Package, SvError> {
        let loc = self.loc();
        self.expect_kw("package")?;
        let _ = self.eat_kw("automatic") || self.eat_kw("static");
        let name = self.ident()?;
        self.expect_op(";")?;
        let mut items = Vec::new();
        while !self.eat_kw("endpackage") {
            if self.at_end() {
                return Err(self.err(format!("missing endpackage for `{name}`")));
            }
            if self.eat_op(";") {
                continue;
            }
            items.extend(self.parse_pkg_item()?);
        }
        self.end_label();
        Ok(Package { name, items, loc })
    }

    /// One package / `$unit` item (may produce several declarations).
    fn parse_pkg_item(&mut self) -> Result<Vec<PkgItem>, SvError> {
        if self.is_kw("import") {
            return self.parse_import().map(|v| v.into_iter().map(|(p, i)| PkgItem::Import(p, i)).collect());
        }
        if self.is_kw("export") {
            while !self.eat_op(";") {
                self.p += 1;
            }
            return Ok(vec![]);
        }
        if self.is_kw("class") || (self.is_kw("virtual") && self.is_kw_at(1, "class")) || (self.is_kw("interface") && self.is_kw_at(1, "class")) {
            return Ok(vec![PkgItem::Class(Rc::new(self.parse_class()?))]);
        }
        if self.is_kw("typedef") {
            return Ok(vec![PkgItem::Typedef(self.parse_typedef()?)]);
        }
        if self.is_kw("function") || self.is_kw("task") {
            return Ok(vec![PkgItem::Sub(Rc::new(self.parse_sub(SubQuals::default())?))]);
        }
        if self.is_kw("parameter") || self.is_kw("localparam") {
            return Ok(self.parse_param_decl()?.into_iter().map(PkgItem::Param).collect());
        }
        if self.is_kw("timeunit") || self.is_kw("timeprecision") {
            while !self.eat_op(";") {
                self.p += 1;
            }
            return Ok(vec![]);
        }
        let quals = self.parse_var_quals();
        let ty = self.parse_data_type()?;
        let decls = self.parse_declarators(ty, quals)?;
        Ok(decls.into_iter().map(PkgItem::Var).collect())
    }

    pub(crate) fn parse_import(&mut self) -> Result<Vec<(String, Option<String>)>, SvError> {
        self.expect_kw("import")?;
        let mut out = Vec::new();
        if matches!(self.tok(), Some(Tok::Str(_))) {
            // DPI import: not supported.
            return Err(self.err("DPI imports are not supported by the testbench engine"));
        }
        loop {
            let p = self.ident()?;
            self.expect_op("::")?;
            let item = if self.eat_op("*") { None } else { Some(self.ident()?) };
            out.push((p, item));
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";")?;
        Ok(out)
    }

    // ---- modules / interfaces ---------------------------------------------

    fn parse_module(&mut self) -> Result<ModuleDecl, SvError> {
        let loc = self.loc();
        let kw = match self.tok() {
            Some(Tok::Id(s)) => s.clone(),
            _ => unreachable!(),
        };
        self.p += 1;
        let is_interface = kw == "interface";
        let is_program = kw == "program";
        let endkw = format!("end{kw}");
        let _ = self.eat_kw("automatic") || self.eat_kw("static");
        let name = self.ident()?;
        let mut items: Vec<ModItem> = Vec::new();
        while self.is_kw("import") {
            for (p, i) in self.parse_import()? {
                items.push(ModItem::Import(p, i));
            }
        }
        let params = if self.is_op("#") { self.parse_param_ports()? } else { Vec::new() };
        let mut ports = Vec::new();
        if self.eat_op("(") {
            if !self.eat_op(")") {
                let mut dir = Dir::Input;
                let mut ty = DataType::Int { kind: IntKind::Logic, signed: None, packed: vec![] };
                loop {
                    if self.eat_op(".*") {
                        return Err(self.err("`.*` in a port list is not supported"));
                    }
                    let mut explicit = false;
                    for (k, d) in [("input", Dir::Input), ("output", Dir::Output), ("inout", Dir::Inout), ("ref", Dir::Ref)] {
                        if self.eat_kw(k) {
                            dir = d;
                            explicit = true;
                        }
                    }
                    let _ = self.eat_kw("wire") || self.eat_kw("var");
                    // A type is present unless the next tokens are `name ,` / `name )`.
                    let bare_name = self.peek_ident().is_some() && (self.is_op_at(1, ",") || self.is_op_at(1, ")") || self.is_op_at(1, "["));
                    if !bare_name {
                        ty = self.parse_data_type()?;
                    } else if explicit {
                        ty = DataType::Int { kind: IntKind::Logic, signed: None, packed: vec![] };
                    }
                    let pname = self.ident()?;
                    while self.is_op("[") {
                        let n = self.skip_balanced_from(0).ok_or_else(|| self.err("unbalanced `[`"))?;
                        self.p += n;
                    }
                    ports.push(PortDecl { dir, ty: ty.clone(), name: pname });
                    if self.eat_op(")") {
                        break;
                    }
                    self.expect_op(",")?;
                }
            }
        }
        self.expect_op(";")?;
        while !self.eat_kw(&endkw) {
            if self.at_end() {
                return Err(self.err(format!("missing {endkw} for `{name}`")));
            }
            if self.eat_op(";") {
                continue;
            }
            self.parse_mod_item(&mut items)?;
        }
        self.end_label();
        Ok(ModuleDecl { name, is_interface, is_program, params, ports, items, loc })
    }

    fn parse_mod_item(&mut self, items: &mut Vec<ModItem>) -> Result<(), SvError> {
        let loc = self.loc();
        if self.eat_kw("initial") {
            let s = self.parse_stmt()?;
            items.push(ModItem::Initial(Rc::from(vec![s]), loc));
            return Ok(());
        }
        if self.eat_kw("final") {
            let s = self.parse_stmt()?;
            items.push(ModItem::Final(Rc::from(vec![s]), loc));
            return Ok(());
        }
        for (k, kind) in [("always", AlwaysKind::Always), ("always_comb", AlwaysKind::Comb), ("always_ff", AlwaysKind::Ff), ("always_latch", AlwaysKind::Latch)] {
            if self.eat_kw(k) {
                let s = self.parse_stmt()?;
                items.push(ModItem::Always(kind, Rc::from(vec![s]), loc));
                return Ok(());
            }
        }
        if self.eat_kw("assign") {
            loop {
                let l = self.parse_postfix()?;
                self.expect_op("=")?;
                let r = self.parse_expr()?;
                items.push(ModItem::Assign(l, r, loc));
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(";")?;
            return Ok(());
        }
        if self.is_kw("import") {
            for (p, i) in self.parse_import()? {
                items.push(ModItem::Import(p, i));
            }
            return Ok(());
        }
        if self.is_kw("function") || self.is_kw("task") {
            items.push(ModItem::Sub(Rc::new(self.parse_sub(SubQuals::default())?)));
            return Ok(());
        }
        if self.is_kw("typedef") {
            items.push(ModItem::Typedef(self.parse_typedef()?));
            return Ok(());
        }
        if self.is_kw("parameter") || self.is_kw("localparam") {
            for p in self.parse_param_decl()? {
                items.push(ModItem::Param(p));
            }
            return Ok(());
        }
        if self.is_kw("class") || (self.is_kw("virtual") && self.is_kw_at(1, "class")) {
            items.push(ModItem::Class(Rc::new(self.parse_class()?)));
            return Ok(());
        }
        if self.is_kw("clocking") || (self.is_kw("default") && self.is_kw_at(1, "clocking")) {
            items.push(ModItem::Clocking(self.parse_clocking()?));
            return Ok(());
        }
        if self.is_kw("modport") || self.is_kw("timeunit") || self.is_kw("timeprecision") || self.is_kw("genvar") {
            while !self.eat_op(";") {
                if self.at_end() {
                    return Err(self.err("unterminated declaration"));
                }
                self.p += 1;
            }
            return Ok(());
        }
        if self.is_kw("generate") {
            return Err(self.err("generate blocks are not supported in testbench modules"));
        }
        if self.is_kw("property") || self.is_kw("sequence") || self.is_kw("covergroup") {
            return Err(self.err(format!(
                "`{}` is not supported by the class-based testbench engine",
                self.t[self.p].src()
            )));
        }
        // Instance: `Id [#(...)] Id (` .
        if let Some(m) = self.peek_ident().map(str::to_string) {
            let mut k = 1;
            if self.is_op_at(1, "#") {
                k = self.skip_balanced_from(2).ok_or_else(|| self.err("unbalanced #("))?;
            }
            if matches!(self.tok_at(k), Some(Tok::Id(_))) && self.is_op_at(k + 1, "(") {
                self.p += 1;
                let params = if self.eat_op("#") { self.parse_param_args()? } else { Vec::new() };
                let name = self.ident()?;
                let conns = self.parse_conns()?;
                self.expect_op(";")?;
                items.push(ModItem::Inst(Instance { module: m, params, name, conns, loc }));
                return Ok(());
            }
        }
        let _ = self.eat_kw("wire");
        let quals = self.parse_var_quals();
        let ty = self.parse_data_type()?;
        for d in self.parse_declarators(ty, quals)? {
            items.push(ModItem::Var(d));
        }
        Ok(())
    }

    fn parse_conns(&mut self) -> Result<Vec<(Option<String>, Option<Expr>)>, SvError> {
        self.expect_op("(")?;
        let mut v = Vec::new();
        if self.eat_op(")") {
            return Ok(v);
        }
        loop {
            if self.eat_op(".*") {
                v.push((Some(".*".into()), None));
            } else if self.eat_op(".") {
                let n = self.ident()?;
                if self.eat_op("(") {
                    let e = if self.is_op(")") { None } else { Some(self.parse_expr()?) };
                    self.expect_op(")")?;
                    v.push((Some(n), e));
                } else {
                    v.push((Some(n.clone()), Some(Expr::Ident(n))));
                }
            } else {
                v.push((None, Some(self.parse_expr()?)));
            }
            if self.eat_op(")") {
                break;
            }
            self.expect_op(",")?;
        }
        Ok(v)
    }

    fn parse_clocking(&mut self) -> Result<ClockingDecl, SvError> {
        let is_default = self.eat_kw("default");
        self.expect_kw("clocking")?;
        let name = if self.peek_ident().is_some() { self.ident()? } else { String::from("__default") };
        self.expect_op("@")?;
        let event = self.parse_event_terms()?;
        self.expect_op(";")?;
        let mut signals = Vec::new();
        while !self.eat_kw("endclocking") {
            if self.at_end() {
                return Err(self.err("missing endclocking"));
            }
            if self.is_kw("default") {
                while !self.eat_op(";") {
                    self.p += 1;
                }
                continue;
            }
            let mut is_in = false;
            let mut is_out = false;
            if self.eat_kw("input") {
                is_in = true;
                self.skip_skew();
            }
            if self.eat_kw("output") {
                is_out = true;
                self.skip_skew();
            }
            if self.eat_kw("inout") {
                is_in = true;
                is_out = true;
            }
            loop {
                let n = self.ident()?;
                signals.push((n, is_in, is_out));
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(";")?;
        }
        self.end_label();
        Ok(ClockingDecl { name, event, signals, is_default })
    }

    fn skip_skew(&mut self) {
        // `#1`, `#1step`, `#0`, `posedge`… are accepted and ignored: inputs
        // sample just before the clock edge, outputs drive just after it.
        if self.eat_kw("posedge") || self.eat_kw("negedge") || self.eat_kw("edge") {}
        if self.eat_op("#") {
            self.p += 1;
            if self.is_kw("step") {
                self.p += 1;
            }
        }
    }

    // ---- parameters ---------------------------------------------------------

    /// `#( parameter int N = 4, type T = int, M )`
    pub(crate) fn parse_param_ports(&mut self) -> Result<Vec<ParamDecl>, SvError> {
        self.expect_op("#")?;
        self.expect_op("(")?;
        let mut out = Vec::new();
        if self.eat_op(")") {
            return Ok(out);
        }
        let mut last_is_type = false;
        let mut last_ty: Option<DataType> = None;
        loop {
            let local = self.eat_kw("localparam");
            let mut explicit = self.eat_kw("parameter") || local;
            if self.eat_kw("type") {
                last_is_type = true;
                explicit = true;
                last_ty = None;
            } else if explicit || !(self.peek_ident().is_some() && (self.is_op_at(1, "=") || self.is_op_at(1, ",") || self.is_op_at(1, ")"))) {
                // A value parameter, possibly with a data type.
                last_is_type = false;
                if !(self.peek_ident().is_some() && (self.is_op_at(1, "=") || self.is_op_at(1, ",") || self.is_op_at(1, ")"))) {
                    last_ty = Some(self.parse_data_type()?);
                } else if explicit {
                    last_ty = None;
                }
            }
            let name = self.ident()?;
            let default = if self.eat_op("=") {
                Some(if last_is_type { ParamArg::Type(self.parse_data_type()?) } else { ParamArg::Expr(self.parse_expr()?) })
            } else {
                None
            };
            out.push(ParamDecl { is_type: last_is_type, ty: last_ty.clone(), name, default, local });
            if self.eat_op(")") {
                break;
            }
            self.expect_op(",")?;
        }
        Ok(out)
    }

    /// `parameter int A = 1, B = 2;` / `localparam type T = int;`
    pub(crate) fn parse_param_decl(&mut self) -> Result<Vec<ParamDecl>, SvError> {
        let local = self.is_kw("localparam");
        self.p += 1;
        let is_type = self.eat_kw("type");
        let ty = if !is_type && !(self.peek_ident().is_some() && self.is_op_at(1, "=")) {
            Some(self.parse_data_type()?)
        } else {
            None
        };
        let mut out = Vec::new();
        loop {
            let name = self.ident()?;
            while self.is_op("[") {
                let n = self.skip_balanced_from(0).ok_or_else(|| self.err("unbalanced `[`"))?;
                self.p += n;
            }
            self.expect_op("=")?;
            let default = Some(if is_type { ParamArg::Type(self.parse_data_type()?) } else { ParamArg::Expr(self.parse_expr()?) });
            out.push(ParamDecl { is_type, ty: ty.clone(), name, default, local });
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";")?;
        Ok(out)
    }

    /// After `#`: `( args )` or a single literal (`#5` is not legal here).
    pub(crate) fn parse_param_args(&mut self) -> Result<Vec<(Option<String>, ParamArg)>, SvError> {
        self.expect_op("(")?;
        let mut out = Vec::new();
        if self.eat_op(")") {
            return Ok(out);
        }
        loop {
            if self.eat_op(".") {
                let n = self.ident()?;
                self.expect_op("(")?;
                let a = self.parse_param_arg()?;
                self.expect_op(")")?;
                out.push((Some(n), a));
            } else {
                out.push((None, self.parse_param_arg()?));
            }
            if self.eat_op(")") {
                break;
            }
            self.expect_op(",")?;
        }
        Ok(out)
    }

    /// A parameter value: a data type when it unambiguously starts one,
    /// otherwise an expression (a bare identifier stays an expression and
    /// is resolved as a type or value during elaboration).
    pub(crate) fn parse_param_arg(&mut self) -> Result<ParamArg, SvError> {
        let is_type = match self.tok() {
            Some(Tok::Id(s)) => {
                BUILTIN_TYPES.contains(&s.as_str()) || s == "virtual" || s == "enum" || s == "struct"
                    || (!is_reserved(s) && (self.is_op_at(1, "#") || (self.is_op_at(1, "::") && self.type_path_then_end())))
            }
            _ => false,
        };
        if is_type {
            Ok(ParamArg::Type(self.parse_data_type()?))
        } else {
            Ok(ParamArg::Expr(self.parse_expr()?))
        }
    }

    /// `a::b::c` followed by `,` / `)` (a scoped type, not an expression).
    fn type_path_then_end(&self) -> bool {
        let mut k = 0;
        loop {
            if !matches!(self.tok_at(k), Some(Tok::Id(_))) {
                return false;
            }
            k += 1;
            if self.is_op_at(k, "::") {
                k += 1;
                continue;
            }
            return self.is_op_at(k, ",") || self.is_op_at(k, ")");
        }
    }

    // ---- types ---------------------------------------------------------------

    pub(crate) fn parse_packed_dims(&mut self) -> Result<Vec<(Expr, Expr)>, SvError> {
        let mut v = Vec::new();
        while self.is_op("[") {
            self.p += 1;
            let a = self.parse_expr()?;
            self.expect_op(":")?;
            let b = self.parse_expr()?;
            self.expect_op("]")?;
            v.push((a, b));
        }
        Ok(v)
    }

    pub(crate) fn parse_data_type(&mut self) -> Result<DataType, SvError> {
        let s = match self.tok() {
            Some(Tok::Id(s)) => s.clone(),
            Some(Tok::Op("[")) => {
                let packed = self.parse_packed_dims()?;
                return Ok(DataType::Implicit { signed: false, packed });
            }
            _ => return Err(self.err("expected a data type")),
        };
        let int_kind = match s.as_str() {
            "bit" => Some(IntKind::Bit),
            "logic" => Some(IntKind::Logic),
            "reg" => Some(IntKind::Reg),
            "int" => Some(IntKind::Int),
            "integer" => Some(IntKind::Integer),
            "byte" => Some(IntKind::Byte),
            "shortint" => Some(IntKind::Shortint),
            "longint" => Some(IntKind::Longint),
            "time" => Some(IntKind::Time),
            _ => None,
        };
        if let Some(kind) = int_kind {
            self.p += 1;
            let signed = if self.eat_kw("signed") {
                Some(true)
            } else if self.eat_kw("unsigned") {
                Some(false)
            } else {
                None
            };
            let packed = self.parse_packed_dims()?;
            return Ok(DataType::Int { kind, signed, packed });
        }
        match s.as_str() {
            "real" | "realtime" | "shortreal" => {
                self.p += 1;
                Ok(DataType::Real)
            }
            "string" => {
                self.p += 1;
                Ok(DataType::Str)
            }
            "event" => {
                self.p += 1;
                Ok(DataType::Event)
            }
            "void" => {
                self.p += 1;
                Ok(DataType::Void)
            }
            "chandle" => {
                self.p += 1;
                Ok(DataType::Chandle)
            }
            "signed" | "unsigned" => {
                self.p += 1;
                let packed = self.parse_packed_dims()?;
                Ok(DataType::Implicit { signed: s == "signed", packed })
            }
            "virtual" => {
                self.p += 1;
                let _ = self.eat_kw("interface");
                let iface = self.ident()?;
                if self.is_op("#") {
                    let n = self.skip_balanced_from(1).ok_or_else(|| self.err("unbalanced #("))?;
                    self.p += n;
                }
                let modport = if self.is_op(".") && matches!(self.tok_at(1), Some(Tok::Id(_))) {
                    self.p += 1;
                    Some(self.ident()?)
                } else {
                    None
                };
                Ok(DataType::Virtual { iface, modport })
            }
            "enum" => {
                self.p += 1;
                let base = if self.is_op("{") {
                    DataType::Int { kind: IntKind::Int, signed: None, packed: vec![] }
                } else {
                    self.parse_data_type()?
                };
                self.expect_op("{")?;
                let mut items = Vec::new();
                loop {
                    let n = self.ident()?;
                    let v = if self.eat_op("=") { Some(self.parse_expr()?) } else { None };
                    items.push((n, v));
                    if self.eat_op("}") {
                        break;
                    }
                    self.expect_op(",")?;
                }
                Ok(DataType::Enum { base: Box::new(base), items })
            }
            "struct" | "union" => {
                self.p += 1;
                let packed = self.eat_kw("packed");
                let _ = self.eat_kw("signed") || self.eat_kw("unsigned");
                self.expect_op("{")?;
                let mut fields = Vec::new();
                while !self.eat_op("}") {
                    let _ = self.eat_kw("rand") || self.eat_kw("randc");
                    let ty = self.parse_data_type()?;
                    loop {
                        let n = self.ident()?;
                        fields.push((ty.clone(), n));
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    self.expect_op(";")?;
                }
                if s == "union" {
                    return Err(self.err("unions are not supported"));
                }
                Ok(DataType::Struct { packed, fields })
            }
            "type" => Err(self.err("`type(...)` operator is not supported")),
            _ if is_reserved(&s) => Err(self.err("expected a data type")),
            _ => {
                let path = self.parse_type_path()?;
                // Packed dims on a user type (`word_t [3:0]`) are not supported.
                Ok(DataType::Named { path })
            }
        }
    }

    pub(crate) fn parse_type_path(&mut self) -> Result<Vec<Seg>, SvError> {
        let mut path = Vec::new();
        loop {
            let name = self.ident()?;
            let params = if self.is_op("#") && self.is_op_at(1, "(") {
                self.p += 1;
                Some(self.parse_param_args()?.into_iter().map(|(n, a)| match n {
                    Some(n) => ParamArg::Named(n, Box::new(a)),
                    None => a,
                }).collect())
            } else {
                None
            };
            path.push(Seg { name, params });
            if self.is_op("::") && matches!(self.tok_at(1), Some(Tok::Id(_))) {
                self.p += 1;
                continue;
            }
            break;
        }
        Ok(path)
    }

    pub(crate) fn parse_udims(&mut self) -> Result<Vec<UDim>, SvError> {
        let mut v = Vec::new();
        while self.eat_op("[") {
            if self.eat_op("]") {
                v.push(UDim::Dynamic);
                continue;
            }
            if self.eat_op("$") {
                let bound = if self.eat_op(":") { Some(self.parse_expr()?) } else { None };
                self.expect_op("]")?;
                v.push(UDim::Queue(bound));
                continue;
            }
            if self.eat_op("*") {
                self.expect_op("]")?;
                v.push(UDim::Assoc(None));
                continue;
            }
            if let Some(Tok::Id(s)) = self.tok() {
                if matches!(s.as_str(), "string" | "int" | "integer" | "byte" | "longint" | "shortint" | "bit" | "logic") && self.is_op_at(1, "]") || matches!(s.as_str(), "bit" | "logic" | "int") && self.is_op_at(1, "[") {
                    let ty = self.parse_data_type()?;
                    self.expect_op("]")?;
                    v.push(UDim::Assoc(Some(ty)));
                    continue;
                }
            }
            let a = self.parse_expr()?;
            let b = if self.eat_op(":") { Some(self.parse_expr()?) } else { None };
            self.expect_op("]")?;
            v.push(UDim::Fixed(a, b));
        }
        Ok(v)
    }

    pub(crate) fn parse_var_quals(&mut self) -> (RandKind, bool, bool) {
        let mut rand = RandKind::None;
        let mut st = false;
        let mut cst = false;
        loop {
            if self.eat_kw("rand") {
                rand = RandKind::Rand;
            } else if self.eat_kw("randc") {
                rand = RandKind::Randc;
            } else if self.eat_kw("static") {
                st = true;
            } else if self.eat_kw("const") {
                cst = true;
            } else if self.eat_kw("local") || self.eat_kw("protected") || self.eat_kw("automatic") || self.eat_kw("var") {
            } else {
                break;
            }
        }
        (rand, st, cst)
    }

    pub(crate) fn parse_declarators(&mut self, ty: DataType, q: (RandKind, bool, bool)) -> Result<Vec<VarDecl>, SvError> {
        let mut out = Vec::new();
        loop {
            let loc = self.loc();
            let name = self.ident()?;
            let udims = self.parse_udims()?;
            let init = if self.eat_op("=") { Some(self.parse_expr()?) } else { None };
            out.push(VarDecl { ty: ty.clone(), name, udims, init, rand: q.0, is_static: q.1, is_const: q.2, loc });
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";")?;
        Ok(out)
    }

    pub(crate) fn parse_typedef(&mut self) -> Result<Typedef, SvError> {
        self.expect_kw("typedef")?;
        if self.eat_kw("class") || (self.is_kw("interface") && self.is_kw_at(1, "class") && { self.p += 2; true }) {
            let name = self.ident()?;
            self.expect_op(";")?;
            return Ok(Typedef { name, ty: None, udims: vec![] });
        }
        if self.peek_ident().is_some() && self.is_op_at(1, ";") {
            // `typedef foo;` forward declaration.
            let name = self.ident()?;
            self.expect_op(";")?;
            return Ok(Typedef { name, ty: None, udims: vec![] });
        }
        let ty = self.parse_data_type()?;
        let name = self.ident()?;
        let udims = self.parse_udims()?;
        self.expect_op(";")?;
        Ok(Typedef { name, ty: Some(ty), udims })
    }

    // ---- classes ----------------------------------------------------------------

    fn parse_class(&mut self) -> Result<ClassDecl, SvError> {
        let loc = self.loc();
        let is_virtual = self.eat_kw("virtual");
        let _ = self.eat_kw("interface");
        self.expect_kw("class")?;
        let _ = self.eat_kw("automatic") || self.eat_kw("static");
        let name = self.ident()?;
        let params = if self.is_op("#") { self.parse_param_ports()? } else { Vec::new() };
        let extends = if self.eat_kw("extends") {
            let t = DataType::Named { path: self.parse_type_path()? };
            if self.is_op("(") {
                // `extends base(args)` passes constructor args: not supported.
                return Err(self.err("`extends base(args)` is not supported; call super.new(args) in new()"));
            }
            Some(t)
        } else {
            None
        };
        if self.eat_kw("implements") {
            loop {
                self.parse_type_path()?;
                if !self.eat_op(",") {
                    break;
                }
            }
        }
        self.expect_op(";")?;
        let mut items = Vec::new();
        while !self.eat_kw("endclass") {
            if self.at_end() {
                return Err(self.err(format!("missing endclass for `{name}`")));
            }
            if self.eat_op(";") {
                continue;
            }
            self.parse_class_item(&mut items)?;
        }
        self.end_label();
        *self.uid += 1;
        Ok(ClassDecl { name, is_virtual, params, extends, items, loc, uid: *self.uid })
    }

    fn parse_class_item(&mut self, items: &mut Vec<ClassItem>) -> Result<(), SvError> {
        if self.is_kw("typedef") {
            items.push(ClassItem::Typedef(self.parse_typedef()?));
            return Ok(());
        }
        if self.is_kw("parameter") || self.is_kw("localparam") {
            for p in self.parse_param_decl()? {
                items.push(ClassItem::Param(p));
            }
            return Ok(());
        }
        if self.is_kw("class") || (self.is_kw("virtual") && self.is_kw_at(1, "class")) {
            items.push(ClassItem::Class(Rc::new(self.parse_class()?)));
            return Ok(());
        }
        if self.is_kw("covergroup") {
            return Err(self.err("covergroups are not supported by the class-based testbench engine"));
        }
        // Qualifier scan.
        let start = self.p;
        let mut q = SubQuals::default();
        let mut is_static_c = false;
        loop {
            if self.eat_kw("virtual") {
                q.is_virtual = true;
            } else if self.eat_kw("pure") {
                q.is_pure = true;
            } else if self.eat_kw("extern") {
                q.is_extern = true;
            } else if self.eat_kw("static") {
                q.is_static = true;
                is_static_c = true;
            } else if self.eat_kw("local") || self.eat_kw("protected") {
                q.is_local = true;
            } else {
                break;
            }
        }
        if self.is_kw("function") || self.is_kw("task") {
            let s = self.parse_sub(q)?;
            items.push(ClassItem::Method(Rc::new(s)));
            return Ok(());
        }
        if self.is_kw("constraint") {
            let c = self.parse_constraint(is_static_c)?;
            items.push(ClassItem::Constraint(c));
            return Ok(());
        }
        self.p = start;
        let quals = self.parse_var_quals();
        let ty = self.parse_data_type()?;
        for d in self.parse_declarators(ty, quals)? {
            items.push(ClassItem::Prop(d));
        }
        Ok(())
    }

    fn parse_constraint(&mut self, is_static: bool) -> Result<ConstraintDecl, SvError> {
        let loc = self.loc();
        self.expect_kw("constraint")?;
        let name = self.ident()?;
        if self.eat_op(";") {
            // Extern/pure constraint prototype.
            return Ok(ConstraintDecl { name, items: vec![], is_static, loc });
        }
        let items = self.parse_constraint_block()?;
        Ok(ConstraintDecl { name, items, is_static, loc })
    }

    // ---- subroutines ----------------------------------------------------------

    pub(crate) fn parse_sub(&mut self, mut quals: SubQuals) -> Result<SubDecl, SvError> {
        let loc = self.loc();
        let is_task = self.is_kw("task");
        self.p += 1;
        if self.eat_kw("static") {
            // lifetime qualifier; class static methods are marked before `function`.
        }
        let _ = self.eat_kw("automatic");
        // Return type (functions): try a type followed by a name.
        let mut ret = None;
        if !is_task {
            let save = self.p;
            let implicit = self.peek_ident().is_some()
                && (self.is_op_at(1, "(") || self.is_op_at(1, ";") || (self.is_op_at(1, "::") && matches!(self.tok_at(2), Some(Tok::Id(_))) && (self.is_op_at(3, "(") || self.is_op_at(3, ";"))));
            if self.is_kw("new") {
                // constructor
            } else if !implicit {
                match self.parse_data_type() {
                    Ok(t) => ret = Some(t),
                    Err(e) => {
                        self.p = save;
                        return Err(e);
                    }
                }
            } else {
                ret = Some(DataType::Int { kind: IntKind::Logic, signed: None, packed: vec![] });
            }
        }
        let mut name = if self.eat_kw("new") { "new".to_string() } else { self.ident()? };
        let mut class_scope = None;
        if self.eat_op("::") {
            class_scope = Some(name);
            name = if self.eat_kw("new") { "new".to_string() } else { self.ident()? };
        }
        let mut args = Vec::new();
        if self.eat_op("(") {
            if !self.eat_op(")") {
                let mut dir = Dir::Input;
                let mut ty = DataType::Int { kind: IntKind::Logic, signed: None, packed: vec![] };
                loop {
                    let mut explicit_dir = false;
                    if self.eat_kw("const") {}
                    for (k, d) in [("input", Dir::Input), ("output", Dir::Output), ("inout", Dir::Inout), ("ref", Dir::Ref)] {
                        if self.eat_kw(k) {
                            dir = d;
                            explicit_dir = true;
                        }
                    }
                    let _ = self.eat_kw("var");
                    let bare = self.peek_ident().is_some() && (self.is_op_at(1, ",") || self.is_op_at(1, ")") || self.is_op_at(1, "=") || self.is_op_at(1, "["));
                    if !bare {
                        ty = self.parse_data_type()?;
                    } else if explicit_dir {
                        ty = DataType::Int { kind: IntKind::Logic, signed: None, packed: vec![] };
                    }
                    let aname = self.ident()?;
                    let udims = self.parse_udims()?;
                    let default = if self.eat_op("=") { Some(self.parse_expr()?) } else { None };
                    args.push(ArgDecl { dir, ty: ty.clone(), name: aname, udims, default });
                    if self.eat_op(")") {
                        break;
                    }
                    self.expect_op(",")?;
                }
            }
        }
        self.expect_op(";")?;
        if quals.is_pure || quals.is_extern {
            if quals.is_extern {
                quals.is_pure = false;
            }
            return Ok(SubDecl { name, is_task, ret, args, body: Rc::from(vec![]), quals, loc, class_scope });
        }
        let endkw = if is_task { "endtask" } else { "endfunction" };
        let mut body = Vec::new();
        while !self.eat_kw(endkw) {
            if self.at_end() {
                return Err(self.err(format!("missing {endkw} for `{name}`")));
            }
            // Old-style argument declarations inside the body.
            if (self.is_kw("input") || self.is_kw("output") || self.is_kw("inout")) && args.is_empty() {
                let dir = match self.tok() {
                    Some(Tok::Id(s)) if s == "output" => Dir::Output,
                    Some(Tok::Id(s)) if s == "inout" => Dir::Inout,
                    _ => Dir::Input,
                };
                self.p += 1;
                let ty = if self.peek_ident().is_some() && (self.is_op_at(1, ";") || self.is_op_at(1, ",")) {
                    DataType::Int { kind: IntKind::Logic, signed: None, packed: vec![] }
                } else {
                    self.parse_data_type()?
                };
                loop {
                    let aname = self.ident()?;
                    args.push(ArgDecl { dir, ty: ty.clone(), name: aname, udims: vec![], default: None });
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op(";")?;
                continue;
            }
            if self.looks_like_decl() {
                let q = self.parse_var_quals();
                let ty = self.parse_data_type()?;
                for d in self.parse_declarators(ty, q)? {
                    body.push(S { loc: d.loc, k: SK::Decl(d) });
                }
                continue;
            }
            body.push(self.parse_stmt()?);
        }
        self.end_label();
        Ok(SubDecl { name, is_task, ret, args, body: Rc::from(body), quals, loc, class_scope })
    }
}

/// Parse preprocessed tokens into `unit`.
pub fn parse_tokens(toks: Vec<Token>, map: &SourceMap, uid: &mut u32, unit: &mut SourceUnit) -> Result<(), SvError> {
    let mut p = Parser::new(toks, map, uid);
    p.parse_unit(unit)
}
