//! Vivado UG994 IP Integrator Block Design (`BlockDesign`), Connection Automation,
//! Address Editor, BD DRC Engine, Synthesizable SystemVerilog & Crossbar Generator,
//! and Vivado TCL Command Bridge.

pub mod address_drc;
pub mod codegen;
pub mod graph;
pub mod tcl;

pub use address_drc::*;
pub use codegen::*;
pub use graph::*;
pub use helion_ipxact::{
    AddressBlock, BusInterface, BusInterfaceType, InterfaceMode, IpComponent, IpCore, IpParameter,
    IpPort, MemoryMap, ParamValue, PortDirection, PortMapEntry, RegisterDef, ResetPolarity,
    find_ip_component, parametric_catalog,
};
pub use tcl::*;

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct BlockDesign {
    pub name: String,
    pub cores: Vec<IpCore>,
}

#[derive(Clone, Debug)]
pub struct ValidateReport {
    pub ok: bool,
    pub errors: Vec<String>,
}

impl BlockDesign {
    pub fn new(name: &str) -> Self {
        create_bd_design(name)
    }

    pub fn create_cell(&mut self, vlnv_or_name: &str, instance_name: &str) -> Result<String, String> {
        create_bd_cell(self, vlnv_or_name, instance_name)
    }

    pub fn create_port(
        &mut self,
        name: &str,
        direction: PortDirection,
        width: u32,
        port_type: Option<BusInterfaceType>,
    ) -> Result<String, String> {
        create_bd_port(self, name, direction, width, port_type)
    }

    pub fn create_intf_port(
        &mut self,
        name: &str,
        bus_type: BusInterfaceType,
        mode: InterfaceMode,
    ) -> Result<String, String> {
        create_bd_intf_port(self, name, bus_type, mode)
    }

    pub fn set_cell_property(
        &mut self,
        instance_name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        set_bd_cell_property(self, instance_name, key, value)
    }

    pub fn connect_intf_net(
        &mut self,
        intf_net_name: Option<&str>,
        ep1: &str,
        ep2: &str,
    ) -> Result<String, String> {
        connect_bd_intf_net(self, intf_net_name, ep1, ep2)
    }

    pub fn connect_net(&mut self, net_name: Option<&str>, endpoints: &[&str]) -> Result<String, String> {
        connect_bd_net(self, net_name, endpoints)
    }

    pub fn apply_automation(&mut self, rule: BdAutomationRule) -> Result<String, String> {
        apply_bd_automation(self, rule)
    }

    pub fn assign_address(&mut self) -> Result<Vec<BdAddressSegment>, String> {
        assign_bd_address(self)
    }

    pub fn assign_address_explicit(
        &mut self,
        slave_endpoint: &str,
        base_address: u64,
        range: u64,
    ) -> Result<BdAddressSegment, String> {
        assign_bd_address_explicit(self, slave_endpoint, base_address, range)
    }

    pub fn validate_design(&self) -> BdDrcReport {
        validate_bd_design(self)
    }

    pub fn generate_top_sv(&self) -> Result<String, String> {
        generate_top_sv(self)
    }

    pub fn make_wrapper(&self) -> Result<String, String> {
        make_wrapper(self)
    }

    pub fn save_design(&self, path: &Path) -> Result<PathBuf, String> {
        save_bd_design(self, path)
    }

    pub fn exec_tcl(&mut self, script: &str) -> Result<String, String> {
        exec_bd_tcl(self, script)
    }

    pub fn graph(&self) -> BdGraph {
        get_or_build_bd_graph(self)
    }
}

pub fn validate(bd: &BlockDesign) -> ValidateReport {
    if get_explicit_bd_graph(bd).is_some() {
        let drc = validate_bd_design(bd);
        return ValidateReport {
            ok: drc.ok,
            errors: drc.errors,
        };
    }
    let mut errors = Vec::new();
    if bd.name.is_empty() {
        errors.push("empty name".into());
    }
    for c in &bd.cores {
        if c.bus != "Helion-MM" && c.bus != "Helion-ST" {
            errors.push(format!("{}: bus {} not Helion-MM/ST", c.name, c.bus));
        }
    }
    ValidateReport {
        ok: errors.is_empty(),
        errors,
    }
}

pub fn emit_sv(bd: &BlockDesign) -> Result<String, String> {
    if get_explicit_bd_graph(bd).is_some() {
        return generate_top_sv(bd);
    }
    let v = validate(bd);
    if !v.ok {
        return Err(v.errors.join("; "));
    }
    let mut s = format!(
        "module {} (\n  input logic clk,\n  input logic resetn\n);\n",
        bd.name
    );
    for c in &bd.cores {
        s.push_str(&format!(
            "  {} #() u_{} (.clk(clk), .resetn(resetn));\n",
            c.name, c.name
        ));
    }
    s.push_str("endmodule\n");
    Ok(s)
}
#[cfg(test)]
mod tests {
    use super::*;
    use helion_ipxact::{pack_gpio, pack_uart};

    #[test]
    fn validate_and_emit() {
        let bd = BlockDesign {
            name: "sys".into(),
            cores: vec![pack_uart(), pack_gpio()],
        };
        assert!(validate(&bd).ok);
        let sv = emit_sv(&bd).unwrap();
        assert!(sv.contains("module sys"));
        assert!(sv.contains("h_uart"));
        assert!(sv.contains("h_gpio"));
    }

    #[test]
    fn multi_slave_soc_automation_address_assign_drc_and_sv() {
        let mut bd = create_bd_design("soc_system");
        create_bd_cell(&mut bd, "axi4_master_cpu", "cpu_0").unwrap();
        create_bd_cell(&mut bd, "proc_sys_reset", "rst_0").unwrap();
        create_bd_cell(&mut bd, "axi_gpio", "gpio_0").unwrap();
        create_bd_cell(&mut bd, "axi_uartlite", "uart_0").unwrap();
        create_bd_cell(&mut bd, "axi_timer", "timer_0").unwrap();
        create_bd_cell(&mut bd, "axi_bram_ctrl", "bram_ctrl_0").unwrap();

        // Configure GPIO to 16 bits
        set_bd_cell_property(&mut bd, "gpio_0", "CONFIG.C_GPIO_WIDTH", "16").unwrap();

        // Connect slaves using UG994 connection automation (inserts axi_interconnect_0 AND axi_protocol_converter for AXI4 cpu_0!)
        for slave in ["gpio_0", "uart_0", "timer_0", "bram_ctrl_0"] {
            apply_bd_automation(
                &mut bd,
                BdAutomationRule::Axi4 {
                    target_cell: slave.into(),
                    target_intf: "S_AXI".into(),
                    master: Some("/cpu_0/M_AXI".into()),
                    clk_source: Some("aclk".into()),
                    rst_source: None,
                },
            )
            .unwrap();
        }

        // Export GPIO and UART to external board ports, and attach BRAM generator to bram_ctrl_0
        apply_bd_automation(
            &mut bd,
            BdAutomationRule::Board {
                target_cell: "gpio_0".into(),
                target_intf: "GPIO".into(),
                external_port_name: Some("gpio_led".into()),
            },
        )
        .unwrap();
        apply_bd_automation(
            &mut bd,
            BdAutomationRule::Board {
                target_cell: "uart_0".into(),
                target_intf: "UART".into(),
                external_port_name: Some("usb_uart".into()),
            },
        )
        .unwrap();
        apply_bd_automation(
            &mut bd,
            BdAutomationRule::BramCntlr {
                target_cell: "bram_ctrl_0".into(),
                target_intf: "BRAM_PORTA".into(),
            },
        )
        .unwrap();

        // Before address assignment, DRC must flag unmapped slaves (BD-DRC-003)
        let pre_drc = validate_bd_design(&bd);
        assert!(!pre_drc.ok);
        assert!(
            pre_drc.diagnostics.iter().any(|d| d.code == "BD-DRC-003"),
            "{:?}",
            pre_drc.diagnostics
        );

        // Automatically assign non-overlapping power-of-two aligned addresses
        let segs = assign_bd_address(&mut bd).unwrap();
        assert_eq!(segs.len(), 4);
        let bram_seg = segs.iter().find(|s| s.slave_cell == "bram_ctrl_0").unwrap();
        assert_eq!(bram_seg.base_address, 0xC000_0000);
        assert_eq!(bram_seg.range, 0x2000);

        // Now DRC must pass with 0 errors
        let post_drc = validate_bd_design(&bd);
        assert!(post_drc.ok, "DRC errors: {:?}", post_drc.errors);

        // Generate top-level SystemVerilog + wrapper
        let wrapper_sv = make_wrapper(&bd).unwrap();
        assert!(wrapper_sv.contains("module soc_system_axi_interconnect_0"));
        assert!(wrapper_sv.contains("module axi_protocol_converter"));
        assert!(wrapper_sv.contains("module axi_gpio"));
        assert!(wrapper_sv.contains("module axi_uartlite"));
        assert!(wrapper_sv.contains("module axi_timer"));
        assert!(wrapper_sv.contains("module axi_bram_ctrl"));
        assert!(wrapper_sv.contains("module blk_mem_gen"));
        assert!(wrapper_sv.contains("module proc_sys_reset"));
        assert!(wrapper_sv.contains("module soc_system ("));
        assert!(wrapper_sv.contains("module soc_system_wrapper ("));
        assert!(wrapper_sv.contains(".C_GPIO_WIDTH(16)"));
    }

    #[test]
    fn drc_detects_overlap_aperture_alignment_and_clock_domain_mismatch() {
        let mut bd = create_bd_design("drc_test");
        create_bd_cell(&mut bd, "axi_cpu", "cpu_0").unwrap();
        create_bd_cell(&mut bd, "axi_gpio", "gpio_0").unwrap();
        create_bd_cell(&mut bd, "axi_uartlite", "uart_0").unwrap();

        apply_bd_automation(
            &mut bd,
            BdAutomationRule::Axi4 {
                target_cell: "gpio_0".into(),
                target_intf: "S_AXI".into(),
                master: Some("cpu_0/M_AXI".into()),
                clk_source: Some("aclk".into()),
                rst_source: Some("aresetn".into()),
            },
        )
        .unwrap();
        apply_bd_automation(
            &mut bd,
            BdAutomationRule::Axi4 {
                target_cell: "uart_0".into(),
                target_intf: "S_AXI".into(),
                master: Some("cpu_0/M_AXI".into()),
                clk_source: Some("aclk".into()),
                rst_source: Some("aresetn".into()),
            },
        )
        .unwrap();

        // 1. Overlapping address assignment -> BD-DRC-002
        assign_bd_address_explicit(&mut bd, "gpio_0/S_AXI", 0x4000_0000, 0x1000).unwrap();
        assign_bd_address_explicit(&mut bd, "uart_0/S_AXI", 0x4000_0800, 0x1000).unwrap();
        let rep_overlap = validate_bd_design(&bd);
        assert!(!rep_overlap.ok);
        assert!(rep_overlap.diagnostics.iter().any(|d| d.code == "BD-DRC-002"));

        // 2. Non-power-of-two range or smaller than slave aperture -> BD-DRC-004
        assign_bd_address_explicit(&mut bd, "uart_0/S_AXI", 0x4001_0000, 0x1000).unwrap();
        assign_bd_address_explicit(&mut bd, "gpio_0/S_AXI", 0x4000_0000, 0x100).unwrap(); // gpio needs 9 bits = 0x200
        let rep_aperture = validate_bd_design(&bd);
        assert!(!rep_aperture.ok);
        assert!(rep_aperture.diagnostics.iter().any(|d| d.code == "BD-DRC-004"));

        // Fix gpio_0 aperture
        assign_bd_address_explicit(&mut bd, "gpio_0/S_AXI", 0x4000_0000, 0x1000).unwrap();
        assert!(validate_bd_design(&bd).ok);

        // 3. Put uart_0 on a different clock domain without a CDC bridge -> BD-DRC-005
        create_bd_port(&mut bd, "clk_slow", PortDirection::In, 1, Some(BusInterfaceType::Clock)).unwrap();
        connect_bd_net(&mut bd, Some("slow_clk_net"), &["clk_slow", "uart_0/s_axi_aclk"]).unwrap();
        let rep_cdc = validate_bd_design(&bd);
        assert!(!rep_cdc.ok);
        assert!(rep_cdc.diagnostics.iter().any(|d| d.code == "BD-DRC-005"));
    }

    #[test]
    fn exec_bd_tcl_end_to_end_script() {
        let mut bd = create_bd_design("placeholder");
        let script = r#"
            create_bd_design tcl_soc
            create_bd_cell -type ip -vlnv xilinx.com:ip:axi_cpu:1.0 cpu_0
            create_bd_cell -type ip -vlnv xilinx.com:ip:axi_gpio:2.0 gpio_0
            create_bd_cell -type ip -vlnv xilinx.com:ip:axi_uartlite:2.0 uart_0
            create_bd_cell -type ip -vlnv xilinx.com:ip:axi_timer:2.0 timer_0
            create_bd_cell -type ip -vlnv xilinx.com:ip:cdc_fifo:2.0 axis_fifo_0
            set_property -dict [list CONFIG.C_GPIO_WIDTH 8 CONFIG.C_BAUDRATE 115200] [get_bd_cells gpio_0]
            apply_bd_automation -rule xilinx.com:bd_rule:axi4 -config { Master "/cpu_0/M_AXI" Clk "aclk" } [get_bd_intf_pins gpio_0/S_AXI]
            apply_bd_automation -rule xilinx.com:bd_rule:axi4 -config { Master "/cpu_0/M_AXI" Clk "aclk" } [get_bd_intf_pins uart_0/S_AXI]
            apply_bd_automation -rule xilinx.com:bd_rule:axi4 -config { Master "/cpu_0/M_AXI" Clk "aclk" } [get_bd_intf_pins timer_0/S_AXI]
            assign_bd_address
            validate_bd_design
            make_wrapper -files [get_files tcl_soc.bd] -top
        "#;
        let sv = exec_bd_tcl(&mut bd, script).unwrap();
        assert_eq!(bd.name, "tcl_soc");
        assert!(sv.contains("module tcl_soc_wrapper"));
        assert!(sv.contains("module tcl_soc_axi_interconnect_0"));
        assert!(sv.contains(".C_GPIO_WIDTH(8)"));

        let tmp_dir = std::env::temp_dir().join("helion_bd_tcl_test");
        let saved = save_bd_design(&bd, &tmp_dir).unwrap();
        assert!(saved.exists());
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn multi_master_crossbar_and_dual_clock_cdc_fifo() {
        let mut bd = BlockDesign::new("multi_master_soc");
        bd.create_cell("axi_cpu", "cpu_a").unwrap();
        bd.create_cell("axi4_master_cpu", "cpu_b").unwrap();
        bd.create_cell("axi_quad_spi", "spi_0").unwrap();
        bd.create_cell("axi_iic", "iic_0").unwrap();
        bd.create_cell("axi_intc", "intc_0").unwrap();
        bd.create_cell("cdc_fifo", "fifo_a").unwrap();
        bd.create_cell("cdc_fifo", "fifo_b").unwrap();
        bd.create_cell("apb_gpio", "apb_0").unwrap();
        bd.create_cell("wb_gpio", "wb_0").unwrap();

        for slave in ["spi_0", "iic_0", "intc_0"] {
            bd.apply_automation(BdAutomationRule::Axi4 {
                target_cell: slave.into(),
                target_intf: "S_AXI".into(),
                master: None, // connects both cpu_a and cpu_b!
                clk_source: Some("aclk".into()),
                rst_source: Some("aresetn".into()),
            })
            .unwrap();
        }

        // Dual-clock AXI4-Stream CDC pipeline: fifo_a (aclk -> clk_stream) -> fifo_b (clk_stream -> aclk)
        bd.create_port("clk_stream", PortDirection::In, 1, Some(BusInterfaceType::Clock))
            .unwrap();
        bd.connect_net(Some("stream_clk_net"), &["clk_stream", "fifo_a/m_axis_aclk", "fifo_b/s_axis_aclk"])
            .unwrap();
        bd.connect_intf_net(Some("axis_pipe"), "fifo_a/M_AXIS", "fifo_b/S_AXIS")
            .unwrap();

        // Export SPI and I2C board interfaces
        bd.apply_automation(BdAutomationRule::Board {
            target_cell: "spi_0".into(),
            target_intf: "SPI_0".into(),
            external_port_name: Some("qspi_flash".into()),
        })
        .unwrap();
        bd.apply_automation(BdAutomationRule::Board {
            target_cell: "iic_0".into(),
            target_intf: "IIC".into(),
            external_port_name: Some("i2c_bus".into()),
        })
        .unwrap();

        // Wire interrupt controller output to cpu_a/irq
        bd.connect_intf_net(Some("cpu_irq_net"), "intc_0/IRQ", "cpu_a/IRQ")
            .unwrap();

        let segs = bd.assign_address().unwrap();
        // 2 masters x 3 slaves = 6 address segments
        assert_eq!(segs.len(), 6);

        let drc = bd.validate_design();
        assert!(drc.ok, "DRC failed: {:?}", drc.errors);

        let sv = bd.generate_top_sv().unwrap();
        assert!(sv.contains("parameter int C_NUM_MASTERS = 2"));
        assert!(sv.contains("parameter int C_NUM_SLAVES  = 3"));
        assert!(sv.contains("s00_axi_awaddr"));
        assert!(sv.contains("s01_axi_awaddr"));
        assert!(sv.contains("m02_axi_awaddr"));
        assert!(sv.contains("module axi_quad_spi"));
        assert!(sv.contains("module axi_iic"));
        assert!(sv.contains("module axi_intc"));
        assert!(sv.contains("module cdc_fifo"));
    }
}
