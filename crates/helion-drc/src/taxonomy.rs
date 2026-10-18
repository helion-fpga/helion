//! Complete Vivado UG906 / UG949 UltraFast Design Methodology & DRC Rule Taxonomy.
//!
//! Covers all 7 major Vivado rule decks plus physical/IO placement checks:
//! - `TIMING-1` .. `TIMING-50` (50 Timing & Clocking rules)
//! - `SYNTH-1` .. `SYNTH-15` (15 Synthesis & RTL structure rules)
//! - `METHODOLOGY-1` .. `METHODOLOGY-25` (25 UG949 UltraFast Design Methodology rules)
//! - `CDC-1` .. `CDC-20` (20 Clock Domain Crossing rules)
//! - `PDRC-1` .. `PDRC-20` (20 Physical Design Rule Checks)
//! - `REQP-1` .. `REQP-30` (30 Primitive Required Pin / Connectivity checks)
//! - `AVAL-1` .. `AVAL-20` (20 Primitive Architecture Attribute / Value checks)

use crate::DrcSeverity;

/// Major Vivado UG906 / UG949 rule deck family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuleDeckCategory {
    Timing,
    Synth,
    Methodology,
    Cdc,
    Pdrc,
    Reqp,
    Aval,
    PlacementIo,
}

impl RuleDeckCategory {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Timing => "TIMING",
            Self::Synth => "SYNTH",
            Self::Methodology => "METHODOLOGY",
            Self::Cdc => "CDC",
            Self::Pdrc => "PDRC",
            Self::Reqp => "REQP",
            Self::Aval => "AVAL",
            Self::PlacementIo => "PLIO",
        }
    }

    pub fn ug_doc(self) -> &'static str {
        match self {
            Self::Timing | Self::Cdc => "UG906",
            Self::Synth | Self::Methodology => "UG949",
            Self::Pdrc | Self::Reqp | Self::Aval | Self::PlacementIo => "UG953/UG893",
        }
    }
}

/// Complete specification of one DRC / Methodology rule in the Helion UG906/UG949 catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleDefinition {
    pub id: String,
    pub category: RuleDeckCategory,
    pub number: u16,
    pub default_severity: DrcSeverity,
    pub title: String,
    pub ug_reference: &'static str,
    pub description: String,
    pub tcl_remediation_template: String,
    pub rtl_remediation_hint: String,
}

fn timing_rule(num: u16) -> RuleDefinition {
    let (sev, title, desc, tcl, rtl) = match num {
        1 => (
            DrcSeverity::Warning,
            "Invalid clock waveform",
            "Primary clock waveform has invalid duty cycle or non-monotonic edge specification.",
            "create_clock -period 10.000 -waveform {0.000 5.000} [get_ports {<OBJ>}]",
            "Ensure primary clock input connects directly to an oscillator or MMCM/PLL port.",
        ),
        2 => (
            DrcSeverity::Warning,
            "Invalid primary clock source pin",
            "A primary clock is defined on an internal combinational LUT pin instead of a top-level port or clock primitive.",
            "create_clock -period 10.000 [get_ports {clk}]",
            "Move create_clock to the top-level clock input port and use create_generated_clock for internal dividers.",
        ),
        3 => (
            DrcSeverity::Warning,
            "Invalid primary clock on clock modifying block",
            "A primary create_clock constraint overrides an MMCM/PLL output where a generated clock is auto-derived.",
            "reset_clock [get_pins {<OBJ>}]",
            "Let the timing engine auto-derive MMCM/PLL output clocks instead of overriding with create_clock.",
        ),
        4 => (
            DrcSeverity::Warning,
            "Invalid primary clock redefinition on a clock tree",
            "Multiple primary clock definitions exist along the same clock network without -add.",
            "create_clock -period 10.000 -name clk_alt -add [get_ports {<OBJ>}]",
            "Remove duplicate create_clock constraints or specify -add for multiplexed clocks.",
        ),
        5 => (
            DrcSeverity::Warning,
            "Invalid generated clock source pin",
            "Generated clock master source pin does not carry a valid upstream clock.",
            "create_generated_clock -source [get_ports {clk}] -divide_by 2 [get_pins {<OBJ>}]",
            "Verify the -source pin of create_generated_clock is driven by a defined primary clock.",
        ),
        6 => (
            DrcSeverity::Warning,
            "No common primary clock between related clocks",
            "Timed paths exist between two clocks that do not share a common primary clock source and have no false_path or clock_groups exception.",
            "set_clock_groups -asynchronous -group [get_clocks {<OBJ>}]",
            "Insert a 2-FF or FIFO CDC synchronizer and declare asynchronous clock groups in XDC.",
        ),
        7 => (
            DrcSeverity::Error,
            "No common node between related clocks",
            "Cross-clock path has no common clock tree root node; skew cannot be bounded safely.",
            "set_clock_groups -asynchronous -group [get_clocks {<OBJ>}]",
            "Synchronize signals crossing unrelated clock trees using ASYNC_REG synchronizer stages.",
        ),
        8 => (
            DrcSeverity::Error,
            "No common period between related clocks",
            "Related synchronous clocks have unexpandable fractional period ratios over 1000 cycles.",
            "set_max_delay -datapath_only 5.000 -from [get_clocks {<OBJ>}]",
            "Treat non-integer frequency domains as asynchronous and use a dual-clock FIFO or handshake.",
        ),
        14 => (
            DrcSeverity::Warning,
            "LUT on the clock tree",
            "A combinational LUT6 drives sequential CLK pins, introducing routing skew and glitch risk.",
            "set_property CLOCK_DEDICATED_ROUTE FALSE [get_nets {<OBJ>}]",
            "Replace LUT clock gating with synchronous clock-enable (CE) logic or a BUFGCE primitive.",
        ),
        16 => (
            DrcSeverity::Warning,
            "Large setup violation",
            "Critical path exhibits a severe negative setup slack (> 1.0 ns) requiring logic restructuring.",
            "set_property PHYS_OPT_MODIFIED TRUE [get_cells {<OBJ>}]",
            "Pipeline deep combinational LUT cones by inserting an intermediate HFF register stage.",
        ),
        17 => (
            DrcSeverity::Warning,
            "Non-clocked sequential cell",
            "Sequential flip-flop CLK pin is not reached by any defined primary or generated clock.",
            "create_clock -period 10.000 -name sys_clk [get_ports {clk}]",
            "Connect the sequential cell CLK pin to the system clock net and define create_clock in XDC.",
        ),
        18 => (
            DrcSeverity::Warning,
            "Missing input or output delay",
            "Top-level I/O port participates in synchronous timing paths but lacks set_input_delay or set_output_delay.",
            "set_input_delay -clock [get_clocks sys_clk] 2.000 [get_ports {<OBJ>}]\nset_output_delay -clock [get_clocks sys_clk] 2.000 [get_ports {<OBJ>}]",
            "Register top-level I/O signals in IOB flip-flops and constrain board arrival/departure delays.",
        ),
        20 => (
            DrcSeverity::Warning,
            "Non-clocked latch",
            "Transparent latch gate pin is not constrained by a valid clock waveform.",
            "create_clock -period 10.000 [get_nets {<OBJ>}]",
            "Ensure all branches of combinational always_comb / case blocks assign outputs to avoid inferred latches.",
        ),
        27 => (
            DrcSeverity::Warning,
            "Invalid primary clock on hierarchical pin",
            "Primary clock is attached to a hierarchical module boundary pin instead of a physical leaf port/pin.",
            "create_clock -period 10.000 [get_ports {<OBJ>}]",
            "Attach create_clock to the top-level package port rather than an internal hierarchy pin.",
        ),
        _ => (
            if num <= 10 || num == 35 {
                DrcSeverity::Warning
            } else {
                DrcSeverity::Advisory
            },
            "UG906 timing constraint methodology check",
            "Timing exception, clock interaction, or path segmentation check flagged by UG906 timing engine.",
            "report_timing_summary -name timing_check_#NUM\n# Review constraint scope on {<OBJ>}",
            "Review XDC clock definitions and timing exceptions for the flagged timing endpoints.",
        ),
    };
    RuleDefinition {
        id: format!("TIMING-{num}"),
        category: RuleDeckCategory::Timing,
        number: num,
        default_severity: sev,
        title: title.to_string(),
        ug_reference: "UG906",
        description: desc.to_string(),
        tcl_remediation_template: tcl.replace("#NUM", &num.to_string()),
        rtl_remediation_hint: rtl.to_string(),
    }
}

fn synth_rule(num: u16) -> RuleDefinition {
    let (sev, title, desc, tcl, rtl) = match num {
        1 => (
            DrcSeverity::Warning,
            "High fanout net",
            "Net drives a large number of load pins without replication or global buffer promotion.",
            "set_property MAX_FANOUT 32 [get_nets {<OBJ>}]",
            "Replicate the source flip-flop or register tree in RTL to bound fanout per driver.",
        ),
        2 => (
            DrcSeverity::Warning,
            "Unconnected port",
            "Top-level port has no internal net endpoints in the synthesized netlist.",
            "# Remove unused port {<OBJ>} or tie to constant driver",
            "Connect the port in top-level RTL or remove unused interface ports.",
        ),
        3 => (
            DrcSeverity::Error,
            "Combinational timing loop",
            "A cycle of combinational LUT6 cells exists without an intervening sequential HFF register.",
            "set_disable_timing [get_cells {<OBJ>}]",
            "Break the combinational feedback loop by inserting a clocked flip-flop stage.",
        ),
        4 => (
            DrcSeverity::Warning,
            "Inferred transparent latch",
            "Incomplete conditional assignment inferred a level-sensitive latch.",
            "# Inspect latch cell {<OBJ>}",
            "Assign default values at the top of always_comb blocks so all paths are covered.",
        ),
        5 => (
            DrcSeverity::Advisory,
            "Unread sequential register or dangling net",
            "Cell output drives a net with no downstream load pins.",
            "set_property DONT_TOUCH TRUE [get_cells {<OBJ>}]",
            "Connect the unused output to downstream logic or remove dead state registers.",
        ),
        6 => (
            DrcSeverity::Error,
            "Multi-driven net",
            "More than one active cell output pin (`O` or `Q`) drives the same net.",
            "# Resolve multiple drivers on net {<OBJ>}",
            "Ensure each net is driven by at most one continuous assign or always_ff block.",
        ),
        7 => (
            DrcSeverity::Warning,
            "Unelaborated BlackBox cell instance",
            "Design contains a BlackBox cell whose internal logic is not elaborated for fabric packing.",
            "update_compile_order -fileset sources_1\n# Bind implementation for BlackBox {<OBJ>}",
            "Provide the synthesizable RTL module or hard IP primitive mapping for the black box.",
        ),
        8 => (
            DrcSeverity::Warning,
            "Unpipelined DSP48E1 multiplier/ALU block",
            "DSP48E1 cell operates without internal MREG/PREG pipeline registers, limiting Fmax.",
            "set_property PREG 1 [get_cells {<OBJ>}]\nset_property MREG 1 [get_cells {<OBJ>}]",
            "Add 2-3 pipeline register stages around the multiplier/accumulator in RTL so DSP48E1 packs MREG and PREG.",
        ),
        9 => (
            DrcSeverity::Warning,
            "Block RAM output register (DOA_REG/DOB_REG) not enabled",
            "BRAM cell has unregistered read data outputs, incurring large clock-to-out delay.",
            "set_property DOA_REG 1 [get_cells {<OBJ>}]",
            "Add an extra pipeline register on the BRAM read data bus to infer the hard DOA_REG output flop.",
        ),
        10 => (
            DrcSeverity::Advisory,
            "Wide multiplexer cone",
            "Combinational LUT tree exceeds 3 levels of multiplexing without pipeline registers.",
            "set_property MAX_FANOUT 16 [get_cells {<OBJ>}]",
            "Restructure wide case/mux trees into pipelined binary stages.",
        ),
        11 => (
            DrcSeverity::Advisory,
            "Shift register chain without reset",
            "Back-to-back flip-flop chain can pack into SRL16E/SRL32E if reset is omitted.",
            "set_property SHREG_EXTRACT YES [get_cells {<OBJ>}]",
            "Remove reset from pure data-pipeline shift registers to enable compact SRL extraction.",
        ),
        12 => (
            DrcSeverity::Warning,
            "Asynchronous reset without deassertion synchronizer",
            "Reset net is asserted/deasserted asynchronously without a synchronizing flip-flop chain.",
            "set_property ASYNC_REG TRUE [get_cells {<OBJ>}]",
            "Use a synchronous reset or a 2-FF reset bridge with asynchronous assert and synchronous deassert.",
        ),
        13 => (
            DrcSeverity::Warning,
            "LUT-based clock gating detected in RTL",
            "Combinational AND/MUX LUT gates a clock net instead of driving sequential CE pins.",
            "set_property CLOCK_DEDICATED_ROUTE FALSE [get_nets {<OBJ>}]",
            "Use synchronous clock-enable (`if (ce) q <= d;`) instead of gating the clock signal.",
        ),
        14 => (
            DrcSeverity::Error,
            "Incomplete soft-lowered RTL cone",
            "Design has non-empty SOFT diagnostics (`MapResult::softs`); unlowered cones cannot claim hardware PASS.",
            "# Inspect soft diagnostic on {<OBJ>}",
            "Rewrite unsupported SV/VHDL constructs into synthesizable structural/always_ff RTL.",
        ),
        _ => (
            DrcSeverity::Advisory,
            "Long carry chain exceeds column height",
            "Carry4 chain length exceeds a single clock region column.",
            "# Pipeline arithmetic carry chain {<OBJ>}",
            "Split wide >64-bit adders across two clock cycles.",
        ),
    };
    RuleDefinition {
        id: format!("SYNTH-{num}"),
        category: RuleDeckCategory::Synth,
        number: num,
        default_severity: sev,
        title: title.to_string(),
        ug_reference: "UG949",
        description: desc.to_string(),
        tcl_remediation_template: tcl.to_string(),
        rtl_remediation_hint: rtl.to_string(),
    }
}

fn methodology_rule(num: u16) -> RuleDefinition {
    let (sev, title, desc, tcl, rtl) = match num {
        1 => (
            DrcSeverity::Warning,
            "Synchronous reset preference (UG949)",
            "UltraFast Design Methodology recommends active-high synchronous resets for CLB flip-flops, DSPs, and BRAMs.",
            "# Review reset topology on {<OBJ>}",
            "Convert `always_ff @(posedge clk or posedge rst)` to synchronous `always_ff @(posedge clk)` where possible.",
        ),
        2 => (
            DrcSeverity::Warning,
            "Control set proliferation",
            "Design uses excessive unique (CLK, CE, SR) control sets, restricting CLB BLE packing density.",
            "set_property CONTROL_SET_THRESHOLD 8 [current_design]",
            "Avoid narrow (<4 bit) registers with unique clock-enable or reset signals; fold enables into LUT inputs.",
        ),
        3 => (
            DrcSeverity::Warning,
            "High-fanout synchronous reset or enable net",
            "Control net fans out to >64 endpoints without a register replication tree.",
            "set_property MAX_FANOUT 32 [get_nets {<OBJ>}]",
            "Add a 2-stage pipeline tree on global synchronous reset and clock-enable signals.",
        ),
        4 => (
            DrcSeverity::Advisory,
            "Unregistered top-level input port",
            "Input port drives deep combinational logic directly without an input IOB register.",
            "set_property IOB TRUE [get_ports {<OBJ>}]",
            "Register top-level inputs immediately inside the boundary module to pack into IOB flip-flops.",
        ),
        5 => (
            DrcSeverity::Warning,
            "Unregistered top-level output port",
            "Output IOB is driven by combinational LUT logic instead of a dedicated flip-flop Q.",
            "set_property IOB TRUE [get_ports {<OBJ>}]",
            "Drive top-level output ports from a dedicated flip-flop without fanout to internal logic.",
        ),
        6 => (
            DrcSeverity::Advisory,
            "BRAM power-gating enable tied high",
            "Block RAM `ENARDEN` / `ENBWREN` is permanently active instead of asserted only on read/write cycles.",
            "set_property RAM_DECOMP power [get_cells {<OBJ>}]",
            "Drive BRAM enable pins with qualified read/write valid strobes to reduce dynamic BRAM power.",
        ),
        7 => (
            DrcSeverity::Advisory,
            "DSP48E1 input pipeline registers (AREG/BREG) omitted",
            "DSP48E1 A/B inputs are driven directly by fabric routing without AREG/BREG.",
            "set_property AREG 1 [get_cells {<OBJ>}]\nset_property BREG 1 [get_cells {<OBJ>}]",
            "Register multiplier operands before the multiply operation so AREG/BREG pack inside DSP48E1.",
        ),
        8 => (
            DrcSeverity::Warning,
            "Unconstrained package pin location (LOC)",
            "Top-level port lacks an explicit package pin `LOC` constraint prior to bitstream generation.",
            "set_property PACKAGE_PIN IOB_X2Y0 [get_ports {<OBJ>}]",
            "Assign all top-level ports to physical package pins in the project XDC file.",
        ),
        9 => (
            DrcSeverity::Warning,
            "Default IOSTANDARD relied upon without explicit XDC property",
            "Top-level port uses implicit default IOSTANDARD instead of an explicit board voltage standard.",
            "set_property IOSTANDARD LVCMOS33 [get_ports {<OBJ>}]",
            "Specify explicit IOSTANDARD constraints matching the board schematic bank VCCO.",
        ),
        10 => (
            DrcSeverity::Advisory,
            "Excessive DONT_TOUCH attributes inhibiting optimization",
            "Multiple logic cells carry DONT_TOUCH, preventing LUT consolidation and replication.",
            "set_property DONT_TOUCH FALSE [get_cells {<OBJ>}]",
            "Use DONT_TOUCH or mark_debug only on nets/cells actively needed for hardware probing.",
        ),
        _ => (
            DrcSeverity::Advisory,
            "UG949 UltraFast Design Methodology guideline",
            "Design hierarchy, floorplanning, or XDC constraint ordering check from UG949.",
            "report_methodology -name methodology_#NUM\n# Inspect {<OBJ>}",
            "Follow UG949 UltraFast Design Methodology closure guidelines for the flagged object.",
        ),
    };
    RuleDefinition {
        id: format!("METHODOLOGY-{num}"),
        category: RuleDeckCategory::Methodology,
        number: num,
        default_severity: sev,
        title: title.to_string(),
        ug_reference: "UG949",
        description: desc.to_string(),
        tcl_remediation_template: tcl.replace("#NUM", &num.to_string()),
        rtl_remediation_hint: rtl.to_string(),
    }
}

fn cdc_rule(num: u16) -> RuleDefinition {
    let (sev, title, desc, tcl, rtl) = match num {
        1 => (
            DrcSeverity::Error,
            "1-bit unsynchronized clock domain crossing",
            "Single-bit signal crosses between distinct clock domains without a multi-stage synchronizer.",
            "set_property ASYNC_REG TRUE [get_cells {<OBJ>}]\nset_false_path -to [get_pins {<OBJ>/D}]",
            "Synchronize 1-bit control signals using a 2-stage or 3-stage flip-flop chain with (* ASYNC_REG = \"TRUE\" *).",
        ),
        2 => (
            DrcSeverity::Warning,
            "1-bit synchronized CDC missing ASYNC_REG property",
            "Back-to-back synchronizer flip-flops cross clock domains but lack ASYNC_REG placement constraints.",
            "set_property ASYNC_REG TRUE [get_cells {<OBJ>}]",
            "Add `(* ASYNC_REG = \"TRUE\" *)` to all synchronizer stages so the placer co-locates them in one CLB slice.",
        ),
        3 => (
            DrcSeverity::Warning,
            "1-bit CDC with combinational logic before synchronizer",
            "A combinational LUT sits between the source flip-flop and the destination synchronizer D pin, risking glitches.",
            "set_property DONT_TOUCH TRUE [get_cells {<OBJ>}]",
            "Register the signal in the source clock domain immediately before crossing into the destination synchronizer.",
        ),
        4 => (
            DrcSeverity::Error,
            "Multi-bit bus unsynchronized clock domain crossing",
            "Coherent multi-bit bus crosses clock domains without Gray coding, FIFO, or req/ack handshake.",
            "set_bus_skew -from [get_cells {<OBJ>}] 2.000\nset_max_delay -datapath_only 5.000 -from [get_cells {<OBJ>}]",
            "Use a Gray-coded pointer + dual-clock BRAM FIFO or a CE-mux handshake for multi-bit bus crossings.",
        ),
        5 => (
            DrcSeverity::Warning,
            "Multi-bit CDC synchronized with independent 2-FF chains",
            "Multi-bit bus bits are individually double-flopped, allowing bit-skew incoherency at the receiver.",
            "set_bus_skew 1.000 [get_nets {<OBJ>}]",
            "Convert binary counters to Gray code before crossing or use an asynchronous FIFO.",
        ),
        6 => (
            DrcSeverity::Warning,
            "Clock domain crossing convergence",
            "Multiple synchronized CDC paths reconverge in a downstream combinational LUT, reintroducing skew hazards.",
            "# Inspect reconvergent CDC cone at {<OBJ>}",
            "Synchronize a single control strobe or Gray-coded word rather than reconverging separate synchronized signals.",
        ),
        10 => (
            DrcSeverity::Error,
            "Combinational logic on CDC path",
            "Cross-clock path traverses combinational LUT6 logic between launch and capture domains.",
            "set_false_path -through [get_cells {<OBJ>}]",
            "Drive the CDC crossing directly from a launch-domain flip-flop Q into the capture-domain synchronizer D.",
        ),
        _ => (
            DrcSeverity::Warning,
            "UG906 Clock Domain Crossing structural check",
            "Clock domain crossing topology requires synchronization or timing exception qualification.",
            "report_cdc -details -name cdc_#NUM\nset_property ASYNC_REG TRUE [get_cells {<OBJ>}]",
            "Verify CDC synchronizer topology and apply ASYNC_REG and appropriate timing exceptions.",
        ),
    };
    RuleDefinition {
        id: format!("CDC-{num}"),
        category: RuleDeckCategory::Cdc,
        number: num,
        default_severity: sev,
        title: title.to_string(),
        ug_reference: "UG906",
        description: desc.to_string(),
        tcl_remediation_template: tcl.replace("#NUM", &num.to_string()),
        rtl_remediation_hint: rtl.to_string(),
    }
}

fn pdrc_rule(num: u16) -> RuleDefinition {
    let (sev, title, desc, tcl, rtl) = match num {
        1 => (
            DrcSeverity::Error,
            "CLB BLE site over-occupancy conflict",
            "More than one LUTFF cluster is assigned to the same physical (CLB_XxYy, BLE) slot.",
            "place_design -directive Explore\n# Resolve site conflict at {<OBJ>}",
            "Remove conflicting LOC/BEL physical constraints on CLB cells.",
        ),
        2 => (
            DrcSeverity::Error,
            "IOB bank VCCO voltage conflict",
            "Multiple ports placed in the same I/O bank require incompatible VCCO supply voltages.",
            "set_property IOSTANDARD LVCMOS33 [get_ports {<OBJ>}]",
            "Group ports with identical VCCO requirements into the same physical I/O bank.",
        ),
        3 => (
            DrcSeverity::Warning,
            "Routing switchbox congestion / overused tile",
            "PathFinder routing left one or more switchbox resources over-subscribed.",
            "route_design -directive Explore",
            "Reduce local interconnect congestion by pipelining high-fanout nets or relaxing tight pblocks.",
        ),
        4 => (
            DrcSeverity::Warning,
            "CARRY4 vertical column boundary break",
            "Carry chain crosses a CLB column boundary without dedicated CIN/COUT cascade routing.",
            "# Constrain carry chain {<OBJ>} within a single CLB column",
            "Keep single-cycle arithmetic widths <= 32 bits per stage.",
        ),
        _ => (
            DrcSeverity::Advisory,
            "Physical DRC site and routing legality check",
            "Physical placement/routing rule check across CLB, BRAM, DSP, and IOB tiles.",
            "report_drc -ruledecks {default} -name pdrc_#NUM",
            "Verify physical placement and routing constraints for the flagged site.",
        ),
    };
    RuleDefinition {
        id: format!("PDRC-{num}"),
        category: RuleDeckCategory::Pdrc,
        number: num,
        default_severity: sev,
        title: title.to_string(),
        ug_reference: "UG953",
        description: desc.to_string(),
        tcl_remediation_template: tcl.replace("#NUM", &num.to_string()),
        rtl_remediation_hint: rtl.to_string(),
    }
}

fn reqp_rule(num: u16) -> RuleDefinition {
    let (sev, title, desc, tcl, rtl) = match num {
        1 => (
            DrcSeverity::Error,
            "HFF sequential flip-flop missing D input connection",
            "Flip-flop primitive `Hff` has no net connected to its required `D` data input pin.",
            "# Connect D pin on cell {<OBJ>} or remove unused flip-flop",
            "Ensure the flip-flop has a valid data driver in the netlist.",
        ),
        2 => (
            DrcSeverity::Error,
            "HFF sequential flip-flop missing CLK input connection",
            "Flip-flop primitive `Hff` has no net connected to its required `CLK` clock input pin.",
            "connect_net -net [get_nets clk] -objects [get_pins {<OBJ>/CLK}]",
            "Connect the flip-flop clock input to the design clock network.",
        ),
        5 => (
            DrcSeverity::Warning,
            "LUT6 primitive has no connected input pins",
            "Look-Up Table `Lut6` has none of `I0..I5` connected, acting only as a constant generator.",
            "# Tie constant net directly or connect inputs on {<OBJ>}",
            "Verify LUT inputs were not accidentally left unconnected during netlist stitching.",
        ),
        10 => (
            DrcSeverity::Error,
            "RAMB18E1 missing required port clock (CLKARDCLK / CLKBWRCLK)",
            "Block RAM primitive `Bram18e1` is instantiated without a clock net on `CLKARDCLK`.",
            "connect_net -net [get_nets clk] -objects [get_pins {<OBJ>/CLKARDCLK}]",
            "Connect both active port clocks on RAMB18E1 primitives.",
        ),
        15 => (
            DrcSeverity::Warning,
            "DSP48E1 P output bus unconnected",
            "DSP48E1 primitive has no load connected to its `P` output pins.",
            "# Connect P output bus on {<OBJ>} or remove unused DSP48E1",
            "Wire the DSP48E1 `P` output bus to downstream logic.",
        ),
        20 => (
            DrcSeverity::Error,
            "IOB primitive missing required I or PAD pin connection",
            "Output buffer `IobOut` must connect both internal driver pin `I` and external port `PAD`.",
            "connect_net -net [get_nets {<OBJ>}] -objects [get_pins {<OBJ>/I}]",
            "Connect the IOB buffer between the internal driver net and the top-level port.",
        ),
        _ => (
            DrcSeverity::Warning,
            "Primitive required pin connectivity check",
            "Site primitive pin connectivity requirement per UG953 library specification.",
            "# Check required pin connections on cell {<OBJ>}",
            "Connect all mandatory primitive pins according to the Helion primitive guide.",
        ),
    };
    RuleDefinition {
        id: format!("REQP-{num}"),
        category: RuleDeckCategory::Reqp,
        number: num,
        default_severity: sev,
        title: title.to_string(),
        ug_reference: "UG953",
        description: desc.to_string(),
        tcl_remediation_template: tcl.to_string(),
        rtl_remediation_hint: rtl.to_string(),
    }
}

fn aval_rule(num: u16) -> RuleDefinition {
    let (sev, title, desc, tcl, rtl) = match num {
        1 => (
            DrcSeverity::Error,
            "RAMB18E1 illegal READ_WIDTH / WRITE_WIDTH attribute value",
            "Block RAM `Bram18e1` port width must be one of {0, 1, 2, 4, 9, 18, 36}.",
            "set_property READ_WIDTH_A 18 [get_cells {<OBJ>}]\nset_property WRITE_WIDTH_A 18 [get_cells {<OBJ>}]",
            "Configure RAMB18E1 port data widths to legal 7-series values (1, 2, 4, 9, 18, or 36).",
        ),
        5 => (
            DrcSeverity::Error,
            "DSP48E1 illegal OPMODE or ALUMODE encoding",
            "DSP48E1 7-bit `opmode` (`<= 0x7F`) or 4-bit `alumode` (`<= 0x0F`) is outside hardware bounds.",
            "set_property OPMODE 7'b0000101 [get_cells {<OBJ>}]\nset_property ALUMODE 4'b0000 [get_cells {<OBJ>}]",
            "Use legal UG479 7-bit OPMODE and 4-bit ALUMODE encodings on DSP48E1.",
        ),
        10 => (
            DrcSeverity::Advisory,
            "Degenerate constant LUT6 INIT table",
            "LUT6 `INIT` is all-zeros (`0x0`) or all-ones (`0xFFFF_FFFF_FFFF_FFFF`), ignoring all inputs.",
            "# Constant-propagate LUT {<OBJ>} in opt_design",
            "Run LUT constant propagation (`Design::opt_luts`) to simplify constant LUTs.",
        ),
        _ => (
            DrcSeverity::Warning,
            "Primitive architecture attribute value legality check",
            "Cell or port attribute value violates hardware feature map encoding constraints.",
            "# Reset illegal property on {<OBJ>}",
            "Ensure primitive parameters and attributes use HAD-supported values.",
        ),
    };
    RuleDefinition {
        id: format!("AVAL-{num}"),
        category: RuleDeckCategory::Aval,
        number: num,
        default_severity: sev,
        title: title.to_string(),
        ug_reference: "UG953",
        description: desc.to_string(),
        tcl_remediation_template: tcl.to_string(),
        rtl_remediation_hint: rtl.to_string(),
    }
}

fn plio_rule(id: &str) -> Option<RuleDefinition> {
    let (sev, title, desc, tcl, rtl) = match id {
        "PLACE-1" => (
            DrcSeverity::Error,
            "Unplaced LUTFF cluster count mismatch",
            "Number of placed LUTFF site coordinates does not match packed LUTFF cluster count.",
            "place_design",
            "Re-run placement so every packed LUTFF cluster receives a valid site.",
        ),
        "PLACE-2" => (
            DrcSeverity::Error,
            "LUT6 utilization exceeds device capacity",
            "Packed LUTFF count exceeds total LUT6 sites available on the target part.",
            "# Target a larger part or optimize LUT area",
            "Share datapath operators or enable LUT optimization to fit the target device.",
        ),
        "PLACE-3" => (
            DrcSeverity::Error,
            "BLE index out of CLB range",
            "Placed BLE index exceeds `dev.n_ble` for the target CLB tile.",
            "place_design",
            "Ensure manual BEL constraints use valid BLE indices (`0..7`).",
        ),
        "PLACE-4" => (
            DrcSeverity::Error,
            "Duplicate CLB BLE placement collision",
            "Two or more LUTFF clusters are placed on the exact same `(CLB_XxYy, BLE)` resource.",
            "place_design",
            "Remove duplicate BEL/LOC placement constraints.",
        ),
        "CLK-1" => (
            DrcSeverity::Error,
            "Missing clock on registered design",
            "Design contains sequential flip-flops (`Hff`) but has no clock port or `CLK` pin net.",
            "create_clock -period 10.000 [get_ports {clk}]",
            "Add a top-level clock input port and connect it to all sequential `CLK` pins.",
        ),
        "IOSTD-1" => (
            DrcSeverity::Error,
            "Unsupported IOSTANDARD attribute",
            "Port `IOSTANDARD` is not in the target device's HAD legal I/O standard table.",
            "set_property IOSTANDARD LVCMOS33 [get_ports {<OBJ>}]",
            "Select a supported I/O standard (e.g. LVCMOS33, LVCMOS25, LVCMOS18, SSTL15).",
        ),
        "IOSTD-2" => (
            DrcSeverity::Error,
            "Incompatible VCCO standards mixed in one I/O bank",
            "Ports assigned to the same I/O bank require conflicting VCCO voltages.",
            "set_property IOSTANDARD LVCMOS33 [get_ports {<OBJ>}]",
            "Ensure all ports in the same I/O bank share the same VCCO voltage requirement.",
        ),
        "DRIVE-1" => (
            DrcSeverity::Error,
            "Illegal DRIVE strength value",
            "Port `DRIVE` mA value is not a valid HAD drive strength.",
            "set_property DRIVE 12 [get_ports {<OBJ>}]",
            "Use a supported drive strength (4, 8, 12, 16, or 24 mA depending on IOSTANDARD).",
        ),
        "DRIVE-2" => (
            DrcSeverity::Error,
            "DRIVE strength incompatible with port IOSTANDARD",
            "Requested `DRIVE` strength is not supported by the port's `IOSTANDARD`.",
            "set_property DRIVE 12 [get_ports {<OBJ>}]",
            "Reduce DRIVE to <= 12 mA for LVCMOS18 or switch to LVCMOS25/LVCMOS33.",
        ),
        "SLEW-1" => (
            DrcSeverity::Error,
            "Illegal SLEW rate value",
            "Port `SLEW` must be `SLOW` or `FAST`.",
            "set_property SLEW SLOW [get_ports {<OBJ>}]",
            "Set SLEW property to SLOW or FAST.",
        ),
        "PULL-1" => (
            DrcSeverity::Error,
            "Illegal PULLTYPE attribute value",
            "Port `PULLTYPE` must be `NONE`, `PULLUP`, `PULLDOWN`, or `KEEPER`.",
            "set_property PULLTYPE NONE [get_ports {<OBJ>}]",
            "Set PULLTYPE to NONE, PULLUP, PULLDOWN, or KEEPER.",
        ),
        "DIFF-1" => (
            DrcSeverity::Error,
            "Illegal DIFF_TERM attribute value",
            "Port `DIFF_TERM` must be `TRUE` or `FALSE`.",
            "set_property DIFF_TERM FALSE [get_ports {<OBJ>}]",
            "Set DIFF_TERM to TRUE or FALSE.",
        ),
        "DIFF-2" => (
            DrcSeverity::Error,
            "DIFF_TERM not supported for single-ended IOSTANDARD",
            "Internal differential termination requires a differential/SSTL I/O standard.",
            "set_property DIFF_TERM FALSE [get_ports {<OBJ>}]",
            "Disable DIFF_TERM on single-ended LVCMOS ports.",
        ),
        "INTERM-1" => (
            DrcSeverity::Error,
            "Illegal IN_TERM attribute value",
            "Port `IN_TERM` must be `NONE` or `UNTUNED_SPLIT_40|50|60`.",
            "set_property IN_TERM NONE [get_ports {<OBJ>}]",
            "Set IN_TERM to NONE or a valid UNTUNED_SPLIT_* value.",
        ),
        "INTERM-2" => (
            DrcSeverity::Error,
            "IN_TERM not legal for port IOSTANDARD",
            "Split input termination requires SSTL15 or compatible memory interface standard.",
            "set_property IN_TERM NONE [get_ports {<OBJ>}]",
            "Set IN_TERM to NONE on LVCMOS ports or use SSTL15.",
        ),
        "ROUTE-1" => (
            DrcSeverity::Warning,
            "Unrouted IOB pad",
            "Packed IOB has no routed source connection in the switch fabric.",
            "route_design",
            "Ensure all IOB cells are driven by routable fabric resources.",
        ),
        "ROUTE-2" => (
            DrcSeverity::Warning,
            "Overused routing tiles after PathFinder",
            "Router completed with one or more congested/overused routing nodes.",
            "route_design -directive Explore",
            "Reduce routing congestion around hot CLB tiles.",
        ),
        "ROUTE-3" => (
            DrcSeverity::Warning,
            "IOB source net has no LUT/FF driver",
            "Output IOB `from_net` is not driven by any packed LUT or FF output.",
            "# Connect driver to IOB {<OBJ>}",
            "Drive top-level output ports from a valid internal LUT or flip-flop.",
        ),
        "DSP-1" | "BRAM-1" | "IOB-1" => (
            DrcSeverity::Error,
            "Site placement count mismatch",
            "Placed site list length does not match packed primitive count.",
            "place_design",
            "Re-run placement for all packed DSP/BRAM/IOB primitives.",
        ),
        _ => return None,
    };
    Some(RuleDefinition {
        id: id.to_string(),
        category: RuleDeckCategory::PlacementIo,
        number: 1,
        default_severity: sev,
        title: title.to_string(),
        ug_reference: "UG893",
        description: desc.to_string(),
        tcl_remediation_template: tcl.to_string(),
        rtl_remediation_hint: rtl.to_string(),
    })
}

/// Look up any Vivado UG906 / UG949 / UG893 rule definition by ID
/// (`TIMING-1..50`, `SYNTH-1..15`, `METHODOLOGY-1..25`, `CDC-1..20`,
/// `PDRC-1..20`, `REQP-1..30`, `AVAL-1..20`, or `IOSTD-*`/`PLACE-*`/`ROUTE-*`).
pub fn lookup_rule(id: &str) -> Option<RuleDefinition> {
    let norm = id.trim().to_ascii_uppercase();
    if let Some(r) = plio_rule(&norm) {
        return Some(r);
    }
    let (prefix, num_str) = norm.split_once('-')?;
    let num: u16 = num_str.parse().ok()?;
    match prefix {
        "TIMING" if (1..=50).contains(&num) => Some(timing_rule(num)),
        "SYNTH" if (1..=15).contains(&num) => Some(synth_rule(num)),
        "METHODOLOGY" if (1..=25).contains(&num) => Some(methodology_rule(num)),
        "CDC" if (1..=20).contains(&num) => Some(cdc_rule(num)),
        "PDRC" if (1..=20).contains(&num) => Some(pdrc_rule(num)),
        "REQP" if (1..=30).contains(&num) => Some(reqp_rule(num)),
        "AVAL" if (1..=20).contains(&num) => Some(aval_rule(num)),
        _ => None,
    }
}

/// Return all rule definitions belonging to `category`.
pub fn rules_in_deck(category: RuleDeckCategory) -> Vec<RuleDefinition> {
    match category {
        RuleDeckCategory::Timing => (1..=50).map(timing_rule).collect(),
        RuleDeckCategory::Synth => (1..=15).map(synth_rule).collect(),
        RuleDeckCategory::Methodology => (1..=25).map(methodology_rule).collect(),
        RuleDeckCategory::Cdc => (1..=20).map(cdc_rule).collect(),
        RuleDeckCategory::Pdrc => (1..=20).map(pdrc_rule).collect(),
        RuleDeckCategory::Reqp => (1..=30).map(reqp_rule).collect(),
        RuleDeckCategory::Aval => (1..=20).map(aval_rule).collect(),
        RuleDeckCategory::PlacementIo => [
            "PLACE-1", "PLACE-2", "PLACE-3", "PLACE-4", "CLK-1", "DSP-1", "BRAM-1", "IOB-1",
            "IOSTD-1", "IOSTD-2", "DRIVE-1", "DRIVE-2", "SLEW-1", "PULL-1", "DIFF-1", "DIFF-2",
            "INTERM-1", "INTERM-2", "ROUTE-1", "ROUTE-2", "ROUTE-3",
        ]
        .into_iter()
        .filter_map(plio_rule)
        .collect(),
    }
}

/// Return the complete Vivado UG906 / UG949 rule taxonomy catalog
/// (`50 + 15 + 25 + 20 + 20 + 30 + 20 = 180` numbered deck rules + 21 physical/IO rules = 201 rules).
pub fn all_ug906_ug949_rules() -> Vec<RuleDefinition> {
    let mut out = Vec::with_capacity(201);
    for cat in [
        RuleDeckCategory::Timing,
        RuleDeckCategory::Synth,
        RuleDeckCategory::Methodology,
        RuleDeckCategory::Cdc,
        RuleDeckCategory::Pdrc,
        RuleDeckCategory::Reqp,
        RuleDeckCategory::Aval,
        RuleDeckCategory::PlacementIo,
    ] {
        out.extend(rules_in_deck(cat));
    }
    out
}
