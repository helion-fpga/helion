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
