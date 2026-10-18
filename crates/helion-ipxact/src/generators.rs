//! Vivado UG994 / IEEE 1685 Parametric IP Catalog component definitions.

use crate::IpCore;
use crate::model::{
    AddressBlock, BusInterface, BusInterfaceType, InterfaceMode, IpComponent, IpParameter, IpPort,
    MemoryMap, PortDirection, PortMapEntry, RegisterDef, ResetPolarity,
};
use std::collections::BTreeMap;

pub fn pack_axi_interconnect() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_interconnect".into(),
        version: "2.1".into(),
        bus: "AXI4-Lite".into(),
    }
}

pub fn pack_axi_protocol_converter() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_protocol_converter".into(),
        version: "2.1".into(),
        bus: "AXI4".into(),
    }
}

pub fn pack_axi_gpio() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_gpio".into(),
        version: "2.0".into(),
        bus: "AXI4-Lite".into(),
    }
}

pub fn pack_axi_uartlite() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_uartlite".into(),
        version: "2.0".into(),
        bus: "AXI4-Lite".into(),
    }
}

pub fn pack_axi_timer() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_timer".into(),
        version: "2.0".into(),
        bus: "AXI4-Lite".into(),
    }
}

pub fn pack_axi_bram_ctrl() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_bram_ctrl".into(),
        version: "4.1".into(),
        bus: "AXI4-Lite".into(),
    }
}

pub fn pack_cdc_fifo() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "cdc_fifo".into(),
        version: "2.0".into(),
        bus: "AXI4-Stream".into(),
    }
}

pub fn pack_proc_sys_reset() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "proc_sys_reset".into(),
        version: "5.0".into(),
        bus: "Reset".into(),
    }
}

pub fn pack_axi_cpu() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_cpu".into(),
        version: "1.0".into(),
        bus: "AXI4-Lite".into(),
    }
}

pub fn pack_axi4_master_cpu() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi4_master_cpu".into(),
        version: "1.0".into(),
        bus: "AXI4".into(),
    }
}

pub fn pack_axi_quad_spi() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_quad_spi".into(),
        version: "3.2".into(),
        bus: "AXI4-Lite".into(),
    }
}

pub fn pack_axi_iic() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_iic".into(),
        version: "2.1".into(),
        bus: "AXI4-Lite".into(),
    }
}

pub fn pack_axi_intc() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "axi_intc".into(),
        version: "4.1".into(),
        bus: "AXI4-Lite".into(),
    }
}

pub fn pack_blk_mem_gen() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "blk_mem_gen".into(),
        version: "8.4".into(),
        bus: "BRAM_PORT".into(),
    }
}

pub fn pack_apb_gpio() -> IpCore {
    IpCore {
        vendor: "helion.com".into(),
        library: "ip".into(),
        name: "apb_gpio".into(),
        version: "1.0".into(),
        bus: "APB".into(),
    }
}

pub fn pack_wb_gpio() -> IpCore {
    IpCore {
        vendor: "helion.com".into(),
        library: "ip".into(),
        name: "wb_gpio".into(),
        version: "1.0".into(),
        bus: "Wishbone".into(),
    }
}

pub fn component_axi_interconnect(num_masters: u32, num_slaves: u32) -> IpComponent {
    let m = num_masters.max(1);
    let s = num_slaves.max(1);
    let mut interfaces = vec![
        BusInterface::clock_port("ACLK", "aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "ARESETN",
            "aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
    ];
    let mut params = vec![
        IpParameter::int("C_NUM_MASTERS", m as i64, "Number of AXI slave ports (from masters)"),
        IpParameter::int("C_NUM_SLAVES", s as i64, "Number of AXI master ports (to slaves)"),
        IpParameter::int("C_ADDR_WIDTH", 32, "Address bus width"),
        IpParameter::int("C_DATA_WIDTH", 32, "Data bus width"),
    ];
    for idx in 0..m {
        let intf_name = format!("S{idx:02}_AXI");
        let prefix = format!("s{idx:02}_axi");
        interfaces.push(BusInterface::axi4lite_slave(
            &intf_name,
            &prefix,
            32,
            32,
            0,
            "aclk",
            "aresetn",
        ));
    }
    for idx in 0..s {
        let intf_name = format!("M{idx:02}_AXI");
        let prefix = format!("m{idx:02}_axi");
        interfaces.push(BusInterface::axi4lite_master(
            &intf_name,
            &prefix,
            32,
            32,
            "aclk",
            "aresetn",
        ));
        params.push(IpParameter::hex(
            &format!("C_M{idx:02}_BASE_ADDR"),
            0x4000_0000 + (idx as u64) * 0x0001_0000,
            32,
            "Slave aperture base address",
        ));
        params.push(IpParameter::int(
            &format!("C_M{idx:02}_ADDR_WIDTH"),
            16,
            "Slave aperture address width (log2 bytes)",
        ));
    }
    let mut comp = IpComponent {
        core: pack_axi_interconnect(),
        display_name: "AXI Interconnect".into(),
        description: "Parameterized MxS AXI4-Lite crossbar interconnect with address decoder and DECERR responder".into(),
        parameters: params,
        ports: Vec::new(),
        interfaces,
        memory_maps: Vec::new(),
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi_protocol_converter() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("ACLK", "aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "ARESETN",
            "aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4_full(
            "S_AXI",
            "s_axi",
            InterfaceMode::Slave,
            32,
            32,
            0,
            "aclk",
            "aresetn",
        ),
        BusInterface::axi4lite_master("M_AXI", "m_axi", 32, 32, "aclk", "aresetn"),
    ];
    let mut comp = IpComponent {
        core: pack_axi_protocol_converter(),
        display_name: "AXI Protocol Converter".into(),
        description: "AXI4 burst to AXI4-Lite single-beat protocol bridge".into(),
        parameters: vec![
            IpParameter::int("C_ADDR_WIDTH", 32, "Address width"),
            IpParameter::int("C_DATA_WIDTH", 32, "Data width"),
        ],
        ports: Vec::new(),
        interfaces,
        memory_maps: Vec::new(),
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi_gpio() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("S_AXI_ACLK", "s_axi_aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "S_AXI_ARESETN",
            "s_axi_aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4lite_slave(
            "S_AXI",
            "s_axi",
            9,
            32,
            0x1000,
            "s_axi_aclk",
            "s_axi_aresetn",
        ),
        BusInterface {
            name: "GPIO".into(),
            bus_type: BusInterfaceType::Gpio,
            mode: InterfaceMode::Master,
            associated_clock: Some("s_axi_aclk".into()),
            associated_reset: Some("s_axi_aresetn".into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width: 0,
            data_width: 32,
            default_range: 0,
            port_maps: vec![
                PortMapEntry::new("TRI_I", "gpio_io_i", 32, PortDirection::In),
                PortMapEntry::new("TRI_O", "gpio_io_o", 32, PortDirection::Out),
                PortMapEntry::new("TRI_T", "gpio_io_t", 32, PortDirection::Out),
            ],
        },
        BusInterface::interrupt_port("IP2INTC_IRPT", "ip2intc_irpt", InterfaceMode::Master, 1),
    ];
    let memory_maps = vec![MemoryMap {
        name: "S_AXI_MEM".into(),
        interface_name: "S_AXI".into(),
        address_blocks: vec![AddressBlock {
            name: "Reg".into(),
            base_address: 0,
            range: 0x1000,
            width: 32,
            usage: "register".into(),
            registers: vec![
                RegisterDef::new("GPIO_DATA", 0x000, "read-write", 0, "Channel 1 data"),
                RegisterDef::new("GPIO_TRI", 0x004, "read-write", 0xFFFF_FFFF, "Channel 1 tristate"),
                RegisterDef::new("GPIO2_DATA", 0x008, "read-write", 0, "Channel 2 data"),
                RegisterDef::new("GPIO2_TRI", 0x00C, "read-write", 0xFFFF_FFFF, "Channel 2 tristate"),
                RegisterDef::new("GIER", 0x11C, "read-write", 0, "Global interrupt enable"),
                RegisterDef::new("IP_ISR", 0x120, "read-write", 0, "Interrupt status"),
                RegisterDef::new("IP_IER", 0x128, "read-write", 0, "Interrupt enable"),
            ],
        }],
    }];
    let mut comp = IpComponent {
        core: pack_axi_gpio(),
        display_name: "AXI GPIO".into(),
        description: "Parameterized AXI4-Lite General Purpose I/O with tristate and interrupt".into(),
        parameters: vec![
            IpParameter::int("C_GPIO_WIDTH", 32, "GPIO channel 1 width"),
            IpParameter::int("C_IS_DUAL", 0, "Enable second GPIO channel"),
            IpParameter::int("C_INTERRUPT_PRESENT", 1, "Enable interrupt controller"),
            IpParameter::int("C_S_AXI_ADDR_WIDTH", 9, "AXI address width"),
            IpParameter::int("C_S_AXI_DATA_WIDTH", 32, "AXI data width"),
        ],
        ports: Vec::new(),
        interfaces,
        memory_maps,
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi_uartlite() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("S_AXI_ACLK", "s_axi_aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "S_AXI_ARESETN",
            "s_axi_aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4lite_slave(
            "S_AXI",
            "s_axi",
            4,
            32,
            0x1000,
            "s_axi_aclk",
            "s_axi_aresetn",
        ),
        BusInterface {
            name: "UART".into(),
            bus_type: BusInterfaceType::Uart,
            mode: InterfaceMode::Master,
            associated_clock: Some("s_axi_aclk".into()),
            associated_reset: Some("s_axi_aresetn".into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width: 0,
            data_width: 1,
            default_range: 0,
            port_maps: vec![
                PortMapEntry::new("RxD", "rx", 1, PortDirection::In),
                PortMapEntry::new("TxD", "tx", 1, PortDirection::Out),
            ],
        },
        BusInterface::interrupt_port("INTERRUPT", "interrupt", InterfaceMode::Master, 1),
    ];
    let memory_maps = vec![MemoryMap {
        name: "S_AXI_MEM".into(),
        interface_name: "S_AXI".into(),
        address_blocks: vec![AddressBlock {
            name: "Reg".into(),
            base_address: 0,
            range: 0x1000,
            width: 32,
            usage: "register".into(),
            registers: vec![
                RegisterDef::new("RX_FIFO", 0x00, "read-only", 0, "Receive data FIFO"),
                RegisterDef::new("TX_FIFO", 0x04, "write-only", 0, "Transmit data FIFO"),
                RegisterDef::new("STAT_REG", 0x08, "read-only", 0x04, "UART status register"),
                RegisterDef::new("CTRL_REG", 0x0C, "write-only", 0, "UART control register"),
            ],
        }],
    }];
    let mut comp = IpComponent {
        core: pack_axi_uartlite(),
        display_name: "AXI UART Lite".into(),
        description: "AXI4-Lite UART controller with 16-byte TX/RX FIFOs and baud generator".into(),
        parameters: vec![
            IpParameter::int("C_CLK_FREQ_HZ", 100_000_000, "System clock frequency in Hz"),
            IpParameter::int("C_BAUDRATE", 115_200, "Serial baud rate"),
            IpParameter::int("C_DATA_BITS", 8, "Number of data bits (5..8)"),
            IpParameter::int("C_USE_PARITY", 0, "Enable parity bit"),
            IpParameter::int("C_ODD_PARITY", 0, "1 = odd parity, 0 = even parity"),
        ],
        ports: Vec::new(),
        interfaces,
        memory_maps,
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi_timer() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("S_AXI_ACLK", "s_axi_aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "S_AXI_ARESETN",
            "s_axi_aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4lite_slave(
            "S_AXI",
            "s_axi",
            5,
            32,
            0x1000,
            "s_axi_aclk",
            "s_axi_aresetn",
        ),
        BusInterface::interrupt_port("INTERRUPT", "interrupt", InterfaceMode::Master, 1),
    ];
    let memory_maps = vec![MemoryMap {
        name: "S_AXI_MEM".into(),
        interface_name: "S_AXI".into(),
        address_blocks: vec![AddressBlock {
            name: "Reg".into(),
            base_address: 0,
            range: 0x1000,
            width: 32,
            usage: "register".into(),
            registers: vec![
                RegisterDef::new("TCSR0", 0x00, "read-write", 0, "Timer 0 Control and Status"),
                RegisterDef::new("TLR0", 0x04, "read-write", 0, "Timer 0 Load Register"),
                RegisterDef::new("TCR0", 0x08, "read-only", 0, "Timer 0 Counter Register"),
                RegisterDef::new("TCSR1", 0x10, "read-write", 0, "Timer 1 Control and Status"),
                RegisterDef::new("TLR1", 0x14, "read-write", 0, "Timer 1 Load Register"),
                RegisterDef::new("TCR1", 0x18, "read-only", 0, "Timer 1 Counter Register"),
            ],
        }],
    }];
    let mut comp = IpComponent {
        core: pack_axi_timer(),
        display_name: "AXI Timer".into(),
        description: "Dual 32-bit AXI4-Lite timer/counter with auto-reload, capture, and PWM".into(),
        parameters: vec![
            IpParameter::int("C_COUNT_WIDTH", 32, "Counter width in bits"),
            IpParameter::int("C_ONE_TIMER_ONLY", 0, "Disable second timer channel"),
        ],
        ports: vec![
            IpPort::scalar_in("capturetrig0", "1'b0"),
            IpPort::scalar_in("capturetrig1", "1'b0"),
            IpPort::scalar_out("generateout0"),
            IpPort::scalar_out("generateout1"),
            IpPort::scalar_out("pwm0"),
            IpPort::scalar_in("freeze", "1'b0"),
        ],
        interfaces,
        memory_maps,
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi_bram_ctrl() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("S_AXI_ACLK", "s_axi_aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "S_AXI_ARESETN",
            "s_axi_aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4lite_slave(
            "S_AXI",
            "s_axi",
            12,
            32,
            0x2000,
            "s_axi_aclk",
            "s_axi_aresetn",
        ),
        BusInterface {
            name: "BRAM_PORTA".into(),
            bus_type: BusInterfaceType::BramPort,
            mode: InterfaceMode::Master,
            associated_clock: Some("s_axi_aclk".into()),
            associated_reset: Some("s_axi_aresetn".into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width: 12,
            data_width: 32,
            default_range: 0x2000,
            port_maps: vec![
                PortMapEntry::new("CLK", "bram_clk_a", 1, PortDirection::Out),
                PortMapEntry::new("RST", "bram_rst_a", 1, PortDirection::Out),
                PortMapEntry::new("EN", "bram_en_a", 1, PortDirection::Out),
                PortMapEntry::new("WE", "bram_we_a", 4, PortDirection::Out),
                PortMapEntry::new("ADDR", "bram_addr_a", 12, PortDirection::Out),
                PortMapEntry::new("DIN", "bram_wrdata_a", 32, PortDirection::Out),
                PortMapEntry::new("DOUT", "bram_rddata_a", 32, PortDirection::In),
            ],
        },
    ];
    let memory_maps = vec![MemoryMap {
        name: "S_AXI_MEM".into(),
        interface_name: "S_AXI".into(),
        address_blocks: vec![AddressBlock {
            name: "Mem0".into(),
            base_address: 0,
            range: 0x2000,
            width: 32,
            usage: "memory".into(),
            registers: Vec::new(),
        }],
    }];
    let mut comp = IpComponent {
        core: pack_axi_bram_ctrl(),
        display_name: "AXI BRAM Controller".into(),
        description: "AXI4-Lite to BRAM_PORTA controller with optional internal synchronous SRAM".into(),
        parameters: vec![
            IpParameter::int("C_MEMORY_DEPTH", 2048, "Memory depth in words"),
            IpParameter::int("C_S_AXI_ADDR_WIDTH", 13, "AXI byte address width"),
            IpParameter::int("C_S_AXI_DATA_WIDTH", 32, "AXI data width"),
            IpParameter::int("C_INTERNAL_MEM", 1, "Instantiate internal synchronous BRAM array"),
        ],
        ports: Vec::new(),
        interfaces,
        memory_maps,
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_cdc_fifo() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("S_AXIS_ACLK", "s_axis_aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "S_AXIS_ARESETN",
            "s_axis_aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::clock_port("M_AXIS_ACLK", "m_axis_aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "M_AXIS_ARESETN",
            "m_axis_aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4stream(
            "S_AXIS",
            "s_axis",
            InterfaceMode::Slave,
            32,
            "s_axis_aclk",
            "s_axis_aresetn",
        ),
        BusInterface::axi4stream(
            "M_AXIS",
            "m_axis",
            InterfaceMode::Master,
            32,
            "m_axis_aclk",
            "m_axis_aresetn",
        ),
    ];
    let mut comp = IpComponent {
        core: pack_cdc_fifo(),
        display_name: "AXI4-Stream Synchronous/Async CDC FIFO".into(),
        description: "Parameterized synchronous or Gray-code dual-clock asynchronous CDC FIFO".into(),
        parameters: vec![
            IpParameter::int("C_TDATA_WIDTH", 32, "Stream data payload width"),
            IpParameter::int("C_FIFO_DEPTH", 16, "FIFO depth (power of 2)"),
            IpParameter::int("C_IS_ASYNC", 1, "1 = dual-clock Gray-code CDC, 0 = single-clock sync"),
        ],
        ports: vec![
            IpPort::scalar_out("axis_full"),
            IpPort::scalar_out("axis_empty"),
        ],
        interfaces,
        memory_maps: Vec::new(),
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_proc_sys_reset() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("SLOWEST_SYNC_CLK", "slowest_sync_clk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "EXT_RESET_IN",
            "ext_reset_in",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::reset_port(
            "AUX_RESET_IN",
            "aux_reset_in",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::reset_port(
            "MB_RESET",
            "mb_reset",
            InterfaceMode::Master,
            ResetPolarity::ActiveHigh,
        ),
        BusInterface::reset_port(
            "BUS_STRUCT_RESET",
            "bus_struct_reset",
            InterfaceMode::Master,
            ResetPolarity::ActiveHigh,
        ),
        BusInterface::reset_port(
            "PERIPHERAL_RESET",
            "peripheral_reset",
            InterfaceMode::Master,
            ResetPolarity::ActiveHigh,
        ),
        BusInterface::reset_port(
            "INTERCONNECT_ARESETN",
            "interconnect_aresetn",
            InterfaceMode::Master,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::reset_port(
            "PERIPHERAL_ARESETN",
            "peripheral_aresetn",
            InterfaceMode::Master,
            ResetPolarity::ActiveLow,
        ),
    ];
    let mut comp = IpComponent {
        core: pack_proc_sys_reset(),
        display_name: "Processor System Reset".into(),
        description: "Clock/Reset synchronizer with staged release sequencing for bus, interconnect, peripheral, and CPU".into(),
        parameters: vec![
            IpParameter::int("C_EXT_RST_WIDTH", 4, "External reset pulse filter width"),
            IpParameter::int("C_AUX_RST_WIDTH", 4, "Auxiliary reset pulse filter width"),
            IpParameter::int("C_EXT_RESET_HIGH", 0, "External reset active polarity"),
            IpParameter::int("C_AUX_RESET_HIGH", 0, "Auxiliary reset active polarity"),
        ],
        ports: vec![
            IpPort::scalar_in("mb_debug_sys_rst", "1'b0"),
            IpPort::scalar_in("dcm_locked", "1'b1"),
        ],
        interfaces,
        memory_maps: Vec::new(),
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi_cpu() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("ACLK", "aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "ARESETN",
            "aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4lite_master("M_AXI", "m_axi", 32, 32, "aclk", "aresetn"),
        BusInterface::interrupt_port("IRQ", "irq", InterfaceMode::Slave, 1),
    ];
    let mut comp = IpComponent {
        core: pack_axi_cpu(),
        display_name: "RISC-V AXI4-Lite Master CPU".into(),
        description: "Synthesizable 32-bit processor master driving M_AXI".into(),
        parameters: vec![
            IpParameter::hex("C_RESET_VECTOR", 0x0000_0000, 32, "Boot reset vector"),
            IpParameter::int("C_ADDR_WIDTH", 32, "Address width"),
            IpParameter::int("C_DATA_WIDTH", 32, "Data width"),
        ],
        ports: Vec::new(),
        interfaces,
        memory_maps: Vec::new(),
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi4_master_cpu() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("ACLK", "aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "ARESETN",
            "aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4_full(
            "M_AXI",
            "m_axi",
            InterfaceMode::Master,
            32,
            32,
            0,
            "aclk",
            "aresetn",
        ),
        BusInterface::interrupt_port("IRQ", "irq", InterfaceMode::Slave, 1),
    ];
    let mut comp = IpComponent {
        core: pack_axi4_master_cpu(),
        display_name: "AXI4 Burst Master CPU".into(),
        description: "Synthesizable 32-bit processor master with full AXI4 burst interface".into(),
        parameters: vec![
            IpParameter::hex("C_RESET_VECTOR", 0x0000_0000, 32, "Boot reset vector"),
            IpParameter::int("C_ADDR_WIDTH", 32, "Address width"),
            IpParameter::int("C_DATA_WIDTH", 32, "Data width"),
        ],
        ports: Vec::new(),
        interfaces,
        memory_maps: Vec::new(),
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi_quad_spi() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("S_AXI_ACLK", "s_axi_aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "S_AXI_ARESETN",
            "s_axi_aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4lite_slave(
            "S_AXI",
            "s_axi",
            7,
            32,
            0x1000,
            "s_axi_aclk",
            "s_axi_aresetn",
        ),
        BusInterface {
            name: "SPI_0".into(),
            bus_type: BusInterfaceType::Spi,
            mode: InterfaceMode::Master,
            associated_clock: Some("s_axi_aclk".into()),
            associated_reset: Some("s_axi_aresetn".into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width: 0,
            data_width: 1,
            default_range: 0,
            port_maps: vec![
                PortMapEntry::new("SCK_O", "sck_o", 1, PortDirection::Out),
                PortMapEntry::new("IO0_O", "mosi_o", 1, PortDirection::Out),
                PortMapEntry::new("IO1_I", "miso_i", 1, PortDirection::In),
                PortMapEntry::new("SS_O", "ss_o", 1, PortDirection::Out),
            ],
        },
        BusInterface::interrupt_port("IP2INTC_IRPT", "ip2intc_irpt", InterfaceMode::Master, 1),
    ];
    let mut comp = IpComponent {
        core: pack_axi_quad_spi(),
        display_name: "AXI Quad SPI".into(),
        description: "AXI4-Lite SPI Master controller".into(),
        parameters: vec![IpParameter::int("C_SCK_RATIO", 8, "SPI clock divider ratio")],
        ports: Vec::new(),
        interfaces,
        memory_maps: vec![MemoryMap {
            name: "S_AXI_MEM".into(),
            interface_name: "S_AXI".into(),
            address_blocks: vec![AddressBlock {
                name: "Reg".into(),
                base_address: 0,
                range: 0x1000,
                width: 32,
                usage: "register".into(),
                registers: Vec::new(),
            }],
        }],
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi_iic() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("S_AXI_ACLK", "s_axi_aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "S_AXI_ARESETN",
            "s_axi_aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4lite_slave(
            "S_AXI",
            "s_axi",
            9,
            32,
            0x1000,
            "s_axi_aclk",
            "s_axi_aresetn",
        ),
        BusInterface {
            name: "IIC".into(),
            bus_type: BusInterfaceType::I2c,
            mode: InterfaceMode::Master,
            associated_clock: Some("s_axi_aclk".into()),
            associated_reset: Some("s_axi_aresetn".into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width: 0,
            data_width: 1,
            default_range: 0,
            port_maps: vec![
                PortMapEntry::new("SDA_I", "sda_i", 1, PortDirection::In),
                PortMapEntry::new("SDA_O", "sda_o", 1, PortDirection::Out),
                PortMapEntry::new("SDA_T", "sda_t", 1, PortDirection::Out),
                PortMapEntry::new("SCL_I", "scl_i", 1, PortDirection::In),
                PortMapEntry::new("SCL_O", "scl_o", 1, PortDirection::Out),
                PortMapEntry::new("SCL_T", "scl_t", 1, PortDirection::Out),
            ],
        },
        BusInterface::interrupt_port("IIC2INTC_IRPT", "iic2intc_irpt", InterfaceMode::Master, 1),
    ];
    let mut comp = IpComponent {
        core: pack_axi_iic(),
        display_name: "AXI IIC Bus Interface".into(),
        description: "AXI4-Lite I2C Master/Slave controller".into(),
        parameters: vec![IpParameter::int("C_IIC_FREQ", 100_000, "I2C bus frequency in Hz")],
        ports: Vec::new(),
        interfaces,
        memory_maps: vec![MemoryMap {
            name: "S_AXI_MEM".into(),
            interface_name: "S_AXI".into(),
            address_blocks: vec![AddressBlock {
                name: "Reg".into(),
                base_address: 0,
                range: 0x1000,
                width: 32,
                usage: "register".into(),
                registers: Vec::new(),
            }],
        }],
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_axi_intc() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("S_AXI_ACLK", "s_axi_aclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "S_AXI_ARESETN",
            "s_axi_aresetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface::axi4lite_slave(
            "S_AXI",
            "s_axi",
            9,
            32,
            0x1000,
            "s_axi_aclk",
            "s_axi_aresetn",
        ),
        BusInterface::interrupt_port("INTR", "intr", InterfaceMode::Slave, 4),
        BusInterface::interrupt_port("IRQ", "irq", InterfaceMode::Master, 1),
    ];
    let mut comp = IpComponent {
        core: pack_axi_intc(),
        display_name: "AXI Interrupt Controller".into(),
        description: "AXI4-Lite Interrupt Controller aggregating peripheral IRQ lines".into(),
        parameters: vec![IpParameter::int("C_NUM_INTR_INPUTS", 4, "Number of interrupt inputs")],
        ports: Vec::new(),
        interfaces,
        memory_maps: vec![MemoryMap {
            name: "S_AXI_MEM".into(),
            interface_name: "S_AXI".into(),
            address_blocks: vec![AddressBlock {
                name: "Reg".into(),
                base_address: 0,
                range: 0x1000,
                width: 32,
                usage: "register".into(),
                registers: Vec::new(),
            }],
        }],
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_blk_mem_gen() -> IpComponent {
    let interfaces = vec![BusInterface {
        name: "BRAM_PORTA".into(),
        bus_type: BusInterfaceType::BramPort,
        mode: InterfaceMode::Slave,
        associated_clock: Some("clka".into()),
        associated_reset: Some("rsta".into()),
        reset_polarity: Some(ResetPolarity::ActiveHigh),
        addr_width: 12,
        data_width: 32,
        default_range: 0x2000,
        port_maps: vec![
            PortMapEntry::new("CLK", "clka", 1, PortDirection::In),
            PortMapEntry::new("RST", "rsta", 1, PortDirection::In),
            PortMapEntry::new("EN", "ena", 1, PortDirection::In),
            PortMapEntry::new("WE", "wea", 4, PortDirection::In),
            PortMapEntry::new("ADDR", "addra", 12, PortDirection::In),
            PortMapEntry::new("DIN", "dina", 32, PortDirection::In),
            PortMapEntry::new("DOUT", "douta", 32, PortDirection::Out),
        ],
    }];
    let mut comp = IpComponent {
        core: pack_blk_mem_gen(),
        display_name: "Block Memory Generator".into(),
        description: "Synthesizable synchronous Block RAM".into(),
        parameters: vec![
            IpParameter::int("C_MEM_DEPTH", 2048, "Depth in 32-bit words"),
            IpParameter::int("C_ADDR_WIDTH", 12, "Address width"),
            IpParameter::int("C_DATA_WIDTH", 32, "Data width"),
        ],
        ports: Vec::new(),
        interfaces,
        memory_maps: Vec::new(),
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_apb_gpio() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("PCLK", "pclk", InterfaceMode::Slave),
        BusInterface::reset_port(
            "PRESETN",
            "presetn",
            InterfaceMode::Slave,
            ResetPolarity::ActiveLow,
        ),
        BusInterface {
            name: "S_APB".into(),
            bus_type: BusInterfaceType::Apb,
            mode: InterfaceMode::Slave,
            associated_clock: Some("pclk".into()),
            associated_reset: Some("presetn".into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width: 12,
            data_width: 32,
            default_range: 0x1000,
            port_maps: vec![
                PortMapEntry::new("PADDR", "s_apb_paddr", 12, PortDirection::In),
                PortMapEntry::new("PSEL", "s_apb_psel", 1, PortDirection::In),
                PortMapEntry::new("PENABLE", "s_apb_penable", 1, PortDirection::In),
                PortMapEntry::new("PWRITE", "s_apb_pwrite", 1, PortDirection::In),
                PortMapEntry::new("PWDATA", "s_apb_pwdata", 32, PortDirection::In),
                PortMapEntry::new("PRDATA", "s_apb_prdata", 32, PortDirection::Out),
                PortMapEntry::new("PREADY", "s_apb_pready", 1, PortDirection::Out),
                PortMapEntry::new("PSLVERR", "s_apb_pslverr", 1, PortDirection::Out),
            ],
        },
        BusInterface {
            name: "GPIO".into(),
            bus_type: BusInterfaceType::Gpio,
            mode: InterfaceMode::Master,
            associated_clock: Some("pclk".into()),
            associated_reset: Some("presetn".into()),
            reset_polarity: Some(ResetPolarity::ActiveLow),
            addr_width: 0,
            data_width: 32,
            default_range: 0,
            port_maps: vec![
                PortMapEntry::new("TRI_I", "gpio_io_i", 32, PortDirection::In),
                PortMapEntry::new("TRI_O", "gpio_io_o", 32, PortDirection::Out),
                PortMapEntry::new("TRI_T", "gpio_io_t", 32, PortDirection::Out),
            ],
        },
    ];
    let mut comp = IpComponent {
        core: pack_apb_gpio(),
        display_name: "APB GPIO".into(),
        description: "APB3/APB4 GPIO peripheral".into(),
        parameters: vec![IpParameter::int("C_GPIO_WIDTH", 32, "GPIO width")],
        ports: Vec::new(),
        interfaces,
        memory_maps: vec![MemoryMap {
            name: "S_APB_MEM".into(),
            interface_name: "S_APB".into(),
            address_blocks: vec![AddressBlock {
                name: "Reg".into(),
                base_address: 0,
                range: 0x1000,
                width: 32,
                usage: "register".into(),
                registers: Vec::new(),
            }],
        }],
    };
    comp.sync_ports_from_interfaces();
    comp
}

pub fn component_wb_gpio() -> IpComponent {
    let interfaces = vec![
        BusInterface::clock_port("WB_CLK", "wb_clk_i", InterfaceMode::Slave),
        BusInterface::reset_port(
            "WB_RST",
            "wb_rst_i",
            InterfaceMode::Slave,
            ResetPolarity::ActiveHigh,
        ),
        BusInterface {
            name: "S_WB".into(),
            bus_type: BusInterfaceType::Wishbone,
            mode: InterfaceMode::Slave,
            associated_clock: Some("wb_clk_i".into()),
            associated_reset: Some("wb_rst_i".into()),
            reset_polarity: Some(ResetPolarity::ActiveHigh),
            addr_width: 12,
            data_width: 32,
            default_range: 0x1000,
            port_maps: vec![
                PortMapEntry::new("ADR", "wb_adr_i", 12, PortDirection::In),
                PortMapEntry::new("DAT_I", "wb_dat_i", 32, PortDirection::In),
                PortMapEntry::new("DAT_O", "wb_dat_o", 32, PortDirection::Out),
                PortMapEntry::new("WE", "wb_we_i", 1, PortDirection::In),
                PortMapEntry::new("SEL", "wb_sel_i", 4, PortDirection::In),
                PortMapEntry::new("STB", "wb_stb_i", 1, PortDirection::In),
                PortMapEntry::new("CYC", "wb_cyc_i", 1, PortDirection::In),
                PortMapEntry::new("ACK", "wb_ack_o", 1, PortDirection::Out),
            ],
        },
    ];
    let mut comp = IpComponent {
        core: pack_wb_gpio(),
        display_name: "Wishbone GPIO".into(),
        description: "Wishbone B4 GPIO slave peripheral".into(),
        parameters: vec![IpParameter::int("C_GPIO_WIDTH", 32, "GPIO width")],
        ports: Vec::new(),
        interfaces,
        memory_maps: vec![MemoryMap {
            name: "S_WB_MEM".into(),
            interface_name: "S_WB".into(),
            address_blocks: vec![AddressBlock {
                name: "Reg".into(),
                base_address: 0,
                range: 0x1000,
                width: 32,
                usage: "register".into(),
                registers: Vec::new(),
            }],
        }],
    };
    comp.sync_ports_from_interfaces();
    comp
}

/// Returns the Vivado UG994 Parametric IP Catalog components (AXI4, AXI4-Lite, AXI4-Stream, APB, Wishbone, BRAM, Reset).
pub fn parametric_catalog() -> Vec<IpComponent> {
    vec![
        component_axi_interconnect(1, 2),
        component_axi_protocol_converter(),
        component_axi_gpio(),
        component_axi_uartlite(),
        component_axi_timer(),
        component_axi_bram_ctrl(),
        component_cdc_fifo(),
        component_proc_sys_reset(),
        component_axi_cpu(),
        component_axi4_master_cpu(),
        component_axi_quad_spi(),
        component_axi_iic(),
        component_axi_intc(),
        component_blk_mem_gen(),
        component_apb_gpio(),
        component_wb_gpio(),
    ]
}

/// Convert any legacy `IpCore` into a full `IpComponent` descriptor.
pub fn upgrade_core_to_component(core: &IpCore) -> IpComponent {
    if let Some(comp) = parametric_catalog()
        .into_iter()
        .find(|c| c.core.name.eq_ignore_ascii_case(&core.name) || c.vlnv().eq_ignore_ascii_case(&core.vlnv()))
    {
        return comp;
    }
    let bus_type = BusInterfaceType::parse(&core.bus).unwrap_or(BusInterfaceType::HelionMm);
    let is_master = core.name.contains("rv32") || core.name.contains("cpu") || core.name.contains("master");
    let mode = if is_master {
        InterfaceMode::Master
    } else {
        InterfaceMode::Slave
    };
    let intf_name = if is_master { "M_MM" } else { "S_MM" };
    let mut comp = IpComponent {
        core: core.clone(),
        display_name: core.name.clone(),
        description: format!("{} ({})", core.name, core.bus),
        parameters: Vec::new(),
        ports: vec![
            IpPort::scalar_in("clk", "1'b0"),
            IpPort::scalar_in("resetn", "1'b1"),
        ],
        interfaces: vec![
            BusInterface::clock_port("CLK", "clk", InterfaceMode::Slave),
            BusInterface::reset_port("RESETN", "resetn", InterfaceMode::Slave, ResetPolarity::ActiveLow),
            BusInterface {
                name: intf_name.into(),
                bus_type,
                mode,
                associated_clock: Some("clk".into()),
                associated_reset: Some("resetn".into()),
                reset_polarity: Some(ResetPolarity::ActiveLow),
                addr_width: 32,
                data_width: 32,
                default_range: if mode == InterfaceMode::Slave { 0x1000 } else { 0 },
                port_maps: Vec::new(),
            },
        ],
        memory_maps: if mode == InterfaceMode::Slave && bus_type.is_memory_mapped() {
            vec![MemoryMap {
                name: "S_MM_MEM".into(),
                interface_name: intf_name.into(),
                address_blocks: vec![AddressBlock {
                    name: "Reg".into(),
                    base_address: 0,
                    range: 0x1000,
                    width: 32,
                    usage: "register".into(),
                    registers: Vec::new(),
                }],
            }]
        } else {
            Vec::new()
        },
    };
    comp.sync_ports_from_interfaces();
    comp
}

/// Find an `IpComponent` by name, alias, or VLNV across both the Vivado parametric catalog and Helion catalog.
pub fn find_ip_component(spec: &str) -> Option<IpComponent> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    // Check common Vivado aliases first
    let lower = s.to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "axis_data_fifo" | "axi_fifo_mm_s" | "fifo_generator" | "xpm_fifo_async" => "cdc_fifo",
        "axi_crossbar" | "smartconnect" => "axi_interconnect",
        "microblaze" | "microblaze_mcs" | "zynq_ultra_ps_e" | "processing_system7" | "picorv32_axi" => "axi_cpu",
        "rst_ps7_0_100m" | "rst_clk_wiz" => "proc_sys_reset",
        other => other,
    };
    for comp in parametric_catalog() {
        if comp.core.name.eq_ignore_ascii_case(s)
            || comp.core.name.eq_ignore_ascii_case(canonical)
            || comp.vlnv().eq_ignore_ascii_case(s)
            || comp.vlnv().contains(&format!(":{canonical}:"))
        {
            return Some(comp);
        }
    }
    for core in crate::catalog() {
        if core.name.eq_ignore_ascii_case(s) || core.vlnv().eq_ignore_ascii_case(s) {
            return Some(upgrade_core_to_component(&core));
        }
    }
    None
}

impl IpComponent {
    /// Generate synthesizable SystemVerilog for this IP component with parameter overrides.
    pub fn generate_sv(&self, overrides: &BTreeMap<String, String>) -> Result<String, String> {
        crate::rtl::generate_component_sv(self, overrides)
    }
}
