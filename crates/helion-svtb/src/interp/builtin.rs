//! System tasks/functions and built-in methods (strings, arrays, queues,
//! associative arrays, enums, mailboxes, semaphores).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::call::Recv;
use super::*;

fn glob(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => glob(&p[1..], s) || (!s.is_empty() && glob(p, &s[1..])),
        (Some(b'?'), Some(_)) => glob(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => glob(&p[1..], &s[1..]),
        _ => false,
    }
}

impl Engine<'_> {
    fn arg(&mut self, ctx: &Ctx, args: &[Option<Expr>], i: usize) -> Result<Option<Value>, SvError> {
        match args.get(i) {
            Some(Some(e)) => Ok(Some(self.eval(ctx, e)?)),
            _ => Ok(None),
        }
    }

    fn arg_req(&mut self, ctx: &Ctx, args: &[Option<Expr>], i: usize, f: &str) -> Result<Value, SvError> {
        self.arg(ctx, args, i)?.ok_or_else(|| self.rt(format!("{f}: missing argument {}", i + 1)))
    }

    pub(crate) fn urandom32(&mut self) -> u64 {
        self.next_rand() & 0xffff_ffff
    }

    pub(crate) fn sys_call(&mut self, ctx: &Ctx, name: &str, args: &[Option<Expr>], _stmt: bool) -> Result<Value, SvError> {
        let loc = self.cur_loc;
        let none = Value::Int(0, 1, false);
        match name {
            "$display" | "$displayb" | "$displayh" | "$displayo" | "$write" | "$writeb" | "$writeh" | "$info" | "$strobe" => {
                let s = self.format_args(ctx, args)?;
                self.res.displays += 1;
                self.log(Sev::Info, s);
                Ok(none)
            }
            "$warning" => {
                let s = self.format_args(ctx, args)?;
                let at = format!("{}:{}", self.label(loc.file), loc.line);
                self.log(Sev::Warning, format!("{s} ({at})"));
                Ok(none)
            }
            "$error" => {
                let s = self.format_args(ctx, args)?;
                let s = if s.is_empty() { "$error".into() } else { s };
                self.log(Sev::Error, s.clone());
                self.fail(loc, s);
                Ok(none)
            }
            "$fatal" => {
                let rest: Vec<Option<Expr>> = match args.first() {
                    Some(Some(Expr::Num { .. })) => args[1..].to_vec(),
                    _ => args.to_vec(),
                };
                let s = self.format_args(ctx, &rest)?;
                let s = if s.is_empty() { "$fatal".into() } else { s };
                self.log(Sev::Fatal, s.clone());
                self.fail(loc, s);
                self.finished = true;
                Ok(none)
            }
            "$finish" | "$stop" => {
                self.log(Sev::Info, format!("{name} called at {} ns", self.time / 1000));
                self.finished = true;
                self.res.finished = true;
                Ok(none)
            }
            "$time" | "$stime" => Ok(Value::Int(self.time / 1000, 64, false)),
            "$realtime" => Ok(Value::Real(self.time as f64 / 1000.0)),
            "$urandom" => Ok(Value::Int(self.urandom32(), 32, false)),
            "$random" => Ok(Value::Int(self.urandom32(), 32, true)),
            "$urandom_range" => {
                let a = self.arg_req(ctx, args, 0, name)?.as_u64() & 0xffff_ffff;
                let b = self.arg(ctx, args, 1)?.map(|v| v.as_u64() & 0xffff_ffff).unwrap_or(0);
                let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                let r = lo + self.next_rand() % (hi - lo + 1);
                Ok(Value::Int(r, 32, false))
            }
            "$sformatf" | "$psprintf" => Ok(Value::Str(self.format_args(ctx, args)?)),
            "$sformat" | "$swrite" => {
                let dst = match args.first() {
                    Some(Some(e)) => self.lpath(ctx, e)?,
                    _ => return Err(self.rt("$sformat needs a destination")),
                };
                let s = self.format_args(ctx, &args[1..])?;
                self.write_path(&dst, Value::Str(s))?;
                Ok(none)
            }
            "$bits" => match args.first() {
                Some(Some(Expr::Type(t))) => {
                    let ty = self.resolve_type(ctx.tyscope(), t, &[], None)?;
                    Ok(Value::int(ty.width() as i64))
                }
                Some(Some(e)) => {
                    if let Ok(t) = self.expr_as_type(ctx.tyscope(), e) {
                        if !matches!(e, Expr::Ident(n) if self.lookup_var(ctx, n).is_some()) {
                            return Ok(Value::int(t.width() as i64));
                        }
                    }
                    let p = self.lpath(ctx, e)?;
                    let (v, t) = self.read_path(&p)?;
                    Ok(Value::int(match (&v, &t) {
                        (Value::Arr(a), Ty::Arr(el, ..)) => a.items.len() as i64 * el.width() as i64,
                        _ => t.width().max(v.width().min(t.width())) as i64,
                    }))
                }
                _ => Err(self.rt("$bits needs an argument")),
            },
            "$size" | "$high" | "$low" | "$left" | "$right" => {
                let v = self.arg_req(ctx, args, 0, name)?;
                let n = match &v {
                    Value::Arr(a) => a.items.len() as i64,
                    Value::Assoc(m) => m.len() as i64,
                    other => other.width() as i64,
                };
                Ok(Value::int(match name {
                    "$size" => n,
                    "$high" | "$left" => n - 1,
                    _ => 0,
                }))
            }
            "$clog2" => {
                let n = self.arg_req(ctx, args, 0, name)?.as_u64();
                Ok(Value::int(if n <= 1 { 0 } else { 64 - (n - 1).leading_zeros() as i64 }))
            }
            "$signed" => Ok(match self.arg_req(ctx, args, 0, name)? {
                Value::Int(x, w, _) => Value::Int(x, w, true),
                o => o,
            }),
            "$unsigned" => Ok(match self.arg_req(ctx, args, 0, name)? {
                Value::Int(x, w, _) => Value::Int(x, w, false),
                o => o,
            }),
            "$countones" => Ok(Value::int(self.arg_req(ctx, args, 0, name)?.as_u64().count_ones() as i64)),
            "$onehot" => Ok(Value::bit(self.arg_req(ctx, args, 0, name)?.as_u64().count_ones() == 1)),
            "$onehot0" => Ok(Value::bit(self.arg_req(ctx, args, 0, name)?.as_u64().count_ones() <= 1)),
            "$isunknown" => Ok(Value::bit(false)),
            "$itor" => Ok(Value::Real(self.arg_req(ctx, args, 0, name)?.as_f64())),
            "$rtoi" => Ok(Value::Int(self.arg_req(ctx, args, 0, name)?.as_f64().trunc() as i64 as u64, 32, true)),
            "$floor" => Ok(Value::Real(self.arg_req(ctx, args, 0, name)?.as_f64().floor())),
            "$ceil" => Ok(Value::Real(self.arg_req(ctx, args, 0, name)?.as_f64().ceil())),
            "$typename" => {
                let v = self.arg_req(ctx, args, 0, name)?;
                Ok(Value::Str(match &v {
                    Value::Obj(Some(o)) => o.borrow().class.map(|c| self.classes[c as usize].name.clone()).unwrap_or_else(|| "object".into()),
                    Value::Str(_) => "string".into(),
                    Value::Int(_, w, s) => format!("{}[{}:0]", if *s { "int" } else { "bit" }, w - 1),
                    _ => "unknown".into(),
                }))
            }
            "$cast" => {
                let dst = match args.first() {
                    Some(Some(e)) => self.lpath(ctx, e)?,
                    _ => return Err(self.rt("$cast needs a destination")),
                };
                let src = self.arg_req(ctx, args, 1, name)?;
                let (_, dty) = self.read_path(&dst)?;
                let ok = match (&dty, &src) {
                    (Ty::Class(_), Value::Obj(None)) => true,
                    (Ty::Class(c), Value::Obj(Some(o))) => o.borrow().class.is_some_and(|oc| self.is_subclass(oc, *c)),
                    (Ty::Enum(e), v) => e.items.iter().any(|(_, x)| *x == v.as_u64()),
                    (Ty::Class(_), _) => false,
                    _ => true,
                };
                if ok {
                    self.write_path(&dst, src)?;
                }
                Ok(Value::int(ok as i64))
            }
            "$test$plusargs" => {
                let s = self.arg_req(ctx, args, 0, name)?.as_str();
                Ok(Value::bit(self.cfg.plusargs.iter().any(|p| p.trim_start_matches('+').starts_with(&s))))
            }
            "$value$plusargs" => {
                let spec = self.arg_req(ctx, args, 0, name)?.as_str();
                let Some(pct) = spec.find('%') else { return Ok(Value::bit(false)) };
                let prefix = &spec[..pct];
                let conv = spec[pct + 1..].trim_start_matches('0').chars().next().unwrap_or('s');
                let found = self.cfg.plusargs.iter().map(|p| p.trim_start_matches('+').to_string()).find(|p| p.starts_with(prefix));
                let Some(found) = found else { return Ok(Value::bit(false)) };
                let raw = &found[prefix.len()..];
                let v = match conv {
                    'd' => Value::int(raw.parse::<i64>().unwrap_or(0)),
                    'h' | 'x' => Value::Int(u64::from_str_radix(raw, 16).unwrap_or(0), 64, false),
                    'b' => Value::Int(u64::from_str_radix(raw, 2).unwrap_or(0), 64, false),
                    _ => Value::Str(raw.to_string()),
                };
                if let Some(Some(e)) = args.get(1) {
                    let p = self.lpath(ctx, e)?;
                    self.write_path(&p, v)?;
                }
                Ok(Value::bit(true))
            }
            "$dumpfile" | "$dumpvars" | "$dumpon" | "$dumpoff" | "$dumpall" | "$timeformat" | "$printtimescale" | "$srandom" => Ok(none),
            "$helion_uvm_report" => {
                // (severity 0..3, file, line, text) — UVM report back end.
                let sev = self.arg_req(ctx, args, 0, name)?.as_u64();
                let file = self.arg_req(ctx, args, 1, name)?.as_str();
                let line = self.arg_req(ctx, args, 2, name)?.as_u64() as usize;
                let text = self.arg_req(ctx, args, 3, name)?.as_str();
                let s = match sev {
                    0 => Sev::Info,
                    1 => Sev::Warning,
                    2 => Sev::Error,
                    _ => Sev::Fatal,
                };
                self.log(s, text.clone());
                if sev >= 2 {
                    let (f, l) = if file.is_empty() { (self.label(loc.file), loc.line as usize) } else { (file, line) };
                    self.fail_at(f, l, text);
                }
                Ok(none)
            }
            "$helion_glob" => {
                let p = self.arg_req(ctx, args, 0, name)?.as_str();
                let s = self.arg_req(ctx, args, 1, name)?.as_str();
                Ok(Value::bit(glob(p.as_bytes(), s.as_bytes())))
            }
            "$helion_time_ps" => Ok(Value::Int(self.time, 64, false)),
            _ => Err(self.rt(format!("system task/function `{name}` is not supported by the testbench engine"))),
        }
    }

    // ---- methods ----------------------------------------------------------------

    fn recv_val(&mut self, r: &Recv) -> Result<(Value, Ty), SvError> {
        match r {
            Recv::Path(p) => self.read_path(p),
            Recv::Val(v, t) => Ok((v.clone(), t.clone())),
        }
    }

    fn recv_write(&mut self, r: &Recv, v: Value) -> Result<(), SvError> {
        match r {
            Recv::Path(p) => self.write_path(p, v),
            Recv::Val(..) => Err(self.rt("cannot modify a temporary value")),
        }
    }

    fn with_eval(&mut self, ctx: &Ctx, with: Option<&WithClause>, item: &Value, index: usize) -> Result<Option<Value>, SvError> {
        let Some(WithClause::Expr(e)) = with else { return Ok(None) };
        let mut c = ctx.clone();
        let s: ScopeRef = Rc::new(RefCell::new(HashMap::new()));
        let ty = match item {
            Value::Int(_, w, sg) => Ty::int(*w, *sg),
            Value::Str(_) => Ty::Str,
            Value::Real(_) => Ty::Real,
            _ => Ty::Void,
        };
        s.borrow_mut().insert("item".into(), Var { ty, v: item.clone() });
        s.borrow_mut().insert("__index".into(), Var { ty: Ty::int(32, true), v: Value::int(index as i64) });
        c.scopes.push(s);
        Ok(Some(self.eval(&c, e)?))
    }

    pub(crate) fn builtin_method(&mut self, ctx: &Ctx, recv: Recv, name: &str, args: &[Option<Expr>], with: Option<&WithClause>) -> Result<Value, SvError> {
        let (v, ty) = self.recv_val(&recv)?;
        match v {
            Value::Str(s) => self.string_method(ctx, &recv, s, name, args),
            Value::Arr(a) => self.array_method(ctx, &recv, &ty, *a, name, args, with),
            Value::Assoc(m) => self.assoc_method(ctx, &recv, &ty, *m, name, args),
            Value::Obj(Some(o)) => {
                let bi = o.borrow().builtin;
                match bi {
                    Some(Builtin::Mailbox) | Some(Builtin::Semaphore) => self.obj_method(ctx, &o, name, args),
                    _ => Err(self.rt(format!("no method `{name}`"))),
                }
            }
            Value::Int(x, _, _) => {
                if let Ty::Enum(e) = &ty {
                    let pos = e.items.iter().position(|(_, v)| *v == x);
                    let n = e.items.len();
                    return match name {
                        "name" => Ok(Value::Str(pos.map(|p| e.items[p].0.clone()).unwrap_or_default())),
                        "first" => Ok(Value::Int(e.items[0].1, e.w, false)),
                        "last" => Ok(Value::Int(e.items[n - 1].1, e.w, false)),
                        "num" => Ok(Value::int(n as i64)),
                        "next" | "prev" => {
                            let k = self.arg(ctx, args, 0)?.map(|v| v.as_u64() as usize).unwrap_or(1);
                            let p = pos.unwrap_or(0);
                            let q = if name == "next" { (p + k) % n } else { (p + n * k - k) % n };
                            Ok(Value::Int(e.items[q].1, e.w, false))
                        }
                        _ => Err(self.rt(format!("enum has no method `{name}`"))),
                    };
                }
                Err(self.rt(format!("no method `{name}` on an integral value")))
            }
            _ => Err(self.rt(format!("no method `{name}` on this value"))),
        }
    }

    fn string_method(&mut self, ctx: &Ctx, recv: &Recv, s: String, name: &str, args: &[Option<Expr>]) -> Result<Value, SvError> {
        let a0 = |me: &mut Self| me.arg_req(ctx, args, 0, name);
        Ok(match name {
            "len" => Value::int(s.len() as i64),
            "toupper" => Value::Str(s.to_uppercase()),
            "tolower" => Value::Str(s.to_lowercase()),
            "getc" => {
                let i = a0(self)?.as_i64();
                Value::Int(if i >= 0 { s.as_bytes().get(i as usize).copied().unwrap_or(0) as u64 } else { 0 }, 8, false)
            }
            "putc" => {
                let i = a0(self)?.as_i64();
                let c = self.arg_req(ctx, args, 1, name)?.as_u64() as u8;
                let mut b = s.into_bytes();
                if i >= 0 && (i as usize) < b.len() {
                    b[i as usize] = c;
                }
                self.recv_write(recv, Value::Str(String::from_utf8_lossy(&b).into_owned()))?;
                Value::Int(0, 1, false)
            }
            "compare" => {
                let o = a0(self)?.as_str();
                Value::int(match s.cmp(&o) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                })
            }
            "icompare" => {
                let o = a0(self)?.as_str().to_lowercase();
                Value::int(match s.to_lowercase().cmp(&o) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                })
            }
            "substr" => {
                let i = a0(self)?.as_i64();
                let j = self.arg_req(ctx, args, 1, name)?.as_i64();
                if i < 0 || j < i || j as usize >= s.len() {
                    Value::Str(String::new())
                } else {
                    Value::Str(s[i as usize..=j as usize].to_string())
                }
            }
            "atoi" => Value::int(s.trim().trim_start_matches('+').split(|c: char| !(c.is_ascii_digit() || c == '-')).next().and_then(|x| x.parse().ok()).unwrap_or(0)),
            "atohex" => Value::Int(u64::from_str_radix(s.trim(), 16).unwrap_or(0), 32, false),
            "atobin" => Value::Int(u64::from_str_radix(s.trim(), 2).unwrap_or(0), 32, false),
            "atooct" => Value::Int(u64::from_str_radix(s.trim(), 8).unwrap_or(0), 32, false),
            "atoreal" => Value::Real(s.trim().parse().unwrap_or(0.0)),
            "itoa" | "hextoa" | "bintoa" | "octtoa" | "realtoa" => {
                let v = a0(self)?;
                let t = match name {
                    "itoa" => v.as_i64().to_string(),
                    "hextoa" => format!("{:x}", v.as_u64()),
                    "bintoa" => format!("{:b}", v.as_u64()),
                    "octtoa" => format!("{:o}", v.as_u64()),
                    _ => format!("{}", v.as_f64()),
                };
                self.recv_write(recv, Value::Str(t))?;
                Value::Int(0, 1, false)
            }
            _ => return Err(self.rt(format!("string has no method `{name}`"))),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn array_method(&mut self, ctx: &Ctx, recv: &Recv, ty: &Ty, mut a: ArrVal, name: &str, args: &[Option<Expr>], with: Option<&WithClause>) -> Result<Value, SvError> {
        let et = ty.elem().cloned().unwrap_or(Ty::int(32, true));
        let wrap = |items: Vec<Value>| Value::Arr(Box::new(ArrVal { kind: ArrKind::Queue, items }));
        let store = |me: &mut Self, a: ArrVal| me.recv_write(recv, Value::Arr(Box::new(a)));
        match name {
            "size" | "num" => Ok(Value::int(a.items.len() as i64)),
            "delete" => {
                match self.arg(ctx, args, 0)? {
                    Some(i) => {
                        let i = i.as_i64();
                        if i >= 0 && (i as usize) < a.items.len() {
                            a.items.remove(i as usize);
                        }
                    }
                    None => a.items.clear(),
                }
                store(self, a)?;
                Ok(Value::Int(0, 1, false))
            }
            "push_back" | "push_front" => {
                let v = match args.first() {
                    Some(Some(e)) => self.eval_to(ctx, e, &et)?,
                    _ => return Err(self.rt(format!("{name} needs a value"))),
                };
                if name == "push_back" {
                    a.items.push(v);
                } else {
                    a.items.insert(0, v);
                }
                store(self, a)?;
                Ok(Value::Int(0, 1, false))
            }
            "pop_back" | "pop_front" => {
                if a.items.is_empty() {
                    self.log(Sev::Warning, format!("{}:{}: {name}() on an empty queue", self.label(self.cur_loc.file), self.cur_loc.line));
                    return Ok(et.default_value());
                }
                let v = if name == "pop_back" { a.items.pop().unwrap() } else { a.items.remove(0) };
                store(self, a)?;
                Ok(v)
            }
            "insert" => {
                let i = self.arg_req(ctx, args, 0, name)?.as_i64().max(0) as usize;
                let v = match args.get(1) {
                    Some(Some(e)) => self.eval_to(ctx, e, &et)?,
                    _ => return Err(self.rt("insert needs a value")),
                };
                let i = i.min(a.items.len());
                a.items.insert(i, v);
                store(self, a)?;
                Ok(Value::Int(0, 1, false))
            }
            "sort" | "rsort" => {
                let mut keyed: Vec<(Value, Value)> = Vec::new();
                for (k, it) in a.items.iter().enumerate() {
                    let key = self.with_eval(ctx, with, it, k)?.unwrap_or_else(|| it.clone());
                    keyed.push((key, it.clone()));
                }
                keyed.sort_by(|x, y| {
                    let o = match (&x.0, &y.0) {
                        (Value::Str(a), Value::Str(b)) => a.cmp(b),
                        (a, b) => {
                            if a.signed() && b.signed() { a.as_i64().cmp(&b.as_i64()) } else { a.as_u64().cmp(&b.as_u64()) }
                        }
                    };
                    if name == "rsort" { o.reverse() } else { o }
                });
                a.items = keyed.into_iter().map(|x| x.1).collect();
                store(self, a)?;
                Ok(Value::Int(0, 1, false))
            }
            "reverse" => {
                a.items.reverse();
                store(self, a)?;
                Ok(Value::Int(0, 1, false))
            }
            "shuffle" => {
                for i in (1..a.items.len()).rev() {
                    let j = (self.next_rand() % (i as u64 + 1)) as usize;
                    a.items.swap(i, j);
                }
                store(self, a)?;
                Ok(Value::Int(0, 1, false))
            }
            "sum" | "product" => {
                let mut acc = Value::Int(if name == "sum" { 0 } else { 1 }, et.width().max(1), matches!(et, Ty::Int { s: true, .. }));
                for (k, it) in a.items.iter().enumerate() {
                    let x = self.with_eval(ctx, with, it, k)?.unwrap_or_else(|| it.clone());
                    acc = self.binary(if name == "sum" { BinOp::Add } else { BinOp::Mul }, acc, x)?;
                }
                Ok(acc)
            }
            "min" | "max" => {
                let mut best: Option<(Value, Value)> = None;
                for (k, it) in a.items.iter().enumerate() {
                    let key = self.with_eval(ctx, with, it, k)?.unwrap_or_else(|| it.clone());
                    let better = match &best {
                        None => true,
                        Some((bk, _)) => {
                            let lt = self.binary(BinOp::Lt, key.clone(), bk.clone())?.truthy();
                            if name == "min" { lt } else { self.binary(BinOp::Gt, key.clone(), bk.clone())?.truthy() }
                        }
                    };
                    if better {
                        best = Some((key, it.clone()));
                    }
                }
                Ok(wrap(best.map(|b| vec![b.1]).unwrap_or_default()))
            }
            "unique" | "unique_index" => {
                let mut out: Vec<Value> = Vec::new();
                let mut idx = Vec::new();
                for (k, it) in a.items.iter().enumerate() {
                    if !out.contains(it) {
                        out.push(it.clone());
                        idx.push(Value::int(k as i64));
                    }
                }
                Ok(wrap(if name == "unique" { out } else { idx }))
            }
            "find" | "find_index" | "find_first" | "find_first_index" | "find_last" | "find_last_index" => {
                if with.is_none() {
                    return Err(self.rt(format!("{name}() needs `with (…)`")));
                }
                let mut hits = Vec::new();
                for (k, it) in a.items.iter().enumerate() {
                    if self.with_eval(ctx, with, it, k)?.is_some_and(|v| v.truthy()) {
                        hits.push((k, it.clone()));
                    }
                }
                let pick: Vec<(usize, Value)> = match name {
                    "find_first" | "find_first_index" => hits.into_iter().take(1).collect(),
                    "find_last" | "find_last_index" => hits.into_iter().last().into_iter().collect(),
                    _ => hits,
                };
                Ok(wrap(if name.ends_with("index") {
                    pick.into_iter().map(|(k, _)| Value::int(k as i64)).collect()
                } else {
                    pick.into_iter().map(|(_, v)| v).collect()
                }))
            }
            _ => Err(self.rt(format!("array has no method `{name}`"))),
        }
    }

    fn assoc_method(&mut self, ctx: &Ctx, recv: &Recv, ty: &Ty, mut m: std::collections::BTreeMap<Key, Value>, name: &str, args: &[Option<Expr>]) -> Result<Value, SvError> {
        let sk = matches!(ty, Ty::Assoc(_, true));
        match name {
            "num" | "size" => Ok(Value::int(m.len() as i64)),
            "exists" => {
                let k = self.arg_req(ctx, args, 0, name)?.to_key(sk);
                Ok(Value::int(m.contains_key(&k) as i64))
            }
            "delete" => {
                match self.arg(ctx, args, 0)? {
                    Some(k) => {
                        m.remove(&k.to_key(sk));
                    }
                    None => m.clear(),
                }
                self.recv_write(recv, Value::Assoc(Box::new(m)))?;
                Ok(Value::Int(0, 1, false))
            }
            "first" | "last" | "next" | "prev" => {
                let Some(Some(ke)) = args.first() else { return Err(self.rt(format!("{name}() needs a key variable"))) };
                let kp = self.lpath(ctx, ke)?;
                let (cur, _) = self.read_path(&kp)?;
                let cur = cur.to_key(sk);
                let found = match name {
                    "first" => m.keys().next().cloned(),
                    "last" => m.keys().next_back().cloned(),
                    "next" => m.range((std::ops::Bound::Excluded(cur), std::ops::Bound::Unbounded)).next().map(|x| x.0.clone()),
                    _ => m.range(..cur).next_back().map(|x| x.0.clone()),
                };
                match found {
                    Some(k) => {
                        self.write_path(&kp, k.to_value())?;
                        Ok(Value::int(1))
                    }
                    None => Ok(Value::int(0)),
                }
            }
            _ => Err(self.rt(format!("associative array has no method `{name}`"))),
        }
    }

    /// Non-blocking mailbox / semaphore methods (and blocking ones when they
    /// can complete immediately).
    fn obj_method(&mut self, ctx: &Ctx, o: &ObjRef, name: &str, args: &[Option<Expr>]) -> Result<Value, SvError> {
        match name {
            "num" => {
                let b = o.borrow();
                Ok(Value::int(match &b.native {
                    Native::Mailbox { q, .. } => q.len() as i64,
                    _ => 0,
                }))
            }
            "try_get" | "try_peek" => {
                let is_sem = matches!(o.borrow().native, Native::Semaphore { .. });
                if is_sem {
                    let n = self.arg(ctx, args, 0)?.map(|v| v.as_u64()).unwrap_or(1);
                    let mut b = o.borrow_mut();
                    if let Native::Semaphore { keys } = &mut b.native {
                        if *keys >= n {
                            *keys -= n;
                            return Ok(Value::int(1));
                        }
                    }
                    return Ok(Value::int(0));
                }
                let v = {
                    let mut b = o.borrow_mut();
                    match &mut b.native {
                        Native::Mailbox { q, .. } => {
                            if name == "try_get" { q.pop_front() } else { q.front().cloned() }
                        }
                        _ => None,
                    }
                };
                match v {
                    Some(v) => {
                        if let Some(Some(e)) = args.first() {
                            let p = self.lpath(ctx, e)?;
                            self.write_path(&p, v)?;
                        }
                        if name == "try_get" {
                            let id = o.borrow().id;
                            self.notify(&WaitKey::Obj(id));
                        }
                        Ok(Value::int(1))
                    }
                    None => Ok(Value::int(0)),
                }
            }
            "try_put" => {
                let v = self.arg_req(ctx, args, 0, name)?;
                let ok = {
                    let mut b = o.borrow_mut();
                    match &mut b.native {
                        Native::Mailbox { q, bound } => {
                            if *bound == 0 || q.len() < *bound {
                                q.push_back(v);
                                true
                            } else {
                                false
                            }
                        }
                        _ => false,
                    }
                };
                if ok {
                    let id = o.borrow().id;
                    self.notify(&WaitKey::Obj(id));
                }
                Ok(Value::int(ok as i64))
            }
            "get" | "put" | "peek" => {
                if self.blocking_obj_method(ctx, o, name, args)? {
                    Ok(Value::Int(0, 1, false))
                } else {
                    Err(self.rt(format!("`{name}()` would block here; call it as a statement in a task")))
                }
            }
            _ => Err(self.rt(format!("no method `{name}` on mailbox/semaphore"))),
        }
    }

    /// Try a blocking mailbox/semaphore operation; `Ok(false)` = must wait.
    pub(crate) fn blocking_obj_method(&mut self, ctx: &Ctx, o: &ObjRef, name: &str, args: &[Option<Expr>]) -> Result<bool, SvError> {
        let id = o.borrow().id;
        let is_mbx = matches!(o.borrow().native, Native::Mailbox { .. });
        if is_mbx {
            match name {
                "put" => {
                    let v = self.arg_req(ctx, args, 0, name)?;
                    let mut b = o.borrow_mut();
                    if let Native::Mailbox { q, bound } = &mut b.native {
                        if *bound != 0 && q.len() >= *bound {
                            return Ok(false);
                        }
                        q.push_back(v);
                    }
                    drop(b);
                    self.dirty += 1;
                    self.notify(&WaitKey::Obj(id));
                    Ok(true)
                }
                "get" | "peek" => {
                    let v = {
                        let mut b = o.borrow_mut();
                        match &mut b.native {
                            Native::Mailbox { q, .. } => {
                                if name == "get" { q.pop_front() } else { q.front().cloned() }
                            }
                            _ => None,
                        }
                    };
                    let Some(v) = v else { return Ok(false) };
                    if let Some(Some(e)) = args.first() {
                        let p = self.lpath(ctx, e)?;
                        self.write_path(&p, v)?;
                    }
                    if name == "get" {
                        self.notify(&WaitKey::Obj(id));
                    }
                    Ok(true)
                }
                _ => Err(self.rt(format!("no method `{name}` on mailbox"))),
            }
        } else {
            let n = self.arg(ctx, args, 0)?.map(|v| v.as_u64()).unwrap_or(1);
            let mut b = o.borrow_mut();
            let Native::Semaphore { keys } = &mut b.native else { return Err(self.rt("not a semaphore")) };
            match name {
                "get" => {
                    if *keys >= n {
                        *keys -= n;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                }
                "put" => {
                    *keys += n;
                    drop(b);
                    self.dirty += 1;
                    self.notify(&WaitKey::Obj(id));
                    Ok(true)
                }
                _ => Err(self.rt(format!("no method `{name}` on semaphore"))),
            }
        }
    }
}
