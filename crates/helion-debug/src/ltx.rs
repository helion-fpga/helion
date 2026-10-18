//! `.ltx` / Helion Debug Probes Descriptor File (`DebugProbesLtx` — UG908 parity).
//!
//! Maps logical signal/bus names and bit widths to physical ILA/VIO probe
//! channels and BSCAN debug hub slots, with lossless text/file round-trip.

use std::collections::BTreeMap;
use std::path::Path;

/// Direction / role of a debug probe channel in a `.ltx` file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LtxProbeDirection {
    /// ILA sample/trigger input probe (`PROBE0`, `PROBE1`, ...).
    Probe,
    /// VIO input probe monitoring fabric nets (`PROBE_IN0`, ...).
    ProbeIn,
    /// VIO output probe driving virtual stimulus onto fabric nets (`PROBE_OUT0`, ...).
    ProbeOut,
}

impl LtxProbeDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Probe => "PROBE",
            Self::ProbeIn => "PROBE_IN",
            Self::ProbeOut => "PROBE_OUT",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_uppercase().as_str() {
            "PROBE" | "IN" | "ILA" => Ok(Self::Probe),
            "PROBE_IN" | "VIO_IN" => Ok(Self::ProbeIn),
            "PROBE_OUT" | "VIO_OUT" | "OUT" => Ok(Self::ProbeOut),
            other => Err(format!("ltx: unknown probe direction '{other}'")),
        }
    }
}

/// One physical probe channel on an ILA or VIO core in a `.ltx` descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LtxProbeChannel {
    /// Physical probe port identifier (`PROBE0`, `PROBE1`, `PROBE_IN0`, `PROBE_OUT0`).
    pub port_id: String,
    /// Channel index (`0`, `1`, ...).
    pub channel_index: usize,
    /// Direction (`Probe`, `ProbeIn`, `ProbeOut`).
    pub direction: LtxProbeDirection,
    /// Logical signal or bus name (e.g. `"cnt"` for `cnt[3:0]`, or `"q3"` for scalar).
    pub logical_name: String,
    /// MSB index of the logical bus (`0` for scalar).
    pub msb: usize,
    /// LSB index of the logical bus (`0` for scalar).
    pub lsb: usize,
    /// Bit width (` nets.len() `).
    pub width: usize,
    /// Ordered net names (LSB-first: `nets[0]` = LSB, `nets[width - 1]` = MSB).
    pub nets: Vec<String>,
}

impl LtxProbeChannel {
    /// Formatted bus label (`"cnt[3:0]"` for multi-bit bus or `"q3"` for 1-bit scalar).
    pub fn display_signal(&self) -> String {
        if self.width > 1 || self.msb != self.lsb {
            format!("{}[{}:{}]", self.logical_name, self.msb, self.lsb)
        } else {
            self.logical_name.clone()
        }
    }
}

/// Descriptor for one debug core (ILA, VIO, JTAG-to-AXI) inside a `.ltx` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LtxCoreDescriptor {
    pub core_name: String,
    pub core_kind: String,
    pub slot_id: u8,
    pub uuid: String,
    pub clock_net: String,
    pub sample_depth: usize,
    pub trigger_stages: usize,
    pub probes: Vec<LtxProbeChannel>,
}

/// Complete `.ltx` Debug Probes Descriptor for a design.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugProbesLtx {
    pub version: String,
    pub design_name: String,
    pub hub_name: String,
    pub bscan_chain: u8,
    pub cores: Vec<LtxCoreDescriptor>,
}

impl DebugProbesLtx {
    pub fn new(
        design_name: impl Into<String>,
        hub_name: impl Into<String>,
        bscan_chain: u8,
    ) -> Self {
        Self {
            version: "1.0".to_string(),
            design_name: design_name.into(),
            hub_name: hub_name.into(),
            bscan_chain,
            cores: Vec::new(),
        }
    }

    /// Look up a probe channel by logical signal name or physical net name across all cores.
    pub fn find_probe(&self, name: &str) -> Option<(&LtxCoreDescriptor, &LtxProbeChannel)> {
        for core in &self.cores {
            for probe in &core.probes {
                if probe.logical_name == name
                    || probe.port_id == name
                    || probe.display_signal() == name
                    || probe.nets.iter().any(|n| n == name)
                {
                    return Some((core, probe));
                }
            }
        }
        None
    }

    /// Flatten all physical net names referenced by ILA cores in channel order.
    pub fn all_ila_nets(&self) -> Vec<String> {
        let mut out = Vec::new();
        for core in &self.cores {
            if core.core_kind.eq_ignore_ascii_case("ila") {
                for p in &core.probes {
                    for n in &p.nets {
                        if !out.contains(n) {
                            out.push(n.clone());
                        }
                    }
                }
            }
        }
        out
    }

    /// Serialize to a deterministic `.ltx` descriptor string (Vivado `probeData` + Helion LTX v1).
    pub fn to_ltx_string(&self) -> String {
        let mut s = String::new();
        s.push_str("# Helion Debug Probes Descriptor (.ltx) — UG908 Hardware Manager compatible\n");
        s.push_str(&format!("LTX_VERSION {}\n", self.version));
        s.push_str(&format!("DESIGN {}\n", self.design_name));
        s.push_str(&format!(
            "DEBUG_HUB {} BSCAN_CHAIN={}\n",
            self.hub_name, self.bscan_chain
        ));
        for core in &self.cores {
            s.push_str(&format!(
                "CORE {} KIND={} SLOT={} UUID={} CLK={} DEPTH={} STAGES={}\n",
                core.core_name,
                core.core_kind,
                core.slot_id,
                core.uuid,
                core.clock_net,
                core.sample_depth,
                core.trigger_stages
            ));
            for p in &core.probes {
                let nets_csv = p.nets.join(",");
                s.push_str(&format!(
                    "  PROBE {} IDX={} DIR={} SIGNAL={} MSB={} LSB={} WIDTH={} NETS={}\n",
                    p.port_id,
                    p.channel_index,
                    p.direction.as_str(),
                    p.logical_name,
                    p.msb,
                    p.lsb,
                    p.width,
                    nets_csv
                ));
            }
            s.push_str("END_CORE\n");
        }
        s
    }

    /// Parse a `.ltx` descriptor string back into [`DebugProbesLtx`].
    pub fn from_ltx_str(text: &str) -> Result<Self, String> {
        let mut version = "1.0".to_string();
        let mut design_name = String::new();
        let mut hub_name = "u_dbg_hub".to_string();
        let mut bscan_chain = 1u8;
        let mut cores = Vec::new();
        let mut cur_core: Option<LtxCoreDescriptor> = None;

        for (line_no, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let kw = parts.next().unwrap_or("");
            match kw {
                "LTX_VERSION" => {
                    version = parts
                        .next()
                        .ok_or_else(|| format!("ltx:{}: missing version", line_no + 1))?
                        .to_string();
                }
                "DESIGN" => {
                    design_name = parts
                        .next()
                        .ok_or_else(|| format!("ltx:{}: missing design name", line_no + 1))?
                        .to_string();
                }
                "DEBUG_HUB" => {
                    hub_name = parts
                        .next()
                        .ok_or_else(|| format!("ltx:{}: missing hub name", line_no + 1))?
                        .to_string();
                    for kv in parts {
                        if let Some(v) = kv.strip_prefix("BSCAN_CHAIN=") {
                            bscan_chain = v.parse::<u8>().map_err(|e| {
                                format!("ltx:{}: invalid BSCAN_CHAIN '{v}': {e}", line_no + 1)
                            })?;
                        }
                    }
                }
                "CORE" => {
                    if cur_core.is_some() {
                        return Err(format!(
                            "ltx:{}: nested CORE before END_CORE",
                            line_no + 1
                        ));
                    }
                    let core_name = parts
                        .next()
                        .ok_or_else(|| format!("ltx:{}: missing core name", line_no + 1))?
                        .to_string();
                    let mut core_kind = "ila".to_string();
                    let mut slot_id = 0u8;
                    let mut uuid = "00000000000000000000000000000000".to_string();
                    let mut clock_net = "clk".to_string();
                    let mut sample_depth = 64usize;
                    let mut trigger_stages = 1usize;
                    for kv in parts {
                        if let Some(v) = kv.strip_prefix("KIND=") {
                            core_kind = v.to_string();
                        } else if let Some(v) = kv.strip_prefix("SLOT=") {
                            slot_id = v.parse().map_err(|e| {
                                format!("ltx:{}: invalid SLOT '{v}': {e}", line_no + 1)
                            })?;
                        } else if let Some(v) = kv.strip_prefix("UUID=") {
                            uuid = v.to_string();
                        } else if let Some(v) = kv.strip_prefix("CLK=") {
                            clock_net = v.to_string();
                        } else if let Some(v) = kv.strip_prefix("DEPTH=") {
                            sample_depth = v.parse().map_err(|e| {
                                format!("ltx:{}: invalid DEPTH '{v}': {e}", line_no + 1)
                            })?;
                        } else if let Some(v) = kv.strip_prefix("STAGES=") {
                            trigger_stages = v.parse().map_err(|e| {
                                format!("ltx:{}: invalid STAGES '{v}': {e}", line_no + 1)
                            })?;
                        }
                    }
                    cur_core = Some(LtxCoreDescriptor {
                        core_name,
                        core_kind,
                        slot_id,
                        uuid,
                        clock_net,
                        sample_depth,
                        trigger_stages,
                        probes: Vec::new(),
                    });
                }
                "PROBE" => {
                    let core = cur_core.as_mut().ok_or_else(|| {
                        format!("ltx:{}: PROBE outside of CORE block", line_no + 1)
                    })?;
                    let port_id = parts
                        .next()
                        .ok_or_else(|| format!("ltx:{}: missing probe port_id", line_no + 1))?
                        .to_string();
                    let mut channel_index = core.probes.len();
                    let mut direction = LtxProbeDirection::Probe;
                    let mut logical_name = port_id.clone();
                    let mut msb = 0usize;
                    let mut lsb = 0usize;
                    let mut width = 1usize;
                    let mut nets = Vec::new();
                    for kv in parts {
                        if let Some(v) = kv.strip_prefix("IDX=") {
                            channel_index = v.parse().map_err(|e| {
                                format!("ltx:{}: invalid IDX '{v}': {e}", line_no + 1)
                            })?;
                        } else if let Some(v) = kv.strip_prefix("DIR=") {
                            direction = LtxProbeDirection::parse(v)?;
                        } else if let Some(v) = kv.strip_prefix("SIGNAL=") {
                            logical_name = v.to_string();
                        } else if let Some(v) = kv.strip_prefix("MSB=") {
                            msb = v.parse().map_err(|e| {
                                format!("ltx:{}: invalid MSB '{v}': {e}", line_no + 1)
                            })?;
                        } else if let Some(v) = kv.strip_prefix("LSB=") {
                            lsb = v.parse().map_err(|e| {
                                format!("ltx:{}: invalid LSB '{v}': {e}", line_no + 1)
                            })?;
                        } else if let Some(v) = kv.strip_prefix("WIDTH=") {
                            width = v.parse().map_err(|e| {
                                format!("ltx:{}: invalid WIDTH '{v}': {e}", line_no + 1)
                            })?;
                        } else if let Some(v) = kv.strip_prefix("NETS=") {
                            nets = v
                                .split(',')
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string())
                                .collect();
                        }
                    }
                    core.probes.push(LtxProbeChannel {
                        port_id,
                        channel_index,
                        direction,
                        logical_name,
                        msb,
                        lsb,
                        width,
                        nets,
                    });
                }
                "END_CORE" => {
                    let core = cur_core.take().ok_or_else(|| {
                        format!("ltx:{}: END_CORE without matching CORE", line_no + 1)
                    })?;
                    cores.push(core);
                }
                other => {
                    return Err(format!("ltx:{}: unknown directive '{other}'", line_no + 1));
                }
            }
        }
        if cur_core.is_some() {
            return Err("ltx: unterminated CORE block (missing END_CORE)".into());
        }
        if design_name.is_empty() {
            return Err("ltx: missing DESIGN header".into());
        }
        Ok(Self {
            version,
            design_name,
            hub_name,
            bscan_chain,
            cores,
        })
    }

    /// Write the `.ltx` descriptor to `path`.
    pub fn write_ltx(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_ltx_string())
            .map_err(|e| format!("ltx: write {}: {e}", path.display()))
    }

    /// Read and parse a `.ltx` descriptor from `path`.
    pub fn read_ltx(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("ltx: read {}: {e}", path.display()))?;
        Self::from_ltx_str(&text)
    }
}

/// Parse `"bus[bit]"` into `Some(("bus", bit))`.
fn split_bus_bit(net: &str) -> Option<(&str, usize)> {
    let open = net.rfind('[')?;
    let close = net.strip_suffix(']')?;
    let base = &net[..open];
    if base.is_empty() {
        return None;
    }
    let idx_str = &close[open + 1..];
    let bit = idx_str.parse::<usize>().ok()?;
    Some((base, bit))
}

/// Group physical net names into [`LtxProbeChannel`] descriptors.
///
/// Indexed bus nets (`data[0]`, `data[1]`, ...) sharing the same base name are
/// coalesced into a single bus channel (ordered LSB → MSB), while scalar nets
/// each receive their own 1-bit probe channel in deterministic order.
pub fn group_nets_into_probe_channels(
    nets: &[String],
    port_prefix: &str,
    direction: LtxProbeDirection,
) -> Vec<LtxProbeChannel> {
    let mut bus_map: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    let mut order: Vec<(bool, String)> = Vec::new();

    for net in nets {
        if let Some((base, bit)) = split_bus_bit(net) {
            let base_s = base.to_string();
            if !bus_map.contains_key(&base_s) {
                order.push((true, base_s.clone()));
            }
            bus_map.entry(base_s).or_default().push((bit, net.clone()));
        } else {
            order.push((false, net.clone()));
        }
    }

    let mut channels = Vec::with_capacity(order.len());
    for (ch_idx, (is_bus, key)) in order.into_iter().enumerate() {
        let port_id = format!("{port_prefix}{ch_idx}");
        if is_bus {
            let mut bits = bus_map.remove(&key).unwrap_or_default();
            bits.sort_by_key(|(b, _)| *b);
            bits.dedup_by_key(|(b, _)| *b);
            let lsb = bits.first().map(|(b, _)| *b).unwrap_or(0);
            let msb = bits.last().map(|(b, _)| *b).unwrap_or(0);
            let net_list: Vec<String> = bits.into_iter().map(|(_, n)| n).collect();
            let width = net_list.len();
            channels.push(LtxProbeChannel {
                port_id,
                channel_index: ch_idx,
                direction,
                logical_name: key,
                msb,
                lsb,
                width,
                nets: net_list,
            });
        } else {
            channels.push(LtxProbeChannel {
                port_id,
                channel_index: ch_idx,
                direction,
                logical_name: key.clone(),
                msb: 0,
                lsb: 0,
                width: 1,
                nets: vec![key],
            });
        }
    }
    channels
}
