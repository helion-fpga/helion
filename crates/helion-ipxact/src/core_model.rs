//! Port/parameter model of a packaged Helion IP core, read from its RTL and
//! `.helion` manifest, plus the user-IP packager and repository scan.
//!
//! The catalog [`crate::IpCore`] only carries a VLNV and a bus name. Block
//! design emission, the per-IP customization dialog and IP packaging need the
//! real port list and the configurable parameters, which live in the core's
//! RTL (`ip/<core>/<core>.v`) and its `.helion` manifest (`param` lines).
//! Everything here is derived from those files; nothing is canned.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{HelionPackage, IpCore, PortDirection, parse_helion};

/// Type and legal range of a configurable IP parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// `0` / `1` flag.
    Bool,
    /// Signed integer in `min..=max`.
    Int { min: i64, max: i64 },
    /// Unsigned hex value of at most `bits` bits (addresses, masks).
    Hex { bits: u32 },
}

impl ParamKind {
    pub fn label(&self) -> String {
        match self {
            ParamKind::Bool => "bool".into(),
            ParamKind::Int { min, max } => format!("int [{min}..{max}]"),
            ParamKind::Hex { bits } => format!("hex{bits}"),
        }
    }
}

/// One configurable parameter (IP-XACT `ipxact:parameter`, Vivado `CONFIG.*`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreParam {
    pub name: String,
    pub kind: ParamKind,
    /// Default as written in the manifest (validated on load).
    pub default: String,
    pub description: String,
}

/// Parse a decimal, `0x` hex, or Verilog sized literal (`32'h10`, `1'b1`).
pub fn parse_param_number(raw: &str) -> Option<i128> {
    let s = raw.trim().replace('_', "");
    if s.is_empty() {
        return None;
    }
    if let Some((_, rest)) = s.split_once('\'') {
        let rest = rest.trim_start_matches(['s', 'S']);
        let (radix, digits) = match rest.chars().next()? {
            'h' | 'H' => (16, &rest[1..]),
            'd' | 'D' => (10, &rest[1..]),
            'b' | 'B' => (2, &rest[1..]),
            'o' | 'O' => (8, &rest[1..]),
            _ => return None,
        };
        return i128::from_str_radix(digits, radix).ok();
    }
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return i128::from_str_radix(h, 16).ok();
    }
    if let Some(n) = s.strip_prefix('-') {
        return n.parse::<i128>().ok().map(|v| -v);
    }
    s.parse::<i128>().ok()
}

impl CoreParam {
    /// Validate a user value; returns the SystemVerilog literal to emit.
    pub fn validate(&self, raw: &str) -> Result<String, String> {
        let raw = raw.trim();
        let v = parse_param_number(raw)
            .ok_or_else(|| format!("{}: '{raw}' is not a number", self.name))?;
        match &self.kind {
            ParamKind::Bool => match v {
                0 | 1 => Ok(v.to_string()),
                _ => Err(format!("{}: must be 0 or 1", self.name)),
            },
            ParamKind::Int { min, max } => {
                if v < *min as i128 || v > *max as i128 {
                    Err(format!("{}: {v} is outside {min}..{max}", self.name))
                } else {
                    Ok(v.to_string())
                }
            }
            ParamKind::Hex { bits } => {
                if v < 0 || (*bits < 64 && v >= (1i128 << bits)) {
                    Err(format!("{}: must fit in {bits} unsigned bits", self.name))
                } else {
                    Ok(format!("{bits}'h{v:X}"))
                }
            }
        }
    }

    /// Parse a `.helion` `param` line body:
    /// `NAME bool|int|hexN DEFAULT [MIN MAX] description...`.
    pub fn parse_decl(body: &str) -> Result<Self, String> {
        let mut toks = body.split_whitespace();
        let name = toks.next().ok_or("param: missing name")?.to_string();
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("param {name}: bad identifier"));
        }
        let kind_tok = toks.next().ok_or_else(|| format!("param {name}: missing type"))?;
        let default = toks
            .next()
            .ok_or_else(|| format!("param {name}: missing default"))?
            .to_string();
        let kind = match kind_tok {
            "bool" => ParamKind::Bool,
            "int" => {
                let min = toks
                    .next()
                    .and_then(parse_param_number)
                    .ok_or_else(|| format!("param {name}: int needs MIN MAX"))?;
                let max = toks
                    .next()
                    .and_then(parse_param_number)
                    .ok_or_else(|| format!("param {name}: int needs MIN MAX"))?;
                if min > max {
                    return Err(format!("param {name}: MIN > MAX"));
                }
                ParamKind::Int {
                    min: min as i64,
                    max: max as i64,
                }
            }
            k if k.starts_with("hex") => {
                let bits: u32 = k[3..]
                    .parse()
                    .map_err(|_| format!("param {name}: bad type {k}"))?;
                if bits == 0 || bits > 64 {
                    return Err(format!("param {name}: hex width 1..64"));
                }
                ParamKind::Hex { bits }
            }
            other => return Err(format!("param {name}: unknown type {other}")),
        };
        let description = toks.collect::<Vec<_>>().join(" ").trim_matches('"').to_string();
        let p = CoreParam {
            name,
            kind,
            default,
            description,
        };
        p.validate(&p.default)
            .map_err(|e| format!("param default invalid: {e}"))?;
        Ok(p)
    }

    /// The `.helion` line for this parameter (inverse of [`Self::parse_decl`]).
    pub fn to_decl(&self) -> String {
        let mut line = match &self.kind {
            ParamKind::Bool => format!("param {} bool {}", self.name, self.default),
            ParamKind::Int { min, max } => {
                format!("param {} int {} {min} {max}", self.name, self.default)
            }
            ParamKind::Hex { bits } => format!("param {} hex{bits} {}", self.name, self.default),
        };
        if !self.description.is_empty() {
            line.push(' ');
            line.push_str(&self.description);
        }
        line
    }
}

/// One port of a parsed RTL module header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlPort {
    pub name: String,
    pub dir: PortDirection,
    pub width: u32,
}

/// A parsed ANSI Verilog/SystemVerilog module header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtlModule {
    pub name: String,
    /// `parameter NAME = default` in declaration order.
    pub params: Vec<(String, String)>,
    pub ports: Vec<RtlPort>,
}

impl RtlModule {
    pub fn port(&self, name: &str) -> Option<&RtlPort> {
        self.ports.iter().find(|p| p.name == name)
    }
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// Index of the `)` matching the `(` at `open`.
fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Evaluate a constant width expression (`31`, `W-1`, `2*N+1`) against params.
fn eval_const(expr: &str, params: &BTreeMap<String, i128>) -> Option<i128> {
    let toks: Vec<String> = {
        let mut v = Vec::new();
        let mut cur = String::new();
        for c in expr.chars() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '\'' {
                cur.push(c);
            } else {
                if !cur.is_empty() {
                    v.push(std::mem::take(&mut cur));
                }
                if !c.is_whitespace() {
                    v.push(c.to_string());
                }
            }
        }
        if !cur.is_empty() {
            v.push(cur);
        }
        v
    };
    fn atom(t: &str, params: &BTreeMap<String, i128>) -> Option<i128> {
        parse_param_number(t).or_else(|| params.get(t).copied())
    }
    // term := atom (('*'|'/') atom)*; expr := term (('+'|'-') term)*
    let mut i = 0;
    let term = |i: &mut usize| -> Option<i128> {
        let mut v = if toks.get(*i)? == "(" {
            let mut depth = 0;
            let start = *i + 1;
            let mut j = *i;
            loop {
                match toks.get(j)?.as_str() {
                    "(" => depth += 1,
                    ")" => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            *i = j + 1;
            eval_const(&toks[start..j].join(" "), params)?
        } else {
            let v = atom(toks.get(*i)?, params)?;
            *i += 1;
            v
        };
        while let Some(op) = toks.get(*i) {
            match op.as_str() {
                "*" => {
                    *i += 1;
                    v *= atom(toks.get(*i)?, params)?;
                    *i += 1;
                }
                "/" => {
                    *i += 1;
                    let d = atom(toks.get(*i)?, params)?;
                    if d == 0 {
                        return None;
                    }
                    v /= d;
                    *i += 1;
                }
                _ => break,
            }
        }
        Some(v)
    };
    let mut acc = term(&mut i)?;
    while let Some(op) = toks.get(i).cloned() {
        i += 1;
        match op.as_str() {
            "+" => acc += term(&mut i)?,
            "-" => acc -= term(&mut i)?,
            _ => return None,
        }
    }
    Some(acc)
}

/// Parse the ANSI header of `module` (or the first module when `None`).
pub fn parse_rtl_module(text: &str, module: Option<&str>) -> Result<RtlModule, String> {
    let src = strip_comments(text);
    let mut search = 0;
    let (name, after) = loop {
        let Some(at) = src[search..].find("module") else {
            return Err(match module {
                Some(m) => format!("module {m} not found"),
                None => "no module declaration found".into(),
            });
        };
        let at = search + at;
        let boundary_ok = at == 0
            || !src[..at]
                .chars()
                .last()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let rest = &src[at + 6..];
        if !boundary_ok || !rest.starts_with(char::is_whitespace) {
            search = at + 6;
            continue;
        }
        let rest_trim = rest.trim_start();
        let name: String = rest_trim
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        let after = src.len() - rest_trim.len() + name.len();
        if module.is_none_or(|m| m == name) {
            break (name, after);
        }
        search = after;
    };
    let mut pos = after;
    let mut params = Vec::new();
    let mut pvals: BTreeMap<String, i128> = BTreeMap::new();
    let tail = src[pos..].trim_start();
    pos = src.len() - tail.len();
    if tail.starts_with('#') {
        let open = pos + src[pos..].find('(').ok_or("parameter list: missing (")?;
        let close = matching_paren(&src, open).ok_or("parameter list: unbalanced ()")?;
        for decl in src[open + 1..close].split(',') {
            let d = decl.trim();
            let d = d
                .strip_prefix("parameter")
                .or_else(|| d.strip_prefix("localparam"))
                .unwrap_or(d)
                .trim();
            let Some((lhs, rhs)) = d.split_once('=') else {
                continue;
            };
            let pname = lhs
                .split_whitespace()
                .last()
                .unwrap_or("")
                .trim()
                .to_string();
            let val = rhs.trim().to_string();
            if pname.is_empty() {
                continue;
            }
            if let Some(v) = eval_const(&val, &pvals) {
                pvals.insert(pname.clone(), v);
            }
            params.push((pname, val));
        }
        pos = close + 1;
    }
    let open = pos
        + src[pos..]
            .find('(')
            .ok_or_else(|| format!("module {name}: missing port list"))?;
    let semi = src[pos..].find(';').map(|s| pos + s);
    if semi.is_some_and(|s| s < open) {
        // `module m;` — no ports.
        return Ok(RtlModule {
            name,
            params,
            ports: Vec::new(),
        });
    }
    let close = matching_paren(&src, open).ok_or("port list: unbalanced ()")?;
    let mut ports = Vec::new();
    let mut dir: Option<PortDirection> = None;
    let mut width: u32 = 1;
    for decl in src[open + 1..close].split(',') {
        let d = decl.trim();
        if d.is_empty() {
            continue;
        }
        let mut words: Vec<&str> = Vec::new();
        let mut range: Option<String> = None;
        let mut rest = d;
        while !rest.is_empty() {
            rest = rest.trim_start();
            if rest.starts_with('[') {
                let end = rest.find(']').ok_or("port: unbalanced []")?;
                range = Some(rest[1..end].to_string());
                rest = &rest[end + 1..];
            } else {
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == '[')
                    .unwrap_or(rest.len());
                words.push(&rest[..end]);
                rest = &rest[end..];
            }
        }
        let mut new_dir = None;
        for w in &words {
            match *w {
                "input" => new_dir = Some(PortDirection::In),
                "output" => new_dir = Some(PortDirection::Out),
                "inout" => new_dir = Some(PortDirection::InOut),
                _ => {}
            }
        }
        if new_dir.is_some() {
            dir = new_dir;
            width = 1;
        }
        if let Some(r) = &range {
            let (hi, lo) = r.split_once(':').ok_or_else(|| format!("port range [{r}]"))?;
            let hi = eval_const(hi, &pvals)
                .ok_or_else(|| format!("port range [{r}]: not constant"))?;
            let lo = eval_const(lo, &pvals)
                .ok_or_else(|| format!("port range [{r}]: not constant"))?;
            width = ((hi - lo).unsigned_abs() + 1) as u32;
        }
        let Some(dir) = dir else {
            return Err(format!(
                "module {name}: non-ANSI port list (declare directions in the header)"
            ));
        };
        let pname = words
            .last()
            .copied()
            .filter(|w| !matches!(*w, "input" | "output" | "inout" | "wire" | "reg" | "logic"))
            .ok_or_else(|| format!("module {name}: port without a name: {d}"))?;
        ports.push(RtlPort {
            name: pname.to_string(),
            dir,
            width,
        });
    }
    Ok(RtlModule {
        name,
        params,
        ports,
    })
}

/// Bus role a module plays on Helion-MM, inferred from port naming
/// (`mm_valid`/`mm_write`/`mm_addr`/`mm_wdata`/`mm_rdata`/`mm_ready`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmRole {
    /// Drives `mm_valid`/`mm_addr` (a processor or DMA).
    Master,
    /// Samples `mm_valid`/`mm_addr` (a peripheral).
    Slave,
}

/// Helion-MM signal names (the bus interface's logical ports).
pub const MM_SIGNALS: [&str; 7] = [
    "mm_valid", "mm_write", "mm_addr", "mm_wdata", "mm_wstrb", "mm_rdata", "mm_ready",
];

pub fn is_mm_signal(name: &str) -> bool {
    MM_SIGNALS.contains(&name)
}

pub fn is_clock_or_reset(name: &str) -> bool {
    matches!(name, "clk" | "resetn")
}

/// Inferred bus interface of a module (for packaging and IP-XACT export).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferredBus {
    /// IP-XACT interface name (`s_mm`, `m_mm`, `st_in`, `st_out`, `clk`, `resetn`).
    pub name: String,
    /// Bus type label (`Helion-MM`, `Helion-ST`, `clock`, `reset`).
    pub bus: String,
    /// `master` / `slave`.
    pub mode: &'static str,
    pub ports: Vec<String>,
}

impl RtlModule {
    pub fn mm_role(&self) -> Option<MmRole> {
        let v = self.port("mm_valid")?;
        self.port("mm_addr")?;
        self.port("mm_rdata")?;
        Some(match v.dir {
            PortDirection::Out => MmRole::Master,
            _ => MmRole::Slave,
        })
    }

    /// Bus interfaces inferred from port naming conventions.
    pub fn infer_buses(&self) -> Vec<InferredBus> {
        let mut out = Vec::new();
        if self.port("clk").is_some() {
            out.push(InferredBus {
                name: "clk".into(),
                bus: "clock".into(),
                mode: "slave",
                ports: vec!["clk".into()],
            });
        }
        if self.port("resetn").is_some() {
            out.push(InferredBus {
                name: "resetn".into(),
                bus: "reset".into(),
                mode: "slave",
                ports: vec!["resetn".into()],
            });
        }
        if let Some(role) = self.mm_role() {
            out.push(InferredBus {
                name: if role == MmRole::Master { "m_mm" } else { "s_mm" }.into(),
                bus: "Helion-MM".into(),
                mode: if role == MmRole::Master { "master" } else { "slave" },
                ports: self
                    .ports
                    .iter()
                    .filter(|p| is_mm_signal(&p.name))
                    .map(|p| p.name.clone())
                    .collect(),
            });
        }
        for (prefix, name, mode) in [("wr_", "st_in", "slave"), ("rd_", "st_out", "master")] {
            let ports: Vec<String> = self
                .ports
                .iter()
                .filter(|p| p.name.starts_with(prefix))
                .map(|p| p.name.clone())
                .collect();
            if ports.iter().any(|p| p.ends_with("valid")) && ports.iter().any(|p| p.ends_with("ready"))
            {
                out.push(InferredBus {
                    name: name.into(),
                    bus: "Helion-ST".into(),
                    mode,
                    ports,
                });
            }
        }
        out
    }

    /// Primary bus for the catalog: Helion-MM, else Helion-ST, else `None`.
    pub fn primary_bus(&self) -> Option<&'static str> {
        let buses = self.infer_buses();
        if buses.iter().any(|b| b.bus == "Helion-MM") {
            Some("Helion-MM")
        } else if buses.iter().any(|b| b.bus == "Helion-ST") {
            Some("Helion-ST")
        } else {
            None
        }
    }
}

/// A packaged core with its parsed RTL header, parameters and file set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreModel {
    pub core: IpCore,
    pub module: RtlModule,
    pub params: Vec<CoreParam>,
    /// Absolute HDL files the core needs (top first).
    pub files: Vec<PathBuf>,
    /// Manifest path (`ip/<core>/<core>.helion`).
    pub manifest: PathBuf,
}

impl CoreModel {
    pub fn param(&self, name: &str) -> Option<&CoreParam> {
        self.params.iter().find(|p| p.name == name)
    }

    pub fn mm_role(&self) -> Option<MmRole> {
        self.module.mm_role()
    }

    /// Validate a full override map; returns name → SV literal for every
    /// override that differs from the default.
    pub fn validate_overrides(
        &self,
        overrides: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, String> {
        let mut out = BTreeMap::new();
        for (k, v) in overrides {
            let p = self
                .param(k)
                .ok_or_else(|| format!("{}: no parameter {k}", self.core.name))?;
            let lit = p.validate(v)?;
            let default = p.validate(&p.default)?;
            if lit != default {
                out.insert(k.clone(), lit);
            }
        }
        Ok(out)
    }
}

/// Root of the bundled IP repository (`ip/`). `HELION_IP_DIR` wins; then
/// exe-relative (`Helion.app/Contents/Resources/ip` included), the current
/// directory and its ancestors, then the compile-time workspace.
pub fn ip_repo_dir() -> PathBuf {
    let ok = |p: &Path| p.join("README.md").is_file() && p.join("h_uart").is_dir();
    if let Some(p) = std::env::var_os("HELION_IP_DIR").map(PathBuf::from) {
        return p;
    }
    let mut cands = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors().skip(1).take(6) {
            cands.push(anc.join("ip"));
            cands.push(anc.join("Resources").join("ip"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for anc in cwd.ancestors().take(6) {
            cands.push(anc.join("ip"));
        }
    }
    let compile_time = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ip");
    cands.push(compile_time.clone());
    cands
        .into_iter()
        .find(|p| ok(p))
        .unwrap_or(compile_time)
}

/// Load the model of a core packaged at `manifest` (`*.helion`).
pub fn load_core_model_at(manifest: &Path) -> Result<CoreModel, String> {
    let text = std::fs::read_to_string(manifest)
        .map_err(|e| format!("read {}: {e}", manifest.display()))?;
    let mut pkg: HelionPackage = parse_helion(&text)?;
    pkg.manifest_path = manifest.to_path_buf();
    let files = pkg.resolve_files()?;
    let top = pkg.top.clone().unwrap_or_else(|| pkg.name.clone());
    let mut module = None;
    for f in &files {
        let src =
            std::fs::read_to_string(f).map_err(|e| format!("read {}: {e}", f.display()))?;
        if let Ok(m) = parse_rtl_module(&src, Some(&top)) {
            module = Some(m);
            break;
        }
    }
    let module =
        module.ok_or_else(|| format!("{}: top module {top} not found in its files", pkg.name))?;
    for p in &pkg.params {
        if !module.params.iter().any(|(n, _)| n == &p.name) {
            return Err(format!(
                "{}: manifest param {} is not a parameter of module {top}",
                pkg.name, p.name
            ));
        }
    }
    Ok(CoreModel {
        core: pkg.to_ip_core()?,
        module,
        params: pkg.params.clone(),
        files,
        manifest: manifest.to_path_buf(),
    })
}

/// Load a catalog core's model from the bundled repository (`ip/<name>/`).
pub fn load_core_model(name: &str) -> Result<CoreModel, String> {
    load_core_model_in(&ip_repo_dir(), name)
}

/// Load `<repo>/<name>/<name>.helion` (or `package.helion`).
pub fn load_core_model_in(repo: &Path, name: &str) -> Result<CoreModel, String> {
    let dir = repo.join(name);
    for cand in [dir.join(format!("{name}.helion")), dir.join("package.helion")] {
        if cand.is_file() {
            return load_core_model_at(&cand);
        }
    }
    Err(format!(
        "{name}: no RTL package in {} (generator-only core)",
        repo.display()
    ))
}

/// Every `.helion` package one level below `repo` (a user IP repository).
pub fn scan_repository(repo: &Path) -> Vec<CoreModel> {
    let Ok(entries) = std::fs::read_dir(repo) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    let mut out = Vec::new();
    for d in dirs {
        let Some(name) = d.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Ok(m) = load_core_model_in(repo, name) {
            out.push(m);
        }
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Full IEEE 1685-2014 component XML for a core model: bus interfaces
/// (inferred from port naming), model ports with widths, and parameters
/// with `overrides` applied (the customized instance's values).
pub fn component_xml(model: &CoreModel, overrides: &BTreeMap<String, String>) -> String {
    let c = &model.core;
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    x.push_str("<ipxact:component xmlns:ipxact=\"http://www.accellera.org/XMLSchema/IPXACT/1685-2014\">\n");
    for (tag, v) in [
        ("vendor", &c.vendor),
        ("library", &c.library),
        ("name", &c.name),
        ("version", &c.version),
    ] {
        x.push_str(&format!("  <ipxact:{tag}>{}</ipxact:{tag}>\n", xml_escape(v)));
    }
    let buses = model.module.infer_buses();
    if !buses.is_empty() {
        x.push_str("  <ipxact:busInterfaces>\n");
        for b in &buses {
            x.push_str("    <ipxact:busInterface>\n");
            x.push_str(&format!("      <ipxact:name>{}</ipxact:name>\n", b.name));
            x.push_str(&format!(
                "      <ipxact:busType vendor=\"community\" library=\"helion\" name=\"{}\" version=\"1.0\"/>\n",
                xml_escape(&b.bus)
            ));
            x.push_str(&format!("      <ipxact:{}/>\n", b.mode));
            x.push_str("      <ipxact:portMaps>\n");
            for p in &b.ports {
                x.push_str(&format!(
                    "        <ipxact:portMap><ipxact:logicalPort><ipxact:name>{p}</ipxact:name></ipxact:logicalPort><ipxact:physicalPort><ipxact:name>{p}</ipxact:name></ipxact:physicalPort></ipxact:portMap>\n"
                ));
            }
            x.push_str("      </ipxact:portMaps>\n");
            x.push_str("    </ipxact:busInterface>\n");
        }
        x.push_str("  </ipxact:busInterfaces>\n");
    }
    x.push_str("  <ipxact:model>\n    <ipxact:ports>\n");
    for p in &model.module.ports {
        x.push_str(&format!(
            "      <ipxact:port><ipxact:name>{}</ipxact:name><ipxact:wire><ipxact:direction>{}</ipxact:direction>",
            p.name,
            p.dir.as_str()
        ));
        if p.width > 1 {
            x.push_str(&format!(
                "<ipxact:vectors><ipxact:vector><ipxact:left>{}</ipxact:left><ipxact:right>0</ipxact:right></ipxact:vector></ipxact:vectors>",
                p.width - 1
            ));
        }
        x.push_str("</ipxact:wire></ipxact:port>\n");
    }
    x.push_str("    </ipxact:ports>\n  </ipxact:model>\n");
    if !model.params.is_empty() {
        x.push_str("  <ipxact:parameters>\n");
        for p in &model.params {
            let value = overrides
                .get(&p.name)
                .and_then(|v| p.validate(v).ok())
                .unwrap_or_else(|| p.validate(&p.default).unwrap_or_else(|_| p.default.clone()));
            x.push_str(&format!(
                "    <ipxact:parameter parameterId=\"{0}\" type=\"{1}\"><ipxact:name>{0}</ipxact:name><ipxact:description>{2}</ipxact:description><ipxact:value>{3}</ipxact:value></ipxact:parameter>\n",
                p.name,
                xml_escape(&p.kind.label()),
                xml_escape(&p.description),
                xml_escape(&value)
            ));
        }
        x.push_str("  </ipxact:parameters>\n");
    }
    x.push_str("</ipxact:component>\n");
    x
}

/// Request to package a project module as a user IP.
#[derive(Clone, Debug)]
pub struct PackageRequest {
    pub source: PathBuf,
    pub top: String,
    pub vendor: String,
    pub library: String,
    pub version: String,
}

/// Package `req.top` from `req.source` into `<repo>/<top>/`: copies the RTL,
/// writes the `.helion` manifest (bus inferred from port naming; each RTL
/// `parameter` with an integer default becomes a `param` line) and the
/// IP-XACT `component.xml`. Returns the loaded model of the new package.
pub fn package_user_ip(req: &PackageRequest, repo: &Path) -> Result<CoreModel, String> {
    let ident_ok = |s: &str| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
    };
    for (what, v) in [
        ("vendor", &req.vendor),
        ("library", &req.library),
        ("version", &req.version),
    ] {
        if !ident_ok(v) {
            return Err(format!("{what}: use letters, digits, '.', '_' or '-'"));
        }
    }
    let src = std::fs::read_to_string(&req.source)
        .map_err(|e| format!("read {}: {e}", req.source.display()))?;
    let module = parse_rtl_module(&src, Some(&req.top))?;
    if module.ports.is_empty() {
        return Err(format!("module {}: no ports to package", module.name));
    }
    let bus = module.primary_bus().unwrap_or("None");
    let dir = repo.join(&module.name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let file_name = req
        .source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{}.v", module.name));
    std::fs::write(dir.join(&file_name), &src)
        .map_err(|e| format!("write {}: {e}", dir.join(&file_name).display()))?;
    let mut manifest = format!(
        "# Helion IP package (format 1) — packaged from {}\nformat 1\nvlnv {}:{}:{}:{}\nbus {bus}\ntop {}\nfile {file_name}\n",
        req.source.display(),
        req.vendor,
        req.library,
        module.name,
        req.version,
        module.name
    );
    for (name, val) in &module.params {
        if let Some(v) = parse_param_number(val) {
            let (lo, hi) = (v.min(0) as i64, (v.max(1) as i64).saturating_mul(1024).max(1));
            manifest.push_str(&format!(
                "param {name} int {v} {lo} {hi} RTL parameter {name} (packaged default {v})\n"
            ));
        }
    }
    let manifest_path = dir.join(format!("{}.helion", module.name));
    std::fs::write(&manifest_path, &manifest)
        .map_err(|e| format!("write {}: {e}", manifest_path.display()))?;
    let model = load_core_model_at(&manifest_path)?;
    let xml = component_xml(&model, &BTreeMap::new());
    std::fs::write(dir.join("component.xml"), xml)
        .map_err(|e| format!("write component.xml: {e}"))?;
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ansi_header_with_params_and_widths() {
        let m = parse_rtl_module(
            "// c\nmodule top #(parameter W = 8, parameter D = W*2)(\n input wire clk, /* x */\n input [W-1:0] a, b,\n output reg [D-1:0] y\n);\nendmodule\nmodule other(input x); endmodule",
            None,
        )
        .unwrap();
        assert_eq!(m.name, "top");
        assert_eq!(m.params.len(), 2);
        assert_eq!(m.ports.len(), 4);
        assert_eq!(m.port("a").unwrap().width, 8);
        assert_eq!(m.port("b").unwrap().width, 8, "width carries over a list");
        assert_eq!(m.port("y").unwrap().width, 16);
        assert_eq!(m.port("y").unwrap().dir, PortDirection::Out);
        let o = parse_rtl_module("module a(input x); endmodule module other(input x); endmodule", Some("other")).unwrap();
        assert_eq!(o.name, "other");
        assert!(parse_rtl_module("module q(a, b); input a; endmodule", None).is_err());
    }

    #[test]
    fn param_validation_kinds() {
        let b = CoreParam::parse_decl("EN bool 0 Enable it").unwrap();
        assert_eq!(b.validate("1").unwrap(), "1");
        assert!(b.validate("2").is_err());
        let i = CoreParam::parse_decl("N int 4 1 16 Count").unwrap();
        assert!(i.validate("17").is_err());
        assert!(i.validate("abc").is_err());
        assert_eq!(i.validate("0x10").unwrap(), "16");
        let h = CoreParam::parse_decl("BASE hex32 0x0 Reset vector").unwrap();
        assert_eq!(h.validate("0x100").unwrap(), "32'h100");
        assert!(h.validate("0x1_0000_0000").is_err());
        assert!(CoreParam::parse_decl("BAD int 99 1 16").is_err(), "default out of range");
        for p in [&b, &i, &h] {
            assert_eq!(&CoreParam::parse_decl(p.to_decl().trim_start_matches("param ")).unwrap(), p);
        }
    }

    #[test]
    fn bundled_catalog_cores_load_with_bus_roles() {
        let uart = load_core_model("h_uart").expect("h_uart");
        assert_eq!(uart.mm_role(), Some(MmRole::Slave));
        assert!(uart.module.port("tx").is_some());
        let cpu = load_core_model("h_rv32_hb1").expect("h_rv32_hb1");
        assert_eq!(cpu.mm_role(), Some(MmRole::Master));
        assert!(cpu.param("ENABLE_MUL").is_some(), "processor exposes PicoRV32 params");
        assert!(cpu.files.len() >= 2, "wrapper + picorv32 core");
        let fifo = load_core_model("h_sync_fifo").expect("fifo");
        assert_eq!(fifo.module.primary_bus(), Some("Helion-ST"));
        assert!(load_core_model("h_rv64gc").is_err(), "generator-only core has no package");
        let xml = component_xml(&cpu, &BTreeMap::from([("ENABLE_MUL".into(), "1".into())]));
        assert!(xml.contains("<ipxact:name>m_mm</ipxact:name>"));
        assert!(xml.contains("<ipxact:master/>"));
        assert!(xml.contains("parameterId=\"ENABLE_MUL\""));
        assert!(xml.contains("<ipxact:value>1</ipxact:value>"));
    }

    #[test]
    fn package_user_ip_round_trips_through_the_repository() {
        let tmp = std::env::temp_dir().join(format!("helion-pkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let src = tmp.join("my_regs.v");
        std::fs::write(
            &src,
            "module my_regs #(parameter DEPTH = 4)(input wire clk, input wire resetn,\n input wire mm_valid, input wire mm_write, input wire [7:0] mm_addr,\n input wire [31:0] mm_wdata, output reg [31:0] mm_rdata, output wire mm_ready,\n output wire [3:0] leds);\nendmodule\n",
        )
        .unwrap();
        let repo = tmp.join("repo");
        let m = package_user_ip(
            &PackageRequest {
                source: src,
                top: "my_regs".into(),
                vendor: "user".into(),
                library: "ip".into(),
                version: "1.0".into(),
            },
            &repo,
        )
        .unwrap();
        assert_eq!(m.core.bus, "Helion-MM");
        assert_eq!(m.core.vlnv(), "user:ip:my_regs:1.0");
        assert_eq!(m.param("DEPTH").unwrap().default, "4");
        assert!(repo.join("my_regs/component.xml").is_file());
        let found = scan_repository(&repo);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].core.name, "my_regs");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
