//! Headless block design: validate + emit SV + Vivado-class `apply_bd_automation` RISC-V SoC subsystem assembly.

pub use helion_ipxact::{
    CacheConfig, ClintConfig, IpCore, MasterBusKind, PlicConfig, RiscvCpuConfig,
    RiscvDebugModuleConfig, RiscvIsa, emit_cache_sv, emit_clint_sv, emit_plic_sv,
    emit_riscv_cpu_sv, emit_riscv_dm_sv, emit_riscv_subsystem_sv, pack_clint, pack_dcache,
    pack_icache, pack_plic, pack_riscv_dm, pack_rv32, pack_rv32imac, pack_rv64gc,
};

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

pub fn validate(bd: &BlockDesign) -> ValidateReport {
    let mut errors = Vec::new();
    if bd.name.is_empty() {
        errors.push("empty name".into());
    }
    for c in &bd.cores {
        if c.bus != "Helion-MM" && c.bus != "Helion-ST" && c.bus != "Helion-DBG" {
            errors.push(format!("{}: bus {} not Helion-MM/ST/DBG", c.name, c.bus));
        }
    }
    ValidateReport {
        ok: errors.is_empty(),
        errors,
    }
}

pub fn emit_sv(bd: &BlockDesign) -> Result<String, String> {
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

/// Automated connection synthesized by [`apply_bd_automation`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdConnection {
    pub net: String,
    pub src: String,
    pub dst: String,
}

/// Memory-map aperture assigned to a slave core by [`apply_bd_automation`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdAddressSegment {
    pub instance: String,
    pub core_name: String,
    pub base_addr: u64,
    pub high_addr: u64,
    pub size_bytes: u64,
}

/// Interrupt line routed from a peripheral instance to a `h_plic` source slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdIrqBinding {
    pub instance: String,
    pub core_name: String,
    pub plic_source_id: u32,
}

/// Options for one-click Vivado-class `apply_bd_automation` SoC subsystem assembly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdAutomationOpts {
    pub isa: RiscvIsa,
    pub master_bus: MasterBusKind,
    pub reset_vector: u64,
    pub icache: CacheConfig,
    pub dcache: CacheConfig,
    pub include_clint: bool,
    pub include_plic: bool,
    pub include_jtag_dm: bool,
    pub auto_connect_irqs: bool,
    pub auto_assign_addresses: bool,
}

impl Default for BdAutomationOpts {
    fn default() -> Self {
        Self {
            isa: RiscvIsa::Rv32imac,
            master_bus: MasterBusKind::Axi4,
            reset_vector: 0x0000_0000,
            icache: CacheConfig::default_icache(),
            dcache: CacheConfig::default_dcache(),
            include_clint: true,
            include_plic: true,
            include_jtag_dm: true,
            auto_connect_irqs: true,
            auto_assign_addresses: true,
        }
    }
}

impl BdAutomationOpts {
    pub fn rv32imac_axi4() -> Self {
        Self::default()
    }

    pub fn rv64gc_axi4() -> Self {
        Self {
            isa: RiscvIsa::Rv64gc,
            master_bus: MasterBusKind::Axi4,
            reset_vector: 0x8000_0000,
            icache: CacheConfig {
                enabled: true,
                size_bytes: 8192,
                line_bytes: 64,
                ways: 4,
                write_back: false,
            },
            dcache: CacheConfig {
                enabled: true,
                size_bytes: 8192,
                line_bytes: 64,
                ways: 4,
                write_back: true,
            },
            ..Self::default()
        }
    }
}

/// Result of [`apply_bd_automation`] containing the synthesized CPU configuration,
/// automated net connections, assigned address segments, PLIC IRQ bindings, and
/// complete synthesizable SystemVerilog RTL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdAutomationReport {
    pub cpu_config: RiscvCpuConfig,
    pub connections: Vec<BdConnection>,
    pub address_map: Vec<BdAddressSegment>,
    pub irq_bindings: Vec<BdIrqBinding>,
    pub subsystem_sv: String,
}

/// Vivado-class `apply_bd_automation`: one-click RISC-V SoC subsystem assembly.
///
/// Automatically:
/// 1. Inserts the configured `h_rv32imac` / `h_rv64gc` processor core, `h_icache`, `h_dcache`,
///    `h_clint`, `h_plic`, and `h_riscv_dm` JTAG Debug Module into `bd.cores` if not already present.
/// 2. Wires system `clk`, `resetn`, `ndmreset`, `jtag_tck/tms/tdi/tdo`, and master interconnect
///    (`AXI4` / `AXI4-Lite` / `Helion-MM`) across all slave peripherals.
/// 3. Assigns deterministic non-overlapping address apertures to Debug Module (`0x0000_0100`),
///    CLINT (`0x0200_0000`), PLIC (`0x0C00_0000`), and peripheral slaves (`0x4000_0000 + i*0x1000`).
/// 4. Connects peripheral interrupt lines into `h_plic` source slots (`1..=N`) and emits
///    complete synthesizable SystemVerilog for the entire SoC subsystem.
pub fn apply_bd_automation(
    bd: &mut BlockDesign,
    opts: &BdAutomationOpts,
) -> Result<BdAutomationReport, String> {
    if bd.name.is_empty() {
        return Err("apply_bd_automation: empty BlockDesign name".into());
    }

    let ensure_core = |cores: &mut Vec<IpCore>, ip: IpCore| {
        if !cores.iter().any(|c| c.name == ip.name) {
            cores.push(ip);
        }
    };

    match opts.isa {
        RiscvIsa::Rv32imac => ensure_core(&mut bd.cores, pack_rv32imac()),
        RiscvIsa::Rv64gc => ensure_core(&mut bd.cores, pack_rv64gc()),
    }
    if opts.icache.enabled {
        ensure_core(&mut bd.cores, pack_icache());
    }
    if opts.dcache.enabled {
        ensure_core(&mut bd.cores, pack_dcache());
    }
    if opts.include_clint {
        ensure_core(&mut bd.cores, pack_clint());
    }
    if opts.include_plic {
        ensure_core(&mut bd.cores, pack_plic());
    }
    if opts.include_jtag_dm {
        ensure_core(&mut bd.cores, pack_riscv_dm());
    }

    let v = validate(bd);
    if !v.ok {
        return Err(v.errors.join("; "));
    }

    let clint_cfg = ClintConfig::default();
    let plic_cfg = PlicConfig::default();
    let dm_cfg = RiscvDebugModuleConfig::default();
    let cpu_config = RiscvCpuConfig {
        name: format!("{}_riscv_subsys", bd.name),
        isa: opts.isa,
        reset_vector: opts.reset_vector,
        icache: opts.icache,
        dcache: opts.dcache,
        master_bus: opts.master_bus,
        clint: clint_cfg,
        plic: plic_cfg,
        debug_module: dm_cfg,
    };

    let mut connections = Vec::new();
    let mut address_map = Vec::new();
    let mut irq_bindings = Vec::new();

    let cpu_inst = format!("u_{}", opts.isa.core_name());
    if opts.include_jtag_dm {
        connections.push(BdConnection {
            net: "debug_req".into(),
            src: "u_h_riscv_dm.debug_req_o".into(),
            dst: format!("{cpu_inst}.debug_req_i"),
        });
        connections.push(BdConnection {
            net: "ndmreset".into(),
            src: "u_h_riscv_dm.ndmreset_o".into(),
            dst: "subsys_rst_ctrl.ndmreset_i".into(),
        });
        if opts.auto_assign_addresses {
            address_map.push(BdAddressSegment {
                instance: "u_h_riscv_dm".into(),
                core_name: "h_riscv_dm".into(),
                base_addr: dm_cfg.base_addr,
                high_addr: dm_cfg.base_addr + dm_cfg.size_bytes - 1,
                size_bytes: dm_cfg.size_bytes,
            });
        }
    }
    if opts.include_clint {
        connections.push(BdConnection {
            net: "clint_timer_irq".into(),
            src: "u_h_clint.timer_irq_o".into(),
            dst: format!("{cpu_inst}.timer_irq_i"),
        });
        connections.push(BdConnection {
            net: "clint_sw_irq".into(),
            src: "u_h_clint.sw_irq_o".into(),
            dst: format!("{cpu_inst}.sw_irq_i"),
        });
        if opts.auto_assign_addresses {
            address_map.push(BdAddressSegment {
                instance: "u_h_clint".into(),
                core_name: "h_clint".into(),
                base_addr: clint_cfg.base_addr,
                high_addr: clint_cfg.base_addr + clint_cfg.size_bytes - 1,
                size_bytes: clint_cfg.size_bytes,
            });
        }
    }
    if opts.include_plic {
        connections.push(BdConnection {
            net: "plic_meip".into(),
            src: "u_h_plic.meip_o".into(),
            dst: format!("{cpu_inst}.meip_i"),
        });
        connections.push(BdConnection {
            net: "plic_seip".into(),
            src: "u_h_plic.seip_o".into(),
            dst: format!("{cpu_inst}.seip_i"),
        });
        if opts.auto_assign_addresses {
            address_map.push(BdAddressSegment {
                instance: "u_h_plic".into(),
                core_name: "h_plic".into(),
                base_addr: plic_cfg.base_addr,
                high_addr: plic_cfg.base_addr + plic_cfg.size_bytes - 1,
                size_bytes: plic_cfg.size_bytes,
            });
        }
    }

    let mut next_periph_base = 0x4000_0000u64;
    let periph_stride = 0x1000u64;
    let mut next_plic_src = 1u32;

    for (idx, c) in bd.cores.iter().enumerate() {
        if matches!(
            c.name.as_str(),
            "h_rv32imac"
                | "h_rv64gc"
                | "h_rv32_hb1"
                | "h_icache"
                | "h_dcache"
                | "h_clint"
                | "h_plic"
                | "h_riscv_dm"
        ) {
            continue;
        }
        let inst = format!("u_{}_{idx}", c.name);
        connections.push(BdConnection {
            net: "clk".into(),
            src: "clk".into(),
            dst: format!("{inst}.clk"),
        });
        connections.push(BdConnection {
            net: "subsys_rst_n".into(),
            src: "subsys_rst_n".into(),
            dst: format!("{inst}.resetn"),
        });
        if c.bus == "Helion-MM" {
            connections.push(BdConnection {
                net: format!("bus_{}_{idx}", opts.master_bus.as_str().to_ascii_lowercase().replace('-', "_")),
                src: format!("{cpu_inst}.master"),
                dst: format!("{inst}.s_mm"),
            });
            if opts.auto_assign_addresses {
                address_map.push(BdAddressSegment {
                    instance: inst.clone(),
                    core_name: c.name.clone(),
                    base_addr: next_periph_base,
                    high_addr: next_periph_base + periph_stride - 1,
                    size_bytes: periph_stride,
                });
                next_periph_base += periph_stride;
            }
            if opts.auto_connect_irqs && opts.include_plic && next_plic_src < plic_cfg.num_sources {
                irq_bindings.push(BdIrqBinding {
                    instance: inst.clone(),
                    core_name: c.name.clone(),
                    plic_source_id: next_plic_src,
                });
                connections.push(BdConnection {
                    net: format!("irq_{inst}"),
                    src: format!("{inst}.irq"),
                    dst: format!("u_h_plic.irq_sources_i[{next_plic_src}]"),
                });
                next_plic_src += 1;
            }
        }
    }

    let mut subsystem_sv = emit_riscv_subsystem_sv(&cpu_config);
    subsystem_sv.push('\n');
    subsystem_sv.push_str(&emit_sv(bd)?);

    Ok(BdAutomationReport {
        cpu_config,
        connections,
        address_map,
        irq_bindings,
        subsystem_sv,
    })
}

impl BlockDesign {
    pub fn apply_bd_automation(
        &mut self,
        opts: &BdAutomationOpts,
    ) -> Result<BdAutomationReport, String> {
        apply_bd_automation(self, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ipxact::{pack_gpio, pack_spi_mm, pack_timer, pack_uart, pack_watchdog};

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
    fn apply_bd_automation_assembles_rv32imac_and_rv64gc_soc_subsystems() {
        let mut bd = BlockDesign {
            name: "helion_mbv_soc".into(),
            cores: vec![
                pack_uart(),
                pack_gpio(),
                pack_timer(),
                pack_spi_mm(),
                pack_watchdog(),
            ],
        };
        let opts = BdAutomationOpts::rv32imac_axi4();
        let rep = bd.apply_bd_automation(&opts).unwrap();
        assert!(validate(&bd).ok);
        assert!(bd.cores.iter().any(|c| c.name == "h_rv32imac"));
        assert!(bd.cores.iter().any(|c| c.name == "h_icache"));
        assert!(bd.cores.iter().any(|c| c.name == "h_dcache"));
        assert!(bd.cores.iter().any(|c| c.name == "h_clint"));
        assert!(bd.cores.iter().any(|c| c.name == "h_plic"));
        assert!(bd.cores.iter().any(|c| c.name == "h_riscv_dm"));

        // Address map must contain DM (0x100), CLINT (0x0200_0000), PLIC (0x0C00_0000), and 5 slaves @ 0x4000_0000+.
        assert_eq!(rep.address_map.len(), 8);
        assert!(
            rep.address_map
                .iter()
                .any(|s| s.core_name == "h_clint" && s.base_addr == 0x0200_0000)
        );
        assert!(
            rep.address_map
                .iter()
                .any(|s| s.core_name == "h_plic" && s.base_addr == 0x0C00_0000)
        );
        assert_eq!(rep.irq_bindings.len(), 5);
        assert_eq!(rep.irq_bindings[0].plic_source_id, 1);
        assert_eq!(rep.irq_bindings[4].plic_source_id, 5);
        assert!(rep.subsystem_sv.contains("module h_rv32imac"));
        assert!(rep.subsystem_sv.contains("module h_clint"));
        assert!(rep.subsystem_sv.contains("module h_plic"));
        assert!(rep.subsystem_sv.contains("module h_riscv_dm"));
        assert!(rep.subsystem_sv.contains("module helion_mbv_soc"));

        // RV64GC automation test
        let mut bd64 = BlockDesign {
            name: "helion_ps64_soc".into(),
            cores: vec![pack_uart(), pack_gpio()],
        };
        let rep64 = bd64
            .apply_bd_automation(&BdAutomationOpts::rv64gc_axi4())
            .unwrap();
        assert_eq!(rep64.cpu_config.isa, RiscvIsa::Rv64gc);
        assert!(rep64.subsystem_sv.contains("module h_rv64gc"));
        assert!(rep64.subsystem_sv.contains("module helion_ps64_soc"));
    }
}
