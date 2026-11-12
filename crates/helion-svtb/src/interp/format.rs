//! `$display` / `$sformatf` formatting.

use super::*;

impl Engine<'_> {
    /// Format `$display`-style arguments. A leading string literal is the
    /// format; remaining values are appended in decimal.
    pub(crate) fn format_args(&mut self, ctx: &Ctx, args: &[Option<Expr>]) -> Result<String, SvError> {
        let mut vals = Vec::new();
        for a in args {
            vals.push(match a {
                Some(e) => self.eval(ctx, e)?,
                None => Value::Str(" ".into()),
            });
        }
        let fmt_lit = matches!(args.first(), Some(Some(Expr::Str(_))));
        if fmt_lit || matches!(vals.first(), Some(Value::Str(_))) && vals.len() > 1 {
            let fmt = vals[0].as_str();
            let (s, used) = self.format(ctx, &fmt, &vals[1..])?;
            let mut out = s;
            for v in &vals[1 + used..] {
                out.push_str(&self.plain(v));
            }
            return Ok(out);
        }
        Ok(vals.iter().map(|v| self.plain(v)).collect())
    }

    fn plain(&self, v: &Value) -> String {
        match v {
            Value::Str(s) => s.clone(),
            Value::Int(x, w, s) => {
                if *s { sext(*x, *w).to_string() } else { x.to_string() }
            }
            other => show(other),
        }
    }

    /// Apply format string `fmt` to `vals`; returns (text, values consumed).
    pub(crate) fn format(&mut self, ctx: &Ctx, fmt: &str, vals: &[Value]) -> Result<(String, usize), SvError> {
        let b: Vec<char> = fmt.chars().collect();
        let mut out = String::new();
        let mut k = 0usize;
        let mut i = 0usize;
        while i < b.len() {
            let c = b[i];
            if c != '%' {
                out.push(c);
                i += 1;
                continue;
            }
            i += 1;
            if i >= b.len() {
                out.push('%');
                break;
            }
            let mut left = false;
            let mut zero = false;
            if b[i] == '-' {
                left = true;
                i += 1;
            }
            if i < b.len() && b[i] == '0' {
                zero = true;
            }
            let mut width = String::new();
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                width.push(b[i]);
                i += 1;
            }
            if i >= b.len() {
                break;
            }
            let spec = b[i].to_ascii_lowercase();
            i += 1;
            if spec == '%' {
                out.push('%');
                continue;
            }
            if spec == 'm' {
                let path = ctx.this.as_ref().and_then(|o| o.borrow().class).map(|c| self.classes[c as usize].name.clone()).unwrap_or_else(|| self.nss[ctx.ns].name.clone());
                out.push_str(&path);
                continue;
            }
            let v = match vals.get(k) {
                Some(v) => v.clone(),
                None => return Err(self.rt(format!("format `%{spec}` has no matching argument"))),
            };
            k += 1;
            let (wnum, prec) = match width.split_once('.') {
                Some((a, p)) => (a.parse::<usize>().ok(), p.parse::<usize>().ok()),
                None => (width.parse::<usize>().ok(), None),
            };
            let explicit_zero_width = width == "0";
            let s = match spec {
                'd' | 'i' => {
                    let t = match &v {
                        Value::Int(x, w, true) => sext(*x, *w).to_string(),
                        Value::Real(r) => format!("{}", r.round() as i64),
                        other => other.as_u64().to_string(),
                    };
                    if explicit_zero_width || wnum.is_some() {
                        t
                    } else {
                        // Default width: the decimal width of the type.
                        let w = v.width().min(64);
                        let maxlen = mask(u64::MAX, w).to_string().len() + usize::from(v.signed());
                        format!("{t:>maxlen$}")
                    }
                }
                'h' | 'x' => {
                    let w = v.width();
                    let t = format!("{:x}", mask(v.as_u64(), w));
                    if explicit_zero_width || wnum.is_some() { t } else { format!("{t:0>width$}", width = w.div_ceil(4) as usize) }
                }
                'b' => {
                    let w = v.width();
                    let t = format!("{:b}", mask(v.as_u64(), w));
                    if explicit_zero_width || wnum.is_some() { t } else { format!("{t:0>width$}", width = w as usize) }
                }
                'o' => {
                    let w = v.width();
                    let t = format!("{:o}", mask(v.as_u64(), w));
                    if explicit_zero_width || wnum.is_some() { t } else { format!("{t:0>width$}", width = w.div_ceil(3) as usize) }
                }
                's' => match &v {
                    Value::Str(s) => s.clone(),
                    other => other.as_str(),
                },
                'c' => ((v.as_u64() & 0xff) as u8 as char).to_string(),
                't' => {
                    let t = match &v {
                        Value::Real(r) => format!("{r}"),
                        other => other.as_u64().to_string(),
                    };
                    if explicit_zero_width || wnum.is_some() { t } else { format!("{t:>20}") }
                }
                'f' | 'e' | 'g' => {
                    let r = v.as_f64();
                    let p = prec.unwrap_or(6);
                    match spec {
                        'f' => format!("{r:.p$}"),
                        'e' => format!("{r:.p$e}"),
                        _ => format!("{r}"),
                    }
                }
                'p' => self.pretty(&v),
                'u' | 'z' | 'v' | 'l' => v.as_u64().to_string(),
                other => return Err(self.rt(format!("unsupported format specifier `%{other}`"))),
            };
            let s = match wnum {
                Some(w) if !explicit_zero_width && s.len() < w => {
                    if left {
                        format!("{s:<w$}")
                    } else if zero && matches!(spec, 'd' | 'h' | 'x' | 'b' | 'o') {
                        format!("{s:0>w$}")
                    } else {
                        format!("{s:>w$}")
                    }
                }
                _ => s,
            };
            out.push_str(&s);
        }
        Ok((out, k))
    }

    pub(crate) fn pretty(&self, v: &Value) -> String {
        match v {
            Value::Obj(Some(o)) => {
                let b = o.borrow();
                let name = b.class.map(|c| self.classes[c as usize].name.clone()).unwrap_or_else(|| "object".into());
                let mut fields: Vec<(&String, &Var)> = b.fields.iter().collect();
                fields.sort_by(|a, b| a.0.cmp(b.0));
                let parts: Vec<String> = fields
                    .iter()
                    .filter(|(_, v)| !matches!(v.v, Value::Obj(_)))
                    .map(|(n, v)| format!("{n}:{}", show(&v.v)))
                    .collect();
                format!("'{{{name}: {}}}", parts.join(", "))
            }
            other => show(other),
        }
    }
}
