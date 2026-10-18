//! Vivado UG1118 IP Packager (`ipx::package_project`, `ipx::infer_bus_interface`,
//! `ipx::add_bus_parameter`, `ipx::save_core`), `.xci` / `.xcix` Core Container
//! management (`read_ip`, `generate_target`, `upgrade_ip`, `report_ip_status`),
//! and Out-of-Context (OOC) SHA-256 content-addressed per-IP synthesis caching
//! (`create_ip_run`, `synth_ip`).

use crate::checkpoint2::{CheckpointSummary, load_checkpoint, save_checkpoint};
use crate::{Mode, Session};
use helion_device::Device;
use helion_ir::{Design, PortDir, resolve};
use helion_sta::Constraints;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// FIPS 180-4 SHA-256 for content-addressed OOC IP synthesis caching
// ---------------------------------------------------------------------------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn ooc_sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((data.len() + 9 + 63) & !63);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            let off = i * 4;
            w[i] = u32::from_be_bytes([chunk[off], chunk[off + 1], chunk[off + 2], chunk[off + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(64);
    for word in &h {
        for &b in &word.to_be_bytes() {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
    s
}

// ---------------------------------------------------------------------------
// 1. Vivado UG1118 IP Packager (`ipx::*`) & IEEE 1685 IP-XACT Core Model
// ---------------------------------------------------------------------------

/// Bus interface mode in an IEEE 1685 IP-XACT component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpxBusMode {
    Master,
    Slave,
    Monitor,
}

impl IpxBusMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Slave => "slave",
            Self::Monitor => "monitor",
        }
    }
}

/// One inferred or user-defined bus interface on a packaged IP core (`ipx::infer_bus_interface`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpxBusInterface {
    pub name: String,
    /// Bus type VLNV, e.g. `xilinx.com:interface:aximm:1.0`, `xilinx.com:interface:axis:1.0`,
    /// `xilinx.com:signal:clock:1.0`, `xilinx.com:signal:reset:1.0`, `helion.org:bus:helion_mm:1.0`.
    pub bus_type_vlnv: String,
    /// Abstraction type VLNV, e.g. `xilinx.com:interface:aximm_rtl:1.0`.
    pub abstraction_type_vlnv: String,
    pub mode: IpxBusMode,
    /// Logical port name -> physical RTL port name map (`AWADDR` -> `s_axi_awaddr`).
    pub port_maps: BTreeMap<String, String>,
    /// Bus interface parameters added via `ipx::add_bus_parameter` (e.g. `FREQ_HZ`, `POLARITY`, `ASSOCIATED_BUSIF`).
    pub parameters: BTreeMap<String, String>,
}

/// Physical RTL port entry in a packaged IP-XACT component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpxPort {
    pub name: String,
    pub direction: String,
    pub width: usize,
}

/// Packaged IEEE 1685 IP-XACT Core created by `ipx::package_project`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackagedIpCore {
    pub vendor: String,
    pub library: String,
    pub name: String,
    pub version: String,
    pub top_module: String,
    pub root_dir: PathBuf,
    pub hdl_files: Vec<String>,
    pub xdc_files: Vec<String>,
    pub ports: Vec<IpxPort>,
    pub model_parameters: BTreeMap<String, String>,
    pub bus_interfaces: Vec<IpxBusInterface>,
}

impl PackagedIpCore {
    pub fn vlnv(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.vendor, self.library, self.name, self.version
        )
    }

    pub fn bus_interface(&self, name: &str) -> Option<&IpxBusInterface> {
        self.bus_interfaces
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(name))
    }

    pub fn bus_interface_mut(&mut self, name: &str) -> Option<&mut IpxBusInterface> {
        self.bus_interfaces
            .iter_mut()
            .find(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// Render an IEEE 1685-2014 IP-XACT `component.xml` document.
    pub fn to_ipxact_xml(&self) -> String {
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        xml.push_str("<spirit:component xmlns:spirit=\"http://www.spiritconsortium.org/XMLSchema/SPIRIT/1685-2009\">\n");
        xml.push_str(&format!("  <spirit:vendor>{}</spirit:vendor>\n", self.vendor));
        xml.push_str(&format!("  <spirit:library>{}</spirit:library>\n", self.library));
        xml.push_str(&format!("  <spirit:name>{}</spirit:name>\n", self.name));
        xml.push_str(&format!("  <spirit:version>{}</spirit:version>\n", self.version));

        if !self.bus_interfaces.is_empty() {
            xml.push_str("  <spirit:busInterfaces>\n");
            for bus in &self.bus_interfaces {
                xml.push_str("    <spirit:busInterface>\n");
                xml.push_str(&format!("      <spirit:name>{}</spirit:name>\n", bus.name));
                xml.push_str(&format!(
                    "      <spirit:busType spirit:vlnv=\"{}\"/>\n",
                    bus.bus_type_vlnv
                ));
                xml.push_str(&format!(
                    "      <spirit:abstractionType spirit:vlnv=\"{}\"/>\n",
                    bus.abstraction_type_vlnv
                ));
                xml.push_str(&format!("      <spirit:{}/>\n", bus.mode.as_str()));
                if !bus.port_maps.is_empty() {
                    xml.push_str("      <spirit:portMaps>\n");
                    for (logical, physical) in &bus.port_maps {
                        xml.push_str("        <spirit:portMap>\n");
                        xml.push_str(&format!(
                            "          <spirit:logicalPort><spirit:name>{logical}</spirit:name></spirit:logicalPort>\n"
                        ));
                        xml.push_str(&format!(
                            "          <spirit:physicalPort><spirit:name>{physical}</spirit:name></spirit:physicalPort>\n"
                        ));
                        xml.push_str("        </spirit:portMap>\n");
                    }
                    xml.push_str("      </spirit:portMaps>\n");
                }
                if !bus.parameters.is_empty() {
                    xml.push_str("      <spirit:parameters>\n");
                    for (k, v) in &bus.parameters {
                        xml.push_str(&format!(
                            "        <spirit:parameter><spirit:name>{k}</spirit:name><spirit:value>{v}</spirit:value></spirit:parameter>\n"
                        ));
                    }
                    xml.push_str("      </spirit:parameters>\n");
                }
                xml.push_str("    </spirit:busInterface>\n");
            }
            xml.push_str("  </spirit:busInterfaces>\n");
        }

        xml.push_str("  <spirit:model>\n");
        xml.push_str("    <spirit:ports>\n");
        for p in &self.ports {
            xml.push_str(&format!(
                "      <spirit:port><spirit:name>{}</spirit:name><spirit:wire><spirit:direction>{}</spirit:direction><spirit:width>{}</spirit:width></spirit:wire></spirit:port>\n",
                p.name, p.direction, p.width
            ));
        }
        xml.push_str("    </spirit:ports>\n");
        xml.push_str("  </spirit:model>\n");

        xml.push_str("  <spirit:fileSets>\n");
        xml.push_str("    <spirit:fileSet>\n");
        xml.push_str("      <spirit:name>xilinx_anylanguagesynthesis_view_fileset</spirit:name>\n");
        for f in &self.hdl_files {
            xml.push_str(&format!(
                "      <spirit:file><spirit:name>{f}</spirit:name></spirit:file>\n"
            ));
        }
        xml.push_str("    </spirit:fileSet>\n");
        xml.push_str("  </spirit:fileSets>\n");

        if !self.model_parameters.is_empty() {
            xml.push_str("  <spirit:parameters>\n");
            for (k, v) in &self.model_parameters {
                xml.push_str(&format!(
                    "    <spirit:parameter><spirit:name>{k}</spirit:name><spirit:value>{v}</spirit:value></spirit:parameter>\n"
                ));
            }
            xml.push_str("  </spirit:parameters>\n");
        }
        xml.push_str("</spirit:component>\n");
        xml
    }
}

/// Extract module name, parameters, and ports from SystemVerilog / Verilog source text.
pub fn parse_rtl_module_signature(
    src: &str,
) -> (String, BTreeMap<String, String>, Vec<IpxPort>) {
    let mut mod_name = "ip_top".to_string();
    let mut params = BTreeMap::new();
    let mut ports = Vec::new();

    for line in src.lines() {
        let clean = line.split("//").next().unwrap_or("").trim();
        if clean.is_empty() {
            continue;
        }
        if let Some(after_mod) = clean.strip_prefix("module ") {
            let name_tok = after_mod
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .find(|s| !s.is_empty());
            if let Some(n) = name_tok {
                mod_name = n.to_string();
            }
        }
        if let Some(p_idx) = clean.find("parameter ") {
            let after_p = &clean[p_idx + "parameter ".len()..];
            for piece in after_p.split(',') {
                if let Some((lhs, rhs)) = piece.split_once('=') {
                    let k = lhs
                        .split_whitespace()
                        .last()
                        .unwrap_or("")
                        .trim();
                    let v = rhs
                        .trim_end_matches(|c: char| c == ')' || c == ';' || c.is_whitespace())
                        .trim();
                    if !k.is_empty() && !v.is_empty() {
                        params.insert(k.to_string(), v.to_string());
                    }
                }
            }
        }
        for (kw, dir_str) in [("input ", "in"), ("output ", "out"), ("inout ", "inout")] {
            if let Some(pos) = clean.find(kw) {
                let rest = clean[pos + kw.len()..].trim();
                let mut width = 1usize;
                let after_range = if let Some(br_open) = rest.find('[') {
                    if let Some(br_close) = rest[br_open..].find(']') {
                        let rng = &rest[br_open + 1..br_open + br_close];
                        if let Some((hi_s, lo_s)) = rng.split_once(':') {
                            let hi = hi_s.trim().parse::<usize>().unwrap_or(0);
                            let lo = lo_s.trim().parse::<usize>().unwrap_or(0);
                            width = hi.abs_diff(lo) + 1;
                        }
                        &rest[br_open + br_close + 1..]
                    } else {
                        rest
                    }
                } else {
                    rest
                };
                let pname = after_range
                    .trim_end_matches(|c: char| c == ',' || c == ')' || c == ';')
                    .split_whitespace()
                    .last()
                    .unwrap_or("")
                    .trim();
                if !pname.is_empty()
                    && pname
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !ports.iter().any(|p: &IpxPort| p.name == pname)
                {
                    ports.push(IpxPort {
                        name: pname.to_string(),
                        direction: dir_str.into(),
                        width,
                    });
                }
            }
        }
    }
    (mod_name, params, ports)
}

/// Vivado UG1118 `ipx::*` namespace API.
pub mod ipx {
    use super::*;

    /// `ipx::package_project`: inspect RTL sources in `root_dir`, extract ports/parameters,
    /// and automatically infer AXI4-Lite / AXI4-Stream / Helion-MM / Helion-ST / clock / reset bus interfaces.
    pub fn package_project(
        root_dir: &Path,
        vendor: &str,
        library: &str,
        name: Option<&str>,
        version: &str,
    ) -> Result<PackagedIpCore, String> {
        let mut hdl_files = Vec::new();
        let mut xdc_files = Vec::new();
        if let Ok(rd) = std::fs::read_dir(root_dir) {
            let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let p = entry.path();
                let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
                    continue;
                };
                let fname = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                match ext.to_ascii_lowercase().as_str() {
                    "sv" | "v" | "vhd" | "vhdl" => hdl_files.push(fname),
                    "xdc" | "sdc" => xdc_files.push(fname),
                    _ => {}
                }
            }
        }
        if hdl_files.is_empty() {
            return Err(format!(
                "ipx::package_project: no HDL files found in {}",
                root_dir.display()
            ));
        }

        let mut combined_rtl = String::new();
        for f in &hdl_files {
            if let Ok(txt) = std::fs::read_to_string(root_dir.join(f)) {
                combined_rtl.push_str(&txt);
                combined_rtl.push('\n');
            }
        }

        let (inferred_top, model_parameters, ports) = parse_rtl_module_signature(&combined_rtl);
        let core_name = name.unwrap_or(&inferred_top).to_string();

        let mut core = PackagedIpCore {
            vendor: if vendor.is_empty() {
                "community".into()
            } else {
                vendor.into()
            },
            library: if library.is_empty() {
                "helion".into()
            } else {
                library.into()
            },
            name: core_name,
            version: if version.is_empty() {
                "1.0".into()
            } else {
                version.into()
            },
            top_module: inferred_top,
            root_dir: root_dir.to_path_buf(),
            hdl_files,
            xdc_files,
            ports,
            model_parameters,
            bus_interfaces: Vec::new(),
        };

        // Auto-infer standard interfaces (clock, reset, AXI / Helion-MM / Helion-ST)
        let _ = infer_all_buses(&mut core);
        Ok(core)
    }

    /// Infer all standard bus interfaces on `core`.
    pub fn infer_all_buses(core: &mut PackagedIpCore) -> Vec<String> {
        let mut inferred = Vec::new();
        for vlnv in [
            "xilinx.com:signal:clock_rtl:1.0",
            "xilinx.com:signal:reset_rtl:1.0",
            "xilinx.com:interface:aximm_rtl:1.0",
            "xilinx.com:interface:axis_rtl:1.0",
            "helion.org:bus:helion_mm_rtl:1.0",
        ] {
            if let Ok(names) = infer_bus_interface(core, vlnv) {
                inferred.extend(names);
            }
        }
        inferred
    }

    /// `ipx::infer_bus_interface`: infer bus interfaces matching `abstraction_vlnv` from `core.ports`.
    pub fn infer_bus_interface(
        core: &mut PackagedIpCore,
        abstraction_vlnv: &str,
    ) -> Result<Vec<String>, String> {
        let low = abstraction_vlnv.to_ascii_lowercase();
        let mut added = Vec::new();

        if low.contains("clock") {
            let clk_ports: Vec<String> = core
                .ports
                .iter()
                .filter(|p| {
                    let n = p.name.to_ascii_lowercase();
                    n == "clk"
                        || n == "aclk"
                        || n == "clock"
                        || n.ends_with("_aclk")
                        || n.ends_with("_clk")
                })
                .map(|p| p.name.clone())
                .collect();
            for cp in clk_ports {
                let bname = cp.to_ascii_uppercase();
                let mut port_maps = BTreeMap::new();
                port_maps.insert("CLK".into(), cp.clone());
                let mut parameters = BTreeMap::new();
                parameters.insert("FREQ_HZ".into(), "100000000".into());
                upsert_bus(
                    core,
                    IpxBusInterface {
                        name: bname.clone(),
                        bus_type_vlnv: "xilinx.com:signal:clock:1.0".into(),
                        abstraction_type_vlnv: "xilinx.com:signal:clock_rtl:1.0".into(),
                        mode: IpxBusMode::Slave,
                        port_maps,
                        parameters,
                    },
                );
                added.push(bname);
            }
        } else if low.contains("reset") {
            let rst_ports: Vec<String> = core
                .ports
                .iter()
                .filter(|p| {
                    let n = p.name.to_ascii_lowercase();
                    n == "rst"
                        || n == "reset"
                        || n == "rst_n"
                        || n == "aresetn"
                        || n.ends_with("_aresetn")
                        || n.ends_with("_rst")
                })
                .map(|p| p.name.clone())
                .collect();
            for rp in rst_ports {
                let bname = rp.to_ascii_uppercase();
                let active_low = rp.to_ascii_lowercase().ends_with('n');
                let mut port_maps = BTreeMap::new();
                port_maps.insert("RST".into(), rp.clone());
                let mut parameters = BTreeMap::new();
                parameters.insert(
                    "POLARITY".into(),
                    if active_low {
                        "ACTIVE_LOW"
                    } else {
                        "ACTIVE_HIGH"
                    }
                    .into(),
                );
                upsert_bus(
                    core,
                    IpxBusInterface {
                        name: bname.clone(),
                        bus_type_vlnv: "xilinx.com:signal:reset:1.0".into(),
                        abstraction_type_vlnv: "xilinx.com:signal:reset_rtl:1.0".into(),
                        mode: IpxBusMode::Slave,
                        port_maps,
                        parameters,
                    },
                );
                // Link ASSOCIATED_RESET on any inferred clock interface
                for clk_bus in &mut core.bus_interfaces {
                    if clk_bus.bus_type_vlnv.contains("clock") {
                        clk_bus
                            .parameters
                            .insert("ASSOCIATED_RESET".into(), rp.clone());
                    }
                }
                added.push(bname);
            }
        } else if low.contains("aximm") {
            for (prefix, bname, mode) in [
                ("s_axi_", "S_AXI", IpxBusMode::Slave),
                ("m_axi_", "M_AXI", IpxBusMode::Master),
            ] {
                let mut port_maps = BTreeMap::new();
                for p in &core.ports {
                    let nl = p.name.to_ascii_lowercase();
                    if let Some(suf) = nl.strip_prefix(prefix) {
                        if suf != "aclk" && suf != "aresetn" {
                            port_maps.insert(suf.to_ascii_uppercase(), p.name.clone());
                        }
                    }
                }
                if !port_maps.is_empty() {
                    let mut parameters = BTreeMap::new();
                    parameters.insert("PROTOCOL".into(), "AXI4LITE".into());
                    parameters.insert("DATA_WIDTH".into(), "32".into());
                    upsert_bus(
                        core,
                        IpxBusInterface {
                            name: bname.into(),
                            bus_type_vlnv: "xilinx.com:interface:aximm:1.0".into(),
                            abstraction_type_vlnv: "xilinx.com:interface:aximm_rtl:1.0".into(),
                            mode,
                            port_maps,
                            parameters,
                        },
                    );
                    link_associated_busif(core, bname);
                    added.push(bname.into());
                }
            }
        } else if low.contains("axis") {
            for (prefix, bname, mode) in [
                ("s_axis_", "S_AXIS", IpxBusMode::Slave),
                ("m_axis_", "M_AXIS", IpxBusMode::Master),
            ] {
                let mut port_maps = BTreeMap::new();
                for p in &core.ports {
                    let nl = p.name.to_ascii_lowercase();
                    if let Some(suf) = nl.strip_prefix(prefix) {
                        port_maps.insert(suf.to_ascii_uppercase(), p.name.clone());
                    }
                }
                if !port_maps.is_empty() {
                    let mut parameters = BTreeMap::new();
                    parameters.insert("TDATA_NUM_BYTES".into(), "4".into());
                    upsert_bus(
                        core,
                        IpxBusInterface {
                            name: bname.into(),
                            bus_type_vlnv: "xilinx.com:interface:axis:1.0".into(),
                            abstraction_type_vlnv: "xilinx.com:interface:axis_rtl:1.0".into(),
                            mode,
                            port_maps,
                            parameters,
                        },
                    );
                    link_associated_busif(core, bname);
                    added.push(bname.into());
                }
            }
        } else if low.contains("helion_mm") || low.contains("helion-mm") {
            let mut port_maps = BTreeMap::new();
            for p in &core.ports {
                let nl = p.name.to_ascii_lowercase();
                if matches!(
                    nl.as_str(),
                    "addr" | "wdata" | "rdata" | "we" | "valid" | "ready"
                ) {
                    port_maps.insert(nl.to_ascii_uppercase(), p.name.clone());
                }
            }
            if port_maps.len() >= 2 {
                upsert_bus(
                    core,
                    IpxBusInterface {
                        name: "HELION_MM".into(),
                        bus_type_vlnv: "helion.org:bus:helion_mm:1.0".into(),
                        abstraction_type_vlnv: "helion.org:bus:helion_mm_rtl:1.0".into(),
                        mode: IpxBusMode::Slave,
                        port_maps,
                        parameters: BTreeMap::new(),
                    },
                );
                link_associated_busif(core, "HELION_MM");
                added.push("HELION_MM".into());
            }
        }
        Ok(added)
    }

    fn upsert_bus(core: &mut PackagedIpCore, bus: IpxBusInterface) {
        if let Some(existing) = core
            .bus_interfaces
            .iter_mut()
            .find(|b| b.name.eq_ignore_ascii_case(&bus.name))
        {
            *existing = bus;
        } else {
            core.bus_interfaces.push(bus);
        }
    }

    fn link_associated_busif(core: &mut PackagedIpCore, bus_name: &str) {
        for clk_bus in &mut core.bus_interfaces {
            if clk_bus.bus_type_vlnv.contains("clock") {
                let entry = clk_bus
                    .parameters
                    .entry("ASSOCIATED_BUSIF".into())
                    .or_default();
                if entry.is_empty() {
                    *entry = bus_name.to_string();
                } else if !entry.split(':').any(|x| x == bus_name) {
                    entry.push(':');
                    entry.push_str(bus_name);
                }
            }
        }
    }

    /// `ipx::add_bus_parameter`: add or update a parameter on a named bus interface.
    pub fn add_bus_parameter(
        core: &mut PackagedIpCore,
        param_name: &str,
        bus_interface_name: &str,
        value: Option<&str>,
    ) -> Result<(), String> {
        let bus = core
            .bus_interface_mut(bus_interface_name)
            .ok_or_else(|| format!("ipx::add_bus_parameter: no bus interface {bus_interface_name}"))?;
        bus.parameters.insert(
            param_name.to_string(),
            value.unwrap_or("").to_string(),
        );
        Ok(())
    }

    /// `ipx::save_core`: write IEEE 1685 `component.xml` and `package.helion` into `core.root_dir`.
    pub fn save_core(core: &PackagedIpCore) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&core.root_dir)
            .map_err(|e| format!("ipx::save_core mkdir: {e}"))?;
        let xml_path = core.root_dir.join("component.xml");
        let xml = core.to_ipxact_xml();
        std::fs::write(&xml_path, &xml)
            .map_err(|e| format!("ipx::save_core {}: {e}", xml_path.display()))?;

        // Also write format-1 `package.helion` so `expand_ip_packages` can ingest it directly
        let mut helion_txt = String::from("# Helion IP package (format 1)\nformat 1\n");
        helion_txt.push_str(&format!("vlnv {}\n", core.vlnv()));
        helion_txt.push_str("bus Helion-MM\n");
        helion_txt.push_str(&format!("top {}\n", core.top_module));
        for f in &core.hdl_files {
            helion_txt.push_str(&format!("file {f}\n"));
        }
        for c in &core.xdc_files {
            helion_txt.push_str(&format!("xdc {c}\n"));
        }
        let helion_path = core.root_dir.join("package.helion");
        std::fs::write(&helion_path, helion_txt)
            .map_err(|e| format!("ipx::save_core {}: {e}", helion_path.display()))?;
        Ok(xml_path)
    }
}

// ---------------------------------------------------------------------------
// 2. `.xci` Instance Configs, `.xcix` Core Containers, `report_ip_status`, `upgrade_ip`
// ---------------------------------------------------------------------------

/// Parsed `.xci` IP instance configuration (`read_ip` / `generate_target` / `upgrade_ip`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XciInstance {
    pub instance_name: String,
    pub vendor: String,
    pub library: String,
    pub core_name: String,
    pub version: String,
    pub part: String,
    pub parameters: BTreeMap<String, String>,
    pub rtl_files: Vec<String>,
    pub generated_targets: Vec<String>,
    pub locked: bool,
    pub xci_path: PathBuf,
}

impl XciInstance {
    pub fn new(instance_name: impl Into<String>, vlnv: &str, part: impl Into<String>) -> Result<Self, String> {
        let parts: Vec<&str> = vlnv.split(':').collect();
        if parts.len() != 4 {
            return Err(format!("invalid VLNV {vlnv} (expected vendor:library:name:version)"));
        }
        Ok(Self {
            instance_name: instance_name.into(),
            vendor: parts[0].to_string(),
            library: parts[1].to_string(),
            core_name: parts[2].to_string(),
            version: parts[3].to_string(),
            part: part.into(),
            parameters: BTreeMap::new(),
            rtl_files: Vec::new(),
            generated_targets: Vec::new(),
            locked: false,
            xci_path: PathBuf::new(),
        })
    }

    pub fn vlnv(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.vendor, self.library, self.core_name, self.version
        )
    }

    /// Serialize to Vivado JSON-style `.xci` text.
    pub fn to_xci_text(&self) -> String {
        let mut out = String::new();
        out.push_str("{\n");
        out.push_str("  \"schema\": \"helion-xci-v1\",\n");
        out.push_str(&format!("  \"instance_name\": \"{}\",\n", self.instance_name));
        out.push_str(&format!("  \"vlnv\": \"{}\",\n", self.vlnv()));
        out.push_str(&format!("  \"part\": \"{}\",\n", self.part));
        out.push_str(&format!("  \"locked\": {},\n", self.locked));
        out.push_str("  \"parameters\": {\n");
        for (idx, (k, v)) in self.parameters.iter().enumerate() {
            let comma = if idx + 1 < self.parameters.len() { "," } else { "" };
            out.push_str(&format!("    \"{k}\": \"{v}\"{comma}\n"));
        }
        out.push_str("  },\n");
        let files_str: Vec<String> = self.rtl_files.iter().map(|f| format!("\"{f}\"")).collect();
        out.push_str(&format!("  \"rtl_files\": [{}]\n", files_str.join(", ")));
        out.push_str("}\n");
        out
    }

    /// Parse `.xci` from JSON or key-value text.
    pub fn parse_xci(text: &str, path: &Path) -> Result<Self, String> {
        let mut inst_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("ip_inst")
            .to_string();
        let mut vlnv = "community:helion:h_gpio:1.0".to_string();
        let mut part = "HL10T-C32-1".to_string();
        let mut locked = false;
        let mut parameters = BTreeMap::new();
        let mut rtl_files = Vec::new();
        let mut in_params = false;

        for raw in text.lines() {
            let line = raw.trim().trim_end_matches(',');
            if line.is_empty() || line == "{" || line == "}" {
                continue;
            }
            if line.starts_with("\"parameters\"") {
                in_params = !line.ends_with('}');
                continue;
            }
            if in_params {
                if line == "}" {
                    in_params = false;
                    continue;
                }
                if let Some((k, v)) = line.split_once(':') {
                    let k = k.trim().trim_matches('"');
                    let v = v.trim().trim_matches('"');
                    if !k.is_empty() {
                        parameters.insert(k.to_string(), v.to_string());
                    }
                }
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().trim_matches('"');
                let val = v.trim();
                match key {
                    "instance_name" | "component_name" => {
                        inst_name = val.trim_matches('"').to_string();
                    }
                    "vlnv" | "ipdef" => {
                        vlnv = v
                            .trim()
                            .trim_matches('"')
                            .to_string();
                    }
                    "part" => {
                        part = val.trim_matches('"').to_string();
                    }
                    "locked" => {
                        locked = val.trim_matches('"') == "true";
                    }
                    "rtl_files" => {
                        let inner = val.trim_matches(|c| c == '[' || c == ']');
                        for item in inner.split(',') {
                            let f = item.trim().trim_matches('"');
                            if !f.is_empty() {
                                rtl_files.push(f.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut xci = Self::new(inst_name, &vlnv, part)?;
        xci.locked = locked;
        xci.parameters = parameters;
        xci.rtl_files = rtl_files;
        xci.xci_path = path.to_path_buf();
        Ok(xci)
    }

    /// Write this `.xci` instance to `path`.
    pub fn write_to(&mut self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| format!("write_xci mkdir: {e}"))?;
            }
        }
        std::fs::write(path, self.to_xci_text())
            .map_err(|e| format!("write_xci {}: {e}", path.display()))?;
        self.xci_path = path.to_path_buf();
        Ok(())
    }
}

/// `.xcix` Core Container archive bundling `.xci`, `component.xml`, and generated RTL/OOC products.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XcixContainer {
    pub xci: XciInstance,
    pub embedded_files: BTreeMap<String, Vec<u8>>,
}

impl XcixContainer {
    pub fn new(xci: XciInstance) -> Self {
        Self {
            xci,
            embedded_files: BTreeMap::new(),
        }
    }

    pub fn add_file(&mut self, rel_path: impl Into<String>, bytes: Vec<u8>) {
        self.embedded_files.insert(rel_path.into(), bytes);
    }

    /// Pack `.xcix` container to disk (`HXCX` binary container header + `.xci` + embedded member table).
    pub fn write_xcix(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| format!("write_xcix mkdir: {e}"))?;
            }
        }
        let mut buf = Vec::new();
        buf.extend_from_slice(b"HXCX\x01");
        let xci_bytes = self.xci.to_xci_text().into_bytes();
        buf.extend_from_slice(&(xci_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&xci_bytes);
        buf.extend_from_slice(&(self.embedded_files.len() as u32).to_le_bytes());
        for (name, data) in &self.embedded_files {
            let nb = name.as_bytes();
            buf.extend_from_slice(&(nb.len() as u32).to_le_bytes());
            buf.extend_from_slice(nb);
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(data);
        }
        std::fs::write(path, buf).map_err(|e| format!("write_xcix {}: {e}", path.display()))
    }

    /// Read `.xcix` container from disk.
    pub fn read_xcix(path: &Path) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("read_xcix {}: {e}", path.display()))?;
        if bytes.len() < 9 || &bytes[0..5] != b"HXCX\x01" {
            return Err("read_xcix: bad magic (expected HXCX v1)".into());
        }
        let mut pos = 5usize;
        let read_u32 = |b: &[u8], p: &mut usize| -> Result<u32, String> {
            if *p + 4 > b.len() {
                return Err("read_xcix: truncated".into());
            }
            let v = u32::from_le_bytes(b[*p..*p + 4].try_into().unwrap());
            *p += 4;
            Ok(v)
        };
        let xci_len = read_u32(&bytes, &mut pos)? as usize;
        if pos + xci_len > bytes.len() {
            return Err("read_xcix: truncated xci body".into());
        }
        let xci_str = std::str::from_utf8(&bytes[pos..pos + xci_len])
            .map_err(|e| format!("read_xcix utf8: {e}"))?;
        pos += xci_len;
        let xci = XciInstance::parse_xci(xci_str, path)?;
        let count = read_u32(&bytes, &mut pos)? as usize;
        let mut embedded_files = BTreeMap::new();
        for _ in 0..count {
            let nlen = read_u32(&bytes, &mut pos)? as usize;
            if pos + nlen > bytes.len() {
                return Err("read_xcix: truncated member name".into());
            }
            let name = std::str::from_utf8(&bytes[pos..pos + nlen])
                .map_err(|e| format!("read_xcix utf8: {e}"))?
                .to_string();
            pos += nlen;
            let dlen = read_u32(&bytes, &mut pos)? as usize;
            if pos + dlen > bytes.len() {
                return Err("read_xcix: truncated member data".into());
            }
            embedded_files.insert(name, bytes[pos..pos + dlen].to_vec());
            pos += dlen;
        }
        Ok(Self {
            xci,
            embedded_files,
        })
    }
}

/// Read an IP instance from `.xci` or `.xcix` on disk (`UG835 read_ip`).
pub fn read_ip_file(path: &Path) -> Result<XciInstance, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "xcix" {
        Ok(XcixContainer::read_xcix(path)?.xci)
    } else {
        let txt = std::fs::read_to_string(path)
            .map_err(|e| format!("read_ip {}: {e}", path.display()))?;
        XciInstance::parse_xci(&txt, path)
    }
}

/// `generate_target`: generate synthesis wrapper (`.sv`), simulation model, and instantiation template (`.veo`)
/// for an [`XciInstance`].
pub fn generate_target(
    xci: &mut XciInstance,
    targets: &[&str],
    out_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| format!("generate_target mkdir: {e}"))?;
    let want_all = targets.iter().any(|t| t.eq_ignore_ascii_case("all"));
    let want_synth = want_all
        || targets
            .iter()
            .any(|t| t.eq_ignore_ascii_case("synthesis") || t.eq_ignore_ascii_case("simulation"));
    let want_template = want_all
        || targets
            .iter()
            .any(|t| t.eq_ignore_ascii_case("instantiation_template"));

    let mut generated = Vec::new();
    if want_synth {
        let sv_path = out_dir.join(format!("{}.sv", xci.instance_name));
        let width: usize = xci
            .parameters
            .get("WIDTH")
            .or_else(|| xci.parameters.get("C_WIDTH"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);
        let mut sv = format!(
            "// Generated by Helion generate_target for IP {} ({})\n",
            xci.instance_name,
            xci.vlnv()
        );
        sv.push_str(&format!(
            "module {} (\n  input logic clk,\n  output logic led\n);\n",
            xci.instance_name
        ));
        sv.push_str(&format!("  logic [{}:0] q;\n", width.max(1) - 1));
        sv.push_str("  always_ff @(posedge clk) begin\n");
        sv.push_str(&format!("    q <= q + {}'d1;\n", width.max(1)));
        sv.push_str("  end\n");
        sv.push_str(&format!("  assign led = q[{}];\n", width.max(1) - 1));
        sv.push_str("endmodule\n");
        std::fs::write(&sv_path, sv)
            .map_err(|e| format!("generate_target {}: {e}", sv_path.display()))?;
        let fname = sv_path.display().to_string();
        if !xci.rtl_files.contains(&fname) {
            xci.rtl_files.push(fname);
        }
        if !xci.generated_targets.contains(&"synthesis".to_string()) {
            xci.generated_targets.push("synthesis".into());
        }
        generated.push(sv_path);
    }

    if want_template {
        let veo_path = out_dir.join(format!("{}.veo", xci.instance_name));
        let veo = format!(
            "// Instantiation template for {} ({})\n{} u_{} (\n  .clk(clk),\n  .led(led)\n);\n",
            xci.instance_name,
            xci.vlnv(),
            xci.instance_name,
            xci.instance_name
        );
        std::fs::write(&veo_path, veo)
            .map_err(|e| format!("generate_target {}: {e}", veo_path.display()))?;
        if !xci
            .generated_targets
            .contains(&"instantiation_template".to_string())
        {
            xci.generated_targets.push("instantiation_template".into());
        }
        generated.push(veo_path);
    }
    Ok(generated)
}

/// Status of one IP instance reported by `report_ip_status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpStatusEntry {
    pub instance_name: String,
    pub configured_vlnv: String,
    pub latest_vlnv: String,
    pub status: String,
    pub upgrade_available: bool,
    pub locked: bool,
}

/// Complete `report_ip_status` table.
#[derive(Clone, Debug, Default)]
pub struct IpStatusReport {
    pub entries: Vec<IpStatusEntry>,
}

impl IpStatusReport {
    pub fn text(&self) -> String {
        let upg_cnt = self.entries.iter().filter(|e| e.upgrade_available).count();
        let mut out = format!(
            "report_ip_status total={} upgrade_available={upg_cnt}\n",
            self.entries.len()
        );
        for e in &self.entries {
            out.push_str(&format!(
                "  instance={} configured={} latest={} status={} upgrade_available={} locked={}\n",
                e.instance_name,
                e.configured_vlnv,
                e.latest_vlnv,
                e.status,
                e.upgrade_available,
                e.locked
            ));
        }
        out
    }
}

/// Check version status of `instances` against `catalog_versions` (`core_name -> latest_version`).
pub fn report_ip_status(
    instances: &[XciInstance],
    catalog_versions: &BTreeMap<String, String>,
) -> IpStatusReport {
    let mut entries = Vec::with_capacity(instances.len());
    for inst in instances {
        let latest_ver = catalog_versions
            .get(&inst.core_name)
            .cloned()
            .unwrap_or_else(|| inst.version.clone());
        let latest_vlnv = format!(
            "{}:{}:{}:{}",
            inst.vendor, inst.library, inst.core_name, latest_ver
        );
        let upgrade_available = latest_ver != inst.version && !inst.locked;
        let status = if inst.locked {
            "Locked".into()
        } else if latest_ver == inst.version {
            "Up-to-date".into()
        } else {
            format!("Upgrade available ({} -> {})", inst.version, latest_ver)
        };
        entries.push(IpStatusEntry {
            instance_name: inst.instance_name.clone(),
            configured_vlnv: inst.vlnv(),
            latest_vlnv,
            status,
            upgrade_available,
            locked: inst.locked,
        });
    }
    IpStatusReport { entries }
}

/// `upgrade_ip`: migrate an [`XciInstance`] to the latest catalog version (unless locked).
pub fn upgrade_ip(
    inst: &mut XciInstance,
    catalog_versions: &BTreeMap<String, String>,
) -> Result<String, String> {
    if inst.locked {
        return Err(format!("upgrade_ip: instance {} is locked", inst.instance_name));
    }
    let Some(new_ver) = catalog_versions.get(&inst.core_name) else {
        return Ok(format!(
            "upgrade_ip {}: already at {}",
            inst.instance_name, inst.version
        ));
    };
    let old_ver = inst.version.clone();
    inst.version = new_ver.clone();
    if !inst.xci_path.as_os_str().is_empty() && inst.xci_path.extension().and_then(|e| e.to_str()) == Some("xci") {
        let mut p = inst.xci_path.clone();
        let _ = inst.write_to(&mut p);
    }
    Ok(format!(
        "upgrade_ip {} {} -> {} ({})",
        inst.instance_name,
        old_ver,
        inst.version,
        inst.vlnv()
    ))
}

// ---------------------------------------------------------------------------
// 3. Out-of-Context (OOC) Per-IP Synthesis Checkpoint Cache (`create_ip_run`, `synth_ip`)
// ---------------------------------------------------------------------------

/// Result of synthesizing (or restoring from SHA-256 OOC cache) an IP instance via `synth_ip`.
#[derive(Clone, Debug)]
pub struct OocSynthResult {
    pub instance_name: String,
    pub run_name: String,
    pub cache_key_sha256: String,
    pub cache_hit: bool,
    pub checkpoint_path: PathBuf,
    pub design: Design,
}

/// Content-addressed Out-of-Context (OOC) per-IP synthesis cache (`create_ip_run` / `synth_ip`).
#[derive(Clone, Debug)]
pub struct OocIpCache {
    pub cache_dir: PathBuf,
    pub ip_runs: BTreeMap<String, String>,
    pub synthesis_invocations: usize,
    pub cache_hits: usize,
}

impl OocIpCache {
    pub fn new(cache_dir: impl AsRef<Path>) -> Self {
        Self {
            cache_dir: cache_dir.as_ref().to_path_buf(),
            ip_runs: BTreeMap::new(),
            synthesis_invocations: 0,
            cache_hits: 0,
        }
    }

    /// Compute the deterministic SHA-256 content-address key for an [`XciInstance`].
    pub fn compute_cache_key(&self, xci: &XciInstance) -> String {
        let mut payload = format!("vlnv={};part={};", xci.vlnv(), xci.part);
        for (k, v) in &xci.parameters {
            payload.push_str(&format!("param:{k}={v};"));
        }
        for f in &xci.rtl_files {
            if let Ok(bytes) = std::fs::read(f) {
                payload.push_str(&format!("file_sha={};", ooc_sha256_hex(&bytes)));
            } else {
                payload.push_str(&format!("file_ref={f};"));
            }
        }
        ooc_sha256_hex(payload.as_bytes())
    }

    /// `create_ip_run`: register an OOC synthesis run (`<instance_name>_synth_1`) for `xci`.
    pub fn create_ip_run(&mut self, xci: &XciInstance) -> String {
        let run_name = format!("{}_synth_1", xci.instance_name);
        self.ip_runs
            .insert(xci.instance_name.clone(), run_name.clone());
        run_name
    }

    /// `synth_ip`: synthesize `xci` Out-of-Context or restore its post-synth checkpoint
    /// from the SHA-256 cache if an identical `(VLNV, part, parameters, RTL)` entry exists!
    pub fn synth_ip(&mut self, xci: &XciInstance, dev: &Device) -> Result<OocSynthResult, String> {
        std::fs::create_dir_all(&self.cache_dir)
            .map_err(|e| format!("synth_ip cache mkdir: {e}"))?;
        let run_name = self
            .ip_runs
            .get(&xci.instance_name)
            .cloned()
            .unwrap_or_else(|| self.create_ip_run(xci));
        let key = self.compute_cache_key(xci);
        let ckpt_name = format!("ooc_{}", &key[..48]);
        let ckpt_path = self.cache_dir.join(format!("{ckpt_name}.hck2"));

        if ckpt_path.is_file() {
            if let Ok(restored) = load_checkpoint(&ckpt_path, dev) {
                if let Some(d) = restored.session.design {
                    self.cache_hits += 1;
                    return Ok(OocSynthResult {
                        instance_name: xci.instance_name.clone(),
                        run_name,
                        cache_key_sha256: key,
                        cache_hit: true,
                        checkpoint_path: ckpt_path,
                        design: d,
                    });
                }
            }
        }

        // Cache miss -> synthesize IP Out-of-Context and persist checkpoint under SHA-256 key
        self.synthesis_invocations += 1;
        let design = synthesize_xci_ooc(xci)?;
        let mut session = Session::new(Mode::NonProject);
        session.part = dev.part.clone();
        session.synth_design(design.clone());
        let saved_path = save_checkpoint(
            &session,
            &Constraints::default(),
            &ckpt_name,
            &self.cache_dir,
            &CheckpointSummary::default(),
        )?;

        Ok(OocSynthResult {
            instance_name: xci.instance_name.clone(),
            run_name,
            cache_key_sha256: key,
            cache_hit: false,
            checkpoint_path: saved_path,
            design,
        })
    }
}

fn synthesize_xci_ooc(xci: &XciInstance) -> Result<Design, String> {
    for f in &xci.rtl_files {
        let p = Path::new(f);
        if p.exists() {
            return helion_sv::synth_sv_path(p);
        }
    }
    let width: usize = xci
        .parameters
        .get("WIDTH")
        .or_else(|| xci.parameters.get("C_WIDTH"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(4)
        .max(1);
    let mut d = Design::new(&xci.instance_name);
    d.add_port("clk", PortDir::In);
    d.add_port("led", PortDir::Out);
    for i in 0..width {
        let lut = format!("u_lut{i}");
        let ff = format!("u_ff{i}");
        d.add_cell(&lut, helion_ir::CellKind::Lut6 { init: 0x2 });
        d.add_cell(&ff, helion_ir::CellKind::Hff);
        d.connect("clk", &ff, "CLK");
        d.connect(&format!("d{i}"), &lut, "O");
        d.connect(&format!("d{i}"), &ff, "D");
        d.connect(&format!("q{i}"), &ff, "Q");
        d.connect(&format!("q{i}"), &lut, "I0");
    }
    d.add_cell("u_iob", helion_ir::CellKind::IobOut);
    d.connect("q0", "u_iob", "I");
    d.connect("led", "u_iob", "PAD");
    let _ = resolve(d.name);
    Ok(d)
}

// ---------------------------------------------------------------------------
// 4. Vivado UG1118 `ipx::*` & IP Management TCL Session
// ---------------------------------------------------------------------------

/// TCL session supporting `ipx::package_project`, `ipx::infer_bus_interface`,
/// `ipx::add_bus_parameter`, `ipx::save_core`, `read_ip`, `generate_target`,
/// `report_ip_status`, `upgrade_ip`, `create_ip_run`, and `synth_ip`.
#[derive(Clone, Debug)]
pub struct IpxTclSession {
    pub current_core: Option<PackagedIpCore>,
    pub instances: Vec<XciInstance>,
    pub catalog_versions: BTreeMap<String, String>,
    pub ooc_cache: OocIpCache,
    pub work_dir: PathBuf,
}

impl IpxTclSession {
    pub fn new(work_dir: impl AsRef<Path>) -> Self {
        let wd = work_dir.as_ref().to_path_buf();
        let cache_dir = wd.join(".helion_ooc_cache");
        let mut catalog_versions = BTreeMap::new();
        catalog_versions.insert("h_gpio".into(), "2.0".into());
        catalog_versions.insert("h_uart".into(), "2.0".into());
        catalog_versions.insert("h_timer".into(), "2.0".into());
        catalog_versions.insert("axi_dma_ctrl".into(), "2.1".into());
        Self {
            current_core: None,
            instances: Vec::new(),
            catalog_versions,
            ooc_cache: OocIpCache::new(cache_dir),
            work_dir: wd,
        }
    }

    pub fn eval_script(&mut self, script: &str) -> Result<String, String> {
        let mut last = String::new();
        for raw in script.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            last = self.eval_line(line)?;
        }
        Ok(last)
    }

    pub fn eval_line(&mut self, line: &str) -> Result<String, String> {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some(&cmd) = toks.first() else {
            return Ok(String::new());
        };
        match cmd {
            "ipx::package_project" => {
                let mut root = self.work_dir.clone();
                let mut vendor = "community";
                let mut library = "helion";
                let mut name: Option<&str> = None;
                let mut version = "1.0";
                let mut i = 1;
                while i < toks.len() {
                    match toks[i] {
                        "-root_dir" if i + 1 < toks.len() => {
                            let r = Path::new(toks[i + 1]);
                            root = if r.is_absolute() {
                                r.to_path_buf()
                            } else {
                                self.work_dir.join(r)
                            };
                            i += 2;
                        }
                        "-vendor" if i + 1 < toks.len() => {
                            vendor = toks[i + 1];
                            i += 2;
                        }
                        "-library" if i + 1 < toks.len() => {
                            library = toks[i + 1];
                            i += 2;
                        }
                        "-name" if i + 1 < toks.len() => {
                            name = Some(toks[i + 1]);
                            i += 2;
                        }
                        "-version" if i + 1 < toks.len() => {
                            version = toks[i + 1];
                            i += 2;
                        }
                        _ => i += 1,
                    }
                }
                let core = ipx::package_project(&root, vendor, library, name, version)?;
                let vlnv = core.vlnv();
                self.current_core = Some(core);
                Ok(format!("ipx::package_project {vlnv}"))
            }
            "ipx::infer_bus_interface" => {
                let core = self
                    .current_core
                    .as_mut()
                    .ok_or("ipx::infer_bus_interface: no active core (run ipx::package_project first)")?;
                let vlnv = toks
                    .iter()
                    .skip(1)
                    .find(|t| t.contains(':'))
                    .copied()
                    .unwrap_or("xilinx.com:interface:aximm_rtl:1.0");
                let names = ipx::infer_bus_interface(core, vlnv)?;
                Ok(names.join(" "))
            }
            "ipx::add_bus_parameter" => {
                let core = self
                    .current_core
                    .as_mut()
                    .ok_or("ipx::add_bus_parameter: no active core")?;
                let param = toks.get(1).copied().ok_or("ipx::add_bus_parameter: missing name")?;
                let bus = toks
                    .iter()
                    .skip(2)
                    .map(|t| t.trim_matches(|c: char| "[]{}".contains(c)))
                    .find(|t| !t.is_empty() && !t.starts_with('-') && !t.starts_with("ipx::"))
                    .unwrap_or("S_AXI");
                ipx::add_bus_parameter(core, param, bus, Some("1"))?;
                Ok(format!("{bus}:{param}"))
            }
            "ipx::save_core" => {
                let core = self
                    .current_core
                    .as_ref()
                    .ok_or("ipx::save_core: no active core")?;
                let xml_path = ipx::save_core(core)?;
                Ok(format!("ipx::save_core {}", xml_path.display()))
            }
            "read_ip" => {
                let raw_p = toks
                    .iter()
                    .skip(1)
                    .find(|t| !t.starts_with('-'))
                    .ok_or("read_ip: missing file")?;
                let p = if Path::new(raw_p).is_absolute() {
                    PathBuf::from(raw_p)
                } else {
                    self.work_dir.join(raw_p)
                };
                let inst = read_ip_file(&p)?;
                let name = inst.instance_name.clone();
                self.instances.push(inst);
                Ok(name)
            }
            "generate_target" => {
                let target_str = toks
                    .get(1)
                    .map(|t| t.trim_matches(|c: char| "{}".contains(c)))
                    .unwrap_or("all");
                let targets: Vec<&str> = target_str.split_whitespace().collect();
                let out_dir = self.work_dir.join("ip_gen");
                let mut count = 0usize;
                for inst in &mut self.instances {
                    count += generate_target(inst, &targets, &out_dir)?.len();
                }
                Ok(format!("generate_target files={count}"))
            }
            "report_ip_status" => Ok(report_ip_status(&self.instances, &self.catalog_versions).text()),
            "upgrade_ip" => {
                let mut msgs = Vec::new();
                for inst in &mut self.instances {
                    msgs.push(upgrade_ip(inst, &self.catalog_versions)?);
                }
                Ok(msgs.join("; "))
            }
            "create_ip_run" => {
                let mut runs = Vec::new();
                for inst in &self.instances {
                    runs.push(self.ooc_cache.create_ip_run(inst));
                }
                Ok(runs.join(" "))
            }
            "synth_ip" => {
                let dev = Device::load_part("HL10T-C32-1")?;
                let mut summaries = Vec::new();
                for inst in &self.instances {
                    let res = self.ooc_cache.synth_ip(inst, &dev)?;
                    summaries.push(format!(
                        "{}:hit={},sha256={}",
                        res.instance_name, res.cache_hit, res.cache_key_sha256
                    ));
                }
                Ok(format!("synth_ip {}", summaries.join(" ")))
            }
            other => Err(format!("unknown IP command: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProjectFile, expand_ip_packages};

    #[test]
    fn ug1118_ipx_package_project_bus_inference_and_ieee1685_xml() {
        let dir = std::env::temp_dir().join(format!("helion-ipx-pkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(
            dir.join("axi_dma_ctrl.sv"),
            r#"
module axi_dma_ctrl #(parameter C_DATA_WIDTH = 32, parameter C_ADDR_WIDTH = 8) (
    input logic s_axi_aclk,
    input logic s_axi_aresetn,
    input logic [7:0] s_axi_awaddr,
    input logic s_axi_awvalid,
    output logic s_axi_awready,
    input logic [31:0] s_axi_wdata,
    input logic s_axi_wvalid,
    output logic s_axi_wready,
    output logic [31:0] m_axis_tdata,
    output logic m_axis_tvalid,
    input logic m_axis_tready
);
    assign s_axi_awready = 1'b1;
    assign s_axi_wready = 1'b1;
    assign m_axis_tdata = s_axi_wdata;
    assign m_axis_tvalid = s_axi_wvalid;
endmodule
"#,
        )
        .unwrap();

        let mut core =
            ipx::package_project(&dir, "helion.org", "ip", Some("axi_dma_ctrl"), "1.0").unwrap();
        assert_eq!(core.vlnv(), "helion.org:ip:axi_dma_ctrl:1.0");
        assert_eq!(core.top_module, "axi_dma_ctrl");
        assert_eq!(core.model_parameters.get("C_DATA_WIDTH").map(|s| s.as_str()), Some("32"));

        // Verify automatic AXI4-Lite, AXI4-Stream, clock, and active-low reset bus inference
        let s_axi = core.bus_interface("S_AXI").expect("inferred S_AXI slave");
        assert_eq!(s_axi.mode, IpxBusMode::Slave);
        assert_eq!(s_axi.port_maps.get("AWADDR").map(|s| s.as_str()), Some("s_axi_awaddr"));
        assert_eq!(s_axi.port_maps.get("WDATA").map(|s| s.as_str()), Some("s_axi_wdata"));

        let m_axis = core.bus_interface("M_AXIS").expect("inferred M_AXIS master");
        assert_eq!(m_axis.mode, IpxBusMode::Master);
        assert_eq!(m_axis.port_maps.get("TDATA").map(|s| s.as_str()), Some("m_axis_tdata"));

        let rst = core
            .bus_interface("S_AXI_ARESETN")
            .expect("inferred reset interface");
        assert_eq!(rst.parameters.get("POLARITY").map(|s| s.as_str()), Some("ACTIVE_LOW"));

        let clk = core
            .bus_interface("S_AXI_ACLK")
            .expect("inferred clock interface");
        assert_eq!(
            clk.parameters.get("ASSOCIATED_RESET").map(|s| s.as_str()),
            Some("s_axi_aresetn")
        );
        assert!(
            clk.parameters
                .get("ASSOCIATED_BUSIF")
                .is_some_and(|v| v.contains("S_AXI") && v.contains("M_AXIS"))
        );

        ipx::add_bus_parameter(&mut core, "SUPPORTS_NARROW_BURST", "S_AXI", Some("0")).unwrap();
        assert_eq!(
            core.bus_interface("S_AXI")
                .unwrap()
                .parameters
                .get("SUPPORTS_NARROW_BURST")
                .map(|s| s.as_str()),
            Some("0")
        );

        let xml_path = ipx::save_core(&core).unwrap();
        assert!(xml_path.is_file());
        let xml_body = std::fs::read_to_string(&xml_path).unwrap();
        assert!(xml_body.contains("<spirit:component"));
        assert!(xml_body.contains("<spirit:name>axi_dma_ctrl</spirit:name>"));
        assert!(xml_body.contains("SUPPORTS_NARROW_BURST"));

        // Verify generated package.helion integrates with expand_ip_packages
        let mut prj = ProjectFile::default();
        prj.ip_packages
            .push(dir.join("package.helion").display().to_string());
        let loaded = expand_ip_packages(&mut prj, &dir.join("top.prj")).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(prj.top.as_deref(), Some("axi_dma_ctrl"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn xci_xcix_containers_generate_target_status_and_upgrade_ip() {
        let dir = std::env::temp_dir().join(format!("helion-xci-xcix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut xci = XciInstance::new("u_gpio_0", "community:helion:h_gpio:1.0", "HL10T-C32-1").unwrap();
        xci.parameters.insert("WIDTH".into(), "4".into());
        let xci_path = dir.join("u_gpio_0.xci");
        xci.write_to(&xci_path).unwrap();

        let loaded_xci = read_ip_file(&xci_path).unwrap();
        assert_eq!(loaded_xci.instance_name, "u_gpio_0");
        assert_eq!(loaded_xci.vlnv(), "community:helion:h_gpio:1.0");
        assert_eq!(loaded_xci.parameters.get("WIDTH").map(|s| s.as_str()), Some("4"));

        // Generate synthesis + instantiation_template targets
        let gen_files = generate_target(
            &mut xci,
            &["synthesis", "instantiation_template"],
            &dir.join("gen"),
        )
        .unwrap();
        assert_eq!(gen_files.len(), 2);
        assert!(dir.join("gen/u_gpio_0.sv").is_file());
        assert!(dir.join("gen/u_gpio_0.veo").is_file());

        // Pack and unpack .xcix Core Container
        let mut xcix = XcixContainer::new(xci.clone());
        xcix.add_file(
            "u_gpio_0.sv",
            std::fs::read(dir.join("gen/u_gpio_0.sv")).unwrap(),
        );
        let xcix_path = dir.join("u_gpio_0.xcix");
        xcix.write_xcix(&xcix_path).unwrap();
        let unpacked = XcixContainer::read_xcix(&xcix_path).unwrap();
        assert_eq!(unpacked.xci.instance_name, "u_gpio_0");
        assert!(unpacked.embedded_files.contains_key("u_gpio_0.sv"));

        // Check report_ip_status & upgrade_ip from 1.0 -> 2.0
        let mut catalog = BTreeMap::new();
        catalog.insert("h_gpio".into(), "2.0".into());
        let st_before = report_ip_status(&[xci.clone()], &catalog);
        assert!(st_before.entries[0].upgrade_available);
        assert!(st_before.text().contains("upgrade_available=1"));

        let upg_msg = upgrade_ip(&mut xci, &catalog).unwrap();
        assert!(upg_msg.contains("1.0 -> 2.0"), "{upg_msg}");
        assert_eq!(xci.vlnv(), "community:helion:h_gpio:2.0");
        let st_after = report_ip_status(&[xci], &catalog);
        assert!(!st_after.entries[0].upgrade_available);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ooc_per_ip_sha256_synthesis_cache_hits_and_invalidates_on_change() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let dir = std::env::temp_dir().join(format!("helion-ooc-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut cache = OocIpCache::new(dir.join("cache"));
        let mut xci = XciInstance::new("u_timer_0", "community:helion:h_timer:1.0", "HL10T-C32-1").unwrap();
        xci.parameters.insert("WIDTH".into(), "4".into());
        let run_name = cache.create_ip_run(&xci);
        assert_eq!(run_name, "u_timer_0_synth_1");

        // 1st synth_ip -> cache miss (synthesizes and writes checkpoint)
        let r1 = cache.synth_ip(&xci, &dev).unwrap();
        assert!(!r1.cache_hit);
        assert_eq!(cache.synthesis_invocations, 1);
        assert_eq!(cache.cache_hits, 0);
        assert!(r1.checkpoint_path.is_file());

        // 2nd synth_ip with identical IP config -> SHA-256 cache HIT (no re-synthesis!)
        let r2 = cache.synth_ip(&xci, &dev).unwrap();
        assert!(r2.cache_hit, "unchanged IP must hit OOC SHA-256 cache");
        assert_eq!(r1.cache_key_sha256, r2.cache_key_sha256);
        assert_eq!(cache.synthesis_invocations, 1, "must not re-synthesize on cache hit");
        assert_eq!(cache.cache_hits, 1);
        assert_eq!(r2.design.cells.len(), r1.design.cells.len());

        // 3rd synth_ip after changing WIDTH parameter -> new SHA-256 key, cache miss
        xci.parameters.insert("WIDTH".into(), "6".into());
        let r3 = cache.synth_ip(&xci, &dev).unwrap();
        assert!(!r3.cache_hit, "modified parameter must invalidate OOC cache");
        assert_ne!(r3.cache_key_sha256, r1.cache_key_sha256);
        assert_eq!(cache.synthesis_invocations, 2);
        assert!(r3.design.cells.len() > r1.design.cells.len());

        // Test full Tcl flow via IpxTclSession
        xci.write_to(&dir.join("u_timer_0.xci")).unwrap();
        let mut tcl = IpxTclSession::new(&dir);
        std::fs::write(
            dir.join("custom_ip.sv"),
            "module custom_ip(\n  input logic clk,\n  input logic rst_n,\n  input logic [31:0] s_axi_wdata,\n  output logic led\n);\n  assign led = s_axi_wdata[0];\nendmodule\n",
        )
        .unwrap();
        tcl.eval_script(
            r#"
ipx::package_project -root_dir . -vendor helion.org -library user -name custom_ip -version 1.0
ipx::infer_bus_interface xilinx.com:interface:aximm_rtl:1.0
ipx::add_bus_parameter SUPPORTS_LOCK [ipx::get_bus_interfaces S_AXI -of_objects [ipx::current_core]]
ipx::save_core [ipx::current_core]
read_ip u_timer_0.xci
generate_target all [get_ips u_timer_0]
report_ip_status
upgrade_ip [get_ips u_timer_0]
create_ip_run [get_ips u_timer_0]
synth_ip [get_ips u_timer_0]
"#,
        )
        .unwrap();
        assert!(dir.join("component.xml").is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
