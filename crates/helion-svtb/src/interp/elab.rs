//! Elaboration: namespaces, name lookup, type resolution, and class
//! specialization (parameterized classes get one [`ClassInfo`] per distinct
//! parameter set, each with its own static members).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::*;

/// Where a type name is being resolved: an optional class scope plus the
/// namespace of the code.
#[derive(Clone, Copy)]
pub(crate) struct TyScope {
    pub class: Option<ClassId>,
    pub ns: NsId,
}

impl Ctx {
    pub(crate) fn tyscope(&self) -> TyScope {
        TyScope { class: self.class, ns: self.ns }
    }
}

pub(crate) enum ScopeHit {
    Ns(NsId),
    Class(ClassId),
}

fn int_width(kind: IntKind) -> (u32, bool) {
    match kind {
        IntKind::Bit | IntKind::Logic | IntKind::Reg => (1, false),
        IntKind::Int => (32, true),
        IntKind::Integer => (32, true),
        IntKind::Byte => (8, true),
        IntKind::Shortint => (16, true),
        IntKind::Longint => (64, true),
        IntKind::Time => (64, false),
    }
}

pub(crate) fn ty_key(t: &Ty) -> String {
    match t {
        Ty::Int { w, s } => format!("{}{w}", if *s { "s" } else { "u" }),
        Ty::Real => "real".into(),
        Ty::Str => "string".into(),
        Ty::Event => "event".into(),
        Ty::Void => "void".into(),
        Ty::Chandle => "chandle".into(),
        Ty::Class(c) => format!("C{c}"),
        Ty::Builtin(b) => format!("{b:?}"),
        Ty::VIf(n) => format!("vif:{n}"),
        Ty::Enum(e) => format!("enum:{}", e.name),
        Ty::Arr(e, k, n) => format!("{}[{k:?}{n}]", ty_key(e)),
        Ty::Assoc(e, s) => format!("{}[{}]", ty_key(e), if *s { "string" } else { "int" }),
    }
}

impl Engine<'_> {
    pub(crate) fn ty_name(&self, t: &Ty) -> String {
        match t {
            Ty::Class(c) => self.classes[*c as usize].name.clone(),
            Ty::Int { w: 32, s: true } => "int".into(),
            Ty::Int { w, s } => format!("bit{}[{}:0]", if *s { " signed" } else { "" }, w - 1),
            Ty::Enum(e) => e.name.clone(),
            Ty::VIf(n) => format!("virtual {n}"),
            other => ty_key(other),
        }
    }

    // ---- namespace registration --------------------------------------------

    pub(crate) fn add_ns(&mut self, name: &str, kind: NsKind, parent: Option<NsId>) -> NsId {
        self.nss.push(Namespace::new(name, kind, parent));
        self.nss.len() - 1
    }

    /// Register declarations of a package / `$unit` (no evaluation yet).
    pub(crate) fn register_items(&mut self, ns: NsId, items: &[PkgItem]) {
        for it in items {
            match it {
                PkgItem::Class(c) => {
                    self.nss[ns].classes.insert(c.name.clone(), c.clone());
                }
                PkgItem::Sub(s) => match &s.class_scope {
                    Some(cls) => {
                        self.nss[ns].externs.insert((cls.clone(), s.name.clone()), s.clone());
                    }
                    None => {
                        self.nss[ns].subs.insert(s.name.clone(), s.clone());
                    }
                },
                PkgItem::Typedef(t) => {
                    if t.ty.is_some() {
                        self.nss[ns].typedefs.insert(t.name.clone(), t.clone());
                    }
                }
                PkgItem::Import(p, item) => {
                    if let Some(&pid) = self.pkgs.get(p) {
                        match item {
                            None => self.nss[ns].imports.borrow_mut().push(pid),
                            Some(i) => self.nss[ns].item_imports.borrow_mut().push((pid, i.clone())),
                        }
                    }
                }
                PkgItem::Var(_) | PkgItem::Param(_) => {}
            }
        }
    }

    /// Evaluate package-level parameters, enum constants and variables.
    pub(crate) fn init_ns_items(&mut self, ns: NsId, items: &[PkgItem]) -> Result<(), SvError> {
        let ctx = Ctx { scopes: vec![], this: None, class: None, ns };
        for it in items {
            match it {
                PkgItem::Param(p) => {
                    let (ty, v) = self.param_value(&ctx, p)?;
                    if let Some(v) = v {
                        self.nss[ns].vars.borrow_mut().insert(p.name.clone(), Var { ty, v });
                    } else {
                        self.nss[ns].ty_cache.borrow_mut().insert(p.name.clone(), ty);
                    }
                }
                PkgItem::Typedef(t) => {
                    if let Some(DataType::Enum { .. }) = &t.ty {
                        let ty = self.resolve_type(ctx.tyscope(), t.ty.as_ref().unwrap(), &t.udims, Some(&t.name))?;
                        self.nss[ns].ty_cache.borrow_mut().insert(t.name.clone(), ty.clone());
                        self.define_enum_consts_ns(ns, &ty);
                    }
                }
                PkgItem::Var(d) => {
                    if let DataType::Enum { .. } = &d.ty {
                        let ty = self.resolve_type(ctx.tyscope(), &d.ty, &[], None)?;
                        self.define_enum_consts_ns(ns, &ty);
                    }
                    let ty = self.resolve_type(ctx.tyscope(), &d.ty, &d.udims, None)?;
                    let v = match &d.init {
                        Some(e) => self.eval_to(&ctx, e, &ty)?,
                        None => ty.default_value(),
                    };
                    self.nss[ns].vars.borrow_mut().insert(d.name.clone(), Var { ty, v });
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn define_enum_consts_ns(&mut self, ns: NsId, ty: &Ty) {
        if let Ty::Enum(e) = ty {
            for (n, v) in &e.items {
                self.nss[ns].vars.borrow_mut().insert(n.clone(), Var { ty: ty.clone(), v: Value::Int(*v, e.w, false) });
            }
        }
    }

    /// `(type, Some(value))` for a value parameter, `(type, None)` for a type parameter.
    pub(crate) fn param_value(&mut self, ctx: &Ctx, p: &ParamDecl) -> Result<(Ty, Option<Value>), SvError> {
        if p.is_type {
            let ty = match &p.default {
                Some(ParamArg::Type(t)) => self.resolve_type(ctx.tyscope(), t, &[], None)?,
                Some(ParamArg::Expr(e)) => self.expr_as_type(ctx.tyscope(), e)?,
                _ => Ty::int(32, true),
            };
            return Ok((ty, None));
        }
        let e = match &p.default {
            Some(ParamArg::Expr(e)) => e.clone(),
            _ => return Err(self.rt(format!("parameter `{}` needs a value", p.name))),
        };
        let v = self.eval(ctx, &e)?;
        let ty = match &p.ty {
            Some(t) => self.resolve_type(ctx.tyscope(), t, &[], None)?,
            None => match &v {
                Value::Str(_) => Ty::Str,
                Value::Real(_) => Ty::Real,
                Value::Int(_, w, s) => Ty::int(*w, *s),
                _ => Ty::int(32, true),
            },
        };
        let v = convert(v, &ty);
        Ok((ty, Some(v)))
    }

    // ---- lookups ------------------------------------------------------------------

    pub(crate) fn class_chain(&self, c: ClassId) -> Vec<ClassId> {
        let mut v = vec![c];
        let mut cur = self.classes[c as usize].parent.get();
        while let Some(p) = cur {
            v.push(p);
            cur = self.classes[p as usize].parent.get();
        }
        v
    }

    pub(crate) fn is_subclass(&self, a: ClassId, b: ClassId) -> bool {
        self.class_chain(a).contains(&b)
    }

    /// Variable / constant lookup from `ctx`.
    pub(crate) fn lookup_var(&self, ctx: &Ctx, name: &str) -> Option<Place> {
        for s in ctx.scopes.iter().rev() {
            if s.borrow().contains_key(name) {
                return Some(Place::Local(s.clone(), name.to_string()));
            }
        }
        if let Some(c) = ctx.class {
            if let Some(o) = &ctx.this {
                if o.borrow().fields.contains_key(name) {
                    return Some(Place::Field(o.clone(), name.to_string()));
                }
            }
            let mut cls = Some(c);
            while let Some(k) = cls {
                for kk in self.class_chain(k) {
                    let info = &self.classes[kk as usize];
                    if info.statics.borrow().contains_key(name) {
                        return Some(Place::Static(kk, name.to_string()));
                    }
                    if let Some(ParamVal::Val(v)) = info.params.get(name) {
                        let ty = match v {
                            Value::Str(_) => Ty::Str,
                            Value::Int(_, w, s) => Ty::int(*w, *s),
                            _ => Ty::int(32, true),
                        };
                        return Some(Place::Const(v.clone(), ty));
                    }
                }
                cls = self.classes[k as usize].outer;
            }
        }
        self.ns_lookup_var(ctx.ns, name, 0)
    }

    fn ns_lookup_local(&self, ns: NsId, name: &str) -> Option<Place> {
        let n = &self.nss[ns];
        if n.vars.borrow().contains_key(name) {
            return Some(Place::NsVar(ns, name.to_string()));
        }
        if let Some(&id) = n.sigs.borrow().get(name) {
            return Some(Place::Sig(id));
        }
        if let Some(i) = n.ifaces.borrow().get(name) {
            return Some(Place::Const(Value::VIf(Some(i.clone())), Ty::VIf(i.decl.name.as_str().into())));
        }
        None
    }

    pub(crate) fn ns_lookup_var(&self, ns: NsId, name: &str, depth: usize) -> Option<Place> {
        if depth > 16 {
            return None;
        }
        if let Some(p) = self.ns_lookup_local(ns, name) {
            return Some(p);
        }
        let n = &self.nss[ns];
        for (pk, item) in n.item_imports.borrow().iter() {
            if item == name {
                if let Some(p) = self.ns_lookup_local(*pk, name) {
                    return Some(p);
                }
            }
        }
        for pk in n.imports.borrow().iter() {
            if let Some(p) = self.ns_lookup_local(*pk, name) {
                return Some(p);
            }
        }
        n.parent.and_then(|p| self.ns_lookup_var(p, name, depth + 1))
    }

    /// Method `name` visible from class `c` (walking parents): (defining class, decl).
    pub(crate) fn find_method(&self, c: ClassId, name: &str) -> Option<(ClassId, Rc<SubDecl>)> {
        for k in self.class_chain(c) {
            if let Some(s) = self.classes[k as usize].methods.get(name) {
                return Some((k, s.clone()));
            }
        }
        None
    }

    pub(crate) fn method_is_virtual(&self, c: ClassId, name: &str) -> bool {
        self.class_chain(c).into_iter().any(|k| self.classes[k as usize].methods.get(name).is_some_and(|s| s.quals.is_virtual))
    }

    pub(crate) fn ns_lookup_sub(&self, ns: NsId, name: &str, depth: usize) -> Option<(NsId, Rc<SubDecl>)> {
        if depth > 16 {
            return None;
        }
        let n = &self.nss[ns];
        if let Some(s) = n.subs.get(name) {
            return Some((ns, s.clone()));
        }
        for (pk, item) in n.item_imports.borrow().iter() {
            if item == name {
                if let Some(s) = self.nss[*pk].subs.get(name) {
                    return Some((*pk, s.clone()));
                }
            }
        }
        for pk in n.imports.borrow().iter() {
            if let Some(s) = self.nss[*pk].subs.get(name) {
                return Some((*pk, s.clone()));
            }
        }
        n.parent.and_then(|p| self.ns_lookup_sub(p, name, depth + 1))
    }

    // ---- types ---------------------------------------------------------------------

    pub(crate) fn const_u64(&mut self, sc: TyScope, e: &Expr) -> Result<i64, SvError> {
        let ctx = Ctx { scopes: vec![], this: None, class: sc.class, ns: sc.ns };
        Ok(self.eval(&ctx, e)?.as_i64())
    }

    pub(crate) fn resolve_type(&mut self, sc: TyScope, dt: &DataType, udims: &[UDim], name: Option<&str>) -> Result<Ty, SvError> {
        let mut base = match dt {
            DataType::Int { kind, signed, packed } => {
                let (mut w, mut s) = int_width(*kind);
                if !packed.is_empty() {
                    w = 1;
                    for (a, b) in packed {
                        let a = self.const_u64(sc, a)?;
                        let b = self.const_u64(sc, b)?;
                        w *= ((a - b).unsigned_abs() + 1) as u32;
                    }
                    if matches!(kind, IntKind::Int | IntKind::Integer | IntKind::Byte | IntKind::Shortint | IntKind::Longint) {
                        return Err(self.rt("packed dimensions on a predefined integer type are not allowed"));
                    }
                }
                if let Some(sg) = signed {
                    s = *sg;
                }
                if w > 64 {
                    return Err(self.rt(format!("{w}-bit vectors are wider than the 64-bit testbench value limit")));
                }
                Ty::Int { w, s }
            }
            DataType::Implicit { signed, packed } => {
                let mut w = 1u32;
                for (a, b) in packed {
                    let a = self.const_u64(sc, a)?;
                    let b = self.const_u64(sc, b)?;
                    w *= ((a - b).unsigned_abs() + 1) as u32;
                }
                if w > 64 {
                    return Err(self.rt(format!("{w}-bit vectors are wider than the 64-bit testbench value limit")));
                }
                Ty::Int { w, s: *signed }
            }
            DataType::Real => Ty::Real,
            DataType::Str => Ty::Str,
            DataType::Event => Ty::Event,
            DataType::Void => Ty::Void,
            DataType::Chandle => Ty::Chandle,
            DataType::Virtual { iface, .. } => {
                if !self.modules.get(iface).is_some_and(|m| m.is_interface) {
                    return Err(self.rt(format!("unknown interface `{iface}`")));
                }
                Ty::VIf(iface.as_str().into())
            }
            DataType::Enum { base, items } => {
                let bt = self.resolve_type(sc, base, &[], None)?;
                let w = bt.width();
                let mut vals = Vec::new();
                let mut next = 0u64;
                for (n, v) in items {
                    let val = match v {
                        Some(e) => self.const_u64(sc, e)? as u64,
                        None => next,
                    };
                    vals.push((n.clone(), mask(val, w)));
                    next = val.wrapping_add(1);
                }
                Ty::Enum(Rc::new(EnumInfo { name: name.unwrap_or("enum").to_string(), w, items: vals }))
            }
            DataType::Struct { .. } => return Err(self.rt("structs are not supported by the class-based testbench engine; use a class")),
            DataType::Named { path } => self.resolve_named(sc, path)?,
        };
        for d in udims.iter().rev() {
            base = match d {
                UDim::Fixed(a, b) => {
                    let n = match b {
                        None => self.const_u64(sc, a)? as usize,
                        Some(b) => {
                            let a = self.const_u64(sc, a)?;
                            let b = self.const_u64(sc, b)?;
                            ((a - b).unsigned_abs() + 1) as usize
                        }
                    };
                    Ty::Arr(Rc::new(base), ArrKind::Fixed, n)
                }
                UDim::Dynamic => Ty::Arr(Rc::new(base), ArrKind::Dynamic, 0),
                UDim::Queue(_) => Ty::Arr(Rc::new(base), ArrKind::Queue, 0),
                UDim::Assoc(k) => {
                    let s = matches!(k, Some(DataType::Str));
                    Ty::Assoc(Rc::new(base), s)
                }
            };
        }
        Ok(base)
    }

    /// A bare identifier / scoped name used as a type parameter value.
    pub(crate) fn expr_as_type(&mut self, sc: TyScope, e: &Expr) -> Result<Ty, SvError> {
        match e {
            Expr::Ident(n) => self.resolve_named(sc, &[Seg { name: n.clone(), params: None }]),
            Expr::Scoped(p) => self.resolve_named(sc, p),
            Expr::Type(t) => self.resolve_type(sc, t, &[], None),
            _ => Err(self.rt("expected a type")),
        }
    }

    /// Resolve the scope named by `segs` (all but the member): a package or a class.
    pub(crate) fn resolve_scope(&mut self, sc: TyScope, segs: &[Seg]) -> Result<ScopeHit, SvError> {
        let first = &segs[0];
        let mut cur = if first.params.is_none() && self.pkgs.contains_key(&first.name) && self.lookup_type_name(sc, &first.name).is_none() {
            ScopeHit::Ns(self.pkgs[&first.name])
        } else {
            match self.resolve_named(sc, std::slice::from_ref(first))? {
                Ty::Class(c) => ScopeHit::Class(c),
                Ty::Builtin(_) => return Err(self.rt(format!("`{}` has no static members", first.name))),
                _ => return Err(self.rt(format!("`{}` is not a class or package", first.name))),
            }
        };
        for s in &segs[1..] {
            let sub = match &cur {
                ScopeHit::Ns(n) => TyScope { class: None, ns: *n },
                ScopeHit::Class(c) => TyScope { class: Some(*c), ns: self.classes[*c as usize].ns },
            };
            let t = match &cur {
                ScopeHit::Ns(n) => {
                    let n = *n;
                    self.resolve_in_ns_only(n, s, sc)?
                }
                ScopeHit::Class(_) => self.resolve_named_with_args(sub, s, sc)?,
            };
            cur = match t {
                Ty::Class(c) => ScopeHit::Class(c),
                _ => return Err(self.rt(format!("`{}` is not a class", s.name))),
            };
        }
        Ok(cur)
    }

    fn resolve_in_ns_only(&mut self, ns: NsId, s: &Seg, argsc: TyScope) -> Result<Ty, SvError> {
        if let Some(d) = self.nss[ns].classes.get(&s.name).cloned() {
            return self.specialize(d, ns, None, s.params.as_deref(), argsc).map(Ty::Class);
        }
        if let Some(t) = self.nss[ns].ty_cache.borrow().get(&s.name).cloned() {
            return Ok(t);
        }
        if let Some(td) = self.nss[ns].typedefs.get(&s.name).cloned() {
            let ty = self.resolve_type(TyScope { class: None, ns }, td.ty.as_ref().unwrap(), &td.udims, Some(&s.name))?;
            self.nss[ns].ty_cache.borrow_mut().insert(s.name.clone(), ty.clone());
            return Ok(ty);
        }
        Err(self.rt(format!("package `{}` has no type `{}`", self.nss[ns].name, s.name)))
    }

    pub(crate) fn resolve_named(&mut self, sc: TyScope, path: &[Seg]) -> Result<Ty, SvError> {
        if path.len() == 1 {
            return self.resolve_named_with_args(sc, &path[0], sc);
        }
        let scope = self.resolve_scope(sc, &path[..path.len() - 1])?;
        let last = &path[path.len() - 1];
        match scope {
            ScopeHit::Ns(n) => self.resolve_in_ns_only(n, last, sc),
            ScopeHit::Class(c) => {
                let inner = TyScope { class: Some(c), ns: self.classes[c as usize].ns };
                self.resolve_named_with_args(inner, last, sc)
            }
        }
    }

    /// Kinds of named things a type name can hit.
    pub(crate) fn lookup_type_name(&self, sc: TyScope, name: &str) -> Option<()> {
        let mut cls = sc.class;
        while let Some(k) = cls {
            for kk in self.class_chain(k) {
                let info = &self.classes[kk as usize];
                if matches!(info.params.get(name), Some(ParamVal::Type(_)))
                    || info.typedefs.borrow().contains_key(name)
                    || info.decl.items.iter().any(|i| matches!(i, ClassItem::Typedef(t) if t.name == name) || matches!(i, ClassItem::Class(c) if c.name == name))
                {
                    return Some(());
                }
            }
            cls = self.classes[k as usize].outer;
        }
        None
    }

    /// Resolve one segment `name#(params)` from scope `sc`; parameter
    /// arguments resolve in `argsc`.
    pub(crate) fn resolve_named_with_args(&mut self, sc: TyScope, seg: &Seg, argsc: TyScope) -> Result<Ty, SvError> {
        let name = seg.name.as_str();
        // Class scope: type params, typedefs, nested classes (walk parents/outer).
        let mut cls = sc.class;
        while let Some(k) = cls {
            for kk in self.class_chain(k) {
                let info = self.classes[kk as usize].clone();
                if let Some(ParamVal::Type(t)) = info.params.get(name) {
                    return Ok(t.clone());
                }
                if let Some(t) = info.typedefs.borrow().get(name).cloned() {
                    return Ok(t);
                }
                for it in &info.decl.items {
                    match it {
                        ClassItem::Typedef(t) if t.name == name && t.ty.is_some() => {
                            let isc = TyScope { class: Some(kk), ns: info.ns };
                            let ty = self.resolve_type(isc, t.ty.as_ref().unwrap(), &t.udims, Some(name))?;
                            info.typedefs.borrow_mut().insert(name.to_string(), ty.clone());
                            return Ok(ty);
                        }
                        ClassItem::Class(d) if d.name == name => {
                            return self.specialize(d.clone(), info.ns, Some(kk), seg.params.as_deref(), argsc).map(Ty::Class);
                        }
                        _ => {}
                    }
                }
            }
            cls = self.classes[k as usize].outer;
        }
        match name {
            "mailbox" => return Ok(Ty::Builtin(Builtin::Mailbox)),
            "semaphore" => return Ok(Ty::Builtin(Builtin::Semaphore)),
            "process" => return Ok(Ty::Builtin(Builtin::Process)),
            _ => {}
        }
        // Namespace chain.
        let mut ns = Some(sc.ns);
        let mut depth = 0;
        while let Some(n) = ns {
            depth += 1;
            if depth > 16 {
                break;
            }
            let mut cands = vec![n];
            for (pk, item) in self.nss[n].item_imports.borrow().iter() {
                if item == name {
                    cands.push(*pk);
                }
            }
            cands.extend(self.nss[n].imports.borrow().iter().copied());
            for c in cands {
                if let Some(d) = self.nss[c].classes.get(name).cloned() {
                    return self.specialize(d, c, None, seg.params.as_deref(), argsc).map(Ty::Class);
                }
                if let Some(t) = self.nss[c].ty_cache.borrow().get(name).cloned() {
                    return Ok(t);
                }
                if let Some(td) = self.nss[c].typedefs.get(name).cloned() {
                    let ty = self.resolve_type(TyScope { class: None, ns: c }, td.ty.as_ref().unwrap(), &td.udims, Some(name))?;
                    self.nss[c].ty_cache.borrow_mut().insert(name.to_string(), ty.clone());
                    return Ok(ty);
                }
            }
            ns = self.nss[n].parent;
        }
        if self.modules.get(name).is_some_and(|m| m.is_interface) {
            return Ok(Ty::VIf(name.into()));
        }
        Err(self.rt(format!("unknown type `{name}`")))
    }

    // ---- class specialization --------------------------------------------------------

    pub(crate) fn specialize(
        &mut self,
        decl: Rc<ClassDecl>,
        ns: NsId,
        outer: Option<ClassId>,
        args: Option<&[ParamArg]>,
        argsc: TyScope,
    ) -> Result<ClassId, SvError> {
        // Bind parameters (positional, named, defaults).
        let ports: Vec<&ParamDecl> = decl.params.iter().filter(|p| !p.local).collect();
        let mut bound: Vec<(String, ParamVal)> = Vec::new();
        let args = args.unwrap_or(&[]);
        let mut pos = 0usize;
        let argctx = Ctx { scopes: vec![], this: None, class: argsc.class, ns: argsc.ns };
        for p in &ports {
            let mut given: Option<&ParamArg> = None;
            for a in args {
                if let ParamArg::Named(n, v) = a {
                    if *n == p.name {
                        given = Some(v);
                    }
                }
            }
            if given.is_none() {
                while pos < args.len() && matches!(args[pos], ParamArg::Named(..)) {
                    pos += 1;
                }
                if pos < args.len() {
                    given = Some(&args[pos]);
                    pos += 1;
                }
            }
            let val = match given {
                Some(a) => {
                    if p.is_type {
                        ParamVal::Type(match a {
                            ParamArg::Type(t) => self.resolve_type(argsc, t, &[], None)?,
                            ParamArg::Expr(e) => self.expr_as_type(argsc, e)?,
                            ParamArg::Named(..) => unreachable!(),
                        })
                    } else {
                        let e = match a {
                            ParamArg::Expr(e) => e.clone(),
                            _ => return Err(self.rt(format!("parameter `{}` of `{}` needs a value, not a type", p.name, decl.name))),
                        };
                        let v = self.eval(&argctx, &e)?;
                        ParamVal::Val(v)
                    }
                }
                None => {
                    // Defaults resolve in the class's declaring scope.
                    let dctx = Ctx { scopes: vec![], this: None, class: outer, ns };
                    let (ty, v) = self.param_value(&dctx, p)?;
                    match v {
                        Some(v) => ParamVal::Val(v),
                        None => ParamVal::Type(ty),
                    }
                }
            };
            bound.push((p.name.clone(), val));
        }
        let mut key = format!("{}:{}", decl.uid, decl.name);
        let mut disp = decl.name.clone();
        if !bound.is_empty() {
            let parts: Vec<String> = bound
                .iter()
                .map(|(_, v)| match v {
                    ParamVal::Type(t) => ty_key(t),
                    ParamVal::Val(v) => crate::value::show(v),
                })
                .collect();
            key.push_str(&format!("#({})", parts.join(",")));
            let dparts: Vec<String> = bound
                .iter()
                .map(|(_, v)| match v {
                    ParamVal::Type(t) => self.ty_name(t),
                    ParamVal::Val(v) => crate::value::show(v),
                })
                .collect();
            disp.push_str(&format!("#({})", dparts.join(",")));
        }
        if let Some(o) = outer {
            key = format!("{o}::{key}");
        }
        if let Some(&id) = self.class_keys.get(&key) {
            return Ok(id);
        }
        // Methods (+ out-of-class bodies).
        let mut methods = HashMap::new();
        let mut constraints = Vec::new();
        for it in &decl.items {
            match it {
                ClassItem::Method(m) => {
                    let body = if m.quals.is_extern {
                        self.find_extern(ns, &decl.name, &m.name).map(|ext| {
                            let mut s = (*ext).clone();
                            s.quals = m.quals.clone();
                            s.quals.is_extern = false;
                            if s.args.is_empty() && !m.args.is_empty() {
                                s.args = m.args.clone();
                            }
                            Rc::new(s)
                        })
                    } else {
                        Some(m.clone())
                    };
                    match body {
                        Some(b) => {
                            methods.insert(m.name.clone(), b);
                        }
                        None => return Err(self.err_at(m.loc, format!("extern method `{}::{}` has no body", decl.name, m.name))),
                    }
                }
                ClassItem::Constraint(c) => constraints.push(c.clone()),
                _ => {}
            }
        }
        let ctor = methods.get("new").cloned().map(|sub| {
            let mut super_args = None;
            let mut rest: Vec<S> = sub.body.to_vec();
            if let Some(first) = rest.first() {
                if let SK::Expr(Expr::Call { f, args, loc, .. }) = &first.k {
                    if let Expr::Member(b, n) = f.as_ref() {
                        if matches!(b.as_ref(), Expr::Super) && n == "new" {
                            super_args = Some((args.clone(), *loc));
                            rest.remove(0);
                        }
                    }
                }
            }
            Ctor { sub, super_args, rest: Rc::from(rest) }
        });
        let id = self.classes.len() as ClassId;
        let info = Rc::new(ClassInfo {
            id,
            name: disp,
            decl: decl.clone(),
            parent: std::cell::Cell::new(None),
            params: bound.into_iter().collect(),
            ns,
            outer,
            statics: RefCell::new(HashMap::new()),
            methods,
            typedefs: RefCell::new(HashMap::new()),
            fields: RefCell::new(None),
            ctor,
            constraints,
        });
        self.classes.push(info.clone());
        self.class_keys.insert(key, id);
        let sc = TyScope { class: Some(id), ns };
        if let Some(ext) = &decl.extends {
            let saved = self.cur_loc;
            self.cur_loc = decl.loc;
            let pt = match ext {
                DataType::Named { path } => {
                    // Resolve the base without looking in this class's own
                    // typedefs (which may name the base itself).
                    let psc = TyScope { class: None, ns };
                    if path.len() == 1 {
                        let mut s = path[0].clone();
                        // Type params of this class are visible to `extends`.
                        if let Some(ParamVal::Type(t)) = info.params.get(&s.name) {
                            t.clone()
                        } else {
                            if outer.is_some() {
                                s = s.clone();
                            }
                            let osc = TyScope { class: outer, ns };
                            let _ = psc;
                            self.resolve_named_with_args(osc, &s, sc)?
                        }
                    } else {
                        self.resolve_named(sc, path)?
                    }
                }
                _ => return Err(self.err_at(decl.loc, "a class can only extend a class")),
            };
            self.cur_loc = saved;
            match pt {
                Ty::Class(p) => info.parent.set(Some(p)),
                _ => return Err(self.err_at(decl.loc, format!("`{}` extends a non-class type", decl.name))),
            }
        }
        // Static members, class parameters, enum constants.
        let ctx = Ctx { scopes: vec![], this: None, class: Some(id), ns };
        for it in &decl.items {
            match it {
                ClassItem::Param(p) => {
                    let (ty, v) = self.param_value(&ctx, p)?;
                    match v {
                        Some(v) => {
                            info.statics.borrow_mut().insert(p.name.clone(), Var { ty, v });
                        }
                        None => {
                            info.typedefs.borrow_mut().insert(p.name.clone(), ty);
                        }
                    }
                }
                ClassItem::Typedef(t) => {
                    if let Some(DataType::Enum { .. }) = &t.ty {
                        let ty = self.resolve_type(sc, t.ty.as_ref().unwrap(), &t.udims, Some(&t.name))?;
                        info.typedefs.borrow_mut().insert(t.name.clone(), ty.clone());
                        if let Ty::Enum(e) = &ty {
                            for (n, v) in &e.items {
                                info.statics.borrow_mut().insert(n.clone(), Var { ty: ty.clone(), v: Value::Int(*v, e.w, false) });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for it in &decl.items {
            if let ClassItem::Prop(d) = it {
                if d.is_static {
                    self.cur_loc = d.loc;
                    let ty = self.resolve_type(sc, &d.ty, &d.udims, None)?;
                    info.statics.borrow_mut().insert(d.name.clone(), Var::new(ty));
                }
            }
        }
        for it in &decl.items {
            if let ClassItem::Prop(d) = it {
                if d.is_static {
                    if let Some(init) = &d.init {
                        self.cur_loc = d.loc;
                        let ty = info.statics.borrow()[&d.name].ty.clone();
                        let v = self.eval_to(&ctx, init, &ty)?;
                        info.statics.borrow_mut().get_mut(&d.name).unwrap().v = v;
                    }
                }
            }
        }
        // Eagerly resolve typedefs so registries in them self-register
        // (UVM factory registration via `typedef …_registry#(T) type_id`).
        for it in &decl.items {
            if let ClassItem::Typedef(t) = it {
                if let Some(ty) = &t.ty {
                    if !info.typedefs.borrow().contains_key(&t.name) {
                        self.cur_loc = decl.loc;
                        let r = self.resolve_type(sc, ty, &t.udims, Some(&t.name))?;
                        info.typedefs.borrow_mut().insert(t.name.clone(), r);
                    }
                }
            }
        }
        Ok(id)
    }

    fn find_extern(&self, ns: NsId, cls: &str, m: &str) -> Option<Rc<SubDecl>> {
        let mut n = Some(ns);
        while let Some(k) = n {
            if let Some(s) = self.nss[k].externs.get(&(cls.to_string(), m.to_string())) {
                return Some(s.clone());
            }
            n = self.nss[k].parent;
        }
        None
    }

    /// Instance properties of class `c` itself (not ancestors), resolved.
    pub(crate) fn own_fields(&mut self, c: ClassId) -> Result<Rc<Vec<(String, Ty, RandKind)>>, SvError> {
        let info = self.classes[c as usize].clone();
        if let Some(f) = info.fields.borrow().as_ref() {
            return Ok(f.clone());
        }
        let sc = TyScope { class: Some(c), ns: info.ns };
        let mut v = Vec::new();
        for it in &info.decl.items {
            if let ClassItem::Prop(d) = it {
                if !d.is_static {
                    self.cur_loc = d.loc;
                    let ty = self.resolve_type(sc, &d.ty, &d.udims, None)?;
                    v.push((d.name.clone(), ty, d.rand));
                }
            }
        }
        let rc = Rc::new(v);
        *info.fields.borrow_mut() = Some(rc.clone());
        Ok(rc)
    }

    /// Specialize every non-parameterized class of `ns` (runs static
    /// initializers and resolves typedefs).
    pub(crate) fn elab_classes(&mut self, ns: NsId) -> Result<(), SvError> {
        let mut decls: Vec<Rc<ClassDecl>> = self.nss[ns].classes.values().filter(|d| d.params.is_empty()).cloned().collect();
        decls.sort_by_key(|d| (d.loc.file, d.loc.line));
        for d in decls {
            let sc = TyScope { class: None, ns };
            self.specialize(d, ns, None, None, sc)?;
        }
        Ok(())
    }
}
