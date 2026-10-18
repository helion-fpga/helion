//! IEEE 1800 SystemVerilog DPI-C (`import "DPI-C" function ...`) registration and
//! foreign-function invocation bridge (`DpiRegistry`).
//!
//! Supports parsing standard SystemVerilog DPI-C import declarations, binding Rust/C
//! callbacks with signature and argument-type validation, and recording invocation
//! traces during simulation.

use std::collections::HashMap;
use std::sync::Arc;

/// IEEE 1800 DPI-C scalar data type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DpiType {
    Void,
    Bit,
    Byte,
    ShortInt,
    Int,
    LongInt,
    UnsignedInt,
    UnsignedLongInt,
    String,
    Chandle,
}

impl DpiType {
    /// Parse a SystemVerilog DPI type token (`void`, `bit`, `byte`, `shortint`, `int`,
    /// `longint`, `int unsigned`, `longint unsigned`, `string`, `chandle`).
    pub fn parse_sv(raw: &str) -> Result<Self, String> {
        let s = raw.trim();
        // Strip packed range if present (e.g. `bit [63:0]` or `logic [31:0]`).
        let base = if let Some((prefix, _)) = s.split_once('[') {
            prefix.trim()
        } else {
            s
        };
        if s.contains('[') && (base == "bit" || base == "logic") {
            return Ok(Self::UnsignedLongInt);
        }
        match base {
            "void" => Ok(Self::Void),
            "bit" | "logic" => Ok(Self::Bit),
            "byte" | "char" => Ok(Self::Byte),
            "shortint" => Ok(Self::ShortInt),
            "int" | "integer" => Ok(Self::Int),
            "int unsigned" | "uint32_t" => Ok(Self::UnsignedInt),
            "longint" => Ok(Self::LongInt),
            "longint unsigned" | "uint64_t" => Ok(Self::UnsignedLongInt),
            "string" => Ok(Self::String),
            "chandle" => Ok(Self::Chandle),
            other => Err(format!("dpi: unsupported SystemVerilog DPI type `{other}`")),
        }
    }
}

/// Direction of a DPI-C formal parameter (`input`, `output`, `inout`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DpiParamDir {
    In,
    Out,
    InOut,
}

/// One formal parameter in a [`DpiSignature`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DpiParamDecl {
    pub name: String,
    pub ty: DpiType,
    pub dir: DpiParamDir,
}

/// Runtime value passed across the DPI-C bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DpiValue {
    Void,
    Bit(bool),
    Int(i64),
    UInt(u64),
    Str(String),
    Chandle(usize),
}

impl DpiValue {
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Self::Bit(b) => Some(i64::from(b)),
            Self::Int(v) => Some(v),
            Self::UInt(v) => Some(v as i64),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Self::Bit(b) => Some(u64::from(b)),
            Self::Int(v) => Some(v as u64),
            Self::UInt(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Self::Bit(b) => Some(b),
            Self::Int(v) => Some(v != 0),
            Self::UInt(v) => Some(v != 0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Check if this value is compatible with formal [`DpiType`].
    pub fn matches_type(&self, ty: DpiType) -> bool {
        match (self, ty) {
            (Self::Void, DpiType::Void) => true,
            (Self::Bit(_), DpiType::Bit | DpiType::Byte | DpiType::ShortInt | DpiType::Int | DpiType::LongInt | DpiType::UnsignedInt | DpiType::UnsignedLongInt) => true,
            (Self::Int(_), DpiType::Bit | DpiType::Byte | DpiType::ShortInt | DpiType::Int | DpiType::LongInt | DpiType::UnsignedInt | DpiType::UnsignedLongInt) => true,
            (Self::UInt(_), DpiType::Bit | DpiType::Byte | DpiType::ShortInt | DpiType::Int | DpiType::LongInt | DpiType::UnsignedInt | DpiType::UnsignedLongInt) => true,
            (Self::Str(_), DpiType::String) => true,
            (Self::Chandle(_), DpiType::Chandle) => true,
            _ => false,
        }
    }
}

/// Parsed SystemVerilog `import "DPI-C"` function signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DpiSignature {
    pub c_name: String,
    pub sv_name: String,
    pub is_pure: bool,
    pub is_context: bool,
    pub return_type: DpiType,
    pub params: Vec<DpiParamDecl>,
}

impl DpiSignature {
    /// Parse a SystemVerilog `import "DPI-C"` declaration such as:
    /// - `import "DPI-C" function int c_add(input int a, input int b);`
    /// - `import "DPI-C" pure function bit [63:0] crc64(input longint data);`
    /// - `import "DPI-C" context c_step = function void sv_step(input int cyc);`
    pub fn parse_import(decl: &str) -> Result<Self, String> {
        let trimmed = decl.trim().trim_end_matches(';').trim();
        let rest = trimmed
            .strip_prefix("import")
            .ok_or_else(|| format!("dpi parse: declaration must start with `import`: {decl}"))?
            .trim();
        let rest = rest
            .strip_prefix("\"DPI-C\"")
            .or_else(|| rest.strip_prefix("\"DPI\""))
            .ok_or_else(|| format!("dpi parse: expected \"DPI-C\" spec string: {decl}"))?
            .trim();

        let mut is_pure = false;
        let mut is_context = false;
        let mut cur = rest;
        loop {
            if let Some(r) = cur.strip_prefix("pure") {
                if r.starts_with(char::is_whitespace) {
                    is_pure = true;
                    cur = r.trim();
                    continue;
                }
            }
            if let Some(r) = cur.strip_prefix("context") {
                if r.starts_with(char::is_whitespace) {
                    is_context = true;
                    cur = r.trim();
                    continue;
                }
            }
            break;
        }

        // Optional C linkage name before `= function`.
        let mut explicit_c_name: Option<String> = None;
        let fn_body = if let Some((before_eq, after_eq)) = cur.split_once('=') {
            let cname = before_eq.trim();
            if cname.is_empty() {
                return Err(format!("dpi parse: empty C identifier before '=': {decl}"));
            }
            explicit_c_name = Some(cname.to_string());
            after_eq.trim()
        } else {
            cur
        };

        let after_fn = fn_body
            .strip_prefix("function")
            .ok_or_else(|| format!("dpi parse: expected `function` keyword: {decl}"))?
            .trim();

        let (ret_and_name, params_raw) = after_fn
            .split_once('(')
            .ok_or_else(|| format!("dpi parse: missing parameter list `(...)`: {decl}"))?;
        let params_raw = params_raw
            .strip_suffix(')')
            .ok_or_else(|| format!("dpi parse: missing closing `)` in parameter list: {decl}"))?
            .trim();

        let ret_and_name = ret_and_name.trim();
        let last_space = ret_and_name
            .rfind(char::is_whitespace)
            .ok_or_else(|| format!("dpi parse: missing return type or function name: {decl}"))?;
        let ret_str = ret_and_name[..last_space].trim();
        let sv_name = ret_and_name[last_space..].trim().to_string();
        if sv_name.is_empty() {
            return Err(format!("dpi parse: empty function name: {decl}"));
        }
        let return_type = DpiType::parse_sv(ret_str)?;
        let c_name = explicit_c_name.unwrap_or_else(|| sv_name.clone());

        let mut params = Vec::new();
        if !params_raw.is_empty() && params_raw != "void" {
            for part in params_raw.split(',') {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                let (dir, rem) = if let Some(r) = p.strip_prefix("input") {
                    if r.starts_with(char::is_whitespace) {
                        (DpiParamDir::In, r.trim())
                    } else {
                        (DpiParamDir::In, p)
                    }
                } else if let Some(r) = p.strip_prefix("output") {
                    if r.starts_with(char::is_whitespace) {
                        (DpiParamDir::Out, r.trim())
                    } else {
                        (DpiParamDir::In, p)
                    }
                } else if let Some(r) = p.strip_prefix("inout") {
                    if r.starts_with(char::is_whitespace) {
                        (DpiParamDir::InOut, r.trim())
                    } else {
                        (DpiParamDir::In, p)
                    }
                } else {
                    (DpiParamDir::In, p)
                };
                let split_idx = rem.rfind(char::is_whitespace).ok_or_else(|| {
                    format!("dpi parse: parameter `{p}` missing type or name in {sv_name}")
                })?;
                let ty_str = rem[..split_idx].trim();
                let pname = rem[split_idx..].trim().to_string();
                let ty = DpiType::parse_sv(ty_str)?;
                params.push(DpiParamDecl {
                    name: pname,
                    ty,
                    dir,
                });
            }
        }

        Ok(Self {
            c_name,
            sv_name,
            is_pure,
            is_context,
            return_type,
            params,
        })
    }
}

/// Recorded invocation of a DPI-C function during simulation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DpiCallRecord {
    pub sv_name: String,
    pub c_name: String,
    pub args: Vec<DpiValue>,
    pub result: DpiValue,
}

type DpiCallback = Arc<dyn Fn(&[DpiValue]) -> Result<DpiValue, String> + Send + Sync>;

#[derive(Clone)]
struct DpiEntry {
    sig: DpiSignature,
    callback: DpiCallback,
    calls: u64,
}

impl std::fmt::Debug for DpiEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DpiEntry")
            .field("sig", &self.sig)
            .field("calls", &self.calls)
            .finish()
    }
}

/// Registry and execution bridge for SystemVerilog DPI-C imported functions.
#[derive(Clone, Debug, Default)]
pub struct DpiRegistry {
    entries: HashMap<String, DpiEntry>,
    c_name_alias: HashMap<String, String>,
    trace: Vec<DpiCallRecord>,
}

impl DpiRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a SystemVerilog `import "DPI-C" function ...` declaration and bind its implementation.
    pub fn register_import<F>(&mut self, import_decl: &str, callback: F) -> Result<(), String>
    where
        F: Fn(&[DpiValue]) -> Result<DpiValue, String> + Send + Sync + 'static,
    {
        let sig = DpiSignature::parse_import(import_decl)?;
        self.register_signature(sig, callback);
        Ok(())
    }

    /// Register a pre-parsed [`DpiSignature`] and its callback implementation.
    pub fn register_signature<F>(&mut self, sig: DpiSignature, callback: F)
    where
        F: Fn(&[DpiValue]) -> Result<DpiValue, String> + Send + Sync + 'static,
    {
        let sv_name = sig.sv_name.clone();
        let c_name = sig.c_name.clone();
        self.c_name_alias.insert(c_name, sv_name.clone());
        self.entries.insert(
            sv_name,
            DpiEntry {
                sig,
                callback: Arc::new(callback),
                calls: 0,
            },
        );
    }

    /// Look up the [`DpiSignature`] of a registered function by SV name or C linkage name.
    pub fn signature(&self, name: &str) -> Option<&DpiSignature> {
        if let Some(e) = self.entries.get(name) {
            return Some(&e.sig);
        }
        let sv = self.c_name_alias.get(name)?;
        self.entries.get(sv).map(|e| &e.sig)
    }

    /// Number of times `name` has been invoked.
    pub fn call_count(&self, name: &str) -> u64 {
        if let Some(e) = self.entries.get(name) {
            return e.calls;
        }
        if let Some(sv) = self.c_name_alias.get(name) {
            if let Some(e) = self.entries.get(sv) {
                return e.calls;
            }
        }
        0
    }

    /// Total DPI-C function invocations across all registered functions.
    pub fn total_calls(&self) -> u64 {
        self.entries.values().map(|e| e.calls).sum()
    }

    /// Recorded DPI-C call trace.
    pub fn call_trace(&self) -> &[DpiCallRecord] {
        &self.trace
    }

    /// Clear the recorded DPI-C call trace.
    pub fn clear_trace(&mut self) {
        self.trace.clear();
    }

    /// Invoke a registered DPI-C function by SV name or C linkage name, validating
    /// argument count, parameter types, and return type.
    pub fn call(&mut self, name: &str, args: &[DpiValue]) -> Result<DpiValue, String> {
        let key = if self.entries.contains_key(name) {
            name.to_string()
        } else if let Some(sv) = self.c_name_alias.get(name) {
            sv.clone()
        } else {
            return Err(format!("dpi call: unregistered DPI-C function `{name}`"));
        };

        let entry = self.entries.get_mut(&key).unwrap();
        if args.len() != entry.sig.params.len() {
            return Err(format!(
                "dpi call `{}`: expected {} arguments, got {}",
                entry.sig.sv_name,
                entry.sig.params.len(),
                args.len()
            ));
        }
        for (idx, (arg, param)) in args.iter().zip(entry.sig.params.iter()).enumerate() {
            if !arg.matches_type(param.ty) {
                return Err(format!(
                    "dpi call `{}`: argument {} (`{}`) value {arg:?} does not match {:?}",
                    entry.sig.sv_name, idx, param.name, param.ty
                ));
            }
        }

        let res = (entry.callback)(args)?;
        if !res.matches_type(entry.sig.return_type) {
            return Err(format!(
                "dpi call `{}`: return value {res:?} does not match declared return type {:?}",
                entry.sig.sv_name, entry.sig.return_type
            ));
        }
        entry.calls += 1;
        let rec = DpiCallRecord {
            sv_name: entry.sig.sv_name.clone(),
            c_name: entry.sig.c_name.clone(),
            args: args.to_vec(),
            result: res.clone(),
        };
        self.trace.push(rec);
        Ok(res)
    }
}
