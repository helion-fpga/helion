//! Vivado HLS (UG902) inline `#pragma HLS ...` and TCL `set_directive_...` parser.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionType {
    Complete,
    Block,
    Cyclic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlineMode {
    Inline,
    Off,
    Recursive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataflowChannelKind {
    Fifo,
    PingPong,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationKind {
    Operation,
    Function,
    Core,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterfaceMode {
    SAxiLite,
    MAxi,
    Axis,
    ApFifo,
    ApMemory,
    ApCtrlHs,
    ApCtrlNone,
    ApCtrlChain,
    ApNone,
    ApVld,
    ApHs,
}

impl InterfaceMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "s_axilite" | "axilite" => Some(Self::SAxiLite),
            "m_axi" | "axi_master" => Some(Self::MAxi),
            "axis" | "axi_stream" => Some(Self::Axis),
            "ap_fifo" | "fifo" => Some(Self::ApFifo),
            "ap_memory" | "bram" | "ram_1p" | "ram_2p" | "ram_t2p" => Some(Self::ApMemory),
            "ap_ctrl_hs" => Some(Self::ApCtrlHs),
            "ap_ctrl_none" => Some(Self::ApCtrlNone),
            "ap_ctrl_chain" => Some(Self::ApCtrlChain),
            "ap_none" => Some(Self::ApNone),
            "ap_vld" => Some(Self::ApVld),
            "ap_hs" | "ap_ack" => Some(Self::ApHs),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SAxiLite => "s_axilite",
            Self::MAxi => "m_axi",
            Self::Axis => "axis",
            Self::ApFifo => "ap_fifo",
            Self::ApMemory => "ap_memory",
            Self::ApCtrlHs => "ap_ctrl_hs",
            Self::ApCtrlNone => "ap_ctrl_none",
            Self::ApCtrlChain => "ap_ctrl_chain",
            Self::ApNone => "ap_none",
            Self::ApVld => "ap_vld",
            Self::ApHs => "ap_hs",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineStyle {
    Stp,
    Flp,
    Frp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PipelineDirective {
    pub ii: Option<u32>,
    pub rewind: bool,
    pub enable_flush: bool,
    pub style: PipelineStyle,
    pub off: bool,
    pub target: Option<String>,
}

impl Default for PipelineDirective {
    fn default() -> Self {
        Self {
            ii: Some(1),
            rewind: false,
            enable_flush: false,
            style: PipelineStyle::Stp,
            off: false,
            target: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnrollDirective {
    /// `None` means complete unroll; `Some(k)` means unroll by factor `k`.
    pub factor: Option<usize>,
    pub skip_exit_check: bool,
    pub off: bool,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArrayPartitionDirective {
    pub variable: String,
    pub kind: PartitionType,
    pub factor: Option<usize>,
    /// 1-based dimension index (`0` means all dimensions).
    pub dim: usize,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineDirective {
    pub mode: InlineMode,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataflowDirective {
    pub interval: Option<u32>,
    pub channel: DataflowChannelKind,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllocationDirective {
    pub instances: String,
    pub limit: usize,
    pub kind: AllocationKind,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceDirective {
    pub mode: InterfaceMode,
    pub port: String,
    pub bundle: Option<String>,
    pub depth: Option<usize>,
    pub offset: Option<String>,
    pub register: bool,
    pub target: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependenceType {
    Inter,
    Intra,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependenceDirection {
    Raw,
    War,
    Waw,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependenceDirective {
    pub variable: String,
    pub dep_type: DependenceType,
    pub dependent: bool,
    pub direction: DependenceDirection,
    pub distance: Option<u32>,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HlsDirective {
    Pipeline(PipelineDirective),
    Unroll(UnrollDirective),
    ArrayPartition(ArrayPartitionDirective),
    Inline(InlineDirective),
    Dataflow(DataflowDirective),
    Allocation(AllocationDirective),
    Interface(InterfaceDirective),
    Dependence(DependenceDirective),
}

impl HlsDirective {
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::Pipeline(d) => d.target.as_deref(),
            Self::Unroll(d) => d.target.as_deref(),
            Self::ArrayPartition(d) => d.target.as_deref(),
            Self::Inline(d) => d.target.as_deref(),
            Self::Dataflow(d) => d.target.as_deref(),
            Self::Allocation(d) => d.target.as_deref(),
            Self::Interface(d) => d.target.as_deref(),
            Self::Dependence(d) => d.target.as_deref(),
        }
    }

    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        let t = Some(target.into());
        match &mut self {
            Self::Pipeline(d) => d.target = t,
            Self::Unroll(d) => d.target = t,
            Self::ArrayPartition(d) => d.target = t,
            Self::Inline(d) => d.target = t,
            Self::Dataflow(d) => d.target = t,
            Self::Allocation(d) => d.target = t,
            Self::Interface(d) => d.target = t,
            Self::Dependence(d) => d.target = t,
        }
        self
    }
}

/// Configuration for an HLS synthesis run (clock period, top function, external TCL directives).
#[derive(Clone, Debug)]
pub struct HlsConfig {
    pub target_clock_ns: f64,
    pub top_function: Option<String>,
    pub directives: Vec<HlsDirective>,
    pub part_name: String,
}

impl Default for HlsConfig {
    fn default() -> Self {
        Self {
            target_clock_ns: 10.0,
            top_function: None,
            directives: Vec::new(),
            part_name: "xc7a35tcpg236-1".into(),
        }
    }
}

impl HlsConfig {
    pub fn from_tcl(tcl: &str) -> Result<Self, String> {
        parse_tcl_config(tcl)
    }
}

/// Split pragma tokens handling `key = value` (with optional spaces around `=`) and quoted strings.
fn split_pragma_tokens(rest: &str) -> Vec<String> {
    let mut raw = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in rest.chars() {
        if c == '"' || c == '\'' {
            in_quotes = !in_quotes;
            continue;
        }
        if !in_quotes && c == '=' {
            if !cur.is_empty() {
                raw.push(std::mem::take(&mut cur));
            }
            raw.push("=".into());
            continue;
        }
        if !in_quotes && (c.is_whitespace() || c == ',') {
            if !cur.is_empty() {
                raw.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        raw.push(cur);
    }
    // Merge `key`, `=`, `val` into `key=val`.
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if i + 2 < raw.len() && raw[i + 1] == "=" {
            out.push(format!("{}={}", raw[i], raw[i + 2]));
            i += 3;
        } else if raw[i] != "=" {
            out.push(raw[i].clone());
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

fn parse_kv_and_flags(tokens: &[String]) -> (BTreeMap<String, String>, Vec<String>) {
    let mut kv = BTreeMap::new();
    let mut flags = Vec::new();
    for tok in tokens {
        if let Some((k, v)) = tok.split_once('=') {
            kv.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        } else {
            flags.push(tok.trim().to_string());
        }
    }
    (kv, flags)
}

/// Parse a single `#pragma HLS ...` or `#pragma AP ...` line.
/// Returns `Ok(None)` if the line is not an HLS pragma.
pub fn parse_pragma_line(line: &str) -> Result<Option<HlsDirective>, String> {
    let trimmed = line.split("//").next().unwrap_or("").trim();
    let Some(after_hash) = trimmed.strip_prefix('#') else {
        return Ok(None);
    };
    let after_hash = after_hash.trim_start();
    let lower = after_hash.to_ascii_lowercase();
    if !lower.starts_with("pragma") {
        return Ok(None);
    }
    let after_pragma = after_hash["pragma".len()..].trim_start();
    let tokens = split_pragma_tokens(after_pragma);
    if tokens.is_empty() {
        return Ok(None);
    }
    let ns = tokens[0].to_ascii_lowercase();
    if ns != "hls" && ns != "ap" && ns != "autoesl" {
        return Ok(None);
    }
    if tokens.len() < 2 {
        return Err("empty #pragma HLS directive".into());
    }
    let cmd = tokens[1].to_ascii_lowercase();
    let args = &tokens[2..];
    let (kv, flags) = parse_kv_and_flags(args);
    let flags_lower: Vec<String> = flags.iter().map(|f| f.to_ascii_lowercase()).collect();

    match cmd.as_str() {
        "pipeline" => {
            let off = flags_lower.iter().any(|f| f == "off");
            let rewind = flags_lower.iter().any(|f| f == "rewind")
                || kv.get("rewind").is_some_and(|v| v == "true" || v == "1");
            let enable_flush = flags_lower.iter().any(|f| f == "enable_flush");
            let ii = if off {
                None
            } else if let Some(v) = kv.get("ii") {
                Some(
                    v.parse::<u32>()
                        .map_err(|_| format!("invalid PIPELINE II={v}"))?
                        .max(1),
                )
            } else {
                Some(1)
            };
            let style = match kv.get("style").map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("flp") => PipelineStyle::Flp,
                Some("frp") => PipelineStyle::Frp,
                _ => PipelineStyle::Stp,
            };
            Ok(Some(HlsDirective::Pipeline(PipelineDirective {
                ii,
                rewind,
                enable_flush,
                style,
                off,
                target: kv.get("target").cloned(),
            })))
        }
        "unroll" => {
            let off = flags_lower.iter().any(|f| f == "off");
            let skip_exit_check = flags_lower.iter().any(|f| f == "skip_exit_check");
            let factor = if off {
                Some(1)
            } else if let Some(v) = kv.get("factor") {
                Some(
                    v.parse::<usize>()
                        .map_err(|_| format!("invalid UNROLL factor={v}"))?
                        .max(1),
                )
            } else {
                None
            };
            Ok(Some(HlsDirective::Unroll(UnrollDirective {
                factor,
                skip_exit_check,
                off,
                target: kv.get("target").cloned(),
            })))
        }
        "array_partition" => {
            let variable = kv
                .get("variable")
                .or_else(|| kv.get("var"))
                .cloned()
                .or_else(|| {
                    flags
                        .iter()
                        .find(|f| {
                            !matches!(
                                f.to_ascii_lowercase().as_str(),
                                "complete" | "block" | "cyclic"
                            )
                        })
                        .cloned()
                })
                .ok_or_else(|| "ARRAY_PARTITION missing variable=...".to_string())?;
            let kind_str = kv
                .get("type")
                .cloned()
                .or_else(|| {
                    flags_lower
                        .iter()
                        .find(|f| matches!(f.as_str(), "complete" | "block" | "cyclic"))
                        .cloned()
                })
                .unwrap_or_else(|| "complete".into());
            let kind = match kind_str.to_ascii_lowercase().as_str() {
                "complete" => PartitionType::Complete,
                "block" => PartitionType::Block,
                "cyclic" => PartitionType::Cyclic,
                other => return Err(format!("unknown ARRAY_PARTITION type={other}")),
            };
            let factor = match kv.get("factor") {
                Some(v) => Some(
                    v.parse::<usize>()
                        .map_err(|_| format!("invalid ARRAY_PARTITION factor={v}"))?
                        .max(1),
                ),
                None => {
                    if kind == PartitionType::Complete {
                        None
                    } else {
                        Some(2)
                    }
                }
            };
            let dim = match kv.get("dim") {
                Some(v) => v
                    .parse::<usize>()
                    .map_err(|_| format!("invalid ARRAY_PARTITION dim={v}"))?,
                None => 1,
            };
            Ok(Some(HlsDirective::ArrayPartition(
                ArrayPartitionDirective {
                    variable,
                    kind,
                    factor,
                    dim,
                    target: kv.get("target").cloned(),
                },
            )))
        }
        "inline" => {
            let mode = if flags_lower.iter().any(|f| f == "off")
                || kv.get("mode").is_some_and(|v| v.eq_ignore_ascii_case("off"))
            {
                InlineMode::Off
            } else if flags_lower.iter().any(|f| f == "recursive")
                || kv
                    .get("mode")
                    .is_some_and(|v| v.eq_ignore_ascii_case("recursive"))
            {
                InlineMode::Recursive
            } else {
                InlineMode::Inline
            };
            Ok(Some(HlsDirective::Inline(InlineDirective {
                mode,
                target: kv.get("target").or_else(|| kv.get("region")).cloned(),
            })))
        }
        "dataflow" => {
            let interval = match kv.get("interval") {
                Some(v) => Some(
                    v.parse::<u32>()
                        .map_err(|_| format!("invalid DATAFLOW interval={v}"))?,
                ),
                None => None,
            };
            let channel = match kv
                .get("default_channel")
                .or_else(|| kv.get("channel"))
                .map(|s| s.to_ascii_lowercase())
                .as_deref()
            {
                Some("pingpong") | Some("pipo") => DataflowChannelKind::PingPong,
                _ => DataflowChannelKind::Fifo,
            };
            Ok(Some(HlsDirective::Dataflow(DataflowDirective {
                interval,
                channel,
                target: kv.get("target").cloned(),
            })))
        }
        "allocation" => {
            let instances = kv
                .get("instances")
                .or_else(|| kv.get("instance"))
                .cloned()
                .ok_or_else(|| "ALLOCATION missing instances=...".to_string())?;
            let limit = kv
                .get("limit")
                .ok_or_else(|| "ALLOCATION missing limit=...".to_string())?
                .parse::<usize>()
                .map_err(|_| "invalid ALLOCATION limit".to_string())?;
            let kind = if flags_lower.iter().any(|f| f == "function")
                || kv
                    .get("type")
                    .is_some_and(|v| v.eq_ignore_ascii_case("function"))
            {
                AllocationKind::Function
            } else if flags_lower.iter().any(|f| f == "core")
                || kv
                    .get("type")
                    .is_some_and(|v| v.eq_ignore_ascii_case("core"))
            {
                AllocationKind::Core
            } else {
                AllocationKind::Operation
            };
            Ok(Some(HlsDirective::Allocation(AllocationDirective {
                instances,
                limit,
                kind,
                target: kv.get("target").cloned(),
            })))
        }
        "interface" => {
            let mode_str = kv
                .get("mode")
                .cloned()
                .or_else(|| {
                    flags
                        .iter()
                        .find(|f| InterfaceMode::parse(f).is_some())
                        .cloned()
                })
                .ok_or_else(|| "INTERFACE missing mode".to_string())?;
            let mode = InterfaceMode::parse(&mode_str)
                .ok_or_else(|| format!("unsupported INTERFACE mode={mode_str}"))?;
            let port = kv
                .get("port")
                .cloned()
                .unwrap_or_else(|| "return".into());
            let bundle = kv.get("bundle").cloned();
            let depth = match kv.get("depth") {
                Some(v) => Some(
                    v.parse::<usize>()
                        .map_err(|_| format!("invalid INTERFACE depth={v}"))?,
                ),
                None => None,
            };
            let offset = kv.get("offset").cloned();
            let register = flags_lower.iter().any(|f| f == "register")
                || kv
                    .get("register")
                    .is_some_and(|v| v == "true" || v == "1" || v.eq_ignore_ascii_case("both"));
            Ok(Some(HlsDirective::Interface(InterfaceDirective {
                mode,
                port,
                bundle,
                depth,
                offset,
                register,
                target: kv.get("target").cloned(),
            })))
        }
        "dependence" => {
            let variable = kv
                .get("variable")
                .or_else(|| kv.get("var"))
                .cloned()
                .unwrap_or_else(|| "*".into());
            let dep_type = if flags_lower.iter().any(|f| f == "intra")
                || kv
                    .get("type")
                    .is_some_and(|v| v.eq_ignore_ascii_case("intra"))
            {
                DependenceType::Intra
            } else {
                DependenceType::Inter
            };
            let dependent = if flags_lower.iter().any(|f| f == "false")
                || kv
                    .get("dependent")
                    .is_some_and(|v| v.eq_ignore_ascii_case("false") || v == "0")
            {
                false
            } else {
                true
            };
            let direction = if flags_lower.iter().any(|f| f == "war")
                || kv
                    .get("direction")
                    .is_some_and(|v| v.eq_ignore_ascii_case("war"))
            {
                DependenceDirection::War
            } else if flags_lower.iter().any(|f| f == "waw")
                || kv
                    .get("direction")
                    .is_some_and(|v| v.eq_ignore_ascii_case("waw"))
            {
                DependenceDirection::Waw
            } else {
                DependenceDirection::Raw
            };
            let distance = match kv.get("distance") {
                Some(v) => Some(
                    v.parse::<u32>()
                        .map_err(|_| format!("invalid DEPENDENCE distance={v}"))?
                        .max(1),
                ),
                None => None,
            };
            Ok(Some(HlsDirective::Dependence(DependenceDirective {
                variable,
                dep_type,
                dependent,
                direction,
                distance,
                target: kv.get("target").cloned(),
            })))
        }
        "loop_tripcount" | "bind_storage" | "bind_op" | "stream" => {
            if cmd == "bind_op" {
                if let Some(op) = kv.get("op") {
                    return Ok(Some(HlsDirective::Allocation(AllocationDirective {
                        instances: op.clone(),
                        limit: usize::MAX,
                        kind: AllocationKind::Operation,
                        target: kv.get("variable").cloned(),
                    })));
                }
            }
            Ok(None)
        }
        other => Err(format!("unsupported #pragma HLS {other}")),
    }
}

/// Extract all inline `#pragma HLS ...` directives from C/C++ source code.
pub fn extract_inline_pragmas(source: &str) -> Result<Vec<HlsDirective>, String> {
    let mut joined_lines = Vec::new();
    let mut buf = String::new();
    for raw_line in source.lines() {
        let line = raw_line.split("//").next().unwrap_or("").trim_end();
        if let Some(stripped) = line.strip_suffix('\\') {
            buf.push_str(stripped);
            buf.push(' ');
        } else if !buf.is_empty() {
            buf.push_str(line);
            joined_lines.push(std::mem::take(&mut buf));
        } else {
            joined_lines.push(line.to_string());
        }
    }
    if !buf.is_empty() {
        joined_lines.push(buf);
    }
    let mut out = Vec::new();
    for line in &joined_lines {
        if let Some(d) = parse_pragma_line(line)? {
            out.push(d);
        }
    }
    Ok(out)
}

fn tokenize_tcl_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut brace_depth = 0usize;
    for c in line.chars() {
        if c == '"' && brace_depth == 0 {
            in_quotes = !in_quotes;
            continue;
        }
        if !in_quotes {
            if c == '{' {
                brace_depth += 1;
                if brace_depth == 1 {
                    continue;
                }
            } else if c == '}' && brace_depth > 0 {
                brace_depth -= 1;
                if brace_depth == 0 {
                    continue;
                }
            }
        }
        if !in_quotes && brace_depth == 0 && c.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Parse Vivado HLS TCL directives (`set_directive_pipeline`, `set_directive_unroll`,
/// `set_directive_array_partition`, `set_directive_inline`, `set_directive_dataflow`,
/// `set_directive_allocation`, `set_directive_interface`, `create_clock`, `set_top`, `set_part`).
pub fn parse_tcl_config(tcl: &str) -> Result<HlsConfig, String> {
    let mut cfg = HlsConfig::default();
    for raw_line in tcl.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let toks = tokenize_tcl_line(line);
        if toks.is_empty() {
            continue;
        }
        let cmd = toks[0].to_ascii_lowercase();
        let args = &toks[1..];
        match cmd.as_str() {
            "create_clock" => {
                let mut i = 0;
                while i < args.len() {
                    if args[i].eq_ignore_ascii_case("-period") && i + 1 < args.len() {
                        let p_str = args[i + 1].trim_end_matches("ns");
                        cfg.target_clock_ns = p_str
                            .parse::<f64>()
                            .map_err(|_| format!("invalid create_clock -period {}", args[i + 1]))?;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            "set_top" => {
                if let Some(top) = args.first() {
                    cfg.top_function = Some(top.clone());
                }
            }
            "set_part" => {
                if let Some(part) = args.last() {
                    cfg.part_name = part.clone();
                }
            }
            "set_directive_pipeline" => {
                let mut ii = Some(1u32);
                let mut off = false;
                let mut rewind = false;
                let mut enable_flush = false;
                let mut style = PipelineStyle::Stp;
                let mut positional = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    match args[i].to_ascii_lowercase().as_str() {
                        "-ii" if i + 1 < args.len() => {
                            let v = args[i + 1]
                                .parse::<u32>()
                                .map_err(|_| format!("invalid -II {}", args[i + 1]))?;
                            ii = Some(v.max(1));
                            i += 2;
                        }
                        "-off" => {
                            off = true;
                            ii = None;
                            i += 1;
                        }
                        "-rewind" => {
                            rewind = true;
                            i += 1;
                        }
                        "-enable_flush" => {
                            enable_flush = true;
                            i += 1;
                        }
                        "-style" if i + 1 < args.len() => {
                            style = match args[i + 1].to_ascii_lowercase().as_str() {
                                "flp" => PipelineStyle::Flp,
                                "frp" => PipelineStyle::Frp,
                                _ => PipelineStyle::Stp,
                            };
                            i += 2;
                        }
                        s if !s.starts_with('-') => {
                            positional.push(args[i].clone());
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                cfg.directives.push(HlsDirective::Pipeline(PipelineDirective {
                    ii,
                    rewind,
                    enable_flush,
                    style,
                    off,
                    target: positional.first().cloned(),
                }));
            }
            "set_directive_unroll" => {
                let mut factor = None;
                let mut off = false;
                let mut skip_exit_check = false;
                let mut positional = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    match args[i].to_ascii_lowercase().as_str() {
                        "-factor" if i + 1 < args.len() => {
                            let v = args[i + 1]
                                .parse::<usize>()
                                .map_err(|_| format!("invalid -factor {}", args[i + 1]))?;
                            factor = Some(v.max(1));
                            i += 2;
                        }
                        "-off" => {
                            off = true;
                            factor = Some(1);
                            i += 1;
                        }
                        "-skip_exit_check" => {
                            skip_exit_check = true;
                            i += 1;
                        }
                        s if !s.starts_with('-') => {
                            positional.push(args[i].clone());
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                cfg.directives.push(HlsDirective::Unroll(UnrollDirective {
                    factor,
                    skip_exit_check,
                    off,
                    target: positional.first().cloned(),
                }));
            }
            "set_directive_array_partition" => {
                let mut kind = PartitionType::Complete;
                let mut factor = None;
                let mut dim = 1usize;
                let mut positional = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    match args[i].to_ascii_lowercase().as_str() {
                        "-type" if i + 1 < args.len() => {
                            kind = match args[i + 1].to_ascii_lowercase().as_str() {
                                "complete" => PartitionType::Complete,
                                "block" => PartitionType::Block,
                                "cyclic" => PartitionType::Cyclic,
                                other => {
                                    return Err(format!(
                                        "unknown set_directive_array_partition -type {other}"
                                    ))
                                }
                            };
                            i += 2;
                        }
                        "-factor" if i + 1 < args.len() => {
                            let v = args[i + 1]
                                .parse::<usize>()
                                .map_err(|_| format!("invalid -factor {}", args[i + 1]))?;
                            factor = Some(v.max(1));
                            i += 2;
                        }
                        "-dim" if i + 1 < args.len() => {
                            dim = args[i + 1]
                                .parse::<usize>()
                                .map_err(|_| format!("invalid -dim {}", args[i + 1]))?;
                            i += 2;
                        }
                        s if !s.starts_with('-') => {
                            positional.push(args[i].clone());
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                let (target, variable) = if positional.len() >= 2 {
                    (Some(positional[0].clone()), positional[1].clone())
                } else if positional.len() == 1 {
                    (None, positional[0].clone())
                } else {
                    return Err("set_directive_array_partition missing variable".into());
                };
                if kind != PartitionType::Complete && factor.is_none() {
                    factor = Some(2);
                }
                cfg.directives
                    .push(HlsDirective::ArrayPartition(ArrayPartitionDirective {
                        variable,
                        kind,
                        factor,
                        dim,
                        target,
                    }));
            }
            "set_directive_inline" => {
                let mut mode = InlineMode::Inline;
                let mut positional = Vec::new();
                for a in args {
                    match a.to_ascii_lowercase().as_str() {
                        "-off" => mode = InlineMode::Off,
                        "-recursive" => mode = InlineMode::Recursive,
                        s if !s.starts_with('-') => positional.push(a.clone()),
                        _ => {}
                    }
                }
                cfg.directives.push(HlsDirective::Inline(InlineDirective {
                    mode,
                    target: positional.first().cloned(),
                }));
            }
            "set_directive_dataflow" => {
                let mut interval = None;
                let mut channel = DataflowChannelKind::Fifo;
                let mut positional = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    match args[i].to_ascii_lowercase().as_str() {
                        "-interval" if i + 1 < args.len() => {
                            interval = args[i + 1].parse::<u32>().ok();
                            i += 2;
                        }
                        "-default_channel" if i + 1 < args.len() => {
                            if args[i + 1].eq_ignore_ascii_case("pingpong") {
                                channel = DataflowChannelKind::PingPong;
                            }
                            i += 2;
                        }
                        s if !s.starts_with('-') => {
                            positional.push(args[i].clone());
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                cfg.directives
                    .push(HlsDirective::Dataflow(DataflowDirective {
                        interval,
                        channel,
                        target: positional.first().cloned(),
                    }));
            }
            "set_directive_allocation" => {
                let mut limit = 1usize;
                let mut kind = AllocationKind::Operation;
                let mut positional = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    match args[i].to_ascii_lowercase().as_str() {
                        "-limit" if i + 1 < args.len() => {
                            limit = args[i + 1]
                                .parse::<usize>()
                                .map_err(|_| format!("invalid -limit {}", args[i + 1]))?;
                            i += 2;
                        }
                        "-type" if i + 1 < args.len() => {
                            kind = match args[i + 1].to_ascii_lowercase().as_str() {
                                "function" => AllocationKind::Function,
                                "core" => AllocationKind::Core,
                                _ => AllocationKind::Operation,
                            };
                            i += 2;
                        }
                        s if !s.starts_with('-') => {
                            positional.push(args[i].clone());
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                let (target, instances) = if positional.len() >= 2 {
                    (Some(positional[0].clone()), positional[1].clone())
                } else if positional.len() == 1 {
                    (None, positional[0].clone())
                } else {
                    return Err("set_directive_allocation missing instances".into());
                };
                cfg.directives
                    .push(HlsDirective::Allocation(AllocationDirective {
                        instances,
                        limit,
                        kind,
                        target,
                    }));
            }
            "set_directive_interface" => {
                let mut mode = None;
                let mut bundle = None;
                let mut depth = None;
                let mut offset = None;
                let mut register = false;
                let mut positional = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    match args[i].to_ascii_lowercase().as_str() {
                        "-mode" if i + 1 < args.len() => {
                            mode = InterfaceMode::parse(&args[i + 1]);
                            i += 2;
                        }
                        "-bundle" if i + 1 < args.len() => {
                            bundle = Some(args[i + 1].clone());
                            i += 2;
                        }
                        "-depth" if i + 1 < args.len() => {
                            depth = args[i + 1].parse::<usize>().ok();
                            i += 2;
                        }
                        "-offset" if i + 1 < args.len() => {
                            offset = Some(args[i + 1].clone());
                            i += 2;
                        }
                        "-register" => {
                            register = true;
                            i += 1;
                        }
                        s if !s.starts_with('-') => {
                            if mode.is_none() {
                                if let Some(m) = InterfaceMode::parse(s) {
                                    mode = Some(m);
                                    i += 1;
                                    continue;
                                }
                            }
                            positional.push(args[i].clone());
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                let mode = mode.ok_or_else(|| "set_directive_interface missing -mode".to_string())?;
                let (target, port) = if positional.len() >= 2 {
                    (Some(positional[0].clone()), positional[1].clone())
                } else if positional.len() == 1 {
                    (None, positional[0].clone())
                } else {
                    (None, "return".into())
                };
                cfg.directives
                    .push(HlsDirective::Interface(InterfaceDirective {
                        mode,
                        port,
                        bundle,
                        depth,
                        offset,
                        register,
                        target,
                    }));
            }
            "set_directive_dependence" => {
                let mut variable = "*".to_string();
                let mut dep_type = DependenceType::Inter;
                let mut dependent = true;
                let mut direction = DependenceDirection::Raw;
                let mut distance = None;
                let mut positional = Vec::new();
                let mut i = 0;
                while i < args.len() {
                    match args[i].to_ascii_lowercase().as_str() {
                        "-variable" if i + 1 < args.len() => {
                            variable = args[i + 1].clone();
                            i += 2;
                        }
                        "-type" if i + 1 < args.len() => {
                            if args[i + 1].eq_ignore_ascii_case("intra") {
                                dep_type = DependenceType::Intra;
                            }
                            i += 2;
                        }
                        "-dependent" if i + 1 < args.len() => {
                            dependent = !args[i + 1].eq_ignore_ascii_case("false")
                                && args[i + 1] != "0";
                            i += 2;
                        }
                        "-direction" if i + 1 < args.len() => {
                            direction = match args[i + 1].to_ascii_lowercase().as_str() {
                                "war" => DependenceDirection::War,
                                "waw" => DependenceDirection::Waw,
                                _ => DependenceDirection::Raw,
                            };
                            i += 2;
                        }
                        "-distance" if i + 1 < args.len() => {
                            distance = args[i + 1].parse::<u32>().ok().map(|d| d.max(1));
                            i += 2;
                        }
                        s if !s.starts_with('-') => {
                            positional.push(args[i].clone());
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                cfg.directives
                    .push(HlsDirective::Dependence(DependenceDirective {
                        variable,
                        dep_type,
                        dependent,
                        direction,
                        distance,
                        target: positional.first().cloned(),
                    }));
            }
            _ => {}
        }
    }
    Ok(cfg)
}

pub fn parse_tcl_directives(tcl: &str) -> Result<Vec<HlsDirective>, String> {
    Ok(parse_tcl_config(tcl)?.directives)
}
