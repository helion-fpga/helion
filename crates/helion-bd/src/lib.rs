//! Vivado UG994 IP Integrator Block Design (`BlockDesign`), Connection Automation,
//! Address Editor, BD DRC Engine, Synthesizable SystemVerilog & Crossbar Generator,
//! and Vivado TCL Command Bridge.
//! Headless block design: validate + emit SV + Vitis-class SoC BSP (`xparameters.h`, Rust PAC, `lscript.ld`, `.mmi`).
//! Headless block design: validate + emit SV + Vivado-class `apply_bd_automation` RISC-V SoC subsystem assembly.

pub mod address_drc;
pub mod codegen;
pub mod graph;
pub mod tcl;

pub use address_drc::*;
pub use codegen::*;
pub use graph::*;
pub use helion_ipxact::{
    AddressBlock, BusInterface, BusInterfaceType, InterfaceMode, IpComponent, IpCore, IpParameter,
    IpPort, MemoryMap, ParamValue, PortDirection, PortMapEntry, RegisterDef as IpxactRegisterDef,
    ResetPolarity, find_ip_component, parametric_catalog,
};
pub use tcl::*;

use std::path::{Path, PathBuf};
pub use helion_bits::{
    FirmwareImage, FirmwareSegment, MemoryMapInfo, MmiAddressSpace, MmiBitLane,
    build_minimal_elf32, build_minimal_elf64, load_elf_firmware, load_mem_firmware,
    readback_bram_word, updatemem,
};
pub use helion_ipxact::{
    CacheConfig, ClintConfig, MasterBusKind, PlicConfig, RiscvCpuConfig,
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
        graph::apply_bd_automation(self, rule)
    }

    pub fn assign_address(&mut self) -> Result<Vec<graph::BdAddressSegment>, String> {
        assign_bd_address(self)
    }

    pub fn assign_address_explicit(
        &mut self,
        slave_endpoint: &str,
        base_address: u64,
        range: u64,
    ) -> Result<graph::BdAddressSegment, String> {
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

/// Canonical MMIO register descriptor for a Helion-MM peripheral core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterDef {
    pub name: &'static str,
    pub offset: u64,
    pub access: &'static str,
    pub description: &'static str,
}

/// Allocated MMIO aperture for a peripheral instance in a [`BlockDesign`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeripheralRegion {
    pub instance_name: String,
    pub core_name: String,
    pub device_id: u32,
    pub base_addr: u64,
    pub high_addr: u64,
    pub size_bytes: u64,
    pub irq: Option<u32>,
    pub registers: Vec<RegisterDef>,
}

/// Deterministic SoC address map derived from a [`BlockDesign`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocMemoryMap {
    pub design_name: String,
    pub cpu_core: Option<String>,
    pub clk_hz: u64,
    pub imem_base: u64,
    pub imem_size: u64,
    pub dmem_base: u64,
    pub dmem_size: u64,
    pub peripherals: Vec<PeripheralRegion>,
}

/// Complete Vitis-class Board Support Package (BSP) emitted from a [`BlockDesign`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BspBundle {
    pub memory_map: SocMemoryMap,
    pub xparameters_h: String,
    pub rust_pac: String,
    pub lscript_ld: String,
    pub mmi_xml: String,
    pub mmi_json: String,
}

fn canonical_registers(core_name: &str) -> Vec<RegisterDef> {
    match core_name {
        "h_uart" => vec![
            RegisterDef {
                name: "RXDATA",
                offset: 0x00,
                access: "RO",
                description: "UART receive FIFO data byte",
            },
            RegisterDef {
                name: "TXDATA",
                offset: 0x04,
                access: "WO",
                description: "UART transmit FIFO data byte",
            },
            RegisterDef {
                name: "STATUS",
                offset: 0x08,
                access: "RO",
                description: "UART status (bit0 RX_VALID, bit3 TX_FULL)",
            },
            RegisterDef {
                name: "CTRL",
                offset: 0x0C,
                access: "RW",
                description: "UART control & baud divider",
            },
        ],
        "h_gpio" => vec![
            RegisterDef {
                name: "DATA",
                offset: 0x00,
                access: "RW",
                description: "GPIO pad data register",
            },
            RegisterDef {
                name: "TRI",
                offset: 0x04,
                access: "RW",
                description: "GPIO direction / tri-state enable (0=out, 1=in)",
            },
            RegisterDef {
                name: "IER",
                offset: 0x08,
                access: "RW",
                description: "GPIO interrupt enable register",
            },
            RegisterDef {
                name: "ISR",
                offset: 0x0C,
                access: "RW",
                description: "GPIO interrupt status register",
            },
        ],
        "h_timer" => vec![
            RegisterDef {
                name: "TCSR",
                offset: 0x00,
                access: "RW",
                description: "Timer control and status register",
            },
            RegisterDef {
                name: "TLR",
                offset: 0x04,
                access: "RW",
                description: "Timer load / reload value",
            },
            RegisterDef {
                name: "TCR",
                offset: 0x08,
                access: "RO",
                description: "Timer current counter value",
            },
        ],
        "h_pwm" => vec![
            RegisterDef {
                name: "CTRL",
                offset: 0x00,
                access: "RW",
                description: "PWM enable and polarity control",
            },
            RegisterDef {
                name: "PERIOD",
                offset: 0x04,
                access: "RW",
                description: "PWM period in clock cycles",
            },
            RegisterDef {
                name: "DUTY",
                offset: 0x08,
                access: "RW",
                description: "PWM high-time compare duty cycle",
            },
        ],
        "h_spi_mm" => vec![
            RegisterDef {
                name: "CR",
                offset: 0x00,
                access: "RW",
                description: "SPI control register (CPOL/CPHA/enable)",
            },
            RegisterDef {
                name: "SR",
                offset: 0x04,
                access: "RO",
                description: "SPI status register (BUSY/RX_READY)",
            },
            RegisterDef {
                name: "DTR",
                offset: 0x08,
                access: "WO",
                description: "SPI data transmit register",
            },
            RegisterDef {
                name: "DRR",
                offset: 0x0C,
                access: "RO",
                description: "SPI data receive register",
            },
            RegisterDef {
                name: "SSR",
                offset: 0x10,
                access: "RW",
                description: "SPI chip-select assertion mask",
            },
        ],
        "h_watchdog" => vec![
            RegisterDef {
                name: "TWCSR0",
                offset: 0x00,
                access: "RW",
                description: "Watchdog control/status 0",
            },
            RegisterDef {
                name: "LOAD",
                offset: 0x04,
                access: "WO",
                description: "Watchdog reload / kick register",
            },
            RegisterDef {
                name: "COUNT",
                offset: 0x08,
                access: "RO",
                description: "Watchdog countdown value",
            },
        ],
        _ => vec![
            RegisterDef {
                name: "CTRL",
                offset: 0x00,
                access: "RW",
                description: "Control and status word 0",
            },
            RegisterDef {
                name: "DATA",
                offset: 0x04,
                access: "RW",
                description: "Data word 1",
            },
        ],
    }
}

/// Compute the deterministic SoC address map for a [`BlockDesign`].
///
/// Allocates:
/// - `IMEM` (instruction BRAM): `0x0000_0000 .. 0x0000_3FFF` (16 KiB)
/// - `DMEM` (data BRAM): `0x1000_0000 .. 0x1000_3FFF` (16 KiB)
/// - `Helion-MM` peripheral cores: 4 KiB-aligned apertures starting at `0x4000_0000`.
pub fn build_memory_map(bd: &BlockDesign) -> Result<SocMemoryMap, String> {
    let v = validate(bd);
    if !v.ok {
        return Err(v.errors.join("; "));
    }
    let mut cpu_core = None;
    let mut peripherals = Vec::new();
    let mut next_base = 0x4000_0000u64;
    let aperture_size = 0x1000u64; // 4 KiB per peripheral
    let mut dev_id = 0u32;
    let mut irq_idx = 0u32;

    for (idx, c) in bd.cores.iter().enumerate() {
        if c.name.contains("rv32") || c.name.contains("cpu") || c.name.contains("ibex") {
            cpu_core = Some(c.name.clone());
            continue;
        }
        if c.bus != "Helion-MM" {
            continue;
        }
        let inst_name = format!("u_{}_{}", c.name, idx);
        let irq = if matches!(c.name.as_str(), "h_uart" | "h_gpio" | "h_timer" | "h_watchdog" | "h_spi_mm") {
            let id = irq_idx;
            irq_idx += 1;
            Some(id)
        } else {
            None
        };
        peripherals.push(PeripheralRegion {
            instance_name: inst_name,
            core_name: c.name.clone(),
            device_id: dev_id,
            base_addr: next_base,
            high_addr: next_base + aperture_size - 1,
            size_bytes: aperture_size,
            irq,
            registers: canonical_registers(&c.name),
        });
        dev_id += 1;
        next_base += aperture_size;
    }

    Ok(SocMemoryMap {
        design_name: bd.name.clone(),
        cpu_core,
        clk_hz: 100_000_000,
        imem_base: 0x0000_0000,
        imem_size: 0x0000_4000,
        dmem_base: 0x1000_0000,
        dmem_size: 0x0000_4000,
        peripherals,
    })
}

/// Emit a Vivado/Vitis-style C BSP header (`xparameters.h`) from a [`BlockDesign`].
pub fn emit_xparameters_h(bd: &BlockDesign) -> Result<String, String> {
    let map = build_memory_map(bd)?;
    let guard = format!(
        "XPARAMETERS_{}_H",
        map.design_name.to_ascii_uppercase().replace('-', "_")
    );
    let mut out = String::new();
    out.push_str(&format!(
        "/* Auto-generated Vitis/Helion BSP header for BlockDesign '{}' */\n",
        map.design_name
    ));
    out.push_str("#ifndef XPARAMETERS_H\n#define XPARAMETERS_H\n");
    out.push_str(&format!("#ifndef {guard}\n#define {guard}\n\n"));
    out.push_str("#include <stdint.h>\n\n");

    let cpu_label = map.cpu_core.as_deref().unwrap_or("h_rv32_hb1");
    out.push_str(&format!(
        "#define XPAR_CPU_CORE_NAME \"{}\"\n",
        cpu_label
    ));
    out.push_str(&format!(
        "#define XPAR_CPU_CORE_CLOCK_FREQ_HZ {}UL\n",
        map.clk_hz
    ));
    out.push_str(&format!(
        "#define XPAR_BRAM_0_BASEADDR {:#010x}UL\n",
        map.imem_base
    ));
    out.push_str(&format!(
        "#define XPAR_BRAM_0_HIGHADDR {:#010x}UL\n",
        map.imem_base + map.imem_size - 1
    ));
    out.push_str(&format!(
        "#define XPAR_DMEM_0_BASEADDR {:#010x}UL\n",
        map.dmem_base
    ));
    out.push_str(&format!(
        "#define XPAR_DMEM_0_HIGHADDR {:#010x}UL\n\n",
        map.dmem_base + map.dmem_size - 1
    ));
    out.push_str(&format!(
        "#define XPAR_NUM_PERIPHERAL_INSTANCES {}U\n\n",
        map.peripherals.len()
    ));

    for p in &map.peripherals {
        let sym = p.core_name.to_ascii_uppercase().replace('-', "_");
        let inst_sym = format!("{}_{}", sym, p.device_id);
        out.push_str(&format!(
            "/* Peripheral {} ({}) */\n",
            p.instance_name, p.core_name
        ));
        out.push_str(&format!(
            "#define XPAR_{inst_sym}_DEVICE_ID {}U\n",
            p.device_id
        ));
        out.push_str(&format!(
            "#define XPAR_{inst_sym}_BASEADDR {:#010x}UL\n",
            p.base_addr
        ));
        out.push_str(&format!(
            "#define XPAR_{inst_sym}_HIGHADDR {:#010x}UL\n",
            p.high_addr
        ));
        // Canonical first-instance alias (e.g. XPAR_H_UART_BASEADDR)
        if p.device_id == 0 || !out.contains(&format!("#define XPAR_{sym}_BASEADDR ")) {
            out.push_str(&format!(
                "#define XPAR_{sym}_BASEADDR {:#010x}UL\n",
                p.base_addr
            ));
            out.push_str(&format!(
                "#define XPAR_{sym}_HIGHADDR {:#010x}UL\n",
                p.high_addr
            ));
        }
        if let Some(irq) = p.irq {
            out.push_str(&format!("#define XPAR_{inst_sym}_INTR {irq}U\n"));
        }
        for r in &p.registers {
            out.push_str(&format!(
                "#define XPAR_{inst_sym}_{}_OFFSET {:#06x}UL /* {} ({}) */\n",
                r.name, r.offset, r.description, r.access
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!("#endif /* {guard} */\n#endif /* XPARAMETERS_H */\n"));
    Ok(out)
}

/// Emit a self-contained Rust Peripheral Access Crate (PAC / HAL) module from a [`BlockDesign`].
pub fn emit_rust_pac(bd: &BlockDesign) -> Result<String, String> {
    let map = build_memory_map(bd)?;
    let mut out = String::new();
    out.push_str(&format!(
        "//! Auto-generated Rust PAC / HAL for Helion BlockDesign `{}`.\n",
        map.design_name
    ));
    out.push_str("#![allow(dead_code)]\n\n");
    out.push_str(&format!(
        "pub const DESIGN_NAME: &str = \"{}\";\n",
        map.design_name
    ));
    out.push_str(&format!(
        "pub const CPU_CLK_HZ: u64 = {};\n",
        map.clk_hz
    ));
    out.push_str(&format!(
        "pub const IMEM_BASE: usize = {:#010x};\n",
        map.imem_base
    ));
    out.push_str(&format!(
        "pub const IMEM_SIZE: usize = {:#010x};\n",
        map.imem_size
    ));
    out.push_str(&format!(
        "pub const DMEM_BASE: usize = {:#010x};\n",
        map.dmem_base
    ));
    out.push_str(&format!(
        "pub const DMEM_SIZE: usize = {:#010x};\n\n",
        map.dmem_size
    ));

    for p in &map.peripherals {
        let mod_name = format!("{}_{}", p.core_name.replace('-', "_"), p.device_id);
        out.push_str(&format!("pub mod {mod_name} {{\n"));
        out.push_str(&format!(
            "    pub const INSTANCE: &str = \"{}\";\n",
            p.instance_name
        ));
        out.push_str(&format!(
            "    pub const DEVICE_ID: u32 = {};\n",
            p.device_id
        ));
        out.push_str(&format!(
            "    pub const BASE_ADDR: usize = {:#010x};\n",
            p.base_addr
        ));
        out.push_str(&format!(
            "    pub const HIGH_ADDR: usize = {:#010x};\n",
            p.high_addr
        ));
        out.push_str(&format!(
            "    pub const SIZE_BYTES: usize = {:#06x};\n",
            p.size_bytes
        ));
        if let Some(irq) = p.irq {
            out.push_str(&format!("    pub const IRQ: u32 = {irq};\n"));
        }
        for r in &p.registers {
            out.push_str(&format!(
                "    /// {} ({})\n    pub const {}_OFFSET: usize = {:#06x};\n",
                r.description, r.access, r.name, r.offset
            ));
        }
        out.push_str("\n    #[derive(Clone, Copy, Debug)]\n");
        out.push_str("    pub struct Mmio;\n");
        out.push_str("    impl Mmio {\n");
        out.push_str("        #[inline]\n");
        out.push_str("        pub const fn base(self) -> usize { BASE_ADDR }\n");
        for r in &p.registers {
            let fn_low = r.name.to_ascii_lowercase();
            out.push_str(&format!(
                "        #[inline]\n        pub const fn {fn_low}_addr(self) -> usize {{ BASE_ADDR + {}_OFFSET }}\n",
                r.name
            ));
        }
        out.push_str("    }\n");
        out.push_str("}\n\n");
    }
    Ok(out)
}

/// Emit a RISC-V GNU LD / LLVM LLD linker script (`lscript.ld`) matching the [`BlockDesign`] memory map.
pub fn emit_linker_script(bd: &BlockDesign) -> Result<String, String> {
    let map = build_memory_map(bd)?;
    let mut out = String::new();
    out.push_str(&format!(
        "/* Auto-generated Vitis/Helion RISC-V linker script (lscript.ld) for '{}' */\n",
        map.design_name
    ));
    out.push_str("OUTPUT_ARCH(\"riscv\")\n");
    out.push_str("ENTRY(_start)\n\n");
    out.push_str("MEMORY\n{\n");
    out.push_str(&format!(
        "  IMEM (rx)  : ORIGIN = {:#010x}, LENGTH = {:#010x}\n",
        map.imem_base, map.imem_size
    ));
    out.push_str(&format!(
        "  DMEM (rwx) : ORIGIN = {:#010x}, LENGTH = {:#010x}\n",
        map.dmem_base, map.dmem_size
    ));
    out.push_str("}\n\n");
    out.push_str("SECTIONS\n{\n");
    out.push_str("  .text : ALIGN(4)\n  {\n");
    out.push_str("    KEEP(*(.init))\n");
    out.push_str("    *(.text .text.*)\n");
    out.push_str("    *(.rodata .rodata.*)\n");
    out.push_str("    . = ALIGN(4);\n");
    out.push_str("    _etext = .;\n");
    out.push_str("  } > IMEM\n\n");
    out.push_str("  .data : ALIGN(4)\n  {\n");
    out.push_str("    _sdata = .;\n");
    out.push_str("    *(.data .data.*)\n");
    out.push_str("    *(.sdata .sdata.*)\n");
    out.push_str("    . = ALIGN(4);\n");
    out.push_str("    _edata = .;\n");
    out.push_str("  } > DMEM AT > IMEM\n\n");
    out.push_str("  .bss (NOLOAD) : ALIGN(4)\n  {\n");
    out.push_str("    _sbss = .;\n");
    out.push_str("    *(.bss .bss.*)\n");
    out.push_str("    *(.sbss .sbss.*)\n");
    out.push_str("    *(COMMON)\n");
    out.push_str("    . = ALIGN(4);\n");
    out.push_str("    _ebss = .;\n");
    out.push_str("  } > DMEM\n\n");
    out.push_str("  PROVIDE(_stack_top = ORIGIN(DMEM) + LENGTH(DMEM));\n");
    out.push_str("}\n");
    Ok(out)
}

/// Emit a Vivado UG898 [`MemoryMapInfo`] (`.mmi`) descriptor for the [`BlockDesign`]'s instruction BRAM.
pub fn emit_mmi(bd: &BlockDesign, part: &str, bram_count: usize) -> Result<MemoryMapInfo, String> {
    let map = build_memory_map(bd)?;
    let n_brams = bram_count.max(1);
    let bytes_per_bram = 64u64 * 8; // 512 bytes per BRAM18 INIT bank
    let proc_name = map
        .cpu_core
        .clone()
        .unwrap_or_else(|| format!("u_{}_cpu", map.design_name));
    let mut lanes = Vec::with_capacity(n_brams);
    let mut cur_addr = map.imem_base;
    for i in 0..n_brams {
        let end = cur_addr + bytes_per_bram - 1;
        lanes.push(MmiBitLane {
            cell_name: format!("u_imem_bram_{i}"),
            placed_site: format!("BRAM_X10Y{}", 1 + i * 2),
            bram_major: i as u16,
            msb: 63,
            lsb: 0,
            addr_begin: cur_addr,
            addr_end: end,
            parity_bits: 0,
        });
        cur_addr = end + 1;
    }
    Ok(MemoryMapInfo {
        part: part.into(),
        spaces: vec![MmiAddressSpace {
            name: format!("{}_imem", map.design_name),
            proc_name,
            addr_begin: map.imem_base,
            addr_end: cur_addr - 1,
            bus_width: 64,
            lanes,
        }],
    })
}

/// Generate the complete Vitis-class SoC BSP bundle (`xparameters.h`, Rust PAC, `lscript.ld`, `.mmi` XML/JSON).
pub fn generate_bsp(bd: &BlockDesign, part: &str) -> Result<BspBundle, String> {
    let memory_map = build_memory_map(bd)?;
    let xparameters_h = emit_xparameters_h(bd)?;
    let rust_pac = emit_rust_pac(bd)?;
    let lscript_ld = emit_linker_script(bd)?;
    let mmi = emit_mmi(bd, part, 4)?;
    let mmi_xml = mmi.to_xml();
    let mmi_json = mmi.to_json();
    Ok(BspBundle {
        memory_map,
        xparameters_h,
        rust_pac,
        lscript_ld,
        mmi_xml,
        mmi_json,
    })
}

impl BlockDesign {
    pub fn memory_map(&self) -> Result<SocMemoryMap, String> {
        build_memory_map(self)
    }

    pub fn emit_xparameters_h(&self) -> Result<String, String> {
        emit_xparameters_h(self)
    }

    pub fn emit_rust_pac(&self) -> Result<String, String> {
        emit_rust_pac(self)
    }

    pub fn emit_linker_script(&self) -> Result<String, String> {
        emit_linker_script(self)
    }

    pub fn emit_mmi(&self, part: &str, bram_count: usize) -> Result<MemoryMapInfo, String> {
        emit_mmi(self, part, bram_count)
    }

    pub fn generate_bsp(&self, part: &str) -> Result<BspBundle, String> {
        generate_bsp(self, part)
    }
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
    use helion_bits::Bitstream;
    use helion_device::Device;
    use helion_ipxact::{
        pack_gpio, pack_pwm, pack_rv32, pack_spi_mm, pack_sync_fifo, pack_timer, pack_uart,
        pack_watchdog,
    };

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
            graph::apply_bd_automation(
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
        graph::apply_bd_automation(
            &mut bd,
            BdAutomationRule::Board {
                target_cell: "gpio_0".into(),
                target_intf: "GPIO".into(),
                external_port_name: Some("gpio_led".into()),
            },
        )
        .unwrap();
        graph::apply_bd_automation(
            &mut bd,
            BdAutomationRule::Board {
                target_cell: "uart_0".into(),
                target_intf: "UART".into(),
                external_port_name: Some("usb_uart".into()),
            },
        )
        .unwrap();
        graph::apply_bd_automation(
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

        graph::apply_bd_automation(
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
        graph::apply_bd_automation(
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

    #[test]
    fn generate_bsp_xparameters_rust_pac_lscript_and_updatemem() {
        let bd = BlockDesign {
            name: "helion_soc".into(),
            cores: vec![
                pack_rv32(),
                pack_uart(),
                pack_gpio(),
                pack_timer(),
                pack_pwm(),
                pack_spi_mm(),
                pack_watchdog(),
                pack_sync_fifo(), // Helion-ST core (not MMIO-mapped)
            ],
        };

        let bsp = bd.generate_bsp("HL10T-C32-1").unwrap();
        assert_eq!(bsp.memory_map.cpu_core.as_deref(), Some("h_rv32_hb1"));
        assert_eq!(bsp.memory_map.peripherals.len(), 6);
        assert_eq!(bsp.memory_map.peripherals[0].base_addr, 0x4000_0000);
        assert_eq!(bsp.memory_map.peripherals[1].base_addr, 0x4000_1000);

        // 1. C header xparameters.h checks
        assert!(bsp.xparameters_h.contains("#ifndef XPARAMETERS_H"));
        assert!(bsp.xparameters_h.contains("#define XPAR_CPU_CORE_CLOCK_FREQ_HZ 100000000UL"));
        assert!(bsp.xparameters_h.contains("#define XPAR_H_UART_0_BASEADDR 0x40000000UL"));
        assert!(bsp.xparameters_h.contains("#define XPAR_H_UART_0_TXDATA_OFFSET 0x0004UL"));
        assert!(bsp.xparameters_h.contains("#define XPAR_H_GPIO_1_BASEADDR 0x40001000UL"));
        assert!(bsp.xparameters_h.contains("#define XPAR_H_TIMER_2_BASEADDR 0x40002000UL"));
        assert!(bsp.xparameters_h.contains("#define XPAR_H_SPI_MM_4_BASEADDR 0x40004000UL"));

        // 2. Rust PAC module checks
        assert!(bsp.rust_pac.contains("pub mod h_uart_0"));
        assert!(bsp.rust_pac.contains("pub const BASE_ADDR: usize = 0x40000000;"));
        assert!(bsp.rust_pac.contains("pub const TXDATA_OFFSET: usize = 0x0004;"));
        assert!(bsp.rust_pac.contains("pub mod h_gpio_1"));
        assert!(bsp.rust_pac.contains("pub const fn txdata_addr(self) -> usize"));

        // 3. Linker script lscript.ld checks
        assert!(bsp.lscript_ld.contains("OUTPUT_ARCH(\"riscv\")"));
        assert!(bsp.lscript_ld.contains("ENTRY(_start)"));
        assert!(bsp.lscript_ld.contains("IMEM (rx)  : ORIGIN = 0x00000000, LENGTH = 0x00004000"));
        assert!(bsp.lscript_ld.contains("DMEM (rwx) : ORIGIN = 0x10000000, LENGTH = 0x00004000"));
        assert!(bsp.lscript_ld.contains("PROVIDE(_stack_top = ORIGIN(DMEM) + LENGTH(DMEM));"));

        // 4. End-to-end MMI parse + ELF32 firmware splice via updatemem
        let mmi = MemoryMapInfo::parse(&bsp.mmi_xml).unwrap();
        let mmi_from_json = MemoryMapInfo::parse(&bsp.mmi_json).unwrap();
        assert_eq!(mmi, mmi_from_json);

        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut bits = Bitstream::empty(&dev);
        let elf = build_minimal_elf32(0x0000_0000, 0x0000_0000, &0xCAFE_F00D_1234_5678u64.to_le_bytes());
        let fw = load_elf_firmware(&elf).unwrap();
        let n = updatemem(&dev, &mut bits, &mmi, "", &fw).unwrap();
        assert_eq!(n, 1);
        assert_eq!(readback_bram_word(&bits, 0, 0), 0xCAFE_F00D_1234_5678u64);
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
