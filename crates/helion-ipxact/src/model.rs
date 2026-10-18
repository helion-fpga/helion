//! IEEE 1685-2014 (IP-XACT) rich component model: bus interfaces, port maps,
//! configurable parameters, address blocks, and memory maps.

use crate::IpCore;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// Supported bus interface standards across Vivado UG994 IP Integrator and Helion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BusInterfaceType {
    Axi4,
    Axi4Lite,
    Axi4Stream,
    Apb,
    Wishbone,
    Clock,
    Reset,
    Interrupt,
    Gpio,
    Uart,
    Spi,
    I2c,
    BramPort,
    HelionMm,
    HelionSt,
    HelionDbg,
}

impl BusInterfaceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Axi4 => "AXI4",
            Self::Axi4Lite => "AXI4-Lite",
            Self::Axi4Stream => "AXI4-Stream",
            Self::Apb => "APB",
            Self::Wishbone => "Wishbone",
            Self::Clock => "Clock",
            Self::Reset => "Reset",
            Self::Interrupt => "Interrupt",
            Self::Gpio => "GPIO",
            Self::Uart => "UART",
            Self::Spi => "SPI",
            Self::I2c => "I2C",
            Self::BramPort => "BRAM_PORT",
            Self::HelionMm => "Helion-MM",
            Self::HelionSt => "Helion-ST",
            Self::HelionDbg => "Helion-DBG",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let norm = s.trim().to_ascii_uppercase().replace(['-', '_'], "");
        match norm.as_str() {
            "AXI4" | "AXI4FULL" | "AXIMM" => Some(Self::Axi4),
            "AXI4LITE" | "AXILITE" | "AXIL" => Some(Self::Axi4Lite),
            "AXI4STREAM" | "AXISTREAM" | "AXIS" => Some(Self::Axi4Stream),
            "APB" | "APB3" | "APB4" => Some(Self::Apb),
            "WISHBONE" | "WB" => Some(Self::Wishbone),
            "CLOCK" | "CLK" => Some(Self::Clock),
            "RESET" | "RST" | "ARESETN" => Some(Self::Reset),
            "INTERRUPT" | "INTR" | "IRQ" => Some(Self::Interrupt),
            "GPIO" => Some(Self::Gpio),
            "UART" => Some(Self::Uart),
            "SPI" => Some(Self::Spi),
            "I2C" | "IIC" => Some(Self::I2c),
            "BRAMPORT" | "BRAM" => Some(Self::BramPort),
            "HELIONMM" => Some(Self::HelionMm),
            "HELIONST" => Some(Self::HelionSt),
            "HELIONDBG" => Some(Self::HelionDbg),
            _ => None,
        }
    }

    /// True if this interface carries memory-mapped address/data transactions.
    pub fn is_memory_mapped(&self) -> bool {
        matches!(
            self,
            Self::Axi4
                | Self::Axi4Lite
                | Self::Apb
                | Self::Wishbone
                | Self::BramPort
                | Self::HelionMm
        )
    }

    /// True if this interface is AXI4 or AXI4-Lite.
    pub fn is_axi_mm(&self) -> bool {
        matches!(self, Self::Axi4 | Self::Axi4Lite)
    }

    /// True if an `axi_protocol_converter` or `axi_interconnect` can bridge `self` (master) to `target` (slave).
    pub fn can_bridge_to(&self, target: Self) -> bool {
        if *self == target {
            return true;
        }
        matches!(
            (*self, target),
            (Self::Axi4, Self::Axi4Lite)
                | (Self::Axi4Lite, Self::Axi4)
                | (Self::Axi4, Self::Apb)
                | (Self::Axi4Lite, Self::Apb)
                | (Self::Axi4, Self::Wishbone)
                | (Self::Axi4Lite, Self::Wishbone)
        )
    }
}

impl fmt::Display for BusInterfaceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BusInterfaceType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown bus interface type '{s}'"))
    }
}

/// Bus interface direction / role in IEEE 1685.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InterfaceMode {
    Master,
    Slave,
    Monitor,
    System,
}

impl InterfaceMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Slave => "slave",
            Self::Monitor => "monitor",
            Self::System => "system",
        }
    }

    pub fn is_compatible_with(&self, other: Self) -> bool {
        matches!(
            (*self, other),
            (Self::Master, Self::Slave)
                | (Self::Slave, Self::Master)
                | (Self::Monitor, _)
                | (_, Self::Monitor)
        )
    }
}

impl fmt::Display for InterfaceMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Physical port direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PortDirection {
    In,
    Out,
    InOut,
}

impl PortDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
            Self::InOut => "inout",
        }
    }

    pub fn sv_keyword(&self) -> &'static str {
        match self {
            Self::In => "input",
            Self::Out => "output",
            Self::InOut => "inout",
        }
    }
}

/// Reset polarity metadata for clock/reset domain DRC and automation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResetPolarity {
    ActiveLow,
    ActiveHigh,
}

/// Physical HDL port on an IP component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpPort {
    pub name: String,
    pub direction: PortDirection,
    pub width: u32,
    pub width_expr: Option<String>,
    pub default_value: Option<String>,
}

impl IpPort {
    pub fn scalar_in(name: &str, default_val: &str) -> Self {
        Self {
            name: name.into(),
            direction: PortDirection::In,
            width: 1,
            width_expr: None,
            default_value: Some(default_val.into()),
        }
    }

    pub fn scalar_out(name: &str) -> Self {
        Self {
            name: name.into(),
            direction: PortDirection::Out,
            width: 1,
            width_expr: None,
            default_value: None,
        }
    }

    pub fn vec_in(name: &str, width: u32, default_val: &str) -> Self {
        Self {
            name: name.into(),
            direction: PortDirection::In,
            width,
            width_expr: None,
            default_value: Some(default_val.into()),
        }
    }

    pub fn vec_out(name: &str, width: u32) -> Self {
        Self {
            name: name.into(),
            direction: PortDirection::Out,
            width,
            width_expr: None,
            default_value: None,
        }
    }
}

/// Logical-to-physical signal mapping inside an IP-XACT bus interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortMapEntry {
    pub logical_name: String,
    pub physical_name: String,
    pub width: u32,
    pub direction: PortDirection,
}

impl PortMapEntry {
    pub fn new(logical: &str, physical: &str, width: u32, direction: PortDirection) -> Self {
        Self {
            logical_name: logical.into(),
            physical_name: physical.into(),
            width,
            direction,
        }
    }
}

/// IEEE 1685 busInterface descriptor on an IP component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusInterface {
    pub name: String,
    pub bus_type: BusInterfaceType,
    pub mode: InterfaceMode,
    pub associated_clock: Option<String>,
    pub associated_reset: Option<String>,
    pub reset_polarity: Option<ResetPolarity>,
    pub addr_width: u32,
    pub data_width: u32,
    pub default_range: u64,
    pub port_maps: Vec<PortMapEntry>,
}

impl BusInterface {
    /// Construct a standard AXI4-Lite Slave interface with prefix (e.g. `"s_axi"`).
    pub fn axi4lite_slave(
        intf_name: &str,
        prefix: &str,
        addr_width: u32,
        data_width: u32,
        default_range: u64,
        clk_port: &str,
        rst_port: &str,
    ) -> Self {
        let strb_w = (data_width / 8).max(1);
        Self {
            name: intf_name.into(),
            bus_type: BusInterfaceType::Axi4Lite,
            mode: InterfaceMode::Slave,
            associated_clock: Some(clk_port.into()),
            associated_reset: Some(rst_port.into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width,
            data_width,
            default_range,
            port_maps: vec![
                PortMapEntry::new("AWADDR", &format!("{prefix}_awaddr"), addr_width, PortDirection::In),
                PortMapEntry::new("AWPROT", &format!("{prefix}_awprot"), 3, PortDirection::In),
                PortMapEntry::new("AWVALID", &format!("{prefix}_awvalid"), 1, PortDirection::In),
                PortMapEntry::new("AWREADY", &format!("{prefix}_awready"), 1, PortDirection::Out),
                PortMapEntry::new("WDATA", &format!("{prefix}_wdata"), data_width, PortDirection::In),
                PortMapEntry::new("WSTRB", &format!("{prefix}_wstrb"), strb_w, PortDirection::In),
                PortMapEntry::new("WVALID", &format!("{prefix}_wvalid"), 1, PortDirection::In),
                PortMapEntry::new("WREADY", &format!("{prefix}_wready"), 1, PortDirection::Out),
                PortMapEntry::new("BRESP", &format!("{prefix}_bresp"), 2, PortDirection::Out),
                PortMapEntry::new("BVALID", &format!("{prefix}_bvalid"), 1, PortDirection::Out),
                PortMapEntry::new("BREADY", &format!("{prefix}_bready"), 1, PortDirection::In),
                PortMapEntry::new("ARADDR", &format!("{prefix}_araddr"), addr_width, PortDirection::In),
                PortMapEntry::new("ARPROT", &format!("{prefix}_arprot"), 3, PortDirection::In),
                PortMapEntry::new("ARVALID", &format!("{prefix}_arvalid"), 1, PortDirection::In),
                PortMapEntry::new("ARREADY", &format!("{prefix}_arready"), 1, PortDirection::Out),
                PortMapEntry::new("RDATA", &format!("{prefix}_rdata"), data_width, PortDirection::Out),
                PortMapEntry::new("RRESP", &format!("{prefix}_rresp"), 2, PortDirection::Out),
                PortMapEntry::new("RVALID", &format!("{prefix}_rvalid"), 1, PortDirection::Out),
                PortMapEntry::new("RREADY", &format!("{prefix}_rready"), 1, PortDirection::In),
            ],
        }
    }

    /// Construct a standard AXI4-Lite Master interface with prefix (e.g. `"m_axi"`).
    pub fn axi4lite_master(
        intf_name: &str,
        prefix: &str,
        addr_width: u32,
        data_width: u32,
        clk_port: &str,
        rst_port: &str,
    ) -> Self {
        let strb_w = (data_width / 8).max(1);
        Self {
            name: intf_name.into(),
            bus_type: BusInterfaceType::Axi4Lite,
            mode: InterfaceMode::Master,
            associated_clock: Some(clk_port.into()),
            associated_reset: Some(rst_port.into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width,
            data_width,
            default_range: 0,
            port_maps: vec![
                PortMapEntry::new("AWADDR", &format!("{prefix}_awaddr"), addr_width, PortDirection::Out),
                PortMapEntry::new("AWPROT", &format!("{prefix}_awprot"), 3, PortDirection::Out),
                PortMapEntry::new("AWVALID", &format!("{prefix}_awvalid"), 1, PortDirection::Out),
                PortMapEntry::new("AWREADY", &format!("{prefix}_awready"), 1, PortDirection::In),
                PortMapEntry::new("WDATA", &format!("{prefix}_wdata"), data_width, PortDirection::Out),
                PortMapEntry::new("WSTRB", &format!("{prefix}_wstrb"), strb_w, PortDirection::Out),
                PortMapEntry::new("WVALID", &format!("{prefix}_wvalid"), 1, PortDirection::Out),
                PortMapEntry::new("WREADY", &format!("{prefix}_wready"), 1, PortDirection::In),
                PortMapEntry::new("BRESP", &format!("{prefix}_bresp"), 2, PortDirection::In),
                PortMapEntry::new("BVALID", &format!("{prefix}_bvalid"), 1, PortDirection::In),
                PortMapEntry::new("BREADY", &format!("{prefix}_bready"), 1, PortDirection::Out),
                PortMapEntry::new("ARADDR", &format!("{prefix}_araddr"), addr_width, PortDirection::Out),
                PortMapEntry::new("ARPROT", &format!("{prefix}_arprot"), 3, PortDirection::Out),
                PortMapEntry::new("ARVALID", &format!("{prefix}_arvalid"), 1, PortDirection::Out),
                PortMapEntry::new("ARREADY", &format!("{prefix}_arready"), 1, PortDirection::In),
                PortMapEntry::new("RDATA", &format!("{prefix}_rdata"), data_width, PortDirection::In),
                PortMapEntry::new("RRESP", &format!("{prefix}_rresp"), 2, PortDirection::In),
                PortMapEntry::new("RVALID", &format!("{prefix}_rvalid"), 1, PortDirection::In),
                PortMapEntry::new("RREADY", &format!("{prefix}_rready"), 1, PortDirection::Out),
            ],
        }
    }

    /// Construct a full AXI4 Master or Slave interface.
    pub fn axi4_full(
        intf_name: &str,
        prefix: &str,
        mode: InterfaceMode,
        addr_width: u32,
        data_width: u32,
        default_range: u64,
        clk_port: &str,
        rst_port: &str,
    ) -> Self {
        let mut base = if mode == InterfaceMode::Master {
            Self::axi4lite_master(intf_name, prefix, addr_width, data_width, clk_port, rst_port)
        } else {
            Self::axi4lite_slave(
                intf_name,
                prefix,
                addr_width,
                data_width,
                default_range,
                clk_port,
                rst_port,
            )
        };
        base.bus_type = BusInterfaceType::Axi4;
        let req_dir = if mode == InterfaceMode::Master {
            PortDirection::Out
        } else {
            PortDirection::In
        };
        let resp_dir = if mode == InterfaceMode::Master {
            PortDirection::In
        } else {
            PortDirection::Out
        };
        base.port_maps.extend([
            PortMapEntry::new("AWLEN", &format!("{prefix}_awlen"), 8, req_dir),
            PortMapEntry::new("AWSIZE", &format!("{prefix}_awsize"), 3, req_dir),
            PortMapEntry::new("AWBURST", &format!("{prefix}_awburst"), 2, req_dir),
            PortMapEntry::new("WLAST", &format!("{prefix}_wlast"), 1, req_dir),
            PortMapEntry::new("ARLEN", &format!("{prefix}_arlen"), 8, req_dir),
            PortMapEntry::new("ARSIZE", &format!("{prefix}_arsize"), 3, req_dir),
            PortMapEntry::new("ARBURST", &format!("{prefix}_arburst"), 2, req_dir),
            PortMapEntry::new("RLAST", &format!("{prefix}_rlast"), 1, resp_dir),
        ]);
        base
    }

    /// Construct an AXI4-Stream interface.
    pub fn axi4stream(
        intf_name: &str,
        prefix: &str,
        mode: InterfaceMode,
        data_width: u32,
        clk_port: &str,
        rst_port: &str,
    ) -> Self {
        let keep_w = (data_width / 8).max(1);
        let (fwd, bwd) = if mode == InterfaceMode::Master {
            (PortDirection::Out, PortDirection::In)
        } else {
            (PortDirection::In, PortDirection::Out)
        };
        Self {
            name: intf_name.into(),
            bus_type: BusInterfaceType::Axi4Stream,
            mode,
            associated_clock: Some(clk_port.into()),
            associated_reset: Some(rst_port.into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width: 0,
            data_width,
            default_range: 0,
            port_maps: vec![
                PortMapEntry::new("TDATA", &format!("{prefix}_tdata"), data_width, fwd),
                PortMapEntry::new("TKEEP", &format!("{prefix}_tkeep"), keep_w, fwd),
                PortMapEntry::new("TLAST", &format!("{prefix}_tlast"), 1, fwd),
                PortMapEntry::new("TVALID", &format!("{prefix}_tvalid"), 1, fwd),
                PortMapEntry::new("TREADY", &format!("{prefix}_tready"), 1, bwd),
            ],
        }
    }

    pub fn clock_port(intf_name: &str, port_name: &str, mode: InterfaceMode) -> Self {
        let dir = if mode == InterfaceMode::Master {
            PortDirection::Out
        } else {
            PortDirection::In
        };
        Self {
            name: intf_name.into(),
            bus_type: BusInterfaceType::Clock,
            mode,
            associated_clock: None,
            associated_reset: None,
            reset_polarity: None,
            addr_width: 0,
            data_width: 1,
            default_range: 0,
            port_maps: vec![PortMapEntry::new("CLK", port_name, 1, dir)],
        }
    }

    pub fn reset_port(
        intf_name: &str,
        port_name: &str,
        mode: InterfaceMode,
        polarity: ResetPolarity,
    ) -> Self {
        let dir = if mode == InterfaceMode::Master {
            PortDirection::Out
        } else {
            PortDirection::In
        };
        Self {
            name: intf_name.into(),
            bus_type: BusInterfaceType::Reset,
            mode,
            associated_clock: None,
            associated_reset: None,
            reset_polarity: Some(polarity),
            addr_width: 0,
            data_width: 1,
            default_range: 0,
            port_maps: vec![PortMapEntry::new("RST", port_name, 1, dir)],
        }
    }

    pub fn interrupt_port(intf_name: &str, port_name: &str, mode: InterfaceMode, width: u32) -> Self {
        let dir = if mode == InterfaceMode::Master {
            PortDirection::Out
        } else {
            PortDirection::In
        };
        Self {
            name: intf_name.into(),
            bus_type: BusInterfaceType::Interrupt,
            mode,
            associated_clock: None,
            associated_reset: None,
            reset_polarity: None,
            addr_width: 0,
            data_width: width,
            default_range: 0,
            port_maps: vec![PortMapEntry::new("INTERRUPT", port_name, width, dir)],
        }
    }
}

/// Register definition inside an IP-XACT addressBlock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterDef {
    pub name: String,
    pub offset: u64,
    pub width: u32,
    pub access: String,
    pub reset_value: u64,
    pub description: String,
}

impl RegisterDef {
    pub fn new(name: &str, offset: u64, access: &str, reset_value: u64, description: &str) -> Self {
        Self {
            name: name.into(),
            offset,
            width: 32,
            access: access.into(),
            reset_value,
            description: description.into(),
        }
    }
}

/// Contiguous register aperture inside a slave memoryMap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressBlock {
    pub name: String,
    pub base_address: u64,
    pub range: u64,
    pub width: u32,
    pub usage: String,
    pub registers: Vec<RegisterDef>,
}

/// Slave memory map exposed over a memory-mapped bus interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryMap {
    pub name: String,
    pub interface_name: String,
    pub address_blocks: Vec<AddressBlock>,
}

/// Typed default value for an IP configuration parameter (`CONFIG.*`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamValue {
    Int(i64),
    Hex(u64, u32),
    Str(String),
    Bool(bool),
}

impl ParamValue {
    pub fn to_sv_literal(&self) -> String {
        match self {
            Self::Int(v) => v.to_string(),
            Self::Hex(v, bits) => format!("{bits}'h{v:0width$X}", width = ((*bits as usize) + 3) / 4),
            Self::Str(s) => format!("\"{s}\""),
            Self::Bool(b) => if *b { "1".into() } else { "0".into() },
        }
    }

    pub fn as_i64(&self) -> i64 {
        match self {
            Self::Int(v) => *v,
            Self::Hex(v, _) => *v as i64,
            Self::Str(s) => parse_int_or_hex(s).unwrap_or(0),
            Self::Bool(b) => i64::from(*b),
        }
    }
}

pub fn parse_int_or_hex(s: &str) -> Option<i64> {
    let t = s.trim().replace('_', "");
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok().map(|v| v as i64)
    } else if let Some((_, hex)) = t.split_once("'h").or_else(|| t.split_once("'H")) {
        u64::from_str_radix(hex, 16).ok().map(|v| v as i64)
    } else if let Some((_, dec)) = t.split_once("'d").or_else(|| t.split_once("'D")) {
        dec.parse::<i64>().ok()
    } else {
        t.parse::<i64>().ok()
    }
}

/// Configurable parameter on a parametric IP component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpParameter {
    pub name: String,
    pub default_value: ParamValue,
    pub description: String,
}

impl IpParameter {
    pub fn int(name: &str, val: i64, description: &str) -> Self {
        Self {
            name: name.into(),
            default_value: ParamValue::Int(val),
            description: description.into(),
        }
    }

    pub fn hex(name: &str, val: u64, bits: u32, description: &str) -> Self {
        Self {
            name: name.into(),
            default_value: ParamValue::Hex(val, bits),
            description: description.into(),
        }
    }
}

/// Complete IEEE 1685-2014 parametric IP component descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpComponent {
    pub core: IpCore,
    pub display_name: String,
    pub description: String,
    pub parameters: Vec<IpParameter>,
    pub ports: Vec<IpPort>,
    pub interfaces: Vec<BusInterface>,
    pub memory_maps: Vec<MemoryMap>,
}

impl IpComponent {
    pub fn vlnv(&self) -> String {
        self.core.vlnv()
    }

    pub fn get_interface(&self, name: &str) -> Option<&BusInterface> {
        self.interfaces
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(name))
    }

    pub fn get_param_i64(&self, overrides: &BTreeMap<String, String>, name: &str) -> i64 {
        let stripped = name.strip_prefix("CONFIG.").unwrap_or(name);
        if let Some(v) = overrides
            .get(stripped)
            .or_else(|| overrides.get(&format!("CONFIG.{stripped}")))
        {
            if let Some(parsed) = parse_int_or_hex(v) {
                return parsed;
            }
        }
        self.parameters
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(stripped))
            .map(|p| p.default_value.as_i64())
            .unwrap_or(0)
    }

    /// Populate physical `ports` from all `interfaces` port maps if not already present.
    pub fn sync_ports_from_interfaces(&mut self) {
        for intf in &self.interfaces {
            for pm in &intf.port_maps {
                if !self.ports.iter().any(|p| p.name == pm.physical_name) {
                    let default_val = if pm.direction == PortDirection::In {
                        if intf.bus_type == BusInterfaceType::Reset
                            && intf.reset_polarity == Some(ResetPolarity::ActiveLow)
                        {
                            Some("1'b1".into())
                        } else {
                            Some("'0".into())
                        }
                    } else {
                        None
                    };
                    self.ports.push(IpPort {
                        name: pm.physical_name.clone(),
                        direction: pm.direction,
                        width: pm.width,
                        width_expr: None,
                        default_value: default_val,
                    });
                }
            }
        }
    }

    /// Serialize this component to rich IEEE 1685-2014 IP-XACT XML.
    pub fn to_ipxact_xml(&self) -> String {
        let mut s = String::new();
        s.push_str("<?xml version=\"1.0\"?>\n");
        s.push_str(
            "<ipxact:component xmlns:ipxact=\"http://www.accellera.org/XMLSchema/IPXACT/1685-2014\">\n",
        );
        s.push_str(&format!("  <ipxact:vendor>{}</ipxact:vendor>\n", self.core.vendor));
        s.push_str(&format!(
            "  <ipxact:library>{}</ipxact:library>\n",
            self.core.library
        ));
        s.push_str(&format!("  <ipxact:name>{}</ipxact:name>\n", self.core.name));
        s.push_str(&format!(
            "  <ipxact:version>{}</ipxact:version>\n",
            self.core.version
        ));
        s.push_str("  <ipxact:busInterfaces>\n");
        if self.interfaces.is_empty() {
            s.push_str("    <ipxact:busInterface>\n");
            s.push_str("      <ipxact:name>s_mm</ipxact:name>\n");
            s.push_str(&format!(
                "      <ipxact:description>{}</ipxact:description>\n",
                self.core.bus
            ));
            s.push_str("    </ipxact:busInterface>\n");
        } else {
            for intf in &self.interfaces {
                s.push_str("    <ipxact:busInterface>\n");
                s.push_str(&format!("      <ipxact:name>{}</ipxact:name>\n", intf.name));
                s.push_str(&format!(
                    "      <ipxact:busType>{}</ipxact:busType>\n",
                    intf.bus_type
                ));
                s.push_str(&format!("      <ipxact:mode>{}</ipxact:mode>\n", intf.mode));
                s.push_str(&format!(
                    "      <ipxact:description>{}</ipxact:description>\n",
                    self.core.bus
                ));
                s.push_str("    </ipxact:busInterface>\n");
            }
        }
        s.push_str("  </ipxact:busInterfaces>\n");
        if !self.memory_maps.is_empty() {
            s.push_str("  <ipxact:memoryMaps>\n");
            for mm in &self.memory_maps {
                s.push_str("    <ipxact:memoryMap>\n");
                s.push_str(&format!("      <ipxact:name>{}</ipxact:name>\n", mm.name));
                for ab in &mm.address_blocks {
                    s.push_str("      <ipxact:addressBlock>\n");
                    s.push_str(&format!("        <ipxact:name>{}</ipxact:name>\n", ab.name));
                    s.push_str(&format!(
                        "        <ipxact:baseAddress>0x{:X}</ipxact:baseAddress>\n",
                        ab.base_address
                    ));
                    s.push_str(&format!(
                        "        <ipxact:range>0x{:X}</ipxact:range>\n",
                        ab.range
                    ));
                    s.push_str(&format!(
                        "        <ipxact:width>{}</ipxact:width>\n",
                        ab.width
                    ));
                    s.push_str("      </ipxact:addressBlock>\n");
                }
                s.push_str("    </ipxact:memoryMap>\n");
            }
            s.push_str("  </ipxact:memoryMaps>\n");
        }
        if !self.parameters.is_empty() {
            s.push_str("  <ipxact:parameters>\n");
            for p in &self.parameters {
                s.push_str("    <ipxact:parameter>\n");
                s.push_str(&format!("      <ipxact:name>{}</ipxact:name>\n", p.name));
                s.push_str(&format!(
                    "      <ipxact:value>{}</ipxact:value>\n",
                    p.default_value.to_sv_literal()
                ));
                s.push_str("    </ipxact:parameter>\n");
            }
            s.push_str("  </ipxact:parameters>\n");
        }
        s.push_str("</ipxact:component>\n");
        s
    }
}
