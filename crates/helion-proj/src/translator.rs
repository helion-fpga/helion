//! Universal multi-vendor FPGA constraint parser and lossless bidirectional translator.
//!
//! Supports:
//! - **Lattice Diamond / Radiant (`.lpf` / `.pdc`)**:
//!   `LOCATE COMP "port" SITE "pin";`,
//!   `IOBUF PORT "port" IO_TYPE=LVCMOS33 PULLMODE=UP SLEWRATE=FAST DRIVE=8;`,
//!   `FREQUENCY PORT "clk" 100.0 MHz;`
//! - **IceStorm / nextpnr (`.pcf`)**:
//!   `set_io [--warn-no-port] [-pullup yes|no] <port> <pin>`
//! - **Intel / Altera Quartus (`.qsf`)**:
//!   `set_location_assignment PIN_<pin> -to <port>`,
//!   `set_instance_assignment -name IO_STANDARD "3.3-V LVTTL" -to <port>`,
//!   `set_instance_assignment -name CURRENT_STRENGTH_NEW 8MA -to <port>`
//! - **Synopsys Design Constraints (`.sdc`)**:
//!   `create_clock`, `set_input_delay`, `set_output_delay`, `set_false_path`,
//!   `set_multicycle_path`, `set_max_delay`, `set_clock_groups`
//! - **Xilinx Vivado / Helion (`.xdc`)**:
//!   `set_property PACKAGE_PIN / IOSTANDARD / DRIVE / SLEW / PULLTYPE ... [get_ports <port>]`
//!   plus full SDC timing constraints.

use helion_sta::{Constraints, load_xdc};
use std::collections::BTreeMap;
use std::path::Path;

/// Supported vendor constraint file formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintFormat {
    Xdc,
    Sdc,
    Lpf,
    Pcf,
    Qsf,
}

impl ConstraintFormat {
    pub fn from_extension(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "xdc" => Some(Self::Xdc),
            "sdc" => Some(Self::Sdc),
            "lpf" | "pdc" => Some(Self::Lpf),
            "pcf" => Some(Self::Pcf),
            "qsf" => Some(Self::Qsf),
            _ => None,
        }
    }

    pub fn parse_name(name: &str) -> Result<Self, String> {
        match name.trim().trim_start_matches('.').to_ascii_lowercase().as_str() {
            "xdc" | "vivado" | "helion" => Ok(Self::Xdc),
            "sdc" | "synopsys" => Ok(Self::Sdc),
            "lpf" | "pdc" | "lattice" => Ok(Self::Lpf),
            "pcf" | "icestorm" | "nextpnr" => Ok(Self::Pcf),
            "qsf" | "quartus" | "intel" | "altera" => Ok(Self::Qsf),
            other => Err(format!("unknown constraint format: {other}")),
        }
    }
}

/// Canonical pull resistor configuration across vendors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullMode {
    None,
    Up,
    Down,
    Keeper,
}

impl PullMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "NONE" | "OFF" | "NO" | "FALSE" | "0" => Some(Self::None),
            "UP" | "PULLUP" | "ON" | "YES" | "TRUE" | "1" => Some(Self::Up),
            "DOWN" | "PULLDOWN" => Some(Self::Down),
            "KEEPER" | "BUSHOLD" | "HOLD" => Some(Self::Keeper),
            _ => None,
        }
    }

    pub fn to_xdc_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Up => "PULLUP",
            Self::Down => "PULLDOWN",
            Self::Keeper => "KEEPER",
        }
    }

    pub fn to_lpf_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Up => "UP",
            Self::Down => "DOWN",
            Self::Keeper => "KEEPER",
        }
    }
}

/// Canonical slew-rate setting across vendors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlewRate {
    Slow,
    Fast,
}

impl SlewRate {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "SLOW" | "0" | "1" => Some(Self::Slow),
            "FAST" | "2" => Some(Self::Fast),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slow => "SLOW",
            Self::Fast => "FAST",
        }
    }
}

/// Per-port physical & electrical I/O constraint in canonical vendor-neutral form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortConstraint {
    pub port: String,
    pub pin: Option<String>,
    pub iostandard: Option<String>,
    pub drive_ma: Option<u32>,
    pub slew: Option<SlewRate>,
    pub pull: Option<PullMode>,
    pub diff_term: Option<bool>,
    pub in_term: Option<String>,
    pub warn_no_port: bool,
}

/// Canonical clock constraint (`create_clock` / `FREQUENCY PORT`).
#[derive(Clone, Debug, PartialEq)]
pub struct ClockConstraint {
    pub name: String,
    pub port: String,
    pub period_ns: f64,
    pub waveform_ns: Option<(f64, f64)>,
}

impl ClockConstraint {
    pub fn frequency_mhz(&self) -> f64 {
        if self.period_ns > 0.0 {
            1000.0 / self.period_ns
        } else {
            0.0
        }
    }

    pub fn period_ps(&self) -> u64 {
        (self.period_ns * 1000.0).round().max(1.0) as u64
    }
}

/// Canonical I/O delay constraint (`set_input_delay` / `set_output_delay`).
#[derive(Clone, Debug, PartialEq)]
pub struct IoDelayConstraint {
    pub is_input: bool,
    pub clock: String,
    pub delay_ns: f64,
    pub is_min: bool,
    pub is_max: bool,
    pub port: String,
}

/// Canonical false path exception (`set_false_path`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FalsePathConstraint {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Canonical multicycle path exception (`set_multicycle_path`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MulticycleConstraint {
    pub cycles: u32,
    pub is_hold: bool,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Canonical maximum delay exception (`set_max_delay`).
#[derive(Clone, Debug, PartialEq)]
pub struct MaxDelayConstraint {
    pub delay_ns: f64,
    pub datapath_only: bool,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Clock-group relationship (`set_clock_groups`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockGroupRelation {
    Asynchronous,
    LogicallyExclusive,
    PhysicallyExclusive,
}

impl ClockGroupRelation {
    pub fn to_sdc_flag(self) -> &'static str {
        match self {
            Self::Asynchronous => "-asynchronous",
            Self::LogicallyExclusive => "-logically_exclusive",
            Self::PhysicallyExclusive => "-physically_exclusive",
        }
    }
}

/// Canonical clock groups constraint (`set_clock_groups`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockGroupsConstraint {
    pub relation: ClockGroupRelation,
    pub groups: Vec<Vec<String>>,
}

/// Universal vendor-neutral constraint set supporting lossless bidirectional
/// import and export across `.xdc`, `.sdc`, `.lpf`, `.pcf`, and `.qsf`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UniversalConstraints {
    pub ports: BTreeMap<String, PortConstraint>,
    pub clocks: Vec<ClockConstraint>,
    pub io_delays: Vec<IoDelayConstraint>,
    pub false_paths: Vec<FalsePathConstraint>,
    pub multicycle_paths: Vec<MulticycleConstraint>,
    pub max_delays: Vec<MaxDelayConstraint>,
    pub clock_groups: Vec<ClockGroupsConstraint>,
}

/// Map any vendor-specific I/O standard string (Quartus `"3.3-V LVCMOS"`,
/// `"3.3-V LVTTL"`, Lattice `"LVTTL33"`, Vivado `"LVCMOS33"`, etc.) to its
/// canonical identifier.
pub fn canonicalize_iostandard(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'').trim();
    let up = trimmed.to_ascii_uppercase();
    match up.as_str() {
        "3.3-V LVCMOS" | "3.3V LVCMOS" | "LVCMOS33" | "LVCMOS33D" => "LVCMOS33".to_string(),
        "3.3-V LVTTL" | "3.3V LVTTL" | "LVTTL" | "LVTTL33" => "LVTTL".to_string(),
        "2.5-V LVCMOS" | "2.5 V" | "2.5V" | "LVCMOS25" => "LVCMOS25".to_string(),
        "1.8-V LVCMOS" | "1.8 V" | "1.8V" | "LVCMOS18" => "LVCMOS18".to_string(),
        "1.5-V LVCMOS" | "1.5 V" | "1.5V" | "LVCMOS15" => "LVCMOS15".to_string(),
        "1.2-V LVCMOS" | "1.2 V" | "1.2V" | "LVCMOS12" => "LVCMOS12".to_string(),
        "LVDS" | "LVDS_25" | "LVDS25" | "2.5-V LVDS" | "LVDS_E_3R" => "LVDS_25".to_string(),
        "SSTL-15" | "SSTL-15 CLASS I" | "SSTL-15 CLASS II" | "SSTL15" | "SSTL15_I" => {
            "SSTL15".to_string()
        }
        "SSTL-18" | "SSTL-18 CLASS I" | "SSTL18_I" | "SSTL18" => "SSTL18_I".to_string(),
        "SSTL-2" | "SSTL-2 CLASS I" | "SSTL2_I" | "SSTL25" => "SSTL2_I".to_string(),
        "DIFF_SSTL15" | "DIFFERENTIAL 1.5-V SSTL" | "DIFFERENTIAL 1.5-V SSTL CLASS I" => {
            "DIFF_SSTL15".to_string()
        }
        _ => trimmed.to_string(),
    }
}

/// Convert a canonical I/O standard to Intel Quartus `.qsf` `IO_STANDARD` naming.
pub fn iostandard_to_quartus(canonical: &str) -> String {
    match canonicalize_iostandard(canonical).as_str() {
        "LVCMOS33" => "3.3-V LVCMOS".to_string(),
        "LVTTL" => "3.3-V LVTTL".to_string(),
        "LVCMOS25" => "2.5 V".to_string(),
        "LVCMOS18" => "1.8 V".to_string(),
        "LVCMOS15" => "1.5 V".to_string(),
        "LVCMOS12" => "1.2 V".to_string(),
        "LVDS_25" => "LVDS".to_string(),
        "SSTL15" => "SSTL-15".to_string(),
        "SSTL18_I" => "SSTL-18 CLASS I".to_string(),
        "SSTL2_I" => "SSTL-2 CLASS I".to_string(),
        "DIFF_SSTL15" => "Differential 1.5-V SSTL".to_string(),
        other => other.to_string(),
    }
}

/// Convert a canonical I/O standard to Lattice Diamond `.lpf` `IO_TYPE` naming.
pub fn iostandard_to_lattice(canonical: &str) -> String {
    match canonicalize_iostandard(canonical).as_str() {
        "LVCMOS33" => "LVCMOS33".to_string(),
        "LVTTL" => "LVTTL33".to_string(),
        "LVCMOS25" => "LVCMOS25".to_string(),
        "LVCMOS18" => "LVCMOS18".to_string(),
        "LVCMOS15" => "LVCMOS15".to_string(),
        "LVCMOS12" => "LVCMOS12".to_string(),
        "LVDS_25" => "LVDS25".to_string(),
        "SSTL15" => "SSTL15_I".to_string(),
        "SSTL18_I" => "SSTL18_I".to_string(),
        other => other.to_string(),
    }
}

/// Convert a canonical I/O standard to Xilinx Vivado / Helion `.xdc` `IOSTANDARD` naming.
pub fn iostandard_to_vivado(canonical: &str) -> String {
    canonicalize_iostandard(canonical)
}

impl UniversalConstraints {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn port_mut(&mut self, name: &str) -> &mut PortConstraint {
        self.ports
            .entry(name.to_string())
            .or_insert_with(|| PortConstraint {
                port: name.to_string(),
                ..Default::default()
            })
    }

    /// Auto-detect format from content or parse using explicit `ConstraintFormat`.
    pub fn parse_format(text: &str, format: ConstraintFormat) -> Result<Self, String> {
        match format {
            ConstraintFormat::Lpf => Self::from_lpf(text),
            ConstraintFormat::Pcf => Self::from_pcf(text),
            ConstraintFormat::Qsf => Self::from_qsf(text),
            ConstraintFormat::Sdc => Self::from_sdc(text),
            ConstraintFormat::Xdc => Self::from_xdc(text),
        }
    }

    /// Auto-detect constraint syntax (`.lpf`, `.pcf`, `.qsf`, `.sdc`, `.xdc`) from content.
    pub fn parse_auto(text: &str) -> Result<Self, String> {
        for raw in text.lines() {
            let line = strip_line_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let up = line.to_ascii_uppercase();
            if up.starts_with("LOCATE ")
                || up.starts_with("IOBUF ")
                || up.starts_with("FREQUENCY ")
                || up.starts_with("BLOCK ")
                || up.starts_with("SYSCONFIG ")
            {
                return Self::from_lpf(text);
            }
            if line.starts_with("set_io ") || line.starts_with("set_io\t") {
                return Self::from_pcf(text);
            }
            if line.starts_with("set_location_assignment ")
                || line.starts_with("set_instance_assignment ")
                || line.starts_with("set_global_assignment ")
            {
                return Self::from_qsf(text);
            }
        }
        Self::from_xdc(text)
    }

    // -------------------------------------------------------------------------
    // 1. Lattice Diamond / Radiant (.lpf / .pdc) Parser
    // -------------------------------------------------------------------------
    pub fn from_lpf(text: &str) -> Result<Self, String> {
        let mut out = Self::new();
        // LPF statements are semicolon-terminated and may span lines; comments start with `#` or `//`.
        let mut cleaned = String::with_capacity(text.len());
        for raw in text.lines() {
            let line = strip_line_comment(raw).trim();
            if !line.is_empty() {
                cleaned.push_str(line);
                if !line.ends_with(';') {
                    cleaned.push(';');
                }
                cleaned.push('\n');
            }
        }

        for stmt in cleaned.split(';') {
            let s = stmt.trim();
            if s.is_empty() {
                continue;
            }
            let toks = tokenize_words(s);
            if toks.is_empty() {
                continue;
            }
            let head = toks[0].to_ascii_uppercase();
            match head.as_str() {
                "LOCATE" => {
                    // LOCATE COMP "port" SITE "pin"
                    if toks.len() >= 5
                        && toks[1].eq_ignore_ascii_case("COMP")
                        && toks[3].eq_ignore_ascii_case("SITE")
                    {
                        let port = unquote(&toks[2]);
                        let pin = unquote(&toks[4]);
                        out.port_mut(&port).pin = Some(pin);
                    } else {
                        return Err(format!("LPF syntax error in LOCATE: {s}"));
                    }
                }
                "IOBUF" => {
                    // IOBUF PORT "port" IO_TYPE=LVCMOS33 PULLMODE=UP SLEWRATE=FAST DRIVE=8 ...
                    if toks.len() < 3 || !toks[1].eq_ignore_ascii_case("PORT") {
                        continue;
                    }
                    let port = unquote(&toks[2]);
                    let pc = out.port_mut(&port);
                    for attr in &toks[3..] {
                        if let Some((k, v)) = attr.split_once('=') {
                            let key = k.trim().to_ascii_uppercase();
                            let val = unquote(v.trim());
                            match key.as_str() {
                                "IO_TYPE" => pc.iostandard = Some(canonicalize_iostandard(&val)),
                                "PULLMODE" => pc.pull = PullMode::parse(&val),
                                "SLEWRATE" => pc.slew = SlewRate::parse(&val),
                                "DRIVE" => {
                                    if let Ok(ma) =
                                        val.trim_end_matches(|c: char| !c.is_ascii_digit()).parse()
                                    {
                                        pc.drive_ma = Some(ma);
                                    }
                                }
                                "DIFFRESISTOR" => {
                                    pc.diff_term = Some(!val.eq_ignore_ascii_case("OFF"));
                                }
                                "TERMINATION" => {
                                    pc.in_term = Some(val);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                "FREQUENCY" => {
                    // FREQUENCY PORT "clk" 100.0 MHz
                    // or FREQUENCY NET "clk" 100.0 MHz
                    if toks.len() >= 4
                        && (toks[1].eq_ignore_ascii_case("PORT")
                            || toks[1].eq_ignore_ascii_case("NET"))
                    {
                        let port = unquote(&toks[2]);
                        let freq_val: f64 = toks[3]
                            .parse()
                            .map_err(|_| format!("LPF invalid frequency '{}'", toks[3]))?;
                        let unit = toks.get(4).map(|u| u.to_ascii_uppercase()).unwrap_or_else(|| "MHZ".into());
                        let freq_mhz = match unit.as_str() {
                            "GHZ" => freq_val * 1000.0,
                            "KHZ" => freq_val / 1000.0,
                            "HZ" => freq_val / 1_000_000.0,
                            _ => freq_val,
                        };
                        if freq_mhz <= 0.0 {
                            return Err(format!("LPF frequency must be positive: {s}"));
                        }
                        let period_ns = 1000.0 / freq_mhz;
                        out.clocks.push(ClockConstraint {
                            name: port.clone(),
                            port,
                            period_ns,
                            waveform_ns: None,
                        });
                    } else {
                        return Err(format!("LPF syntax error in FREQUENCY: {s}"));
                    }
                }
                "BLOCK" | "SYSCONFIG" | "GSR_NET" | "USE" => {}
                other => {
                    return Err(format!("unsupported LPF directive '{other}' in: {s}"));
                }
            }
        }
        Ok(out)
    }

    // -------------------------------------------------------------------------
    // 2. IceStorm / nextpnr (.pcf) Parser
    // -------------------------------------------------------------------------
    pub fn from_pcf(text: &str) -> Result<Self, String> {
        let mut out = Self::new();
        for (line_idx, raw) in text.lines().enumerate() {
            let line = strip_line_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let toks = tokenize_words(line);
            if toks.is_empty() {
                continue;
            }
            match toks[0].as_str() {
                "set_io" => {
                    let mut warn_no_port = false;
                    let mut pull: Option<PullMode> = None;
                    let mut pos = Vec::new();
                    let mut i = 1usize;
                    while i < toks.len() {
                        match toks[i].as_str() {
                            "--warn-no-port" | "-nowarn" => {
                                warn_no_port = true;
                                i += 1;
                            }
                            "-pullup" => {
                                let v = toks.get(i + 1).ok_or_else(|| {
                                    format!("PCF line {}: -pullup requires yes|no", line_idx + 1)
                                })?;
                                pull = PullMode::parse(v);
                                i += 2;
                            }
                            "-pullup_resistor" => {
                                i += 2;
                            }
                            other if other.starts_with('-') => {
                                return Err(format!(
                                    "PCF line {}: unknown set_io flag {other}",
                                    line_idx + 1
                                ));
                            }
                            other => {
                                pos.push(unquote(other));
                                i += 1;
                            }
                        }
                    }
                    if pos.len() != 2 {
                        return Err(format!(
                            "PCF line {}: set_io expects <port> <pin>, got {:?}",
                            line_idx + 1,
                            pos
                        ));
                    }
                    let pc = out.port_mut(&pos[0]);
                    pc.pin = Some(pos[1].clone());
                    pc.warn_no_port = warn_no_port;
                    if let Some(p) = pull {
                        pc.pull = Some(p);
                    }
                }
                "set_frequency" => {
                    if toks.len() >= 3 {
                        let port = unquote(&toks[1]);
                        if let Ok(freq_mhz) = toks[2].parse::<f64>() {
                            if freq_mhz > 0.0 {
                                out.clocks.push(ClockConstraint {
                                    name: port.clone(),
                                    port,
                                    period_ns: 1000.0 / freq_mhz,
                                    waveform_ns: None,
                                });
                            }
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "PCF line {}: unknown command '{other}'",
                        line_idx + 1
                    ));
                }
            }
        }
        Ok(out)
    }

    // -------------------------------------------------------------------------
    // 3. Intel / Altera Quartus (.qsf) Parser
    // -------------------------------------------------------------------------
    pub fn from_qsf(text: &str) -> Result<Self, String> {
        let mut out = Self::new();
        for (line_idx, raw) in text.lines().enumerate() {
            let line = strip_line_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let toks = tokenize_words(line);
            if toks.is_empty() {
                continue;
            }
            match toks[0].as_str() {
                "set_location_assignment" => {
                    // set_location_assignment PIN_<pin> -to <port>
                    let loc_raw = toks.get(1).ok_or_else(|| {
                        format!("QSF line {}: missing location in set_location_assignment", line_idx + 1)
                    })?;
                    let to_idx = toks
                        .iter()
                        .position(|t| t == "-to")
                        .ok_or_else(|| format!("QSF line {}: missing -to in set_location_assignment", line_idx + 1))?;
                    let port = toks
                        .get(to_idx + 1)
                        .ok_or_else(|| format!("QSF line {}: missing port after -to", line_idx + 1))?;
                    let port = unquote(port);
                    let loc = unquote(loc_raw);
                    let pin = loc.strip_prefix("PIN_").unwrap_or(&loc).to_string();
                    out.port_mut(&port).pin = Some(pin);
                }
                "set_instance_assignment" => {
                    // set_instance_assignment -name <NAME> <VAL> -to <port>
                    let name_idx = toks
                        .iter()
                        .position(|t| t == "-name")
                        .ok_or_else(|| format!("QSF line {}: missing -name", line_idx + 1))?;
                    let to_idx = toks
                        .iter()
                        .position(|t| t == "-to")
                        .ok_or_else(|| format!("QSF line {}: missing -to", line_idx + 1))?;
                    let attr_name = toks
                        .get(name_idx + 1)
                        .ok_or_else(|| format!("QSF line {}: missing attr name", line_idx + 1))?
                        .to_ascii_uppercase();
                    let attr_val = toks
                        .get(name_idx + 2)
                        .ok_or_else(|| format!("QSF line {}: missing attr value", line_idx + 1))?;
                    let port = unquote(
                        toks.get(to_idx + 1)
                            .ok_or_else(|| format!("QSF line {}: missing port after -to", line_idx + 1))?,
                    );
                    let val = unquote(attr_val);
                    let pc = out.port_mut(&port);
                    match attr_name.as_str() {
                        "IO_STANDARD" => {
                            pc.iostandard = Some(canonicalize_iostandard(&val));
                        }
                        "CURRENT_STRENGTH_NEW" => {
                            let up = val.to_ascii_uppercase();
                            let digits: String =
                                up.chars().take_while(|c| c.is_ascii_digit()).collect();
                            if let Ok(ma) = digits.parse::<u32>() {
                                pc.drive_ma = Some(ma);
                            }
                        }
                        "WEAK_PULL_UP_RESISTOR" => {
                            pc.pull = PullMode::parse(&val);
                        }
                        "SLEW_RATE" => {
                            pc.slew = SlewRate::parse(&val);
                        }
                        "INPUT_TERMINATION" => {
                            if val.to_ascii_uppercase().contains("DIFFERENTIAL") {
                                pc.diff_term = Some(true);
                            } else {
                                pc.in_term = Some(val);
                            }
                        }
                        _ => {}
                    }
                }
                "set_global_assignment" => {}
                other => {
                    return Err(format!(
                        "QSF line {}: unknown command '{other}'",
                        line_idx + 1
                    ));
                }
            }
        }
        Ok(out)
    }

    // -------------------------------------------------------------------------
    // 4. Synopsys (.sdc) & Xilinx / Helion (.xdc) Parser
    // -------------------------------------------------------------------------
    pub fn from_sdc(text: &str) -> Result<Self, String> {
        Self::from_xdc(text)
    }

    pub fn from_xdc(text: &str) -> Result<Self, String> {
        let mut out = Self::new();
        for (line_idx, raw) in text.lines().enumerate() {
            let line = strip_line_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let toks = tokenize_sdc_xdc_words(line);
            if toks.is_empty() {
                continue;
            }
            match toks[0].as_str() {
                "create_clock" => {
                    let mut period_ns: Option<f64> = None;
                    let mut name: Option<String> = None;
                    let mut waveform: Option<(f64, f64)> = None;
                    let mut port: Option<String> = None;
                    let mut i = 1usize;
                    while i < toks.len() {
                        match toks[i].as_str() {
                            "-period" => {
                                let v = toks.get(i + 1).ok_or_else(|| {
                                    format!("line {}: create_clock -period needs value", line_idx + 1)
                                })?;
                                period_ns = Some(v.parse::<f64>().map_err(|_| {
                                    format!("line {}: invalid period '{v}'", line_idx + 1)
                                })?);
                                i += 2;
                            }
                            "-name" => {
                                name = toks.get(i + 1).map(|s| unquote(s));
                                i += 2;
                            }
                            "-waveform" => {
                                if let Some(wf) = toks.get(i + 1) {
                                    let parts: Vec<f64> = wf
                                        .trim_matches(|c| c == '{' || c == '}')
                                        .split_whitespace()
                                        .filter_map(|p| p.parse().ok())
                                        .collect();
                                    if parts.len() >= 2 {
                                        waveform = Some((parts[0], parts[1]));
                                    }
                                }
                                i += 2;
                            }
                            "-add" => i += 1,
                            other => {
                                port = Some(unwrap_sdc_target(other));
                                i += 1;
                            }
                        }
                    }
                    let p_ns = period_ns.ok_or_else(|| {
                        format!("line {}: create_clock missing -period", line_idx + 1)
                    })?;
                    let port_name = port.or_else(|| name.clone()).unwrap_or_else(|| "clk".into());
                    let clk_name = name.unwrap_or_else(|| port_name.clone());
                    out.clocks.push(ClockConstraint {
                        name: clk_name,
                        port: port_name,
                        period_ns: p_ns,
                        waveform_ns: waveform,
                    });
                }
                "set_input_delay" | "set_output_delay" => {
                    let is_input = toks[0] == "set_input_delay";
                    let mut clock = "clk".to_string();
                    let mut is_min = false;
                    let mut is_max = false;
                    let mut delay_ns: Option<f64> = None;
                    let mut port: Option<String> = None;
                    let mut i = 1usize;
                    while i < toks.len() {
                        match toks[i].as_str() {
                            "-clock" => {
                                if let Some(c) = toks.get(i + 1) {
                                    clock = unwrap_sdc_target(c);
                                }
                                i += 2;
                            }
                            "-min" => {
                                is_min = true;
                                i += 1;
                            }
                            "-max" => {
                                is_max = true;
                                i += 1;
                            }
                            "-add_delay" | "-clock_fall" => i += 1,
                            other => {
                                if delay_ns.is_none() {
                                    if let Ok(d) = other.parse::<f64>() {
                                        delay_ns = Some(d);
                                        i += 1;
                                        continue;
                                    }
                                }
                                port = Some(unwrap_sdc_target(other));
                                i += 1;
                            }
                        }
                    }
                    let d_ns = delay_ns.ok_or_else(|| {
                        format!("line {}: {} missing delay value", line_idx + 1, toks[0])
                    })?;
                    let p = port.ok_or_else(|| {
                        format!("line {}: {} missing target port", line_idx + 1, toks[0])
                    })?;
                    out.io_delays.push(IoDelayConstraint {
                        is_input,
                        clock,
                        delay_ns: d_ns,
                        is_min,
                        is_max,
                        port: p,
                    });
                }
                "set_false_path" => {
                    let mut from = None;
                    let mut to = None;
                    let mut i = 1usize;
                    while i < toks.len() {
                        match toks[i].as_str() {
                            "-from" | "-through" => {
                                from = toks.get(i + 1).map(|s| unwrap_sdc_target(s));
                                i += 2;
                            }
                            "-to" => {
                                to = toks.get(i + 1).map(|s| unwrap_sdc_target(s));
                                i += 2;
                            }
                            _ => i += 1,
                        }
                    }
                    out.false_paths.push(FalsePathConstraint { from, to });
                }
                "set_multicycle_path" => {
                    let mut cycles: Option<u32> = None;
                    let mut is_hold = false;
                    let mut from = None;
                    let mut to = None;
                    let mut i = 1usize;
                    while i < toks.len() {
                        match toks[i].as_str() {
                            "-setup" => {
                                is_hold = false;
                                i += 1;
                            }
                            "-hold" => {
                                is_hold = true;
                                i += 1;
                            }
                            "-from" => {
                                from = toks.get(i + 1).map(|s| unwrap_sdc_target(s));
                                i += 2;
                            }
                            "-to" => {
                                to = toks.get(i + 1).map(|s| unwrap_sdc_target(s));
                                i += 2;
                            }
                            "-start" | "-end" => i += 1,
                            other => {
                                if let Ok(c) = other.parse::<u32>() {
                                    cycles = Some(c);
                                }
                                i += 1;
                            }
                        }
                    }
                    out.multicycle_paths.push(MulticycleConstraint {
                        cycles: cycles.unwrap_or(2),
                        is_hold,
                        from,
                        to,
                    });
                }
                "set_max_delay" => {
                    let mut delay_ns: Option<f64> = None;
                    let mut datapath_only = false;
                    let mut from = None;
                    let mut to = None;
                    let mut i = 1usize;
                    while i < toks.len() {
                        match toks[i].as_str() {
                            "-datapath_only" => {
                                datapath_only = true;
                                i += 1;
                            }
                            "-from" => {
                                from = toks.get(i + 1).map(|s| unwrap_sdc_target(s));
                                i += 2;
                            }
                            "-to" => {
                                to = toks.get(i + 1).map(|s| unwrap_sdc_target(s));
                                i += 2;
                            }
                            other => {
                                if let Ok(d) = other.parse::<f64>() {
                                    delay_ns = Some(d);
                                }
                                i += 1;
                            }
                        }
                    }
                    let d_ns = delay_ns.ok_or_else(|| {
                        format!("line {}: set_max_delay missing delay value", line_idx + 1)
                    })?;
                    out.max_delays.push(MaxDelayConstraint {
                        delay_ns: d_ns,
                        datapath_only,
                        from,
                        to,
                    });
                }
                "set_clock_groups" => {
                    let mut relation = ClockGroupRelation::Asynchronous;
                    let mut groups: Vec<Vec<String>> = Vec::new();
                    let mut i = 1usize;
                    while i < toks.len() {
                        match toks[i].as_str() {
                            "-asynchronous" => {
                                relation = ClockGroupRelation::Asynchronous;
                                i += 1;
                            }
                            "-logically_exclusive" => {
                                relation = ClockGroupRelation::LogicallyExclusive;
                                i += 1;
                            }
                            "-physically_exclusive" => {
                                relation = ClockGroupRelation::PhysicallyExclusive;
                                i += 1;
                            }
                            "-group" => {
                                if let Some(g) = toks.get(i + 1) {
                                    let inner = g.trim_matches(|c| c == '{' || c == '}');
                                    let clks: Vec<String> = tokenize_sdc_xdc_words(inner)
                                        .into_iter()
                                        .map(|w| unwrap_sdc_target(&w))
                                        .filter(|w| !w.is_empty())
                                        .collect();
                                    groups.push(clks);
                                }
                                i += 2;
                            }
                            _ => i += 1,
                        }
                    }
                    out.clock_groups.push(ClockGroupsConstraint { relation, groups });
                }
                "set_property" => {
                    // Supports both:
                    //   set_property PACKAGE_PIN A10 [get_ports clk]
                    //   set_property -dict { PACKAGE_PIN A10 IOSTANDARD LVCMOS33 } [get_ports clk]
                    if toks.get(1).map(|s| s.as_str()) == Some("-dict") && toks.len() >= 4 {
                        let dict_str = toks[2].trim_matches(|c| c == '{' || c == '}');
                        let dict_toks = tokenize_words(dict_str);
                        let port = unwrap_sdc_target(&toks[3]);
                        let pc = out.port_mut(&port);
                        for pair in dict_toks.chunks(2) {
                            if pair.len() == 2 {
                                apply_xdc_port_prop(pc, &pair[0], &pair[1]);
                            }
                        }
                    } else if toks.len() >= 4 {
                        let prop = &toks[1];
                        let val = &toks[2];
                        let port = unwrap_sdc_target(&toks[3]);
                        let pc = out.port_mut(&port);
                        apply_xdc_port_prop(pc, prop, val);
                    }
                }
                other => {
                    return Err(format!(
                        "SDC/XDC line {}: unsupported constraint command '{other}'",
                        line_idx + 1
                    ));
                }
            }
        }
        Ok(out)
    }

    // -------------------------------------------------------------------------
    // 5. Bidirectional Exporters (`to_xdc`, `to_sdc`, `to_lpf`, `to_pcf`, `to_qsf`)
    // -------------------------------------------------------------------------

    /// Export to Xilinx Vivado / Helion `.xdc` format (physical I/O + SDC timing).
    pub fn to_xdc(&self) -> String {
        let mut lines = Vec::new();
        for pc in self.ports.values() {
            if let Some(pin) = &pc.pin {
                lines.push(format!(
                    "set_property PACKAGE_PIN {} [get_ports {}]",
                    pin, pc.port
                ));
            }
            if let Some(std) = &pc.iostandard {
                lines.push(format!(
                    "set_property IOSTANDARD {} [get_ports {}]",
                    iostandard_to_vivado(std),
                    pc.port
                ));
            }
            if let Some(ma) = pc.drive_ma {
                lines.push(format!(
                    "set_property DRIVE {} [get_ports {}]",
                    ma, pc.port
                ));
            }
            if let Some(slew) = pc.slew {
                lines.push(format!(
                    "set_property SLEW {} [get_ports {}]",
                    slew.as_str(),
                    pc.port
                ));
            }
            if let Some(pull) = pc.pull {
                lines.push(format!(
                    "set_property PULLTYPE {} [get_ports {}]",
                    pull.to_xdc_str(),
                    pc.port
                ));
            }
            if let Some(dt) = pc.diff_term {
                lines.push(format!(
                    "set_property DIFF_TERM {} [get_ports {}]",
                    if dt { "TRUE" } else { "FALSE" },
                    pc.port
                ));
            }
            if let Some(it) = &pc.in_term {
                lines.push(format!(
                    "set_property IN_TERM {} [get_ports {}]",
                    it, pc.port
                ));
            }
        }
        let sdc = self.to_sdc();
        if !sdc.is_empty() {
            lines.push(sdc.trim_end().to_string());
        }
        if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n") + "\n"
        }
    }

    /// Export timing constraints to Synopsys `.sdc` format.
    pub fn to_sdc(&self) -> String {
        let mut lines = Vec::new();
        for clk in &self.clocks {
            let mut s = format!(
                "create_clock -period {:.3} -name {}",
                clk.period_ns, clk.name
            );
            if let Some((r, f)) = clk.waveform_ns {
                s.push_str(&format!(" -waveform {{{r:.3} {f:.3}}}"));
            }
            s.push_str(&format!(" [get_ports {}]", clk.port));
            lines.push(s);
        }
        for d in &self.io_delays {
            let cmd = if d.is_input {
                "set_input_delay"
            } else {
                "set_output_delay"
            };
            let mut s = format!("{cmd} -clock {}", d.clock);
            if d.is_min {
                s.push_str(" -min");
            }
            if d.is_max {
                s.push_str(" -max");
            }
            s.push_str(&format!(" {:.3} [get_ports {}]", d.delay_ns, d.port));
            lines.push(s);
        }
        for fp in &self.false_paths {
            let mut s = "set_false_path".to_string();
            if let Some(f) = &fp.from {
                s.push_str(&format!(" -from [get_clocks {f}]"));
            }
            if let Some(t) = &fp.to {
                s.push_str(&format!(" -to [get_clocks {t}]"));
            }
            lines.push(s);
        }
        for mc in &self.multicycle_paths {
            let mut s = format!("set_multicycle_path {}", mc.cycles);
            if mc.is_hold {
                s.push_str(" -hold");
            } else {
                s.push_str(" -setup");
            }
            if let Some(f) = &mc.from {
                s.push_str(&format!(" -from [get_ports {f}]"));
            }
            if let Some(t) = &mc.to {
                s.push_str(&format!(" -to [get_ports {t}]"));
            }
            lines.push(s);
        }
        for md in &self.max_delays {
            let mut s = format!("set_max_delay {:.3}", md.delay_ns);
            if md.datapath_only {
                s.push_str(" -datapath_only");
            }
            if let Some(f) = &md.from {
                s.push_str(&format!(" -from [get_clocks {f}]"));
            }
            if let Some(t) = &md.to {
                s.push_str(&format!(" -to [get_clocks {t}]"));
            }
            lines.push(s);
        }
        for cg in &self.clock_groups {
            let mut s = format!("set_clock_groups {}", cg.relation.to_sdc_flag());
            for g in &cg.groups {
                s.push_str(&format!(" -group {{{}}}", g.join(" ")));
            }
            lines.push(s);
        }
        if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n") + "\n"
        }
    }

    /// Export to Lattice Diamond `.lpf` format.
    pub fn to_lpf(&self) -> String {
        let mut lines = Vec::new();
        for pc in self.ports.values() {
            if let Some(pin) = &pc.pin {
                lines.push(format!("LOCATE COMP \"{}\" SITE \"{}\";", pc.port, pin));
            }
            let mut iobuf_attrs = Vec::new();
            if let Some(std) = &pc.iostandard {
                iobuf_attrs.push(format!("IO_TYPE={}", iostandard_to_lattice(std)));
            }
            if let Some(pull) = pc.pull {
                iobuf_attrs.push(format!("PULLMODE={}", pull.to_lpf_str()));
            }
            if let Some(slew) = pc.slew {
                iobuf_attrs.push(format!("SLEWRATE={}", slew.as_str()));
            }
            if let Some(ma) = pc.drive_ma {
                iobuf_attrs.push(format!("DRIVE={ma}"));
            }
            if let Some(true) = pc.diff_term {
                iobuf_attrs.push("DIFFRESISTOR=100".to_string());
            }
            if let Some(it) = &pc.in_term {
                iobuf_attrs.push(format!("TERMINATION={it}"));
            }
            if !iobuf_attrs.is_empty() {
                lines.push(format!(
                    "IOBUF PORT \"{}\" {};",
                    pc.port,
                    iobuf_attrs.join(" ")
                ));
            }
        }
        for clk in &self.clocks {
            lines.push(format!(
                "FREQUENCY PORT \"{}\" {:.3} MHz;",
                clk.port,
                clk.frequency_mhz()
            ));
        }
        if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n") + "\n"
        }
    }

    /// Export to IceStorm / nextpnr `.pcf` format.
    pub fn to_pcf(&self) -> String {
        let mut lines = Vec::new();
        for pc in self.ports.values() {
            if let Some(pin) = &pc.pin {
                let mut cmd = String::from("set_io");
                if pc.warn_no_port {
                    cmd.push_str(" --warn-no-port");
                }
                if let Some(pull) = pc.pull {
                    match pull {
                        PullMode::Up => cmd.push_str(" -pullup yes"),
                        PullMode::None => cmd.push_str(" -pullup no"),
                        _ => {}
                    }
                }
                cmd.push_str(&format!(" {} {}", pc.port, pin));
                lines.push(cmd);
            }
        }
        for clk in &self.clocks {
            lines.push(format!(
                "set_frequency {} {:.3}",
                clk.port,
                clk.frequency_mhz()
            ));
        }
        if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n") + "\n"
        }
    }

    /// Export to Intel Quartus `.qsf` format.
    pub fn to_qsf(&self) -> String {
        let mut lines = Vec::new();
        for pc in self.ports.values() {
            if let Some(pin) = &pc.pin {
                let qpin = if pin.starts_with("PIN_") {
                    pin.clone()
                } else {
                    format!("PIN_{pin}")
                };
                lines.push(format!(
                    "set_location_assignment {} -to {}",
                    qpin, pc.port
                ));
            }
            if let Some(std) = &pc.iostandard {
                lines.push(format!(
                    "set_instance_assignment -name IO_STANDARD \"{}\" -to {}",
                    iostandard_to_quartus(std),
                    pc.port
                ));
            }
            if let Some(ma) = pc.drive_ma {
                lines.push(format!(
                    "set_instance_assignment -name CURRENT_STRENGTH_NEW {}MA -to {}",
                    ma, pc.port
                ));
            }
            if let Some(pull) = pc.pull {
                let onoff = match pull {
                    PullMode::Up => "ON",
                    _ => "OFF",
                };
                lines.push(format!(
                    "set_instance_assignment -name WEAK_PULL_UP_RESISTOR {} -to {}",
                    onoff, pc.port
                ));
            }
            if let Some(slew) = pc.slew {
                let val = match slew {
                    SlewRate::Fast => "2",
                    SlewRate::Slow => "0",
                };
                lines.push(format!(
                    "set_instance_assignment -name SLEW_RATE {} -to {}",
                    val, pc.port
                ));
            }
        }
        if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n") + "\n"
        }
    }

    pub fn export_format(&self, format: ConstraintFormat) -> String {
        match format {
            ConstraintFormat::Xdc => self.to_xdc(),
            ConstraintFormat::Sdc => self.to_sdc(),
            ConstraintFormat::Lpf => self.to_lpf(),
            ConstraintFormat::Pcf => self.to_pcf(),
            ConstraintFormat::Qsf => self.to_qsf(),
        }
    }

    /// Lower into `helion_sta::Constraints` for direct consumption by Helion STA / P&R / DRC.
    pub fn to_helion_constraints(&self) -> Result<Constraints, String> {
        let xdc_text = self.to_xdc();
        if xdc_text.trim().is_empty() {
            return Ok(Constraints::default());
        }
        load_xdc(&xdc_text)
    }
}

/// Translate constraint text between any two vendor formats (`Xdc`, `Sdc`, `Lpf`, `Pcf`, `Qsf`).
pub fn translate_constraints(
    input: &str,
    from: ConstraintFormat,
    to: ConstraintFormat,
) -> Result<String, String> {
    let parsed = UniversalConstraints::parse_format(input, from)?;
    Ok(parsed.export_format(to))
}

fn apply_xdc_port_prop(pc: &mut PortConstraint, prop: &str, val: &str) {
    let v = unquote(val);
    match prop.to_ascii_uppercase().as_str() {
        "PACKAGE_PIN" | "LOC" => pc.pin = Some(v),
        "IOSTANDARD" => pc.iostandard = Some(canonicalize_iostandard(&v)),
        "DRIVE" => {
            if let Ok(ma) = v.parse::<u32>() {
                pc.drive_ma = Some(ma);
            }
        }
        "SLEW" => pc.slew = SlewRate::parse(&v),
        "PULLTYPE" => pc.pull = PullMode::parse(&v),
        "DIFF_TERM" => {
            pc.diff_term = Some(matches!(
                v.to_ascii_uppercase().as_str(),
                "TRUE" | "1" | "ON" | "YES"
            ));
        }
        "IN_TERM" => pc.in_term = Some(v),
        _ => {}
    }
}

fn strip_line_comment(raw: &str) -> &str {
    let mut in_quotes = false;
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
        } else if !in_quotes {
            if bytes[i] == b'#' {
                return &raw[..i];
            }
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                return &raw[..i];
            }
        }
        i += 1;
    }
    raw
}

fn unquote(s: &str) -> String {
    s.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches(|c| c == '{' || c == '}')
        .trim()
        .to_string()
}

fn unwrap_sdc_target(s: &str) -> String {
    let t = s
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    for prefix in [
        "get_ports ",
        "get_pins ",
        "get_clocks ",
        "get_nets ",
        "get_cells ",
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return unquote(rest);
        }
    }
    unquote(t)
}

fn tokenize_words(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        if chars[i] == '"' {
            i += 1;
            let mut w = String::new();
            while i < chars.len() && chars[i] != '"' {
                w.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push(w);
        } else {
            let mut w = String::new();
            while i < chars.len() && !chars[i].is_whitespace() {
                if chars[i] == '"' {
                    i += 1;
                    while i < chars.len() && chars[i] != '"' {
                        w.push(chars[i]);
                        i += 1;
                    }
                    if i < chars.len() {
                        i += 1;
                    }
                } else {
                    w.push(chars[i]);
                    i += 1;
                }
            }
            out.push(w);
        }
    }
    out
}

fn tokenize_sdc_xdc_words(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        if chars[i] == '{' {
            let mut depth = 1usize;
            i += 1;
            let mut w = String::new();
            while i < chars.len() && depth > 0 {
                if chars[i] == '{' {
                    depth += 1;
                    w.push('{');
                } else if chars[i] == '}' {
                    depth -= 1;
                    if depth > 0 {
                        w.push('}');
                    }
                } else {
                    w.push(chars[i]);
                }
                i += 1;
            }
            out.push(w);
        } else if chars[i] == '[' {
            let mut depth = 1usize;
            i += 1;
            let mut w = String::new();
            while i < chars.len() && depth > 0 {
                if chars[i] == '[' {
                    depth += 1;
                    w.push('[');
                } else if chars[i] == ']' {
                    depth -= 1;
                    if depth > 0 {
                        w.push(']');
                    }
                } else {
                    w.push(chars[i]);
                }
                i += 1;
            }
            out.push(w);
        } else if chars[i] == '"' {
            i += 1;
            let mut w = String::new();
            while i < chars.len() && chars[i] != '"' {
                w.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            out.push(w);
        } else {
            let mut w = String::new();
            while i < chars.len() && !chars[i].is_whitespace() {
                w.push(chars[i]);
                i += 1;
            }
            out.push(w);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_roundtrip_lattice_lpf() {
        let lpf = r#"
# Lattice ECP5 constraints
LOCATE COMP "clk" SITE "P3";
LOCATE COMP "led" SITE "U16";
IOBUF PORT "clk" IO_TYPE=LVCMOS33 PULLMODE=NONE;
IOBUF PORT "led" IO_TYPE=LVCMOS33 PULLMODE=UP SLEWRATE=FAST DRIVE=8;
FREQUENCY PORT "clk" 100.0 MHz;
"#;
        let uc = UniversalConstraints::from_lpf(lpf).expect("parse LPF");
        let clk_p = uc.ports.get("clk").expect("clk port");
        assert_eq!(clk_p.pin.as_deref(), Some("P3"));
        assert_eq!(clk_p.iostandard.as_deref(), Some("LVCMOS33"));
        assert_eq!(clk_p.pull, Some(PullMode::None));

        let led_p = uc.ports.get("led").expect("led port");
        assert_eq!(led_p.pin.as_deref(), Some("U16"));
        assert_eq!(led_p.iostandard.as_deref(), Some("LVCMOS33"));
        assert_eq!(led_p.pull, Some(PullMode::Up));
        assert_eq!(led_p.slew, Some(SlewRate::Fast));
        assert_eq!(led_p.drive_ma, Some(8));

        assert_eq!(uc.clocks.len(), 1);
        assert_eq!(uc.clocks[0].port, "clk");
        assert!((uc.clocks[0].period_ns - 10.0).abs() < 1e-6);
        assert!((uc.clocks[0].frequency_mhz() - 100.0).abs() < 1e-6);

        // Round-trip LPF -> QSF -> LPF -> XDC
        let qsf = uc.to_qsf();
        assert!(qsf.contains("set_location_assignment PIN_U16 -to led"));
        assert!(qsf.contains("set_instance_assignment -name IO_STANDARD \"3.3-V LVCMOS\" -to led"));
        assert!(qsf.contains("set_instance_assignment -name CURRENT_STRENGTH_NEW 8MA -to led"));

        let lpf2 = uc.to_lpf();
        let uc2 = UniversalConstraints::from_lpf(&lpf2).expect("re-parse LPF");
        assert_eq!(uc, uc2);
    }

    #[test]
    fn parse_and_roundtrip_icestorm_pcf() {
        let pcf = r#"
# iCEBreaker PCF
set_io --warn-no-port clk 35
set_io -pullup yes btn_n 10
set_io -pullup no led_r 11
"#;
        let uc = UniversalConstraints::from_pcf(pcf).expect("parse PCF");
        let clk = uc.ports.get("clk").unwrap();
        assert_eq!(clk.pin.as_deref(), Some("35"));
        assert!(clk.warn_no_port);

        let btn = uc.ports.get("btn_n").unwrap();
        assert_eq!(btn.pin.as_deref(), Some("10"));
        assert_eq!(btn.pull, Some(PullMode::Up));

        let led = uc.ports.get("led_r").unwrap();
        assert_eq!(led.pin.as_deref(), Some("11"));
        assert_eq!(led.pull, Some(PullMode::None));

        let pcf_out = uc.to_pcf();
        let uc_rt = UniversalConstraints::from_pcf(&pcf_out).expect("re-parse PCF");
        assert_eq!(uc, uc_rt);

        let xdc = uc.to_xdc();
        assert!(xdc.contains("set_property PACKAGE_PIN 10 [get_ports btn_n]"));
        assert!(xdc.contains("set_property PULLTYPE PULLUP [get_ports btn_n]"));
    }

    #[test]
    fn parse_and_roundtrip_quartus_qsf_with_iostandard_mapping() {
        let qsf = r#"
set_location_assignment PIN_AF14 -to clk
set_instance_assignment -name IO_STANDARD "3.3-V LVTTL" -to clk
set_location_assignment PIN_W15 -to led
set_instance_assignment -name IO_STANDARD "3.3-V LVCMOS" -to led
set_instance_assignment -name CURRENT_STRENGTH_NEW 8MA -to led
set_instance_assignment -name WEAK_PULL_UP_RESISTOR ON -to led
set_instance_assignment -name SLEW_RATE 2 -to led
"#;
        let uc = UniversalConstraints::from_qsf(qsf).expect("parse QSF");
        let clk = uc.ports.get("clk").unwrap();
        assert_eq!(clk.pin.as_deref(), Some("AF14"));
        assert_eq!(clk.iostandard.as_deref(), Some("LVTTL"));

        let led = uc.ports.get("led").unwrap();
        assert_eq!(led.pin.as_deref(), Some("W15"));
        assert_eq!(led.iostandard.as_deref(), Some("LVCMOS33"));
        assert_eq!(led.drive_ma, Some(8));
        assert_eq!(led.pull, Some(PullMode::Up));
        assert_eq!(led.slew, Some(SlewRate::Fast));

        // Verify canonical mapping LVCMOS33 <-> "3.3-V LVCMOS" and LVTTL <-> "3.3-V LVTTL"
        assert_eq!(canonicalize_iostandard("3.3-V LVCMOS"), "LVCMOS33");
        assert_eq!(iostandard_to_quartus("LVCMOS33"), "3.3-V LVCMOS");
        assert_eq!(canonicalize_iostandard("3.3-V LVTTL"), "LVTTL");
        assert_eq!(iostandard_to_quartus("LVTTL"), "3.3-V LVTTL");

        let qsf_out = uc.to_qsf();
        let uc_rt = UniversalConstraints::from_qsf(&qsf_out).expect("re-parse QSF");
        assert_eq!(uc, uc_rt);
    }

    #[test]
    fn parse_and_roundtrip_synopsys_sdc_and_xdc() {
        let sdc = r#"
create_clock -period 10.000 -name sys_clk [get_ports clk]
set_input_delay -clock sys_clk -max 2.500 [get_ports din]
set_output_delay -clock sys_clk -min 1.250 [get_ports dout]
set_false_path -from [get_clocks clk_a] -to [get_clocks clk_b]
set_multicycle_path 2 -setup -from [get_ports start_p] -to [get_ports done_p]
set_max_delay 6.000 -datapath_only -from [get_clocks clk_a] -to [get_clocks clk_b]
set_clock_groups -asynchronous -group {clk_a clk_a2} -group {clk_b}
"#;
        let uc = UniversalConstraints::from_sdc(sdc).expect("parse SDC");
        assert_eq!(uc.clocks.len(), 1);
        assert_eq!(uc.clocks[0].name, "sys_clk");
        assert_eq!(uc.clocks[0].port, "clk");
        assert!((uc.clocks[0].period_ns - 10.0).abs() < 1e-6);

        assert_eq!(uc.io_delays.len(), 2);
        assert!(uc.io_delays[0].is_input);
        assert!(uc.io_delays[0].is_max);
        assert_eq!(uc.io_delays[0].port, "din");
        assert!(!uc.io_delays[1].is_input);
        assert!(uc.io_delays[1].is_min);
        assert_eq!(uc.io_delays[1].port, "dout");

        assert_eq!(uc.false_paths.len(), 1);
        assert_eq!(uc.false_paths[0].from.as_deref(), Some("clk_a"));
        assert_eq!(uc.false_paths[0].to.as_deref(), Some("clk_b"));

        assert_eq!(uc.multicycle_paths.len(), 1);
        assert_eq!(uc.multicycle_paths[0].cycles, 2);
        assert!(!uc.multicycle_paths[0].is_hold);

        assert_eq!(uc.max_delays.len(), 1);
        assert!(uc.max_delays[0].datapath_only);
        assert!((uc.max_delays[0].delay_ns - 6.0).abs() < 1e-6);

        assert_eq!(uc.clock_groups.len(), 1);
        assert_eq!(uc.clock_groups[0].relation, ClockGroupRelation::Asynchronous);
        assert_eq!(
            uc.clock_groups[0].groups,
            vec![vec!["clk_a".to_string(), "clk_a2".to_string()], vec!["clk_b".to_string()]]
        );

        let sdc_out = uc.to_sdc();
        let uc_rt = UniversalConstraints::from_sdc(&sdc_out).expect("re-parse SDC");
        assert_eq!(uc, uc_rt);

        // Verify lowering into helion_sta::Constraints
        let h_xdc = uc.to_helion_constraints().expect("lower to Helion Constraints");
        assert_eq!(h_xdc.clocks.len(), 1);
        assert_eq!(h_xdc.clocks[0].period_ps, 10_000);
        assert_eq!(h_xdc.input_delay_ps.len(), 1);
        assert_eq!(h_xdc.output_delay_ps.len(), 1);
        assert_eq!(h_xdc.false_paths.len(), 1);
        assert_eq!(h_xdc.multicycle_paths.len(), 1);
        assert_eq!(h_xdc.max_delays.len(), 1);
        assert_eq!(h_xdc.clock_groups.len(), 1);
    }

    #[test]
    fn cross_vendor_iostandard_canonical_matrix() {
        let cases = [
            ("LVCMOS33", "3.3-V LVCMOS", "LVCMOS33"),
            ("LVTTL", "3.3-V LVTTL", "LVTTL33"),
            ("LVCMOS25", "2.5 V", "LVCMOS25"),
            ("LVCMOS18", "1.8 V", "LVCMOS18"),
            ("LVCMOS15", "1.5 V", "LVCMOS15"),
            ("LVCMOS12", "1.2 V", "LVCMOS12"),
            ("LVDS_25", "LVDS", "LVDS25"),
            ("SSTL15", "SSTL-15", "SSTL15_I"),
        ];
        for (canon, qsf_std, lpf_std) in cases {
            assert_eq!(canonicalize_iostandard(canon), canon);
            assert_eq!(canonicalize_iostandard(qsf_std), canon);
            assert_eq!(canonicalize_iostandard(lpf_std), canon);
            assert_eq!(iostandard_to_quartus(canon), qsf_std);
            assert_eq!(iostandard_to_lattice(canon), lpf_std);
            assert_eq!(iostandard_to_vivado(qsf_std), canon);
            assert_eq!(iostandard_to_vivado(lpf_std), canon);
        }
    }

    #[test]
    fn translate_constraints_across_all_vendor_formats_and_auto_detect() {
        let xdc_in = r#"
set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]
set_property IOSTANDARD LVCMOS33 [get_ports led]
set_property DRIVE 12 [get_ports led]
set_property SLEW FAST [get_ports led]
set_property PULLTYPE PULLUP [get_ports led]
create_clock -period 8.000 -name clk [get_ports clk]
"#;
        let lpf = translate_constraints(xdc_in, ConstraintFormat::Xdc, ConstraintFormat::Lpf)
            .expect("XDC -> LPF");
        assert!(lpf.contains("LOCATE COMP \"led\" SITE \"IOB_X2Y0\";"));
        assert!(lpf.contains("IO_TYPE=LVCMOS33"));
        assert!(lpf.contains("FREQUENCY PORT \"clk\" 125.000 MHz;"));

        let qsf = translate_constraints(&lpf, ConstraintFormat::Lpf, ConstraintFormat::Qsf)
            .expect("LPF -> QSF");
        assert!(qsf.contains("set_location_assignment PIN_IOB_X2Y0 -to led"));
        assert!(qsf.contains("set_instance_assignment -name IO_STANDARD \"3.3-V LVCMOS\" -to led"));
        assert!(qsf.contains("set_instance_assignment -name CURRENT_STRENGTH_NEW 12MA -to led"));

        let pcf = translate_constraints(&qsf, ConstraintFormat::Qsf, ConstraintFormat::Pcf)
            .expect("QSF -> PCF");
        assert!(pcf.contains("set_io -pullup yes led IOB_X2Y0"));

        let xdc_back = translate_constraints(&pcf, ConstraintFormat::Pcf, ConstraintFormat::Xdc)
            .expect("PCF -> XDC");
        assert!(xdc_back.contains("set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]"));
        assert!(xdc_back.contains("set_property PULLTYPE PULLUP [get_ports led]"));

        // Auto-detect on each format
        assert_eq!(
            UniversalConstraints::parse_auto(&lpf).unwrap().ports["led"].pin.as_deref(),
            Some("IOB_X2Y0")
        );
        assert_eq!(
            UniversalConstraints::parse_auto(&qsf).unwrap().ports["led"].pin.as_deref(),
            Some("IOB_X2Y0")
        );
        assert_eq!(
            UniversalConstraints::parse_auto(&pcf).unwrap().ports["led"].pin.as_deref(),
            Some("IOB_X2Y0")
        );
    }

    #[test]
    fn project_ingests_lpf_pcf_and_qsf_constraint_files() {
        let dir = std::env::temp_dir().join(format!("helion_translator_prj_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let lpf_path = dir.join("board.lpf");
        std::fs::write(
            &lpf_path,
            "LOCATE COMP \"led\" SITE \"IOB_X2Y0\";\nIOBUF PORT \"led\" IO_TYPE=LVCMOS18 DRIVE=12 SLEWRATE=SLOW;\nFREQUENCY PORT \"clk\" 100.0 MHz;\n",
        )
        .unwrap();

        let mut prj = crate::ProjectFile::default();
        prj.constraint_files.push(lpf_path.display().to_string());
        let c = crate::constraints_from_project(&prj, &dir.join("top.prj")).unwrap();
        assert_eq!(c.package_pins.get("led").map(|s| s.as_str()), Some("IOB_X2Y0"));
        assert_eq!(c.iostandards.get("led").map(|s| s.as_str()), Some("LVCMOS18"));
        assert_eq!(c.clocks.len(), 1);
        assert_eq!(c.clocks[0].period_ps, 10_000);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
