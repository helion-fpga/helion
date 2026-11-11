//! Built-in demo waveform: a fixture for headless scenes and view tests.
//!
//! Production never renders these shapes: the Wave pane shows an honest
//! empty state until a real simulation imports a VCD. Scenes and tests that
//! exercise the demo lanes (`wave`, `wave-cursors`) opt in through
//! [`seed_demo_wave`], which flips [`super::wave::WaveState::demo`].
//!
//! [`sample`] draws VCD-like shapes on a uniform 96-sample grid (zero-order
//! hold between events, the `vcd_wave.rs` convention) and
//! [`WaveSigId::VbLed`] packs its members LSB-first like the old
//! `rebuild_virtual_buses`. The static [`SimScope`]/[`objects`] hierarchy
//! backs the demo Scope/Objects panels.

use crate::state::AppState;
use crate::views::wave::WaveRadix;

/// Samples on the demo VCD grid (`vcd_wave.rs` uniform-grid convention).
pub const SAMPLES: usize = 96;

/// Opt the Wave pane into the demo fixture: demo lanes, static scopes, the
/// old default cursor, and VCD dims for the 1 ns demo grid.
pub fn seed_demo_wave(s: &mut AppState) {
    s.wave.demo = true;
    s.wave.cursor = 48;
    s.vcd.timescale_ps = 1000;
    s.vcd.duration_ps = 96_000;
    s.vcd.signals = WaveSigId::DEFAULT
        .iter()
        .map(|id| id.name().to_string())
        .collect();
}

/// Every trace the wave panel can show. Scalars draw stepped levels, buses
/// draw the Vivado envelope; [`WaveSigId::VbLed`] is the virtual bus packed
/// from its members LSB-first (mirrors `rebuild_virtual_buses`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveSigId {
    Clk,
    RstN,
    DivClk,
    Counter,
    Led,
    Seg,
    An,
    Done,
    VbLed,
}

impl WaveSigId {
    pub const ALL: [WaveSigId; 9] = [
        WaveSigId::Clk,
        WaveSigId::RstN,
        WaveSigId::DivClk,
        WaveSigId::Counter,
        WaveSigId::Led,
        WaveSigId::Seg,
        WaveSigId::An,
        WaveSigId::Done,
        WaveSigId::VbLed,
    ];

    /// Traces mounted before any `add_wave`.
    pub const DEFAULT: [WaveSigId; 6] = [
        WaveSigId::Clk,
        WaveSigId::RstN,
        WaveSigId::DivClk,
        WaveSigId::Counter,
        WaveSigId::Led,
        WaveSigId::VbLed,
    ];

    /// Virtual-bus members, LSB first (mirrors `rebuild_virtual_buses`).
    pub const VB_MEMBERS: [WaveSigId; 2] = [WaveSigId::Led, WaveSigId::DivClk];

    pub fn name(self) -> &'static str {
        match self {
            WaveSigId::Clk => "clk",
            WaveSigId::RstN => "rst_n",
            WaveSigId::DivClk => "divclk",
            WaveSigId::Counter => "counter[7:0]",
            WaveSigId::Led => "led[3:0]",
            WaveSigId::Seg => "seg[7:0]",
            WaveSigId::An => "an[3:0]",
            WaveSigId::Done => "done",
            WaveSigId::VbLed => "vb_led[4:0]",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            WaveSigId::Clk => "clk",
            WaveSigId::RstN => "rst-n",
            WaveSigId::DivClk => "divclk",
            WaveSigId::Counter => "counter",
            WaveSigId::Led => "led",
            WaveSigId::Seg => "seg",
            WaveSigId::An => "an",
            WaveSigId::Done => "done",
            WaveSigId::VbLed => "vb-led",
        }
    }

    pub fn width(self) -> u8 {
        match self {
            WaveSigId::Clk | WaveSigId::RstN | WaveSigId::DivClk | WaveSigId::Done => 1,
            WaveSigId::Counter | WaveSigId::Seg => 8,
            WaveSigId::Led | WaveSigId::An => 4,
            WaveSigId::VbLed => 5,
        }
    }

    pub fn is_bus(self) -> bool {
        self.width() > 1
    }

    pub fn name_key(self) -> String {
        format!("wave-name-{}", self.slug())
    }

    pub fn value_key(self) -> String {
        format!("wave-value-{}", self.slug())
    }

    pub fn lane_key(self) -> String {
        format!("wave-lane-{}", self.slug())
    }
}

/// Demo VCD shape: the sample value of `id` at grid index `i`, zero-order
/// hold between the demo events (`vcd_wave.rs` sampling convention).
pub fn sample(id: WaveSigId, i: usize) -> u64 {
    let i = (i.min(SAMPLES.saturating_sub(1))) as u64;
    match id {
        WaveSigId::Clk => i % 2,
        WaveSigId::RstN => u64::from(i >= 8),
        WaveSigId::DivClk => (i / 16) % 2,
        WaveSigId::Counter => (i / 2) & 0xFF,
        WaveSigId::Led => (i / 8) & 0xF,
        WaveSigId::Seg => (i / 4) & 0xFF,
        WaveSigId::An => (i / 32) & 0xF,
        WaveSigId::Done => u64::from(i >= 88),
        WaveSigId::VbLed => pack_virtual(&WaveSigId::VB_MEMBERS, i as usize),
    }
}

/// Pack virtual-bus members LSB-first, masking each to its width (mirrors
/// `Waveform::rebuild_virtual_buses` in `ide.rs`).
pub fn pack_virtual(members: &[WaveSigId], i: usize) -> u64 {
    let mut val = 0u64;
    let mut bit = 0u32;
    for m in members {
        let w = u32::from(m.width().max(1));
        let mask = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
        val |= (sample(*m, i) & mask) << bit;
        bit = bit.saturating_add(w);
    }
    val
}

/// Value of `id` at grid index `i` in `radix` (scalars stay `0`/`1`).
pub fn format_value(id: WaveSigId, i: usize, radix: WaveRadix) -> String {
    radix.format(sample(id, i), id.width())
}

/// Constant-value runs of a bus trace: (start, end-exclusive, value).
/// Scalars return one full-width run; used for the envelope labels.
pub fn bus_runs(id: WaveSigId) -> Vec<(usize, usize, u64)> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut cur = sample(id, 0);
    for i in 1..SAMPLES {
        let v = sample(id, i);
        if v != cur {
            runs.push((start, i, cur));
            start = i;
            cur = v;
        }
    }
    runs.push((start, SAMPLES, cur));
    runs
}

/// Simulation scope in the Scope panel (Vivado sim hierarchy).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SimScope {
    /// Top testbench scope.
    #[default]
    Tb,
    /// DUT instance scope.
    Dut,
    /// Clock-generator scope.
    ClkGen,
}

impl SimScope {
    pub const ALL: [SimScope; 3] = [SimScope::Tb, SimScope::Dut, SimScope::ClkGen];

    pub fn name(self) -> &'static str {
        match self {
            SimScope::Tb => "tb_project1",
            SimScope::Dut => "dut",
            SimScope::ClkGen => "clk_gen",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            SimScope::Tb => "testbench top",
            SimScope::Dut => "project1_demo instance",
            SimScope::ClkGen => "clock generator",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            SimScope::Tb => "wave-scope-tb",
            SimScope::Dut => "wave-scope-dut",
            SimScope::ClkGen => "wave-scope-clkgen",
        }
    }
}

/// One row of the Objects panel: name, data type, and the wave signal it
/// adds (instances and constants have none).
#[derive(Clone, Copy, Debug)]
pub struct SimObject {
    pub name: &'static str,
    pub slug: &'static str,
    pub dtype: &'static str,
    pub sig: Option<WaveSigId>,
    /// Fixed display value (instances, parameters); `None` reads the cursor.
    pub fixed: Option<&'static str>,
}

/// Objects of a scope, in Vivado panel order.
pub fn objects(scope: SimScope) -> &'static [SimObject] {
    match scope {
        SimScope::Tb => &[
            SimObject {
                name: "clk",
                slug: "clk",
                dtype: "wire",
                sig: Some(WaveSigId::Clk),
                fixed: None,
            },
            SimObject {
                name: "rst_n",
                slug: "rst-n",
                dtype: "reg",
                sig: Some(WaveSigId::RstN),
                fixed: None,
            },
            SimObject {
                name: "done",
                slug: "done",
                dtype: "wire",
                sig: Some(WaveSigId::Done),
                fixed: None,
            },
            SimObject {
                name: "dut",
                slug: "dut",
                dtype: "project1_demo",
                sig: None,
                fixed: Some("-"),
            },
            SimObject {
                name: "clk_gen",
                slug: "clk-gen",
                dtype: "module",
                sig: None,
                fixed: Some("-"),
            },
        ],
        SimScope::Dut => &[
            SimObject {
                name: "clk",
                slug: "clk",
                dtype: "input",
                sig: Some(WaveSigId::Clk),
                fixed: None,
            },
            SimObject {
                name: "rst_n",
                slug: "rst-n",
                dtype: "input",
                sig: Some(WaveSigId::RstN),
                fixed: None,
            },
            SimObject {
                name: "divclk",
                slug: "divclk",
                dtype: "reg",
                sig: Some(WaveSigId::DivClk),
                fixed: None,
            },
            SimObject {
                name: "counter[7:0]",
                slug: "counter",
                dtype: "reg [7:0]",
                sig: Some(WaveSigId::Counter),
                fixed: None,
            },
            SimObject {
                name: "led[3:0]",
                slug: "led",
                dtype: "output [3:0]",
                sig: Some(WaveSigId::Led),
                fixed: None,
            },
            SimObject {
                name: "seg[7:0]",
                slug: "seg",
                dtype: "output [7:0]",
                sig: Some(WaveSigId::Seg),
                fixed: None,
            },
            SimObject {
                name: "an[3:0]",
                slug: "an",
                dtype: "output [3:0]",
                sig: Some(WaveSigId::An),
                fixed: None,
            },
        ],
        SimScope::ClkGen => &[
            SimObject {
                name: "clk",
                slug: "clk",
                dtype: "reg",
                sig: Some(WaveSigId::Clk),
                fixed: None,
            },
            SimObject {
                name: "half_period",
                slug: "half-period",
                dtype: "integer",
                sig: None,
                fixed: Some("5 ns"),
            },
        ],
    }
}

/// Fixture state-name source for the FSM decoder scenes/tests (the scan
/// reads `localparam` groups exactly as it would from a real buffer).
pub const PROTOCOL_FIXTURE_HDL: &str = "module bus_ctrl;\n\
    localparam [1:0] IDLE = 2'd0, LOAD = 2'd1, SHIFT = 2'd2, DONE = 2'd3;\n\
    endmodule\n";

/// Protocol fixture for the decoder/analog scenes and tests: a VCD (run
/// through the real [`crate::views::wave::import_vcd_text`] parser) with
/// SPI, UART, AXI-Lite, an FSM state register, bit-blasted counter bits,
/// and a sampled sine on a signed bus. Never mounted in production.
pub fn protocol_fixture() -> crate::views::wave::LiveWave {
    const N: usize = 240;
    let mut sig: Vec<(&str, u8, Vec<u64>)> = Vec::new();
    let mut push = |name: &'static str, w: u8, v: Vec<u64>| sig.push((name, w, v));
    let clk: Vec<u64> = (0..N).map(|i| (i % 2) as u64).collect();
    // SPI mode 0, 4-sample SCLK period, two frames.
    let (mut sclk, mut mosi, mut miso, mut csn) =
        (vec![0u64; N], vec![0u64; N], vec![0u64; N], vec![1u64; N]);
    for (f, (tx, rx)) in [(0xA5u8, 0x5Au8), (0x3C, 0xC3)].iter().enumerate() {
        let base = 8 + f * 48;
        for i in base - 2..base + 34 {
            csn[i] = 0;
        }
        for b in 0..8 {
            let s0 = base + b * 4;
            let (mo, mi) = (u64::from((tx >> (7 - b)) & 1), u64::from((rx >> (7 - b)) & 1));
            for k in 0..4 {
                mosi[s0 + k] = mo;
                miso[s0 + k] = mi;
                sclk[s0 + k] = u64::from(k >= 2);
            }
        }
    }
    // UART 8N1, 6 samples per bit: "OK".
    let mut tx = vec![1u64; N];
    let mut at = 110;
    for byte in *b"OK" {
        let bits = std::iter::once(0)
            .chain((0..8).map(|i| u64::from((byte >> i) & 1)))
            .chain(std::iter::once(1));
        for b in bits {
            for k in 0..6 {
                tx[at + k] = b;
            }
            at += 6;
        }
        at += 6;
    }
    // AXI-Lite: write 0x10 <- 0xBEEF at 20 (B at 38), read 0x14 -> 0x1234
    // at 60 (R at 80).
    let pulse = |i: usize, len: usize| -> Vec<u64> {
        (0..N).map(|k| u64::from(k >= i && k < i + len)).collect()
    };
    let hold = |from: usize, v: u64| -> Vec<u64> {
        (0..N).map(|k| if k >= from { v } else { 0 }).collect()
    };
    // FSM: IDLE, LOAD, SHIFT, DONE, IDLE ...
    let state: Vec<u64> = (0..N)
        .map(|i| match i {
            0..=19 => 0,
            20..=39 => 1,
            40..=139 => 2,
            140..=169 => 3,
            _ => 0,
        })
        .collect();
    let cnt: Vec<u64> = (0..N).map(|i| ((i / 8) % 16) as u64).collect();
    let dac: Vec<u64> = (0..N)
        .map(|i| {
            let v = (64.0 * (i as f64 * std::f64::consts::TAU / 120.0).sin()).round() as i64;
            (v as u64) & 0xFF
        })
        .collect();
    let saw: Vec<u64> = (0..N).map(|i| ((i / 6) % 16) as u64).collect();
    push("clk", 1, clk);
    push("sclk", 1, sclk);
    push("mosi", 1, mosi);
    push("miso", 1, miso);
    push("cs_n", 1, csn);
    push("uart_tx", 1, tx);
    push("state", 2, state);
    push("s_axi_awvalid", 1, pulse(20, 2));
    push("s_axi_awready", 1, pulse(20, 2));
    push("s_axi_awaddr", 8, hold(20, 0x10));
    push("s_axi_wvalid", 1, pulse(20, 2));
    push("s_axi_wready", 1, pulse(20, 2));
    push("s_axi_wdata", 16, hold(20, 0xBEEF));
    push("s_axi_bvalid", 1, pulse(38, 2));
    push("s_axi_bready", 1, pulse(38, 2));
    push("s_axi_bresp", 2, vec![0; N]);
    push("s_axi_arvalid", 1, pulse(60, 2));
    push("s_axi_arready", 1, pulse(60, 2));
    push("s_axi_araddr", 8, hold(60, 0x14));
    push("s_axi_rvalid", 1, pulse(80, 2));
    push("s_axi_rready", 1, pulse(80, 2));
    push("s_axi_rdata", 16, hold(80, 0x1234));
    for b in 0..4 {
        let name: &'static str = ["cnt_0", "cnt_1", "cnt_2", "cnt_3"][b];
        push(name, 1, cnt.iter().map(|v| (v >> b) & 1).collect());
    }
    push("dac", 8, dac);
    push("saw", 4, saw);
    let mut vcd = String::from("$timescale 1ns $end\n$scope module bus_tb $end\n");
    for (k, (name, w, _)) in sig.iter().enumerate() {
        vcd.push_str(&format!("$var wire {w} s{k} {name} $end\n"));
    }
    vcd.push_str("$upscope $end\n$enddefinitions $end\n");
    for i in 0..N {
        let mut changes = String::new();
        for (k, (_, w, v)) in sig.iter().enumerate() {
            if i == 0 || v[i] != v[i - 1] {
                if *w == 1 {
                    changes.push_str(&format!("{}s{k}\n", v[i]));
                } else {
                    changes.push_str(&format!("b{:b} s{k}\n", v[i]));
                }
            }
        }
        if !changes.is_empty() {
            vcd.push_str(&format!("#{i}\n{changes}"));
        }
    }
    vcd.push_str(&format!("#{N}\n"));
    let mut live = crate::views::wave::import_vcd_text(&vcd).expect("fixture VCD parses");
    live.source = "bus_tb protocol fixture".into();
    live
}

/// Mount [`protocol_fixture`] the way a finished sim run does.
pub fn seed_protocol_fixture(s: &mut AppState) {
    let live = protocol_fixture();
    s.vcd.timescale_ps = live.timescale_ps.max(1);
    s.vcd.duration_ps = live.duration_ps;
    s.vcd.signals = live.traces.iter().map(|t| t.name.clone()).collect();
    s.wave.mount_live(live);
}
