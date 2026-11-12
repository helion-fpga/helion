//! Elaboration: parameters, types, generate, hierarchy and drivers.
//!
//! The whole design is flattened. Every signal is a flat bit vector with a
//! driver per bit: a top-level input, a register, or one bit of a graph
//! node. Expressions and procedural code are evaluated in `exec.rs`.

use super::ast::*;
use super::dag::{Dag, N};
use super::lower::{Drv, Flat, Latch, Port, Reg, SigInfo};
use helion_ir::PortDir;
use std::collections::HashMap;
use std::rc::Rc;

pub type R<X> = Result<X, String>;

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    /// Packed vector. Index `i` is bit `i - lo`, or `lo + w - 1 - i` when
    /// declared ascending (`[0:7]`).
    Bits { w: u32, signed: bool, lo: i64, asc: bool },
    /// Packed struct or union; fields with their LSB offset.
    Struct { w: u32, signed: bool, fields: Rc<Vec<(String, Rc<Ty>, u32)>> },
    /// Array of `elem` over indices `lo..=hi`; `asc` when declared `[lo:hi]`.
    Array { elem: Rc<Ty>, lo: i64, hi: i64, asc: bool, packed: bool },
}

impl Ty {
    pub fn bits(w: u32, signed: bool) -> Rc<Ty> {
        Rc::new(Ty::Bits { w, signed, lo: 0, asc: false })
    }
    pub fn w(&self) -> u32 {
        match self {
            Ty::Bits { w, .. } | Ty::Struct { w, .. } => *w,
            Ty::Array { elem, lo, hi, .. } => elem.w() * (hi - lo + 1) as u32,
        }
    }
    pub fn signed(&self) -> bool {
        match self {
            Ty::Bits { signed, .. } | Ty::Struct { signed, .. } => *signed,
            Ty::Array { .. } => false,
        }
    }
    /// Offset of element `i` of an array, or bit `i` of a vector.
    pub fn offset(&self, i: i64) -> Option<u32> {
        match self {
            Ty::Bits { w, lo, asc, .. } => {
                let k = i - lo;
                if k < 0 || k >= *w as i64 {
                    return None;
                }
                Some(if *asc { *w - 1 - k as u32 } else { k as u32 })
            }
            Ty::Array { elem, lo, hi, asc, .. } => {
                if i < *lo || i > *hi {
                    return None;
                }
                let k = if *asc { hi - i } else { i - lo };
                Some(k as u32 * elem.w())
            }
            Ty::Struct { w, .. } => (i >= 0 && i < *w as i64).then_some(i as u32),
        }
    }
    /// Element type and count for indexing.
    pub fn elem(&self) -> (Rc<Ty>, u32) {
        match self {
            Ty::Array { elem, lo, hi, .. } => (elem.clone(), (hi - lo + 1) as u32),
            Ty::Bits { w, .. } | Ty::Struct { w, .. } => (Ty::bits(1, false), *w),
        }
    }
}

#[derive(Clone)]
pub enum Sym {
    Sig(u32),
    Const { n: N, ty: Rc<Ty> },
    Type(Rc<Ty>),
    Func(Rc<FuncDecl>, usize),
    Var(u32),
    /// An instance or a labelled generate block: its scope.
    Inst(usize),
}

pub struct Scope {
    pub parent: Option<usize>,
    pub syms: HashMap<String, Sym>,
    /// Scopes of wildcard-imported packages.
    pub imports: Vec<usize>,
    pub prefix: String,
    /// Dotted hierarchical path of this scope (for register maps).
    pub hier: String,
    /// Generate constructs seen in this scope: an unnamed block of the
    /// n-th one is called `genblk<n>` (IEEE 1800, 27.6).
    pub gen_count: u32,
}

pub struct Sig {
    /// Keep a net of this name in the netlist (`mark_debug`, `keep`).
    pub debug: bool,
    pub name: String,
    /// Dotted hierarchical name.
    pub hier: String,
    pub ty: Rc<Ty>,
    pub drv: Vec<Drv>,
    pub init: Vec<bool>,
    pub line: u32,
}

pub struct VarInfo {
    pub name: String,
    pub ty: Rc<Ty>,
}

pub struct Elab {
    pub dag: Dag,
    pub scopes: Vec<Scope>,
    pub sigs: Vec<Sig>,
    pub vars: Vec<VarInfo>,
    pub regs: Vec<Reg>,
    pub latches: Vec<Latch>,
    /// Instances of the hierarchy: (flattened name, module, port connections).
    pub instances: Vec<(String, String, Vec<(String, String)>)>,
    modules: HashMap<String, Rc<Module>>,
    order: Vec<String>,
    packages: HashMap<String, Rc<Package>>,
    pkg_scope: HashMap<String, usize>,
    unit_scope: usize,
    broken: Vec<(String, String)>,
    pub warnings: Vec<String>,
    pub call_depth: u32,
    pub ret_stack: Vec<Option<u32>>,
    pub ps: super::exec::ProcState,
    inst_depth: u32,
    /// Line of the item being elaborated, for messages.
    pub line: u32,
}

impl Elab {
    pub fn new(src: Source) -> R<Self> {
        let mut e = Elab {
            dag: Dag::default(),
            scopes: vec![Scope { parent: None, syms: HashMap::new(), imports: Vec::new(), prefix: String::new(), hier: String::new(), gen_count: 0 }],
            sigs: Vec::new(),
            vars: Vec::new(),
            regs: Vec::new(),
            latches: Vec::new(),
            instances: Vec::new(),
            modules: HashMap::new(),
            order: Vec::new(),
            packages: HashMap::new(),
            pkg_scope: HashMap::new(),
            unit_scope: 0,
            broken: src.broken.clone(),
            warnings: Vec::new(),
            call_depth: 0,
            ret_stack: Vec::new(),
            ps: Default::default(),
            inst_depth: 0,
            line: 0,
        };
        for m in src.modules {
            e.order.push(m.name.clone());
            e.modules.insert(m.name.clone(), Rc::new(m));
        }
        let mut pkg_order = Vec::new();
        for p in src.packages {
            pkg_order.push(p.name.clone());
            e.packages.insert(p.name.clone(), Rc::new(p));
        }
        for name in pkg_order {
            e.package_scope(&name)?;
        }
        let unit = Rc::new(src.unit);
        e.items(&unit, 0)?;
        Ok(e)
    }

    pub fn err<X>(&self, msg: impl AsRef<str>) -> R<X> {
        Err(format!("line {}: {}", self.line, msg.as_ref()))
    }

    pub fn new_scope(&mut self, parent: usize, prefix: String) -> usize {
        let hier = self.scopes[parent].hier.clone();
        self.scopes.push(Scope { parent: Some(parent), syms: HashMap::new(), imports: Vec::new(), prefix, hier, gen_count: 0 });
        self.scopes.len() - 1
    }

    fn package_scope(&mut self, name: &str) -> R<usize> {
        if let Some(&s) = self.pkg_scope.get(name) {
            return Ok(s);
        }
        let Some(p) = self.packages.get(name).cloned() else {
            return self.err(format!("unknown package '{name}'"));
        };
        let sc = self.new_scope(self.unit_scope, format!("{name}__"));
        self.pkg_scope.insert(name.to_string(), sc);
        self.items(&p.items, sc)?;
        Ok(sc)
    }

    pub fn lookup(&self, sc: usize, name: &str) -> Option<Sym> {
        let mut cur = Some(sc);
        while let Some(s) = cur {
            if let Some(sym) = self.scopes[s].syms.get(name) {
                return Some(sym.clone());
            }
            for &imp in &self.scopes[s].imports {
                if let Some(sym) = self.scopes[imp].syms.get(name) {
                    return Some(sym.clone());
                }
            }
            cur = self.scopes[s].parent;
        }
        None
    }

    pub fn lookup_path(&mut self, sc: usize, path: &[String]) -> R<Sym> {
        if path.len() == 1 {
            return match self.lookup(sc, &path[0]) {
                Some(s) => Ok(s),
                None => self.err(format!("'{}' is not declared", path[0])),
            };
        }
        if path.len() == 2 {
            if path[0] == "$unit" {
                return self.lookup_path(self.unit_scope, &path[1..]);
            }
            let ps = self.package_scope(&path[0])?;
            return match self.scopes[ps].syms.get(&path[1]) {
                Some(s) => Ok(s.clone()),
                None => self.err(format!("'{}' is not declared in package '{}'", path[1], path[0])),
            };
        }
        self.err(format!("cannot resolve '{}'", path.join("::")))
    }

    fn prefix(&self, sc: usize) -> String {
        self.scopes[sc].prefix.clone()
    }

    pub fn new_var(&mut self, sc: usize, name: &str, ty: Rc<Ty>) -> u32 {
        self.vars.push(VarInfo { name: name.to_string(), ty });
        let id = self.vars.len() as u32 - 1;
        self.scopes[sc].syms.insert(name.to_string(), Sym::Var(id));
        id
    }

    fn new_sig(&mut self, sc: usize, name: &str, ty: Rc<Ty>) -> u32 {
        let w = ty.w() as usize;
        let full = format!("{}{}", self.prefix(sc), name);
        let hier = format!("{}{}", self.scopes[sc].hier, name);
        self.sigs.push(Sig { debug: false, name: full, hier, ty, drv: vec![Drv::None; w], init: vec![false; w], line: self.line });
        let id = self.sigs.len() as u32 - 1;
        self.scopes[sc].syms.insert(name.to_string(), Sym::Sig(id));
        id
    }

    // ----------------------------------------------------------------- types

    fn const_dim(&mut self, d: &Dim, sc: usize) -> R<(i64, i64)> {
        Ok((self.const_int(&d.0, sc)?, self.const_int(&d.1, sc)?))
    }

    /// Wrap `base` in packed dimensions (outermost first).
    fn with_dims(&mut self, base: Rc<Ty>, dims: &[Dim], sc: usize) -> R<Rc<Ty>> {
        let mut ty = base;
        for d in dims.iter().rev() {
            let (a, b) = self.const_dim(d, sc)?;
            let (lo, hi, asc) = if a >= b { (b, a, false) } else { (a, b, true) };
            ty = Rc::new(Ty::Array { elem: ty, lo, hi, asc, packed: true });
        }
        Ok(ty)
    }

    pub fn resolve_type(&mut self, t: &DataType, sc: usize) -> R<Rc<Ty>> {
        match t {
            DataType::Vec { signed, dims } => {
                if dims.is_empty() {
                    return Ok(Ty::bits(1, *signed));
                }
                // The innermost dimension is the vector itself.
                let (a, b) = self.const_dim(dims.last().unwrap(), sc)?;
                let (lo, hi, asc) = if a >= b { (b, a, false) } else { (a, b, true) };
                let w = (hi - lo + 1) as u64;
                if w > 1 << 24 {
                    return self.err("vector is too wide");
                }
                let base = Rc::new(Ty::Bits { w: w as u32, signed: *signed, lo, asc });
                self.with_dims(base, &dims[..dims.len() - 1], sc)
            }
            DataType::Int { width, signed } => Ok(Ty::bits(*width, *signed)),
            DataType::Str => self.err("'string' can only be the type of a parameter"),
            DataType::Named { path, dims } => {
                let base = match self.lookup_path(sc, path)? {
                    Sym::Type(t) => t,
                    _ => return self.err(format!("'{}' is not a type", path.join("::"))),
                };
                self.with_dims(base, dims, sc)
            }
            DataType::Enum { base, items, dims } => {
                let bt = match base {
                    Some(b) => self.resolve_type(b, sc)?,
                    None => Ty::bits(32, true),
                };
                let (w, signed) = (bt.w(), bt.signed());
                let mut next: i64 = 0;
                for (name, val) in items {
                    let v = match val {
                        Some(e) => self.const_int(e, sc)?,
                        None => next,
                    };
                    next = v + 1;
                    let n = self.kint(v, w);
                    self.scopes[sc].syms.insert(name.clone(), Sym::Const { n, ty: Ty::bits(w, signed) });
                }
                self.with_dims(Ty::bits(w, signed), dims, sc)
            }
            DataType::Struct { fields, dims, signed, union } => {
                let mut fs: Vec<(String, Rc<Ty>, u32)> = Vec::new();
                let mut tys = Vec::new();
                for (ft, name) in fields {
                    tys.push((name.clone(), self.resolve_type(ft, sc)?));
                }
                let w = if *union { tys.iter().map(|t| t.1.w()).max().unwrap_or(0) } else { tys.iter().map(|t| t.1.w()).sum() };
                let mut off = w;
                for (name, t) in tys {
                    if *union {
                        fs.push((name, t, 0));
                    } else {
                        off -= t.w();
                        fs.push((name, t, off));
                    }
                }
                let base = Rc::new(Ty::Struct { w, signed: *signed, fields: Rc::new(fs) });
                self.with_dims(base, dims, sc)
            }
        }
    }

    /// Apply unpacked dimensions (outermost first) to `base`.
    pub fn with_unpacked(&mut self, base: Rc<Ty>, dims: &[(Expr, Option<Expr>)], sc: usize) -> R<Rc<Ty>> {
        let mut ty = base;
        for (a, b) in dims.iter().rev() {
            let a = self.const_int(a, sc)?;
            let (lo, hi, asc) = match b {
                Some(b) => {
                    let b = self.const_int(b, sc)?;
                    if a <= b { (a, b, true) } else { (b, a, false) }
                }
                None => (0, a - 1, true),
            };
            if hi < lo {
                return self.err("array with no elements");
            }
            ty = Rc::new(Ty::Array { elem: ty, lo, hi, asc, packed: false });
        }
        if ty.w() as u64 > 1 << 22 {
            return self.err("array is too large to build from flip-flops (block RAM inference is not available in this path)");
        }
        Ok(ty)
    }

    pub fn kint(&mut self, v: i64, w: u32) -> N {
        let bits: Vec<bool> = (0..w).map(|i| if i < 64 { (v >> i) & 1 == 1 } else { v < 0 }).collect();
        self.dag.konst(bits)
    }

    // ----------------------------------------------------------------- items

    fn param(&mut self, p: &ParamDecl, sc: usize, over: Option<&ParamArg>) -> R<()> {
        self.line = p.line;
        if p.is_type {
            let ty = match over {
                Some(ParamArg::Type(t)) => self.resolve_type(t, sc)?,
                Some(ParamArg::Expr(Expr::Id(path))) => match self.lookup_path(sc, path)? {
                    Sym::Type(t) => t,
                    _ => return self.err(format!("type parameter '{}' needs a type", p.name)),
                },
                Some(ParamArg::Expr(Expr::Type(t))) => self.resolve_type(t, sc)?,
                Some(_) => return self.err(format!("type parameter '{}' needs a type", p.name)),
                None => match &p.type_value {
                    Some(t) => self.resolve_type(t, sc)?,
                    None => return self.err(format!("type parameter '{}' has no value", p.name)),
                },
            };
            self.scopes[sc].syms.insert(p.name.clone(), Sym::Type(ty));
            return Ok(());
        }
        let declared = match &p.ty {
            // A string parameter is as wide as its value.
            Some(DataType::Str) => None,
            Some(t) => {
                let base = self.resolve_type(t, sc)?;
                Some(self.with_unpacked(base, &p.unpacked, sc)?)
            }
            None if !p.unpacked.is_empty() => Some(self.with_unpacked(Ty::bits(32, true), &p.unpacked, sc)?),
            None => None,
        };
        let no_env = &mut super::exec::Env::default();
        let e = match over {
            Some(ParamArg::Expr(e)) => e.clone(),
            Some(ParamArg::Type(_)) => return self.err(format!("parameter '{}' needs a value, not a type", p.name)),
            None => match &p.value {
                Some(e) => e.clone(),
                None => return self.err(format!("parameter '{}' has no value", p.name)),
            },
        };
        // A parameter with no type takes the type of its value.
        let (n, ty) = match &declared {
            Some(t) => (self.eval_to_type(&e, t, sc, no_env)?, t.clone()),
            None => {
                let v = self.eval_self(&e, sc, no_env)?;
                (v.n, v.ty)
            }
        };
        if self.dag.as_const(n).is_none() {
            return self.err(format!("the value of parameter '{}' is not constant", p.name));
        }
        self.scopes[sc].syms.insert(p.name.clone(), Sym::Const { n, ty });
        Ok(())
    }

    /// Elaborate module-level items in scope `sc`.
    pub fn items(&mut self, items: &[Item], sc: usize) -> R<()> {
        let refs: Vec<&Item> = items.iter().collect();
        self.items_ref(&refs, sc)
    }

    /// Declarations first, then instances and generate blocks, then
    /// assignments and processes: a name may be used before the item that
    /// declares it, and a hierarchical reference may reach into an
    /// instance that appears later in the source.
    pub fn items_ref(&mut self, items: &[&Item], sc: usize) -> R<()> {
        // Groups the parser made from one statement are part of this list.
        let mut flat: Vec<&Item> = Vec::with_capacity(items.len());
        for it in items {
            match it {
                Item::Block(b) if b.label.is_none() && b.items.iter().all(|i| matches!(i, Item::Param(_) | Item::Assign { .. } | Item::Decl { .. })) && b.items.iter().any(|i| matches!(i, Item::Param(_) | Item::Assign { .. })) => {
                    flat.extend(b.items.iter())
                }
                other => flat.push(other),
            }
        }
        for it in &flat {
            if let Item::Func(f) = it {
                self.scopes[sc].syms.insert(f.name.clone(), Sym::Func(f.clone(), sc));
            }
        }
        for phase in 0..3 {
            for it in &flat {
                let p = match it {
                    Item::Decl { .. } | Item::Param(_) | Item::Typedef { .. } | Item::Func(_) | Item::Import { .. } | Item::Genvar(_) | Item::PortDir(_) | Item::Debug(_) => 0,
                    Item::Inst(_) | Item::GenFor { .. } | Item::GenIf { .. } | Item::GenCase { .. } | Item::Block(_) => 1,
                    Item::Assign { .. } | Item::Always { .. } => 2,
                };
                if p == phase {
                    self.item(it, sc)?;
                }
            }
        }
        Ok(())
    }

    /// Scope of a generate block: `n` is the number of its generate
    /// construct in `sc`, which names the block when it has no label.
    fn gen_scope(&mut self, b: &GenBlock, sc: usize, suffix: Option<i64>, n: u32) -> usize {
        let name = match &b.label {
            Some(l) => l.clone(),
            None => format!("genblk{n}"),
        };
        let (flat, hier, key) = match suffix {
            Some(i) => (format!("{name}_{i}_"), format!("{name}[{i}]."), format!("{name}[{i}]")),
            None => (format!("{name}_"), format!("{name}."), name.clone()),
        };
        let prefix = format!("{}{}", self.prefix(sc), flat);
        let s = self.new_scope(sc, prefix);
        self.scopes[s].hier = format!("{}{}", self.scopes[sc].hier, hier);
        self.scopes[sc].syms.insert(key, Sym::Inst(s));
        s
    }

    fn next_gen(&mut self, sc: usize) -> u32 {
        self.scopes[sc].gen_count += 1;
        self.scopes[sc].gen_count
    }

    /// The taken branch of a conditional generate. An `else if` directly
    /// nested in an unnamed `else` is the same construct, not a new level.
    fn gen_if(&mut self, cond: &Expr, then: &GenBlock, els: Option<&GenBlock>, sc: usize, n: u32) -> R<()> {
        let c = self.const_int(cond, sc)? != 0;
        let b = if c { Some(then) } else { els };
        let Some(b) = b else { return Ok(()) };
        if !c && b.label.is_none() && b.items.len() == 1 {
            if let Item::GenIf { cond, then, els, line } = &b.items[0] {
                self.line = *line;
                return self.gen_if(cond, then, els.as_ref(), sc, n);
            }
        }
        let s = self.gen_scope(b, sc, None, n);
        self.items(&b.items, s)
    }

    fn item(&mut self, it: &Item, sc: usize) -> R<()> {
        match it {
            Item::Param(p) => self.param(p, sc, None),
            Item::Typedef { name, ty, unpacked, line } => {
                self.line = *line;
                let base = self.resolve_type(ty, sc)?;
                let t = self.with_unpacked(base, unpacked, sc)?;
                self.scopes[sc].syms.insert(name.clone(), Sym::Type(t));
                Ok(())
            }
            Item::Decl { ty, names, line } => {
                self.line = *line;
                let base = self.resolve_type(ty, sc)?;
                for d in names {
                    let t = self.with_unpacked(base.clone(), &d.unpacked, sc)?;
                    let id = self.new_sig(sc, &d.name, t.clone());
                    if let Some(init) = &d.init {
                        // A variable initializer is its power-up value.
                        let n = self.eval_to_type(init, &t, sc, &mut super::exec::Env::default())?;
                        match self.dag.as_const(n) {
                            Some(b) => self.sigs[id as usize].init = b.to_vec(),
                            None => {
                                // By the language this applies once, at time
                                // zero; it is not a continuous assignment.
                                self.warnings.push(format!(
                                    "line {}: the initializer of '{}' is not constant, so it only applies at time zero and is ignored; write 'assign' (or declare a 'wire') for a continuous assignment",
                                    line, d.name
                                ));
                            }
                        }
                    }
                }
                Ok(())
            }
            Item::Assign { lhs, rhs, line } => {
                self.line = *line;
                self.cont_assign(lhs, rhs, sc)
            }
            Item::Always { kind, sens, body, line } => {
                self.line = *line;
                self.always(*kind, sens, body, sc)
            }
            Item::Inst(d) => {
                self.line = d.line;
                self.instance(d, sc)
            }
            Item::Genvar(_) | Item::Func(_) => Ok(()),
            Item::Debug(names) => {
                for n in names {
                    if let Some(Sym::Sig(id)) = self.scopes[sc].syms.get(n) {
                        self.sigs[*id as usize].debug = true;
                    }
                }
                Ok(())
            }
            Item::Import { pkg, name } => {
                let ps = self.package_scope(pkg)?;
                match name {
                    None => self.scopes[sc].imports.push(ps),
                    Some(n) => match self.scopes[ps].syms.get(n).cloned() {
                        Some(s) => {
                            self.scopes[sc].syms.insert(n.clone(), s);
                        }
                        None => return self.err(format!("'{n}' is not declared in package '{pkg}'")),
                    },
                }
                Ok(())
            }
            Item::Block(b) => {
                let n = self.next_gen(sc);
                let s = self.gen_scope(b, sc, None, n);
                self.items(&b.items, s)
            }
            Item::GenIf { cond, then, els, line } => {
                self.line = *line;
                let n = self.next_gen(sc);
                self.gen_if(cond, then, els.as_ref(), sc, n)
            }
            Item::GenCase { expr, arms, line } => {
                self.line = *line;
                let v = self.const_int(expr, sc)?;
                let mut chosen = None;
                for (vals, b) in arms {
                    if vals.is_empty() {
                        if chosen.is_none() {
                            chosen = Some(b);
                        }
                        continue;
                    }
                    let mut hit = false;
                    for e in vals {
                        if self.const_int(e, sc)? == v {
                            hit = true;
                        }
                    }
                    if hit {
                        chosen = Some(b);
                        break;
                    }
                }
                let n = self.next_gen(sc);
                if let Some(b) = chosen {
                    let s = self.gen_scope(b, sc, None, n);
                    self.items(&b.items, s)?;
                }
                Ok(())
            }
            Item::GenFor { var, init, cond, step, body, line } => {
                self.line = *line;
                let mut i = self.const_int(init, sc)?;
                let Stmt::Assign { rhs: step_rhs, .. } = step else {
                    return self.err("generate loop step must be an assignment");
                };
                let blk = self.next_gen(sc);
                let mut count = 0;
                loop {
                    let s = self.gen_scope(body, sc, Some(i), blk);
                    let n = self.kint(i, 32);
                    self.scopes[s].syms.insert(var.clone(), Sym::Const { n, ty: Ty::bits(32, true) });
                    self.line = *line;
                    if self.const_int(cond, s)? == 0 {
                        break;
                    }
                    self.items(&body.items, s)?;
                    self.line = *line;
                    i = self.const_int(step_rhs, s)?;
                    count += 1;
                    if count > 1 << 20 {
                        return self.err("generate loop does not terminate");
                    }
                }
                Ok(())
            }
            Item::PortDir(_) => Ok(()),
        }
    }

    // ------------------------------------------------------------- drivers

    pub fn drive(&mut self, sig: u32, bit: u32, d: Drv) -> R<()> {
        let s = &mut self.sigs[sig as usize];
        match s.drv[bit as usize] {
            Drv::None => {
                s.drv[bit as usize] = d;
                Ok(())
            }
            Drv::Input => {
                let name = s.name.clone();
                self.err(format!("input '{name}' is assigned inside the design"))
            }
            _ => {
                let name = super::lower::bit_name(&s.name, s.drv.len() as u32, bit);
                self.err(format!("'{name}' has more than one driver"))
            }
        }
    }

    fn cont_assign(&mut self, lhs: &Expr, rhs: &Expr, sc: usize) -> R<()> {
        let env = &mut super::exec::Env::default();
        let targets = self.static_targets(lhs, sc, env)?;
        let total: u32 = targets.iter().map(|t| t.2).sum();
        let v = match (targets.len(), &targets[0].3) {
            (1, Some(ty)) => self.eval_to_type(rhs, &ty.clone(), sc, env)?,
            _ => self.eval_assign(rhs, total, sc, env)?,
        };
        // Targets are MSB first.
        let mut hi = total;
        for (sig, lo, w, _) in targets {
            hi -= w;
            for i in 0..w {
                self.drive(sig, lo + i, Drv::Comb(v, hi + i))?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ hierarchy

    fn instance(&mut self, d: &InstDecl, sc: usize) -> R<()> {
        let Some(m) = self.modules.get(&d.module).cloned() else {
            if let Some((_, why)) = self.broken.iter().find(|b| b.0 == d.module) {
                return self.err(format!("module '{}' (instance '{}') cannot be synthesized: {why}", d.module, d.name));
            }
            return self.err(format!("module '{}' is not defined (instance '{}')", d.module, d.name));
        };
        if m.is_interface {
            return self.err("interface instances are not supported");
        }
        self.inst_depth += 1;
        if self.inst_depth > 200 {
            return self.err("instantiation is recursive");
        }
        let prefix = format!("{}{}_", self.prefix(sc), d.name);
        let child = self.new_scope(self.unit_scope, prefix);
        self.scopes[sc].syms.insert(d.name.clone(), Sym::Inst(child));
        self.scopes[child].hier = format!("{}{}.", self.scopes[sc].hier, d.name);
        let conns: Vec<(String, String)> = d
            .conns
            .iter()
            .filter_map(|c| match (&c.port, &c.expr) {
                (Some(p), Some(Expr::Id(path))) => {
                    // The connected signal by its flattened name.
                    let net = match path.len() == 1 {
                        true => match self.lookup(sc, &path[0]) {
                            Some(Sym::Sig(id)) => self.sigs[id as usize].name.clone(),
                            _ => String::new(),
                        },
                        false => String::new(),
                    };
                    Some((p.clone(), net))
                }
                (Some(p), _) => Some((p.clone(), String::new())),
                _ => None,
            })
            .collect();
        self.instances.push((format!("{}{}", self.prefix(sc), d.name), d.module.clone(), conns));
        // Parameters: overrides are evaluated in the parent's scope.
        let mut over: HashMap<String, ParamArg> = HashMap::new();
        let settable: Vec<&ParamDecl> = m.params.iter().filter(|p| !p.local).collect();
        for (i, (name, arg)) in d.params.iter().enumerate() {
            let pname = match name {
                Some(n) => n.clone(),
                None => match settable.get(i) {
                    Some(p) => p.name.clone(),
                    None => return self.err(format!("too many parameters for '{}'", d.module)),
                },
            };
            // Evaluate now, in the parent's scope, to a constant expression.
            let fixed = match arg {
                ParamArg::Type(t) => ParamArg::Type(t.clone()),
                ParamArg::Expr(Expr::Id(p)) if matches!(self.lookup_path(sc, p), Ok(Sym::Type(_))) => ParamArg::Expr(Expr::Id(p.clone())),
                ParamArg::Expr(e) => {
                    let v = self.eval_self(e, sc, &mut super::exec::Env::default())?;
                    if self.dag.as_const(v.n).is_none() {
                        return self.err(format!("parameter '{pname}' of instance '{}' is not constant", d.name));
                    }
                    let tmp = format!("$p{}", self.scopes[child].syms.len());
                    self.scopes[child].syms.insert(tmp.clone(), Sym::Const { n: v.n, ty: v.ty });
                    ParamArg::Expr(Expr::Id(vec![tmp]))
                }
            };
            // A type argument names a type of the parent: resolve it there.
            let fixed = match fixed {
                ParamArg::Type(t) => {
                    let ty = self.resolve_type(&t, sc)?;
                    let tmp = format!("$t{}", self.scopes[child].syms.len());
                    self.scopes[child].syms.insert(tmp.clone(), Sym::Type(ty));
                    ParamArg::Expr(Expr::Id(vec![tmp]))
                }
                ParamArg::Expr(Expr::Id(p)) if !p[0].starts_with('$') => {
                    let Sym::Type(ty) = self.lookup_path(sc, &p)? else { unreachable!() };
                    let tmp = format!("$t{}", self.scopes[child].syms.len());
                    self.scopes[child].syms.insert(tmp.clone(), Sym::Type(ty));
                    ParamArg::Expr(Expr::Id(vec![tmp]))
                }
                other => other,
            };
            if !m.params.iter().any(|p| p.name == pname) && !m.items.iter().any(|i| matches!(i, Item::Param(p) if p.name == pname && !p.local)) {
                return self.err(format!("module '{}' has no parameter '{pname}'", d.module));
            }
            over.insert(pname, fixed);
        }
        let ports = self.module_body(&m, child, &over, false)?;
        // Connections.
        let mut by_name: HashMap<&str, &Conn> = HashMap::new();
        let mut positional = Vec::new();
        for c in &d.conns {
            match &c.port {
                Some(p) => {
                    by_name.insert(p.as_str(), c);
                }
                None => positional.push(c),
            }
        }
        if !positional.is_empty() && !by_name.is_empty() {
            return self.err("instance mixes positional and named connections");
        }
        for (i, (pname, dir, sig)) in ports.iter().enumerate() {
            self.line = d.line;
            let conn: Option<Expr> = if !positional.is_empty() {
                positional.get(i).and_then(|c| c.expr.clone())
            } else if let Some(c) = by_name.remove(pname.as_str()) {
                c.expr.clone()
            } else if d.wildcard && self.lookup(sc, pname).is_some() {
                Some(Expr::Id(vec![pname.clone()]))
            } else {
                None
            };
            let ty = self.sigs[*sig as usize].ty.clone();
            let w = ty.w();
            match dir {
                Dir::In => match conn {
                    Some(e) => {
                        let v = self.eval_to_type(&e, &ty, sc, &mut super::exec::Env::default())?;
                        for b in 0..w {
                            self.drive(*sig, b, Drv::Comb(v, b))?;
                        }
                    }
                    None => {
                        // Left open: the declared default, or 0 with a warning.
                        let dflt = self.sigs[*sig as usize].init.clone();
                        let name = self.sigs[*sig as usize].name.clone();
                        if dflt.iter().all(|b| !b) {
                            self.warnings.push(format!("line {}: input '{name}' is left unconnected and reads 0", d.line));
                        }
                        let v = self.dag.konst(dflt);
                        for b in 0..w {
                            self.drive(*sig, b, Drv::Comb(v, b))?;
                        }
                    }
                },
                Dir::Out => {
                    if let Some(e) = conn {
                        let env = &mut super::exec::Env::default();
                        let targets = self.static_targets(&e, sc, env)?;
                        let total: u32 = targets.iter().map(|t| t.2).sum();
                        let src = self.dag.read(*sig, w);
                        let v = self.dag.ext(src, total.max(w), ty.signed());
                        let mut hi = total;
                        for (tsig, lo, tw, _) in targets {
                            hi -= tw;
                            for b in 0..tw {
                                self.drive(tsig, lo + b, Drv::Comb(v, hi + b))?;
                            }
                        }
                    }
                }
                Dir::Inout => return self.err(format!("inout port '{pname}' is not supported")),
            }
        }
        if let Some(extra) = by_name.keys().next() {
            self.line = d.line;
            return self.err(format!("module '{}' has no port '{extra}'", d.module));
        }
        self.inst_depth -= 1;
        Ok(())
    }

    /// Declare parameters and ports of `m` in `sc` and elaborate its body.
    /// Returns the ports in declaration order.
    fn module_body(&mut self, m: &Rc<Module>, sc: usize, over: &HashMap<String, ParamArg>, top: bool) -> R<Vec<(String, Dir, u32)>> {
        for it in &m.items {
            if let Item::Import { .. } = it {
                self.item(it, sc)?;
            }
        }
        for p in &m.params {
            self.param(p, sc, if p.local { None } else { over.get(&p.name) })?;
        }
        let mut ports: Vec<(String, Dir, u32)> = Vec::new();
        let mut decls: Vec<PortDecl> = m.ports.clone();
        if !m.port_names.is_empty() {
            // Non-ANSI: directions and types come from the body.
            for name in &m.port_names {
                let Some(pd) = m.items.iter().find_map(|i| match i {
                    Item::PortDir(pd) if &pd.decl.name == name => Some(pd.clone()),
                    _ => None,
                }) else {
                    self.line = m.line;
                    return self.err(format!("port '{name}' of module '{}' has no direction", m.name));
                };
                decls.push(pd);
            }
        }
        // Body parameters that the port types may use come first.
        let mut body: Vec<&Item> = Vec::new();
        let mut early = true;
        for it in &m.items {
            match it {
                Item::Import { .. } => {}
                Item::Param(p) if early => self.param(p, sc, if p.local { None } else { over.get(&p.name) })?,
                Item::Typedef { .. } if early => self.item(it, sc)?,
                Item::PortDir(_) => {}
                Item::Func(f) => {
                    self.scopes[sc].syms.insert(f.name.clone(), Sym::Func(f.clone(), sc));
                    body.push(it);
                }
                _ => {
                    early = false;
                    body.push(it);
                }
            }
        }
        for pd in &decls {
            self.line = pd.line;
            let mut base = self.resolve_type(&pd.ty, sc)?;
            // Non-ANSI: `output [7:0] q; reg [7:0] q;` declares the same signal twice.
            if !m.port_names.is_empty() {
                if let Some(Item::Decl { ty, .. }) = m.items.iter().find(|i| matches!(i, Item::Decl { names, .. } if names.iter().any(|n| n.name == pd.decl.name))) {
                    let t2 = self.resolve_type(ty, sc)?;
                    if t2.w() >= base.w() {
                        base = t2;
                    }
                }
            }
            let ty = self.with_unpacked(base, &pd.decl.unpacked, sc)?;
            let id = self.new_sig(sc, &pd.decl.name, ty.clone());
            if let Some(init) = &pd.decl.init {
                let n = self.eval_to_type(init, &ty, sc, &mut super::exec::Env::default())?;
                if let Some(b) = self.dag.as_const(n) {
                    self.sigs[id as usize].init = b.to_vec();
                }
            }
            if top && pd.dir == Dir::In {
                for b in self.sigs[id as usize].drv.iter_mut() {
                    *b = Drv::Input;
                }
            }
            ports.push((pd.decl.name.clone(), pd.dir, id));
        }
        // The body; a non-ANSI redeclaration of a port is not a new signal.
        let port_set: std::collections::HashSet<&str> = if m.port_names.is_empty() { Default::default() } else { m.port_names.iter().map(|s| s.as_str()).collect() };
        let mut extra: Vec<Item> = Vec::new();
        let mut keep: Vec<&Item> = Vec::new();
        for it in &body {
            if let Item::Decl { ty, names, line } = it {
                if names.iter().any(|n| port_set.contains(n.name.as_str())) {
                    let rest: Vec<Declarator> = names.iter().filter(|n| !port_set.contains(n.name.as_str())).cloned().collect();
                    if !rest.is_empty() {
                        extra.push(Item::Decl { ty: ty.clone(), names: rest, line: *line });
                    }
                    continue;
                }
            }
            keep.push(it);
        }
        keep.extend(extra.iter());
        self.items_ref(&keep, sc)?;
        Ok(ports)
    }

    fn pick_top(&self, top: Option<&str>) -> R<Rc<Module>> {
        if let Some(t) = top {
            return self.modules.get(t).cloned().ok_or_else(|| match self.broken.iter().find(|b| b.0 == t) {
                Some((_, why)) => format!("top module '{t}' cannot be synthesized: {why}"),
                None => format!("top module '{t}' is not defined"),
            });
        }
        if self.modules.is_empty() {
            if let Some((name, why)) = self.broken.last() {
                return Err(format!("module '{name}' cannot be synthesized: {why}"));
            }
        }
        fn insts(items: &[Item], out: &mut Vec<String>) {
            for it in items {
                match it {
                    Item::Inst(d) => out.push(d.module.clone()),
                    Item::GenFor { body, .. } => insts(&body.items, out),
                    Item::GenIf { then, els, .. } => {
                        insts(&then.items, out);
                        if let Some(e) = els {
                            insts(&e.items, out);
                        }
                    }
                    Item::GenCase { arms, .. } => {
                        for (_, b) in arms {
                            insts(&b.items, out);
                        }
                    }
                    Item::Block(b) => insts(&b.items, out),
                    _ => {}
                }
            }
        }
        let mut children: HashMap<&str, Vec<String>> = HashMap::new();
        let mut used = Vec::new();
        for (name, m) in &self.modules {
            let mut v = Vec::new();
            insts(&m.items, &mut v);
            used.extend(v.iter().cloned());
            children.insert(name.as_str(), v);
        }
        // The uninstantiated module with the most modules below it; the
        // last one in the source on a tie.
        fn reach<'a>(n: &'a str, children: &'a HashMap<&str, Vec<String>>, seen: &mut std::collections::HashSet<&'a str>) {
            if !seen.insert(n) {
                return;
            }
            if let Some(c) = children.get(n) {
                for x in c {
                    reach(x.as_str(), children, seen);
                }
            }
        }
        let mut best: Option<(usize, &String)> = None;
        for n in &self.order {
            if self.modules[n].is_interface || used.contains(n) {
                continue;
            }
            let mut seen = Default::default();
            reach(n.as_str(), &children, &mut seen);
            if best.map(|b| seen.len() >= b.0).unwrap_or(true) {
                best = Some((seen.len(), n));
            }
        }
        best.map(|b| b.1)
            .or(self.order.last())
            .map(|n| self.modules[n].clone())
            .ok_or_else(|| "no module in the source".to_string())
    }

    /// Elaborate the design from `top` and return it flat.
    pub fn elaborate(mut self, top: Option<&str>) -> R<(Flat, Vec<String>)> {
        let m = self.pick_top(top)?;
        let sc = self.new_scope(self.unit_scope, String::new());
        let ports = self.module_body(&m, sc, &HashMap::new(), true)?;
        let mut fp = Vec::new();
        for (name, dir, sig) in ports {
            let w = self.sigs[sig as usize].ty.w();
            match dir {
                Dir::In => fp.push(Port { name, dir: PortDir::In, w, node: None, sig: Some(sig) }),
                Dir::Out => {
                    let n = self.dag.read(sig, w);
                    fp.push(Port { name, dir: PortDir::Out, w, node: Some(n), sig: Some(sig) });
                }
                Dir::Inout => return Err(format!("inout port '{name}' on the top module is not supported")),
            }
        }
        let sigs = self
            .sigs
            .into_iter()
            .map(|s| {
                // HDL index of each bit, for plain vectors.
                let index = match &*s.ty {
                    Ty::Bits { w, lo, asc, .. } => Some((0..*w as i64).map(|b| if *asc { lo + *w as i64 - 1 - b } else { lo + b }).collect()),
                    _ => None,
                };
                SigInfo { name: s.name, hier: s.hier, index, w: s.drv.len() as u32, drv: s.drv, init: s.init, debug: s.debug }
            })
            .collect();
        Ok((Flat { top: m.name.clone(), dag: self.dag, sigs, ports: fp, regs: self.regs, latches: self.latches, instances: self.instances }, self.warnings))
    }
}
