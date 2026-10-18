//! Headless block design: validate + emit SV + Vitis-class SoC BSP (`xparameters.h`, Rust PAC, `lscript.ld`, `.mmi`).

pub use helion_bits::{
    FirmwareImage, FirmwareSegment, MemoryMapInfo, MmiAddressSpace, MmiBitLane,
    build_minimal_elf32, build_minimal_elf64, load_elf_firmware, load_mem_firmware,
    readback_bram_word, updatemem,
};
use helion_ipxact::IpCore;

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
}
