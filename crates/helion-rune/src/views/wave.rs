//! Simulation waveform viewer + testbench manager (Vivado SIMULATION layout).
//!
//! New Rune `El` code written from scratch for the Vivado brief. Two center
//! surfaces share this batch:
//!
//! - Wave (`CenterPane::Wave`): the Vivado simulator window — a Scope panel
//!   (testbench hierarchy) over an Objects panel (signals of the scope with
//!   live values and per-row `add_wave`), a signal Name/Value column, and a
//!   trace canvas with a time ruler, the main cursor, A/B measurement
//!   cursors, and named markers. Run/restart/step, zoom, and Hex/Bin radix
//!   live on the toolbar; the readout strip shows cursor/A/B/delta times.
//! - Sim (`CenterPane::Sim`): the testbench list (name + run-status badge)
//!   with a detail pane (DUT instance, port map, clocks, decls, last run,
//!   run history) that launches the run and opens the waveform.
//!
//! Trace data mirrors the `helion-gui` domain read-only: [`sample`] draws
//! the VCD shapes on the uniform viewer grid (`vcd_wave.rs`: one sample per
//! `timescale_ps`, zero-order hold between events), [`WaveSigId::VbLed`]
//! packs its members LSB-first like `Waveform::rebuild_virtual_buses`
//! (`ide.rs`), and [`TbStatus`] labels match `tb.rs::TbStatus`.
//!
//! Live runs (deep-03-sim-vcd-live) bind the real engine: [`launch_tb`]
//! elaborates the RTL (`helion_sv`/`helion_vhdl`, same dispatch as
//! `synth_hdl_path`), runs the `helion-sim` event kernel via
//! `helion_sim::vcd::dump_vcd`, and imports the
//! resulting VCD text with the fresh [`import_vcd_text`] parser (IEEE 1364
//! subset, same grid semantics as `vcd_wave.rs::import_vcd_waves`: uniform
//! grid, zero-order hold, `x`/`z`→0, 1/2/5 coarsening past
//! [`MAX_LIVE_SAMPLES`]). The import lands in [`WaveState::live`]; the viewer
//! renders live lanes, the readout/radix/cursors/markers operate on the live
//! grid, and the sim log streams into [`SimState::log`] (the console
//! Simulation tab renders it). Demo [`sample`] shapes remain as the explicit
//! pre-run fallback, labelled by the live banner.
//!
//! All colors come from [`VivadoTheme`]; no manual geometry beyond the
//! sample grid scaled by the zoom factor.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()/icon()/canvas()`.
//! - `crates/rune/src/shape.rs`: `Shape::new/stroke/line/fill`, `LineCap`.
//! - `crates/rune/src/el/layout.rs`: `.absolute()` overlay nodes.
//! - `crates/rune/src/el/scroll.rs`: `.scroll_x()` trace pan container.
//! - `crates/rune/src/el/handlers.rs`: `.on_press()` selection dispatch.
//! - `crates/rune-kit/src/glyphs.rs`: `TRIANGLE_RIGHT/DOWN` twisties.
//! - `crates/rune/src/tooling.rs`: `AppSpec::poll`
//!   (`fn(&mut S, f64, &mut Tx) -> bool`, called every frame).
//!
//! Engine citations (never invented):
//! - `crates/helion-sv/src/lib.rs:24757`: `synth_sv(source, origin)`.
//! - `crates/helion-vhdl/src/lib.rs:12`: `synth_vhdl(source)`.
//! - `crates/helion-sim/src/vcd.rs:53`: `dump_vcd(design, cycles, timescale_ps)`.
//! - `crates/helion-gui/src/vcd_wave.rs:143`: `import_vcd_waves` grid semantics.
//! - `crates/helion-gui/src/tb.rs:36`: `TB_PS_PER_UNIT` (1ns), `:40` history keep.
//! - `crates/helion-gui/src/ide.rs:4273,4367`: `WaveMarker`/`Waveform` shapes.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use rune::tok::TextStyle;
use rune::{El, LineCap, ScrollSpec, Shape, Tx, canvas, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::state::{AppState, ConsoleLine, MsgSev, RunStatus, ToastKind};
use crate::theme::VivadoTheme;
use crate::views::sources::pane_header;

/// Samples on the demo VCD grid (`vcd_wave.rs` uniform-grid convention).
pub const SAMPLES: usize = 96;
/// Pixels per sample at 100% zoom: the full 96-sample sheet (384pt) fits the
/// trace viewport so every lane, cursor, and marker stays inside the shell.
const BASE_PPS: f32 = 4.0;
/// Trace lane height (compact CAD density).
const LANE_H: f32 = 24.0;
/// Signal Name/Value column width.
const NAME_W: f32 = 208.0;
/// Time ruler height.
const RULER_H: f32 = 22.0;
/// Scope/Objects column width.
const LEFT_W: f32 = 260.0;
/// Scope panel height; Objects takes the rest.
const SCOPE_H: f32 = 112.0;
/// Ruler/grid tick every N samples.
const GRID_STEP: usize = 16;
/// Tree row height (compact CAD density).
const ROW_H: f32 = 20.0;
/// Toolbar height.
const TOOL_H: f32 = 26.0;
/// Zoom step per in/out press.
const ZOOM_STEP: f32 = 1.25;
/// Zoom limits.
const ZOOM_MIN: f32 = 0.25;
/// Zoom limits.
const ZOOM_MAX: f32 = 4.0;

/// Live capture: clock period for the engine run (10ns implicit clock,
/// mirroring `ide.rs` sim defaults and `runs.rs` STA fallback).
pub const LIVE_PERIOD_PS: u64 = 10_000;
/// Live capture cycle cap: the wave grid holds at most [`MAX_LIVE_SAMPLES`]
/// samples, and `dump_vcd` emits two edges per cycle, so 512 cycles stay
/// well inside the grid at 1ps.
pub const LIVE_MAX_CYCLES: u32 = 512;
/// Upper bound on live signals (mirrors `vcd_wave.rs::MAX_WAVE_SIGNALS`).
pub const MAX_LIVE_SIGNALS: usize = 1024;
/// Upper bound on live samples per trace (mirrors `MAX_WAVE_SAMPLES`).
pub const MAX_LIVE_SAMPLES: usize = 8192;
/// Live lanes shown before the "+N more" row (the sheet stays compact).
pub const LIVE_SHOWN: usize = 8;
/// Max sim channel events drained per frame.
const MAX_SIM_DRAIN: usize = 256;
/// Kept sim run-history entries, newest first (mirrors `TB_HISTORY_KEEP`).
pub const SIM_HISTORY_KEEP: usize = 20;

/// Pin/value type: smaller than CAD body text.
const FONT_PIN: TextStyle = TextStyle::new(10.0, 400.0);

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

/// One live trace imported from a real VCD dump: hierarchical signal name,
/// declared width, and uniform-grid samples (`u64`, `x`/`z` already mapped
/// to 0, vectors truncated to 64 bits). Mirrors `ide.rs::WaveTrace` fields
/// the viewer reads (`name`/`samples`/`width`).
#[derive(Clone, Debug, Default)]
pub struct LiveTrace {
    pub name: String,
    pub width: u8,
    pub samples: Vec<u64>,
}

impl LiveTrace {
    pub fn is_bus(&self) -> bool {
        self.width > 1
    }

    pub fn value_at(&self, i: usize) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        self.samples[i.min(self.samples.len() - 1)]
    }

    /// Leaf name after the last `.` scope separator (the Name column shows
    /// the leaf; the full path stays in the tooltip label).
    pub fn leaf(&self) -> &str {
        self.name.rsplit('.').next().unwrap_or(&self.name)
    }
}

/// A real VCD dump resampled onto the viewer grid. Produced only by
/// [`import_vcd_text`] from engine output (`helion_sim::vcd::dump_vcd`);
/// never synthesized.
#[derive(Clone, Debug, Default)]
pub struct LiveWave {
    /// One trace per signal, VCD file order (`clk`, `led`, driven nets).
    pub traces: Vec<LiveTrace>,
    /// Picoseconds per sample on the viewer grid.
    pub timescale_ps: u64,
    /// Last timestamp in the dump, picoseconds.
    pub duration_ps: u64,
    /// True when the signal cap truncated the dump.
    pub truncated: bool,
    /// What produced this import (`"<tb> (N cycles, helion-sim)"`).
    pub source: String,
    /// Engine cycles behind the dump.
    pub cycles: u32,
}

impl LiveWave {
    pub fn sample_len(&self) -> usize {
        self.traces.first().map(|t| t.samples.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.traces.is_empty() || self.sample_len() == 0
    }
}

/// Value of a live trace at grid index `i` in `radix` (scalars stay `0`/`1`).
pub fn format_live_value(trace: &LiveTrace, i: usize, radix: WaveRadix) -> String {
    let v = trace.value_at(i);
    if !trace.is_bus() {
        return if v == 0 { "0".into() } else { "1".into() };
    }
    let w = usize::from(trace.width.max(1));
    match radix {
        WaveRadix::Hex => format!("{:0nib$X}", v, nib = w.div_ceil(4)),
        WaveRadix::Bin => format!("{v:0w$b}", w = w),
    }
}

/// Constant-value runs of a live trace: (start, end-exclusive, value).
pub fn live_bus_runs(trace: &LiveTrace) -> Vec<(usize, usize, u64)> {
    let n = trace.samples.len();
    if n == 0 {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut start = 0;
    let mut cur = trace.samples[0];
    for (i, &v) in trace.samples.iter().enumerate().skip(1) {
        if v != cur {
            runs.push((start, i, cur));
            start = i;
            cur = v;
        }
    }
    runs.push((start, n, cur));
    runs
}

// ---- fresh VCD text importer (IEEE 1364 subset) ----

fn vcd_unit_to_fs(unit: &str) -> Option<u64> {
    match unit {
        "s" => Some(1_000_000_000_000_000),
        "ms" => Some(1_000_000_000_000),
        "us" => Some(1_000_000_000),
        "ns" => Some(1_000_000),
        "ps" => Some(1_000),
        "fs" => Some(1),
        _ => None,
    }
}

fn parse_vcd_timescale(args: &[&str]) -> Result<u64, String> {
    let joined: String = args.join("");
    let split = joined
        .find(|c: char| c.is_alphabetic())
        .ok_or_else(|| format!("bad $timescale: {joined}"))?;
    let (num, unit) = joined.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("bad $timescale number: {joined}"))?;
    if n != 1 && n != 10 && n != 100 {
        return Err(format!("$timescale must be 1/10/100: {joined}"));
    }
    let fs = vcd_unit_to_fs(unit).ok_or_else(|| format!("bad $timescale unit: {joined}"))?;
    Ok(n * fs)
}

/// Round `v` up to a 1/2/5 x 10^n grid step.
fn nice_ceil(v: u64) -> u64 {
    if v == 0 {
        return 1;
    }
    let mut pow10: u64 = 1;
    while pow10 < v {
        pow10 = pow10.saturating_mul(10);
    }
    let mut cand = pow10;
    for m in [5, 2, 1] {
        let c = pow10 / 10 * m;
        if c >= v && c > 0 {
            cand = c;
        }
    }
    cand.max(1)
}

/// Parse one scalar/vector change into a `u64`; `x`/`z` bits become 0 and
/// vectors wider than 64 bits keep the low 64. `None` on illegal chars.
fn parse_vcd_bits(scalar: Option<char>, vector: Option<&str>, width: u32) -> Option<u64> {
    if let Some(c) = scalar {
        return match c {
            '0' => Some(0),
            '1' => Some(1),
            'x' | 'X' | 'z' | 'Z' => Some(0),
            _ => None,
        };
    }
    let bits = vector?;
    let w = (width as usize).min(64);
    let mut val: u64 = 0;
    for (i, ch) in bits.chars().rev().take(w).enumerate() {
        match ch {
            '1' => val |= 1u64 << i,
            '0' | 'x' | 'X' | 'z' | 'Z' => {}
            _ => return None,
        }
    }
    Some(val)
}

struct VcdVar {
    width: u32,
    name: String,
}

/// Parse VCD text into live traces on the viewer grid. Fresh code with the
/// same subset/grid semantics as `vcd_wave.rs::import_vcd_waves`: header
/// `$timescale`/`$scope`/`$var`/`$enddefinitions`, body `#time`/scalar/
/// vector/`$dumpvars`, monotonic timestamps, zero-order hold resampling, and
/// `x`/`z`→0. Malformed input is a loud `Err`, never an empty view.
pub fn import_vcd_text(text: &str) -> Result<LiveWave, String> {
    if text.trim().is_empty() {
        return Err("empty VCD".to_string());
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0usize;
    let mut timescale_fs: u64 = 1_000;
    let mut scope: Vec<String> = Vec::new();
    let mut vars: Vec<(String, VcdVar)> = Vec::new();
    let mut var_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut truncated = false;
    let mut enddefs = false;

    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        match tok {
            "$timescale" => {
                let mut args = Vec::new();
                while i < tokens.len() && tokens[i] != "$end" {
                    args.push(tokens[i]);
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
                timescale_fs = parse_vcd_timescale(&args)?;
            }
            "$scope" => {
                if i + 2 < tokens.len() {
                    scope.push(tokens[i + 1].to_string());
                    i += 3;
                }
            }
            "$upscope" => {
                scope.pop();
                if i < tokens.len() && tokens[i] == "$end" {
                    i += 1;
                }
            }
            "$var" => {
                let mut args = Vec::new();
                while i < tokens.len() && tokens[i] != "$end" {
                    args.push(tokens[i]);
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
                if args.len() >= 4 {
                    let width: u32 = args[1].parse().unwrap_or(1).max(1);
                    let id = args[2].to_string();
                    let reference = args[3..].join("");
                    let mut name = String::new();
                    for (k, s) in scope.iter().enumerate() {
                        if k > 0 {
                            name.push('.');
                        }
                        name.push_str(s);
                    }
                    if !name.is_empty() {
                        name.push('.');
                    }
                    name.push_str(&reference);
                    if var_index.contains_key(&id) {
                        return Err(format!("duplicate $var id: {id}"));
                    }
                    if vars.len() >= MAX_LIVE_SIGNALS {
                        truncated = true;
                    } else {
                        var_index.insert(id.clone(), vars.len());
                        vars.push((id, VcdVar { width, name }));
                    }
                }
            }
            "$enddefinitions" => {
                enddefs = true;
                if i < tokens.len() && tokens[i] == "$end" {
                    i += 1;
                }
                break;
            }
            "$date" | "$version" | "$comment" => {
                while i < tokens.len() && tokens[i] != "$end" {
                    i += 1;
                }
                if i < tokens.len() {
                    i += 1;
                }
            }
            _ => {}
        }
    }
    if !enddefs {
        return Err("missing $enddefinitions".to_string());
    }
    if vars.is_empty() {
        return Err("no $var signals in VCD".to_string());
    }

    let mut events: Vec<Vec<(u64, u64)>> = vec![Vec::new(); vars.len()];
    let mut tick: u64 = 0;
    let mut last_tick: Option<u64> = None;
    let tick_to_ps = |t: u64| t.saturating_mul(timescale_fs) / 1_000;

    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        if tok == "$dumpvars" || tok == "$dumpall" || tok == "$end" {
            continue;
        }
        if tok == "$dumpoff" || tok == "$dumpon" {
            continue;
        }
        if let Some(stripped) = tok.strip_prefix('#') {
            let t: u64 = stripped
                .parse()
                .map_err(|_| format!("bad timestamp: {tok}"))?;
            if let Some(last) = last_tick {
                if t < last {
                    return Err(format!("timestamps go backwards: #{t} after #{last}"));
                }
            }
            last_tick = Some(t);
            tick = t;
            continue;
        }
        let (scalar, vector, id): (Option<char>, Option<&str>, &str) =
            if let Some(bits) = tok.strip_prefix('b').or_else(|| tok.strip_prefix('B')) {
                if i >= tokens.len() {
                    return Err("vector value missing id".to_string());
                }
                let id = tokens[i];
                i += 1;
                (None, Some(bits), id)
            } else {
                let mut chars = tok.chars();
                let val = chars
                    .next()
                    .ok_or_else(|| "empty value token".to_string())?;
                let rest = chars.as_str();
                if rest.is_empty() {
                    return Err(format!("scalar value missing id: {tok}"));
                }
                (Some(val), None, rest)
            };
        let Some(&k) = var_index.get(id) else {
            continue;
        };
        let width = vars[k].1.width;
        let value = parse_vcd_bits(scalar, vector, width)
            .ok_or_else(|| format!("bad value for {id}: {tok}"))?;
        let t_ps = tick_to_ps(tick);
        if let Some(last) = events[k].last_mut() {
            if last.0 == t_ps {
                last.1 = value;
                continue;
            }
        }
        events[k].push((t_ps, value));
    }

    let duration_ps = tick_to_ps(last_tick.unwrap_or(0));
    let vcd_ts_ps = (timescale_fs / 1_000).max(1);
    let min_grid = duration_ps.div_ceil(MAX_LIVE_SAMPLES as u64).max(1);
    let grid_ps = nice_ceil(vcd_ts_ps.max(min_grid));
    let n_samples = (duration_ps / grid_ps) as usize + 1;
    if n_samples > MAX_LIVE_SAMPLES + 1 {
        return Err(format!(
            "dump too long for the viewer grid: {n_samples} samples"
        ));
    }

    let mut traces = Vec::with_capacity(vars.len());
    for (events_k, (_, decl)) in events.iter().zip(vars.iter()) {
        let width = (decl.width as usize).min(64).max(1) as u8;
        let mask = if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        };
        let mut samples = Vec::with_capacity(n_samples);
        let mut ev_idx = 0usize;
        let mut cur: u64 = 0;
        for s in 0..n_samples {
            let t = s as u64 * grid_ps;
            while ev_idx < events_k.len() && events_k[ev_idx].0 <= t {
                cur = events_k[ev_idx].1;
                ev_idx += 1;
            }
            samples.push(cur & mask);
        }
        traces.push(LiveTrace {
            name: decl.name.clone(),
            width,
            samples,
        });
    }

    Ok(LiveWave {
        traces,
        timescale_ps: grid_ps,
        duration_ps,
        truncated,
        source: String::new(),
        cycles: 0,
    })
}

/// Elaborate `design_src` and run the `helion-sim` kernel for `cycles`
/// posedges, returning the imported live wave plus the engine stage lines.
/// Dispatch mirrors `synth_hdl_path` (`.vhd`/`.vhdl` → VHDL, else SV).
pub fn run_engine_vcd(
    design_src: &str,
    origin: &str,
    cycles: u32,
    period_ps: u64,
) -> Result<(LiveWave, Vec<(MsgSev, String)>), String> {
    let mut lines: Vec<(MsgSev, String)> = Vec::new();
    let ext = std::path::Path::new(origin)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let design = if matches!(ext.as_str(), "vhd" | "vhdl") {
        helion_vhdl::synth_vhdl(design_src).map_err(|e| format!("synth_design failed: {e}"))?
    } else {
        helion_sv::synth_sv(design_src, origin).map_err(|e| format!("synth_design failed: {e}"))?
    };
    lines.push((
        MsgSev::Info,
        format!(
            "synth_design cells={} luts={}",
            design.cells.len(),
            design.lut_inits().len()
        ),
    ));
    let cycles = cycles.clamp(1, LIVE_MAX_CYCLES);
    lines.push((
        MsgSev::Info,
        format!(
            "sim_run start cycles={cycles} timescale_ps={}",
            period_ps.max(1)
        ),
    ));
    let vcd = helion_sim::vcd::dump_vcd(&design, cycles, period_ps.max(1));
    let mut live = import_vcd_text(&vcd).map_err(|e| format!("VCD import failed: {e}"))?;
    live.source = format!("{origin} ({cycles} cycles, helion-sim)");
    live.cycles = cycles;
    lines.push((
        MsgSev::Info,
        format!(
            "dump_vcd signals={} samples={} timescale_ps={} duration_ps={}{}",
            live.traces.len(),
            live.sample_len(),
            live.timescale_ps,
            live.duration_ps,
            if live.truncated { " (truncated)" } else { "" },
        ),
    ));
    Ok((live, lines))
}

/// Value radix of the Name/Value column and bus labels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WaveRadix {
    #[default]
    Hex,
    Bin,
}

impl WaveRadix {
    pub fn label(self) -> &'static str {
        match self {
            WaveRadix::Hex => "Hex",
            WaveRadix::Bin => "Bin",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            WaveRadix::Hex => WaveRadix::Bin,
            WaveRadix::Bin => WaveRadix::Hex,
        }
    }
}

/// Value of `id` at grid index `i` in `radix` (scalars stay `0`/`1`).
pub fn format_value(id: WaveSigId, i: usize, radix: WaveRadix) -> String {
    let v = sample(id, i);
    if !id.is_bus() {
        return if v == 0 { "0".into() } else { "1".into() };
    }
    let w = usize::from(id.width());
    match radix {
        WaveRadix::Hex => format!("{:0nib$X}", v, nib = w.div_ceil(4)),
        WaveRadix::Bin => format!("{v:0w$b}", w = w),
    }
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

/// Grid length: live sample count once a sim run imports a VCD, else the
/// demo [`SAMPLES`] fallback.
pub fn sample_len(s: &AppState) -> usize {
    s.wave.sample_len()
}

/// Picoseconds of grid index `i` on the state's VCD grid (live timescale
/// once a run imports; the demo `vcd.timescale_ps` before that).
pub fn time_ps(s: &AppState, i: usize) -> u64 {
    let n = sample_len(s).saturating_sub(1).max(1);
    i.min(n) as u64 * s.vcd.timescale_ps.max(1)
}

/// Compact time label: `48 ns`, falling back to ps off-grid.
pub fn time_label(s: &AppState, i: usize) -> String {
    time_label_ps(time_ps(s, i))
}

pub fn time_label_ps(ps: u64) -> String {
    if ps % 1000 == 0 {
        format!("{} ns", ps / 1000)
    } else {
        format!("{ps} ps")
    }
}

/// Named marker on the sample grid (mirrors `ide.rs::WaveMarker`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveMarker {
    pub name: String,
    pub sample: usize,
}

impl WaveMarker {
    pub fn new(name: &str, sample: usize) -> Self {
        Self {
            name: name.into(),
            sample: sample.min(SAMPLES.saturating_sub(1)),
        }
    }
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

/// Wave-owned UI state: cursor, A/B cursors, markers, selection, added pool
/// signals, scope, zoom, radix, plus the live engine import (when a sim run
/// has produced one) and the live trace selection.
#[derive(Clone, Debug)]
pub struct WaveState {
    pub cursor: usize,
    pub cursor_a: Option<usize>,
    pub cursor_b: Option<usize>,
    pub markers: Vec<WaveMarker>,
    pub selected: Option<WaveSigId>,
    pub added: Vec<WaveSigId>,
    pub scope: SimScope,
    pub zoom: f32,
    pub radix: WaveRadix,
    /// Real engine import from the last sim run (`None` until then; the demo
    /// [`sample`] grid renders while it is `None`).
    pub live: Option<LiveWave>,
    /// Selected live trace index (only meaningful while [`WaveState::live`]
    /// is `Some`).
    pub live_selected: Option<usize>,
}

impl Default for WaveState {
    fn default() -> Self {
        Self {
            cursor: 48,
            cursor_a: None,
            cursor_b: None,
            markers: Vec::new(),
            selected: None,
            added: Vec::new(),
            scope: SimScope::Tb,
            zoom: 1.0,
            radix: WaveRadix::Hex,
            live: None,
            live_selected: None,
        }
    }
}

impl WaveState {
    /// Mounted traces: defaults plus pool signals added via `add_wave`.
    pub fn visible(&self) -> Vec<WaveSigId> {
        let mut out = WaveSigId::DEFAULT.to_vec();
        for id in &self.added {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        out
    }

    pub fn is_visible(&self, id: WaveSigId) -> bool {
        WaveSigId::DEFAULT.contains(&id) || self.added.contains(&id)
    }

    /// Add a pool signal; `false` when already mounted (never duplicates).
    pub fn add_signal(&mut self, id: WaveSigId) -> bool {
        if self.is_visible(id) {
            return false;
        }
        self.added.push(id);
        true
    }

    /// Grid length: live sample count once imported, else demo [`SAMPLES`].
    pub fn sample_len(&self) -> usize {
        self.live
            .as_ref()
            .map(|l| l.sample_len().max(1))
            .unwrap_or(SAMPLES)
    }

    /// Whether live engine traces are mounted (vs the demo fallback grid).
    pub fn has_live(&self) -> bool {
        self.live.as_ref().is_some_and(|l| !l.is_empty())
    }

    /// Live traces shown as lanes (first [`LIVE_SHOWN`]; the rest hide behind
    /// the "+N more" row so the sheet stays compact).
    pub fn live_shown(&self) -> usize {
        self.live
            .as_ref()
            .map(|l| l.traces.len().min(LIVE_SHOWN))
            .unwrap_or(0)
    }

    /// Clamp cursor, A/B, markers, and live selection onto the current grid.
    /// Called after every live import so stale demo indices cannot dangle.
    pub fn clamp_to_grid(&mut self) {
        let last = self.sample_len().saturating_sub(1);
        self.cursor = self.cursor.min(last);
        self.cursor_a = self.cursor_a.map(|a| a.min(last));
        self.cursor_b = self.cursor_b.map(|b| b.min(last));
        for m in &mut self.markers {
            m.sample = m.sample.min(last);
        }
        if let Some(live) = &self.live {
            if self.live_selected.is_some_and(|i| i >= live.traces.len()) {
                self.live_selected = None;
            }
        } else {
            self.live_selected = None;
        }
    }

    pub fn step_cursor(&mut self, d: i32) {
        let last = self.sample_len().saturating_sub(1) as i32;
        let c = self.cursor as i32 + d;
        self.cursor = c.clamp(0, last.max(0)) as usize;
    }

    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom * ZOOM_STEP).min(ZOOM_MAX);
    }

    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom / ZOOM_STEP).max(ZOOM_MIN);
    }

    pub fn zoom_fit(&mut self) {
        self.zoom = 1.0;
    }

    pub fn zoom_label(&self) -> String {
        format!("{}%", (self.zoom * 100.0).round())
    }

    pub fn pps(&self) -> f32 {
        BASE_PPS * self.zoom
    }

    pub fn sheet_w(&self) -> f32 {
        self.sample_len() as f32 * self.pps()
    }
}

/// Per-testbench run status; labels mirror `tb.rs::TbStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TbStatus {
    #[default]
    Idle,
    Running,
    Passed,
    Failed,
}

impl TbStatus {
    pub fn label(self) -> &'static str {
        match self {
            TbStatus::Idle => "Not run",
            TbStatus::Running => "Running",
            TbStatus::Passed => "Passed",
            TbStatus::Failed => "Failed",
        }
    }
}

/// One discovered testbench (mirrors `tb.rs::TbFile` + run summary).
#[derive(Clone, Copy, Debug)]
pub struct TbEntry {
    pub name: &'static str,
    pub path: &'static str,
    pub dut: &'static str,
    pub inst: &'static str,
    pub vcd: &'static str,
    pub units: u64,
}

/// Demo testbenches in discovery order (`tb.rs::discover_testbenches` sorts
/// by name; statuses live in [`SimState`] so runs can mutate them).
pub const TBS: [TbEntry; 2] = [
    TbEntry {
        name: "counter_tb",
        path: "C:/EE214/project_1/project_1.srcs/sim_1/new/counter_tb.v",
        dut: "counter",
        inst: "dut",
        vcd: "counter_tb.vcd",
        units: 400,
    },
    TbEntry {
        name: "project1_tb",
        path: "C:/EE214/project_1/project_1.srcs/sim_1/new/project1_tb.v",
        dut: "project1_demo",
        inst: "dut",
        vcd: "project1_tb.vcd",
        units: 9600,
    },
];

/// Port map rows of a testbench's DUT instance.
pub fn tb_ports(i: usize) -> &'static [(&'static str, &'static str)] {
    match i {
        0 => &[("clk", "clk"), ("rst_n", "rst_n"), ("count[7:0]", "count")],
        _ => &[
            ("clk", "clk"),
            ("rst_n", "rst_n"),
            ("led[15:0]", "led"),
            ("seg[7:0]", "seg"),
            ("an[3:0]", "an"),
        ],
    }
}

/// Clock generators of a testbench (signal, period).
pub fn tb_clocks(i: usize) -> &'static [(&'static str, &'static str)] {
    match i {
        0 => &[("clk", "10 ns")],
        _ => &[("clk", "10 ns"), ("divclk", "320 ns")],
    }
}

/// Testbench decls: (name, width, init), mirroring `tb.rs::TbDecl`.
pub fn tb_decls(i: usize) -> &'static [(&'static str, u32, &'static str)] {
    match i {
        0 => &[("clk", 1, "0"), ("rst_n", 1, "1"), ("count", 8, "-")],
        _ => &[
            ("clk", 1, "0"),
            ("rst_n", 1, "1"),
            ("done", 1, "-"),
            ("errors", 32, "0"),
        ],
    }
}

/// One kept run-history entry (mirrors `TB_HISTORY_KEEP` newest-first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimRun {
    pub tb: &'static str,
    pub status: TbStatus,
    pub units: u64,
    pub errors: u32,
}

/// One worker -> UI sim message. Lines stream live; the terminal event
/// carries the real import (or the real error).
#[derive(Debug)]
pub enum SimEvent {
    Line { text: String, sev: MsgSev },
    Finished(Result<SimArtifacts, String>),
    Cancelled,
}

/// Real sim completion payload: the imported live wave plus the run summary.
#[derive(Clone, Debug, Default)]
pub struct SimArtifacts {
    pub live: LiveWave,
    pub tb: usize,
    pub units: u64,
    pub errors: u32,
    pub summary: String,
}

/// The live sim worker handle: its event channel plus the cancel flag the
/// worker checks between stages.
#[derive(Clone)]
struct ActiveSim {
    tb: usize,
    rx: Arc<Mutex<mpsc::Receiver<SimEvent>>>,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for ActiveSim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveSim")
            .field("tb", &self.tb)
            .field("cancelled", &self.cancel.load(Ordering::Relaxed))
            .finish()
    }
}

/// Sim-owned UI state: list selection, per-testbench statuses, history, the
/// real streamed sim log, and the at-most-one live worker.
#[derive(Clone, Debug)]
pub struct SimState {
    pub selected: Option<usize>,
    pub statuses: Vec<TbStatus>,
    pub history: Vec<SimRun>,
    /// Real buffered sim log lines (the console Simulation tab renders
    /// these; empty until the first run streams).
    pub log: Vec<ConsoleLine>,
    active: Option<ActiveSim>,
}

impl Default for SimState {
    fn default() -> Self {
        Self {
            selected: None,
            statuses: vec![TbStatus::Idle; TBS.len()],
            history: Vec::new(),
            log: Vec::new(),
            active: None,
        }
    }
}

impl SimState {
    pub fn status(&self, i: usize) -> TbStatus {
        self.statuses.get(i).copied().unwrap_or(TbStatus::Idle)
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn active_tb(&self) -> Option<usize> {
        self.active.as_ref().map(|a| a.tb)
    }

    pub fn cancel_requested(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| a.cancel.load(Ordering::Relaxed))
    }

    fn active_rx(&self) -> Option<Arc<Mutex<mpsc::Receiver<SimEvent>>>> {
        self.active.as_ref().map(|a| a.rx.clone())
    }

    fn set_active(&mut self, tb: usize, rx: mpsc::Receiver<SimEvent>, cancel: Arc<AtomicBool>) {
        self.active = Some(ActiveSim {
            tb,
            rx: Arc::new(Mutex::new(rx)),
            cancel,
        });
    }

    fn clear_active(&mut self) {
        self.active = None;
    }

    fn request_cancel(&self) {
        if let Some(a) = &self.active {
            a.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Drop any live sim (flagging cancel first) and reset to idle. The
    /// orphaned worker's sends fail silently and it exits on its own.
    pub fn reset(&mut self) {
        self.request_cancel();
        *self = SimState::default();
    }
}

/// Engine cycles for a testbench: its time units (ns, `TB_PS_PER_UNIT`) over
/// the live period, clamped into the wave grid.
pub fn tb_cycles(i: usize) -> u32 {
    let units = TBS.get(i).map(|t| t.units).unwrap_or(100);
    let cycles =
        (units.saturating_mul(1_000) / LIVE_PERIOD_PS.max(1)).clamp(1, u64::from(LIVE_MAX_CYCLES));
    cycles as u32
}

/// Resolve the editor RTL buffer for a sim run (used when no elaborated
/// Tcl design exists; [`launch_tb`] prefers the live `tcl.design`).
fn resolve_design_src(s: &AppState) -> Result<(String, String), String> {
    if s.editor.lines.iter().all(|l| l.trim().is_empty()) {
        return Err("launch_simulation: the RTL buffer is empty (open a source first)".into());
    }
    let origin = s
        .editor
        .tabs
        .get(s.editor.active)
        .cloned()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "design.v".into());
    Ok((origin, s.editor.lines.join("\n")))
}

/// Launch testbench `tb` on the engine worker. Sets Running immediately (the
/// badge/test assertion window); the worker streams lines and delivers the
/// live VCD import via [`poll_sim`]. Fails without touching state when a sim
/// is already active or no RTL source resolves.
pub fn launch_tb(s: &mut AppState, tb: usize) -> Result<(), String> {
    if tb >= TBS.len() {
        return Err(format!("launch_simulation: no testbench index {tb}"));
    }
    if let Some(busy) = s.sim.active_tb() {
        return Err(format!(
            "{} is already running",
            TBS.get(busy).map(|t| t.name).unwrap_or("sim")
        ));
    }
    // Prefer the elaborated Tcl design when one exists (real `Sim` over the
    // live `helion_ir::Design`); else stage the editor RTL buffer.
    let use_tcl_design = s.tcl.design.is_some();
    let (origin, src) = if use_tcl_design {
        (
            s.tcl
                .design_name
                .clone()
                .unwrap_or_else(|| "tcl_design.v".into()),
            String::new(),
        )
    } else {
        resolve_design_src(s)?
    };
    let tcl_design = if use_tcl_design {
        s.tcl.design.clone()
    } else {
        None
    };
    let cycles = tb_cycles(tb);
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let tb_name = TBS[tb].name.to_string();
    std::thread::Builder::new()
        .name(format!("helion-sim-{tb_name}"))
        .spawn(move || {
            sim_worker(
                tb,
                tb_name,
                origin,
                src,
                tcl_design,
                cycles,
                tx,
                worker_cancel,
            )
        })
        .map_err(|e| format!("failed to spawn sim thread: {e}"))?;
    s.sim.set_active(tb, rx, cancel);
    s.sim.statuses[tb] = TbStatus::Running;
    s.flow.sim = RunStatus::Running;
    s.sim.history.insert(
        0,
        SimRun {
            tb: TBS[tb].name,
            status: TbStatus::Running,
            units: 0,
            errors: 0,
        },
    );
    s.sim.history.truncate(SIM_HISTORY_KEEP);
    Ok(())
}

/// Launch the selected testbench, defaulting to `project1_tb` when nothing
/// is selected (the console Simulation prompt uses this).
pub fn launch_default_tb(s: &mut AppState) -> Result<(), String> {
    let tb = s.sim.selected.unwrap_or(TBS.len() - 1);
    launch_tb(s, tb)
}

/// UI/Tcl entry: echo the verb and launch the default testbench on the
/// real worker, mirroring `runs::launch` (error toast on failure; the sim
/// state is only touched once the worker thread is spawned).
pub fn launch(s: &mut AppState) {
    s.console.lines.push(ConsoleLine {
        text: "launch_simulation".into(),
        sev: MsgSev::Info,
    });
    if let Err(e) = launch_default_tb(s) {
        s.console.lines.push(ConsoleLine {
            text: format!("ERROR: {e}"),
            sev: MsgSev::Error,
        });
        s.toasts
            .push(ToastKind::Error, "Cannot start simulation", &e);
    }
}

/// Flag the active sim for cancellation (the worker stops between stages).
pub fn cancel_sim(s: &mut AppState) {
    match s.sim.active_tb() {
        Some(tb) => {
            s.sim.request_cancel();
            let line = format!(
                "stop_simulation {}",
                TBS.get(tb).map(|t| t.name).unwrap_or("sim")
            );
            s.console.lines.push(ConsoleLine {
                text: line.clone(),
                sev: MsgSev::Info,
            });
            s.sim.log.push(ConsoleLine {
                text: line,
                sev: MsgSev::Info,
            });
        }
        None => {
            s.toasts
                .push(ToastKind::Info, "No active simulation", "Nothing to stop");
        }
    }
}

/// Drain pending sim worker events into console/log/wave state. Returns true
/// when anything changed (drives `AppSpec::poll`'s dirty flag).
pub fn poll_sim(s: &mut AppState) -> bool {
    let tb = match s.sim.active_tb() {
        Some(t) => t,
        None => return false,
    };
    let rx = match s.sim.active_rx() {
        Some(rx) => rx,
        None => return false,
    };
    let mut changed = false;
    for _ in 0..MAX_SIM_DRAIN {
        let next = rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
        match next {
            Ok(event) => {
                deliver_sim(s, tb, event);
                changed = true;
                if !s.sim.is_active() {
                    break;
                }
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                deliver_sim(
                    s,
                    tb,
                    SimEvent::Finished(Err("sim worker exited unexpectedly".into())),
                );
                changed = true;
                break;
            }
        }
    }
    changed
}

/// `AppSpec::poll` wiring for sim: pump the sim channel every frame.
pub fn poll(s: &mut AppState, _t: f64, _tx: &mut Tx) -> bool {
    poll_sim(s)
}

/// Apply one sim worker event: lines go to the Tcl console AND the sim log
/// buffer; the terminal event mounts the live VCD import (or the error).
pub fn deliver_sim(s: &mut AppState, tb: usize, event: SimEvent) {
    match event {
        SimEvent::Line { text, sev } => {
            s.console.lines.push(ConsoleLine {
                text: text.clone(),
                sev,
            });
            s.sim.log.push(ConsoleLine { text, sev });
        }
        SimEvent::Finished(result) => {
            match result {
                Ok(art) => {
                    let n_traces = art.live.traces.len();
                    let n_samples = art.live.sample_len();
                    s.vcd.timescale_ps = art.live.timescale_ps.max(1);
                    s.vcd.duration_ps = art.live.duration_ps;
                    s.vcd.signals = art
                        .live
                        .traces
                        .iter()
                        .take(64)
                        .map(|t| t.name.clone())
                        .collect();
                    s.wave.live = Some(art.live);
                    s.wave.clamp_to_grid();
                    // Move the cursor onto the live grid end so the import is
                    // visible immediately (mirrors `sim_run` cursor-to-end).
                    let last = s.wave.sample_len().saturating_sub(1);
                    s.wave.cursor = last;
                    s.sim.statuses[tb] = TbStatus::Passed;
                    s.flow.sim = RunStatus::Complete;
                    if let Some(h) = s.sim.history.first_mut() {
                        h.status = TbStatus::Passed;
                        h.units = art.units;
                        h.errors = art.errors;
                    }
                    let detail = format!(
                        "{} · {} signals, {} samples",
                        art.summary, n_traces, n_samples
                    );
                    s.toasts
                        .push(ToastKind::Success, "Simulation complete", &detail);
                    let done = format!("sim_run done: {detail}");
                    s.console.lines.push(ConsoleLine {
                        text: done.clone(),
                        sev: MsgSev::Info,
                    });
                    s.sim.log.push(ConsoleLine {
                        text: done,
                        sev: MsgSev::Info,
                    });
                }
                Err(e) => {
                    s.sim.statuses[tb] = TbStatus::Failed;
                    s.flow.sim = RunStatus::Failed;
                    if let Some(h) = s.sim.history.first_mut() {
                        h.status = TbStatus::Failed;
                    }
                    let line = format!("ERROR: {e}");
                    s.console.lines.push(ConsoleLine {
                        text: line.clone(),
                        sev: MsgSev::Error,
                    });
                    s.sim.log.push(ConsoleLine {
                        text: line,
                        sev: MsgSev::Error,
                    });
                    s.toasts.push(
                        ToastKind::Error,
                        "Simulation failed",
                        &truncate_chars(&e, 140),
                    );
                }
            }
            s.sim.clear_active();
        }
        SimEvent::Cancelled => {
            s.sim.statuses[tb] = TbStatus::Idle;
            s.flow.sim = RunStatus::NotStarted;
            if let Some(h) = s.sim.history.first_mut() {
                h.status = TbStatus::Idle;
            }
            let line = format!(
                "{} cancelled by user",
                TBS.get(tb).map(|t| t.name).unwrap_or("sim")
            );
            s.console.lines.push(ConsoleLine {
                text: line.clone(),
                sev: MsgSev::Info,
            });
            s.sim.log.push(ConsoleLine {
                text: line,
                sev: MsgSev::Info,
            });
            s.toasts
                .push(ToastKind::Warning, "Simulation stopped", "Run cancelled");
            s.sim.clear_active();
        }
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// The sim worker body: elaborate, run `helion-sim`, import the VCD, and
/// stream every stage line. Checks cancel between stages.
#[allow(clippy::too_many_arguments)]
fn sim_worker(
    tb: usize,
    tb_name: String,
    origin: String,
    src: String,
    tcl_design: Option<helion_ir::Design>,
    cycles: u32,
    tx: mpsc::Sender<SimEvent>,
    cancel: Arc<AtomicBool>,
) {
    let send = |text: String, sev: MsgSev| {
        let _ = tx.send(SimEvent::Line { text, sev });
    };
    let is_cancelled = || cancel.load(Ordering::Relaxed);
    send(format!("elaborate {tb_name} <- {origin}"), MsgSev::Info);
    let design = match tcl_design {
        Some(d) => d,
        None => {
            if src.trim().is_empty() {
                let _ = tx.send(SimEvent::Finished(Err(
                    "launch_simulation: the RTL buffer is empty (open a source first)".into(),
                )));
                return;
            }
            let ext = std::path::Path::new(&origin)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let synth: Result<helion_ir::Design, String> = if matches!(ext.as_str(), "vhd" | "vhdl")
            {
                helion_vhdl::synth_vhdl(&src).map_err(|e| format!("synth_design failed: {e}"))
            } else {
                helion_sv::synth_sv(&src, &origin).map_err(|e| format!("synth_design failed: {e}"))
            };
            match synth {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(SimEvent::Finished(Err(e)));
                    return;
                }
            }
        }
    };
    send(
        format!(
            "synth_design cells={} luts={}",
            design.cells.len(),
            design.lut_inits().len()
        ),
        MsgSev::Info,
    );
    if is_cancelled() {
        let _ = tx.send(SimEvent::Cancelled);
        return;
    }
    let cycles = cycles.clamp(1, LIVE_MAX_CYCLES);
    send(
        format!("sim_run start cycles={cycles} timescale_ps={LIVE_PERIOD_PS}"),
        MsgSev::Info,
    );
    let vcd = helion_sim::vcd::dump_vcd(&design, cycles, LIVE_PERIOD_PS);
    if is_cancelled() {
        let _ = tx.send(SimEvent::Cancelled);
        return;
    }
    let mut live = match import_vcd_text(&vcd) {
        Ok(l) => l,
        Err(e) => {
            let _ = tx.send(SimEvent::Finished(Err(format!("VCD import failed: {e}"))));
            return;
        }
    };
    live.source = format!("{tb_name} ({cycles} cycles, helion-sim)");
    live.cycles = cycles;
    send(
        format!(
            "dump_vcd signals={} samples={} timescale_ps={} duration_ps={}{}",
            live.traces.len(),
            live.sample_len(),
            live.timescale_ps,
            live.duration_ps,
            if live.truncated { " (truncated)" } else { "" },
        ),
        MsgSev::Info,
    );
    let units = u64::from(cycles).saturating_mul(LIVE_PERIOD_PS) / 1_000;
    let summary = format!("{tb_name} PASSED: sim_time={units} ns");
    send(summary.clone(), MsgSev::Info);
    let _ = tx.send(SimEvent::Finished(Ok(SimArtifacts {
        live,
        tb,
        units,
        errors: 0,
        summary,
    })));
}

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: String) {
    s.console.lines.push(ConsoleLine {
        text: tcl,
        sev: MsgSev::Info,
    });
}

/// Select a trace and cross-probe it to the console like the timing links.
fn select_trace(s: &mut AppState, id: WaveSigId) {
    s.wave.selected = Some(id);
    echo(s, format!("select_objects [get_waves {{{}}}]", id.name()));
}

/// Add an Objects row to the wave, echoing the Vivado verb.
fn add_object(s: &mut AppState, obj: SimObject) {
    let Some(id) = obj.sig else { return };
    if s.wave.add_signal(id) {
        echo(s, format!("add_wave {{{}}}", obj.name));
    }
}

/// Launch the selected testbench on the real engine worker.
fn run_selected_tb(s: &mut AppState) {
    let Some(i) = s.sim.selected else { return };
    if i >= TBS.len() {
        return;
    }
    echo(
        s,
        format!("launch_simulation -simset sim_1 -tb {}", TBS[i].name),
    );
    echo(s, "run 100 ns".into());
    if let Err(e) = launch_tb(s, i) {
        s.console.lines.push(ConsoleLine {
            text: format!("ERROR: {e}"),
            sev: MsgSev::Error,
        });
        s.toasts
            .push(ToastKind::Error, "Cannot start simulation", &e);
    }
}

/// Select a live trace and cross-probe it to the console.
fn select_live_trace(s: &mut AppState, idx: usize) {
    let Some(live) = &s.wave.live else { return };
    let Some(trace) = live.traces.get(idx) else {
        return;
    };
    let name = trace.name.clone();
    s.wave.live_selected = Some(idx);
    echo(s, format!("select_objects [get_waves {{{name}}}]"));
}

fn ui_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text)
}

fn dim_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_UI).color(t.text_dim)
}

fn pin_label(content: &str, t: &VivadoTheme) -> El {
    text(content).font(FONT_PIN).color(t.text_dim)
}

fn mono_value(content: &str, t: &VivadoTheme) -> El {
    text(content).font(VivadoTheme::FONT_MONO).color(t.text)
}

/// Small toolbar button with a text glyph (matches the Sources style).
fn tool_btn(key: &str, label: &str, t: &VivadoTheme) -> El {
    row()
        .key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .h(20.0)
        .px(6.0)
        .items_center()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .hover(t.selection)
        .child(text(label).font(VivadoTheme::FONT_UI).color(t.text))
}

/// Grid + trace shape of one lane: light gridlines, the stepped scalar or
/// the bus envelope with pinched transitions.
fn lane_shape(s: &AppState, t: &VivadoTheme, id: WaveSigId) -> Shape {
    let pps = s.wave.pps();
    let w = s.wave.sheet_w();
    let mut shape = Shape::new();
    let mut g = 0;
    while g <= SAMPLES {
        let x = g as f32 * pps;
        shape = shape.line([x, 0.0], [x, LANE_H], 1.0, LineCap::Butt, t.wave_grid);
        g += GRID_STEP;
    }
    let hi = 4.0;
    let lo = LANE_H - 4.0;
    let mid = LANE_H / 2.0;
    if !id.is_bus() {
        let mut pts = vec![[0.0, if sample(id, 0) == 0 { lo } else { hi }]];
        for i in 1..SAMPLES {
            let x = i as f32 * pps;
            let prev = if sample(id, i - 1) == 0 { lo } else { hi };
            let cur = if sample(id, i) == 0 { lo } else { hi };
            pts.push([x, prev]);
            pts.push([x, cur]);
        }
        pts.push([w, if sample(id, SAMPLES - 1) == 0 { lo } else { hi }]);
        shape = shape.stroke(pts, 1.5, t.wave_trace);
    } else {
        let mut top = Vec::with_capacity(SAMPLES + 1);
        let mut bot = Vec::with_capacity(SAMPLES + 1);
        for i in 0..=SAMPLES {
            let edge = i == 0 || i == SAMPLES || sample(id, i) != sample(id, i - 1);
            let x = (i.min(SAMPLES - 1)) as f32 * pps + if i == SAMPLES { pps } else { 0.0 };
            top.push([x, if edge { mid } else { hi }]);
            bot.push([x, if edge { mid } else { lo }]);
        }
        shape = shape
            .stroke(top, 1.5, t.wave_trace)
            .stroke(bot, 1.5, t.wave_trace);
    }
    shape
}

/// Ruler shape: baseline plus a tick per grid step, spanning the current
/// (demo or live) grid.
fn ruler_shape(s: &AppState, t: &VivadoTheme) -> Shape {
    let pps = s.wave.pps();
    let w = s.wave.sheet_w();
    let n = sample_len(s);
    let mut shape = Shape::new().line(
        [0.0, RULER_H - 0.5],
        [w, RULER_H - 0.5],
        1.0,
        LineCap::Butt,
        t.border,
    );
    let mut g = 0;
    while g <= n {
        let x = g as f32 * pps;
        shape = shape.line([x, 10.0], [x, RULER_H], 1.0, LineCap::Butt, t.text_faint);
        g += GRID_STEP;
    }
    shape
}

/// Grid + trace shape of one live lane: gridlines plus the stepped scalar or
/// bus envelope drawn from the imported engine samples.
fn live_lane_shape(s: &AppState, t: &VivadoTheme, trace: &LiveTrace) -> Shape {
    let pps = s.wave.pps();
    let n = sample_len(s);
    let w = s.wave.sheet_w();
    let mut shape = Shape::new();
    let mut g = 0;
    while g <= n {
        let x = g as f32 * pps;
        shape = shape.line([x, 0.0], [x, LANE_H], 1.0, LineCap::Butt, t.wave_grid);
        g += GRID_STEP;
    }
    let hi = 4.0;
    let lo = LANE_H - 4.0;
    let mid = LANE_H / 2.0;
    let at = |i: usize| trace.value_at(i);
    if !trace.is_bus() {
        let mut pts = vec![[0.0, if at(0) == 0 { lo } else { hi }]];
        for i in 1..n {
            let x = i as f32 * pps;
            let prev = if at(i - 1) == 0 { lo } else { hi };
            let cur = if at(i) == 0 { lo } else { hi };
            // Only emit vertices at transitions plus the hold point: long
            // live grids would otherwise build tens of thousands of points.
            if cur != prev {
                pts.push([x, prev]);
                pts.push([x, cur]);
            }
        }
        pts.push([w, if at(n.saturating_sub(1)) == 0 { lo } else { hi }]);
        shape = shape.stroke(pts, 1.5, t.wave_trace);
    } else {
        let mut top = Vec::new();
        let mut bot = Vec::new();
        for i in 0..=n {
            let edge = i == 0 || i == n || at(i.min(n - 1)) != at((i - 1).min(n - 1));
            let x = (i.min(n - 1)) as f32 * pps + if i == n { pps } else { 0.0 };
            top.push([x, if edge { mid } else { hi }]);
            bot.push([x, if edge { mid } else { lo }]);
        }
        shape = shape
            .stroke(top, 1.5, t.wave_trace)
            .stroke(bot, 1.5, t.wave_trace);
    }
    shape
}

/// Sim toolbar: run controls, zoom, cursor step, A/B + markers, radix.
fn wave_toolbar(s: &AppState, t: &VivadoTheme) -> El {
    let running = s.flow.sim == RunStatus::Running;
    row()
        .key("wave-toolbar")
        .w_full()
        .h(TOOL_H)
        .px(4.0)
        .gap(3.0)
        .items_center()
        .fill(t.header_bg)
        .child(
            tool_btn("wave-run", if running { "❚❚ Pause" } else { "▶ Run" }, t).on_press(
                move |s: &mut AppState| {
                    if running {
                        cancel_sim(s);
                        return;
                    }
                    echo(s, "run 100 ns".into());
                    if let Err(e) = launch_default_tb(s) {
                        s.console.lines.push(ConsoleLine {
                            text: format!("ERROR: {e}"),
                            sev: MsgSev::Error,
                        });
                        s.toasts
                            .push(ToastKind::Error, "Cannot start simulation", &e);
                    }
                },
            ),
        )
        .child(
            tool_btn("wave-restart", "↺ Restart", t).on_press(|s: &mut AppState| {
                s.wave.cursor = 0;
                echo(s, "restart".into());
            }),
        )
        .child(
            tool_btn("wave-step", "Step", t).on_press(|s: &mut AppState| {
                s.wave.step_cursor(1);
                echo(s, "step".into());
            }),
        )
        .child(tool_btn("wave-zoom-out", "−", t).on_press(|s: &mut AppState| s.wave.zoom_out()))
        .child(
            el().key("wave-zoom-label")
                .w(44.0)
                .center()
                .child(dim_label(&s.wave.zoom_label(), t)),
        )
        .child(tool_btn("wave-zoom-in", "+", t).on_press(|s: &mut AppState| s.wave.zoom_in()))
        .child(tool_btn("wave-zoom-fit", "Fit", t).on_press(|s: &mut AppState| s.wave.zoom_fit()))
        .child(
            tool_btn("wave-cursor-prev", "‹", t)
                .on_press(|s: &mut AppState| s.wave.step_cursor(-1)),
        )
        .child(
            tool_btn("wave-cursor-next", "›", t).on_press(|s: &mut AppState| s.wave.step_cursor(1)),
        )
        .child(
            tool_btn("wave-set-a", "Set A", t).on_press(|s: &mut AppState| {
                let c = s.wave.cursor;
                s.wave.cursor_a = Some(c);
                echo(s, format!("set_cursor A {}", time_label(s, c)));
            }),
        )
        .child(
            tool_btn("wave-set-b", "Set B", t).on_press(|s: &mut AppState| {
                let c = s.wave.cursor;
                s.wave.cursor_b = Some(c);
                echo(s, format!("set_cursor B {}", time_label(s, c)));
            }),
        )
        .child(
            tool_btn("wave-add-marker", "+ Marker", t).on_press(|s: &mut AppState| {
                let n = s.wave.markers.len();
                let c = s.wave.cursor;
                let name = format!("M{n}");
                // Clamp to the live grid when one is mounted; the demo
                // `WaveMarker::new` clamp (SAMPLES) would strand markers on
                // long engine dumps.
                let last = s.wave.sample_len().saturating_sub(1);
                s.wave.markers.push(WaveMarker {
                    name: name.clone(),
                    sample: c.min(last),
                });
                echo(s, format!("add_marker {name} {}", time_label(s, c)));
            }),
        )
        .child(
            tool_btn("wave-radix", s.wave.radix.label(), t).on_press(|s: &mut AppState| {
                s.wave.radix = s.wave.radix.toggle();
                echo(
                    s,
                    format!("set_radix {}", s.wave.radix.label().to_lowercase()),
                );
            }),
        )
}

/// Scope panel: the sim hierarchy tree (testbench top, DUT, clock gen).
fn scope_panel(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("wave-scopes")
        .role("tree")
        .w_full()
        .h(SCOPE_H)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .h(ROW_H)
                .px(6.0)
                .items_center()
                .fill(t.header_bg)
                .child(text("Scope").font(VivadoTheme::FONT_TITLE).color(t.text)),
        );
    for scope in SimScope::ALL {
        let selected = s.wave.scope == scope;
        body = body.child(
            row()
                .key(scope.key())
                .role("treeitem")
                .label(format!("{} ({})", scope.name(), scope.detail()))
                .focus_selected(selected)
                .focusable(true)
                .w_full()
                .h(ROW_H)
                .items_center()
                .gap(4.0)
                .pl(6.0)
                .fill(if selected { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    s.wave.scope = scope;
                    echo(s, format!("select_scope {}", scope.name()));
                })
                .child(
                    icon(&glyphs::TRIANGLE_RIGHT, 9.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                )
                .child(ui_label(scope.name(), t)),
        );
    }
    body
}

/// Objects panel: signals of the scope with live cursor values and per-row
/// `add_wave` for signals not yet on the canvas.
fn objects_panel(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("wave-objects")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .key("wave-objects-head")
                .w_full()
                .h(ROW_H)
                .px(6.0)
                .gap(6.0)
                .items_center()
                .fill(t.header_bg)
                .child(el().w(88.0).child(pin_label("Name", t)))
                .child(el().w(56.0).child(pin_label("Value", t)))
                .child(el().grow(1.0).child(pin_label("Type", t))),
        );
    for obj in objects(s.wave.scope).iter().copied() {
        let value = match (obj.fixed, obj.sig) {
            (Some(f), _) => f.into(),
            (None, Some(id)) => format_value(id, s.wave.cursor, s.wave.radix),
            (None, None) => "-".into(),
        };
        let mut r = row()
            .key(format!("wave-object-row-{}", obj.slug))
            .w_full()
            .h(ROW_H)
            .px(6.0)
            .gap(6.0)
            .items_center()
            .hover(t.selection)
            .child(el().w(88.0).child(ui_label(obj.name, t)))
            .child(
                el().key(format!("wave-object-value-{}", obj.slug))
                    .w(56.0)
                    .child(mono_value(&value, t)),
            )
            .child(el().grow(1.0).min_w(0.0).child(pin_label(obj.dtype, t)));
        if let Some(id) = obj.sig {
            if !s.wave.is_visible(id) {
                r = r.child(
                    tool_btn(&format!("wave-object-add-{}", obj.slug), "+", t)
                        .label(format!("Add {} to wave", obj.name))
                        .on_press(move |s: &mut AppState| add_object(s, obj)),
                );
            }
        }
        body = body.child(r);
    }
    body
}

/// Live-source banner: names the engine import behind the lanes, or labels
/// the demo fallback grid so pre-run traces are never mistaken for sim data.
fn live_banner(s: &AppState, t: &VivadoTheme) -> El {
    let (label, ink) = match &s.wave.live {
        Some(live) => (
            format!(
                "Live: {} · {} signals, {} samples{}",
                live.source,
                live.traces.len(),
                live.sample_len(),
                if live.truncated { " (truncated)" } else { "" },
            ),
            t.pass,
        ),
        None if s.sim.is_active() || s.flow.sim == RunStatus::Running => {
            ("Simulating… streaming into the wave viewer".into(), t.link)
        }
        None => (
            "Demo traces — Run Simulation for live VCD data".into(),
            t.text_faint,
        ),
    };
    row()
        .key("wave-live-banner")
        .w_full()
        .h(20.0)
        .px(8.0)
        .items_center()
        .fill(t.header_bg)
        .child(text(label).font(VivadoTheme::FONT_UI).color(ink))
}

/// Live Name/Value column: one row per imported engine trace (first
/// [`LIVE_SHOWN`]), selection shared with the live lanes.
fn live_names_column(s: &AppState, t: &VivadoTheme, live: &LiveWave) -> El {
    let mut body = col()
        .key("wave-names")
        .w(NAME_W)
        .h_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .key("wave-names-head")
                .w_full()
                .h(RULER_H)
                .px(6.0)
                .gap(6.0)
                .items_center()
                .fill(t.header_bg)
                .child(el().w(119.0).child(pin_label("Name", t)))
                .child(el().grow(1.0).child(pin_label("Value", t))),
        );
    for (idx, trace) in live.traces.iter().enumerate().take(LIVE_SHOWN) {
        let selected = s.wave.live_selected == Some(idx);
        body = body.child(
            row()
                .key(format!("wave-live-name-{idx}"))
                .role("treeitem")
                .label(&trace.name)
                .focus_selected(selected)
                .focusable(true)
                .w_full()
                .h(LANE_H)
                .px(6.0)
                .gap(6.0)
                .items_center()
                .fill(if selected { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| select_live_trace(s, idx))
                .child(
                    icon(
                        if trace.is_bus() {
                            &glyphs::TRIANGLE_DOWN
                        } else {
                            &glyphs::TRIANGLE_RIGHT
                        },
                        9.0,
                    )
                    .color(if trace.is_bus() {
                        t.text_dim
                    } else {
                        t.panel_bg
                    })
                    .a11y_hidden(),
                )
                .child(el().w(104.0).child(ui_label(trace.leaf(), t)))
                .child(
                    el().key(format!("wave-live-value-{idx}"))
                        .grow(1.0)
                        .min_w(0.0)
                        .child(mono_value(
                            &format_live_value(trace, s.wave.cursor, s.wave.radix),
                            t,
                        )),
                ),
        );
    }
    if live.traces.len() > LIVE_SHOWN {
        body = body.child(
            row()
                .key("wave-live-more")
                .w_full()
                .h(ROW_H)
                .px(6.0)
                .items_center()
                .child(pin_label(
                    &format!("+{} more in VCD", live.traces.len() - LIVE_SHOWN),
                    t,
                )),
        );
    }
    body
}

/// Signal Name/Value column: live engine rows once imported, else the demo
/// fallback rows.
fn names_column(s: &AppState, t: &VivadoTheme) -> El {
    if let Some(live) = &s.wave.live {
        if !live.is_empty() {
            return live_names_column(s, t, live);
        }
    }
    let mut body = col()
        .key("wave-names")
        .w(NAME_W)
        .h_full()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .key("wave-names-head")
                .w_full()
                .h(RULER_H)
                .px(6.0)
                .gap(6.0)
                .items_center()
                .fill(t.header_bg)
                .child(el().w(119.0).child(pin_label("Name", t)))
                .child(el().grow(1.0).child(pin_label("Value", t))),
        );
    for id in s.wave.visible() {
        let selected = s.wave.selected == Some(id);
        body = body.child(
            row()
                .key(id.name_key())
                .role("treeitem")
                .label(id.name())
                .focus_selected(selected)
                .focusable(true)
                .w_full()
                .h(LANE_H)
                .px(6.0)
                .gap(6.0)
                .items_center()
                .fill(if selected { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| select_trace(s, id))
                .child(
                    icon(
                        if id.is_bus() {
                            &glyphs::TRIANGLE_DOWN
                        } else {
                            &glyphs::TRIANGLE_RIGHT
                        },
                        9.0,
                    )
                    .color(if id.is_bus() { t.text_dim } else { t.panel_bg })
                    .a11y_hidden(),
                )
                .child(el().w(104.0).child(ui_label(id.name(), t)))
                .child(
                    el().key(id.value_key())
                        .grow(1.0)
                        .min_w(0.0)
                        .child(mono_value(
                            &format_value(id, s.wave.cursor, s.wave.radix),
                            t,
                        )),
                ),
        );
    }
    body
}

/// Time ruler: ticks plus labels, with marker flags that jump the cursor.
fn ruler(s: &AppState, t: &VivadoTheme) -> El {
    let pps = s.wave.pps();
    let mut r = el()
        .key("wave-ruler")
        .w(s.wave.sheet_w())
        .h(RULER_H)
        .fill(t.panel_bg)
        .child(canvas(ruler_shape(s, t)).w_full().h_full());
    let n = sample_len(s);
    // Long live grids label every GRID_STEP samples like the demo grid; the
    // step stays readable because labels sit 64px apart at 100% zoom.
    let mut g = 0;
    while g <= n {
        // The end tick keeps its line but no label: the text would run off
        // the sheet edge.
        if g < n {
            let label = time_label(s, g);
            r = r.child(
                text(label)
                    .font(FONT_PIN)
                    .color(t.text_dim)
                    .absolute(g as f32 * pps + 3.0, 1.0),
            );
        }
        g += GRID_STEP;
    }
    for (i, m) in s.wave.markers.iter().enumerate() {
        let x = m.sample as f32 * pps;
        let sample = m.sample;
        let name = m.name.clone();
        r = r.child(
            el().key(format!("wave-marker-{i}"))
                .role("button")
                .label(format!("Marker {name} at {}", time_label(s, sample)))
                .focusable(true)
                .absolute(x - 4.0, 0.0)
                .size(9.0, RULER_H)
                .fill(t.wave_marker)
                .on_press(move |s: &mut AppState| {
                    s.wave.cursor = sample;
                    echo(s, format!("goto_marker {name}"));
                }),
        );
    }
    r
}

/// One trace lane: the trace canvas plus bus value labels on wide runs.
fn lane(s: &AppState, t: &VivadoTheme, id: WaveSigId) -> El {
    let pps = s.wave.pps();
    let mut l = el()
        .key(id.lane_key())
        .w(s.wave.sheet_w())
        .h(LANE_H)
        .fill(if s.wave.selected == Some(id) {
            t.selection
        } else {
            t.panel_bg
        })
        .child(canvas(lane_shape(s, t, id)).w_full().h_full());
    if id.is_bus() {
        for (a, b, _v) in bus_runs(id) {
            let px = (b - a) as f32 * pps;
            if px >= 28.0 {
                let label = format_value(id, a, s.wave.radix);
                l = l.child(
                    text(label)
                        .font(FONT_PIN)
                        .color(t.text)
                        .absolute(a as f32 * pps + 4.0, 5.0),
                );
            }
        }
    }
    l
}

/// One live trace lane: the engine canvas plus bus value labels on wide runs.
fn live_lane(s: &AppState, t: &VivadoTheme, idx: usize, trace: &LiveTrace) -> El {
    let pps = s.wave.pps();
    let mut l = el()
        .key(format!("wave-live-lane-{idx}"))
        .w(s.wave.sheet_w())
        .h(LANE_H)
        .fill(if s.wave.live_selected == Some(idx) {
            t.selection
        } else {
            t.panel_bg
        })
        .child(canvas(live_lane_shape(s, t, trace)).w_full().h_full());
    if trace.is_bus() {
        for (a, b, _v) in live_bus_runs(trace) {
            let px = (b - a) as f32 * pps;
            if px >= 28.0 {
                let label = format_live_value(trace, a, s.wave.radix);
                l = l.child(
                    text(label)
                        .font(FONT_PIN)
                        .color(t.text)
                        .absolute(a as f32 * pps + 4.0, 5.0),
                );
            }
            // Long live buses can hold thousands of narrow runs; labels only
            // mount on wide ones, so no cap is needed.
        }
    }
    l
}

/// Cursor overlays spanning the lanes: main cursor plus A/B when set.
fn cursor_overlays(s: &AppState, t: &VivadoTheme, lanes_h: f32) -> Vec<El> {
    let pps = s.wave.pps();
    let mut out = vec![
        el().key("wave-cursor")
            .role("separator")
            .label(format!("Cursor at {}", time_label(s, s.wave.cursor)))
            .absolute(s.wave.cursor as f32 * pps - 1.0, 0.0)
            .size(2.0, lanes_h)
            .fill(t.wave_cursor),
    ];
    for (key, name, opt) in [
        ("wave-cursor-a", "A", s.wave.cursor_a),
        ("wave-cursor-b", "B", s.wave.cursor_b),
    ] {
        if let Some(c) = opt {
            out.push(
                el().key(key)
                    .role("separator")
                    .label(format!("Cursor {name} at {}", time_label(s, c)))
                    .absolute(c as f32 * pps - 0.5, 0.0)
                    .size(1.0, lanes_h)
                    .fill(t.wave_marker),
            );
        }
    }
    out
}

/// The trace canvas: ruler plus live engine lanes once imported (else the
/// demo lanes) in one horizontal pan container so ticks, traces, and cursors
/// stay aligned.
fn wave_canvas(s: &AppState, t: &VivadoTheme) -> El {
    let live_count = s.wave.live_shown();
    let use_live = s.wave.has_live() && live_count > 0;
    let lanes_h = if use_live {
        live_count as f32 * LANE_H
    } else {
        s.wave.visible().len() as f32 * LANE_H
    };
    let mut sheet = el().w(s.wave.sheet_w()).h(lanes_h).fill(t.panel_bg);
    if use_live {
        if let Some(live) = &s.wave.live {
            for (row, trace) in live.traces.iter().enumerate().take(live_count) {
                let mut l = live_lane(s, t, row, trace);
                l = l.absolute(0.0, row as f32 * LANE_H);
                sheet = sheet.child(l);
            }
        }
    } else {
        for (row, id) in s.wave.visible().iter().enumerate() {
            let mut l = lane(s, t, *id);
            l = l.absolute(0.0, row as f32 * LANE_H);
            sheet = sheet.child(l);
        }
    }
    for overlay in cursor_overlays(s, t, lanes_h) {
        sheet = sheet.child(overlay);
    }
    el().key("wave-canvas")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .scroll_x()
        .child(col().w(s.wave.sheet_w()).child(ruler(s, t)).child(sheet))
}

/// Readout strip: cursor/A/B/delta times plus the selected signal value.
fn wave_readout(s: &AppState, t: &VivadoTheme) -> El {
    let mut r = row()
        .key("wave-readout")
        .w_full()
        .h(22.0)
        .px(8.0)
        .gap(16.0)
        .items_center()
        .fill(t.header_bg)
        .child(dim_label(
            &format!("Cursor: {}", time_label(s, s.wave.cursor)),
            t,
        ));
    if let Some(a) = s.wave.cursor_a {
        r = r.child(dim_label(&format!("A: {}", time_label(s, a)), t));
    }
    if let Some(b) = s.wave.cursor_b {
        r = r.child(dim_label(&format!("B: {}", time_label(s, b)), t));
    }
    if let (Some(a), Some(b)) = (s.wave.cursor_a, s.wave.cursor_b) {
        let da = time_ps(s, a) as i64;
        let db = time_ps(s, b) as i64;
        let d = (db - da).abs() as u64;
        r = r.child(
            el().key("wave-delta")
                .child(dim_label(&format!("Δ: {}", time_label_ps(d)), t)),
        );
    }
    r = r.child(dim_label(&format!("Markers: {}", s.wave.markers.len()), t));
    let sel = if s.wave.has_live() {
        match s
            .wave
            .live_selected
            .and_then(|i| s.wave.live.as_ref()?.traces.get(i))
        {
            Some(trace) => format!(
                "Selected: {} = {} ({})",
                trace.leaf(),
                format_live_value(trace, s.wave.cursor, s.wave.radix),
                s.wave.radix.label().to_lowercase()
            ),
            None => "No signal selected".into(),
        }
    } else {
        match s.wave.selected {
            Some(id) => format!(
                "Selected: {} = {} ({})",
                id.name(),
                format_value(id, s.wave.cursor, s.wave.radix),
                s.wave.radix.label().to_lowercase()
            ),
            None => "No signal selected".into(),
        }
    };
    r.child(el().key("wave-sel").grow(1.0).child(dim_label(&sel, t)))
}

/// Wave center surface: scopes/objects left, names + trace canvas right.
pub fn wave_pane(s: &AppState, t: &VivadoTheme) -> El {
    col()
        .key("wave")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Waveform - sim_1", "wave-pane", t))
        .child(live_banner(s, t))
        .child(wave_toolbar(s, t))
        .child(
            row()
                .key("wave-body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(6.0)
                .padding(6.0)
                .child(
                    col()
                        .key("wave-left")
                        .w(LEFT_W)
                        .h_full()
                        .gap(6.0)
                        .child(scope_panel(s, t))
                        .child(objects_panel(s, t)),
                )
                .child(
                    col()
                        .key("wave-right")
                        .grow(1.0)
                        .h_full()
                        .min_w(0.0)
                        .min_h(0.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(
                            row()
                                .w_full()
                                .gap(0.0)
                                .child(names_column(s, t))
                                .child(wave_canvas(s, t)),
                        ),
                ),
        )
        .child(wave_readout(s, t))
}

/// Status badge ink for a testbench state.
fn tb_ink(status: TbStatus, t: &VivadoTheme) -> rune::Color {
    match status {
        TbStatus::Passed => t.pass,
        TbStatus::Failed => t.fail,
        TbStatus::Running => t.link,
        TbStatus::Idle => t.text_faint,
    }
}

/// Testbench list: discovered files with run-status badges.
fn tb_list(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("sim-list")
        .w(260.0)
        .h_full()
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            row()
                .w_full()
                .h(ROW_H)
                .px(6.0)
                .items_center()
                .fill(t.header_bg)
                .child(
                    text(format!("Testbenches ({})", TBS.len()))
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text),
                ),
        );
    for (i, tb) in TBS.iter().enumerate() {
        let selected = s.sim.selected == Some(i);
        let status = s.sim.status(i);
        body = body.child(
            row()
                .key(format!("sim-tb-{i}"))
                .role("treeitem")
                .label(format!("{} ({})", tb.name, status.label()))
                .focus_selected(selected)
                .focusable(true)
                .w_full()
                .h(28.0)
                .px(6.0)
                .gap(6.0)
                .items_center()
                .fill(if selected { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    s.sim.selected = Some(i);
                    echo(s, format!("select_objects [get_files {}]", TBS[i].name));
                })
                .child(el().grow(1.0).min_w(0.0).child(ui_label(tb.name, t)))
                .child(
                    el().key(format!("sim-tb-status-{i}"))
                        .child(text(status.label()).font(FONT_PIN).color(tb_ink(status, t))),
                ),
        );
    }
    body
}

/// One label/value detail row.
fn detail_row(key: &str, label: &str, value: &str, t: &VivadoTheme) -> El {
    row()
        .key(key)
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .gap(8.0)
        .items_center()
        .child(el().w(120.0).child(pin_label(label, t)))
        .child(el().grow(1.0).min_w(0.0).child(ui_label(value, t)))
}

/// Testbench detail: DUT instance, port map, clocks, decls, last run, and
/// the run / open-waveform actions.
fn tb_detail(s: &AppState, t: &VivadoTheme) -> El {
    let Some(i) = s.sim.selected else {
        return el()
            .key("sim-detail-prompt")
            .grow(1.0)
            .h_full()
            .center()
            .child(dim_label("Select a testbench to see its details", t));
    };
    let i = i.min(TBS.len() - 1);
    let tb = &TBS[i];
    let status = s.sim.status(i);
    let mut body = col()
        .key("sim-detail")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(detail_row("sim-detail-name", "Testbench", tb.name, t))
        .child(detail_row("sim-detail-path", "Path", tb.path, t))
        .child(
            row()
                .key("sim-detail-status")
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .gap(8.0)
                .items_center()
                .child(el().w(120.0).child(pin_label("Status", t)))
                .child(
                    el().grow(1.0).min_w(0.0).child(
                        text(status.label())
                            .font(VivadoTheme::FONT_UI)
                            .color(tb_ink(status, t)),
                    ),
                ),
        )
        .child(detail_row(
            "sim-detail-dut",
            "DUT instance",
            &format!("{} {}", tb.inst, tb.dut),
            t,
        ))
        .child(detail_row(
            "sim-detail-vcd",
            "Wave database",
            &match &s.wave.live {
                Some(live) => live.source.clone(),
                None => format!("{} (not simulated yet)", tb.vcd),
            },
            t,
        ));
    body = body.child(
        row()
            .key("sim-detail-actions")
            .w_full()
            .h(TOOL_H)
            .px(8.0)
            .gap(6.0)
            .items_center()
            .child(tool_btn("sim-detail-run", "▶ Run", t).on_press(run_selected_tb))
            .child(
                tool_btn("sim-detail-stop", "■ Stop", t).on_press(|s: &mut AppState| cancel_sim(s)),
            )
            .child(
                tool_btn("sim-detail-open-wave", "Open Waveform", t).on_press(
                    |s: &mut AppState| {
                        s.navigate(crate::state::CenterPane::Wave);
                        echo(s, "open_wave_config".into());
                    },
                ),
            ),
    );
    body = body.child(
        col().key("sim-detail-ports").w_full().child(
            row()
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .fill(t.header_bg)
                .child(text("Port map").font(VivadoTheme::FONT_TITLE).color(t.text)),
        ),
    );
    for (port, sig) in tb_ports(i) {
        body = body.child(detail_row(&format!("sim-port-{port}"), port, sig, t));
    }
    let clocks: Vec<String> = tb_clocks(i)
        .iter()
        .map(|(c, p)| format!("{c} ({p})"))
        .collect();
    body = body.child(detail_row(
        "sim-detail-clocks",
        "Clocks",
        &clocks.join(", "),
        t,
    ));
    let decls: Vec<String> = tb_decls(i)
        .iter()
        .map(|(n, w, init)| format!("{n}[{w}] = {init}"))
        .collect();
    body = body.child(detail_row(
        "sim-detail-decls",
        "Decls",
        &decls.join(", "),
        t,
    ));
    body = body.child(
        col().key("sim-detail-history").w_full().child(
            row()
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .items_center()
                .fill(t.header_bg)
                .child(
                    text("Run history")
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text),
                ),
        ),
    );
    if s.sim.history.is_empty() {
        body = body.child(detail_row(
            "sim-detail-history-empty",
            "Runs",
            "No runs yet",
            t,
        ));
    }
    for (h, run) in s.sim.history.iter().enumerate() {
        body = body.child(detail_row(
            &format!("sim-detail-history-{h}"),
            run.tb,
            &format!(
                "{} ({} units, {} errors)",
                run.status.label(),
                run.units,
                run.errors
            ),
            t,
        ));
    }
    body
}

/// Sim center surface: testbench list plus detail.
pub fn sim_pane(s: &AppState, t: &VivadoTheme) -> El {
    col()
        .key("sim")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Simulation - sim_1", "sim-pane", t))
        .child(
            row()
                .key("sim-toolbar")
                .w_full()
                .h(TOOL_H)
                .px(4.0)
                .gap(3.0)
                .items_center()
                .fill(t.header_bg)
                .child(
                    tool_btn("sim-discover", "Discover", t).on_press(|s: &mut AppState| {
                        echo(s, "get_files -filter {NAME =~ *_tb*}".into());
                    }),
                ),
        )
        .child(
            row()
                .key("sim-body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(6.0)
                .padding(6.0)
                .child(tb_list(s, t))
                .child(tb_detail(s, t)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        Runtime::new(state, crate::view, spec.size, env)
    }

    fn wave_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Wave;
        s
    }

    fn sim_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Sim;
        s
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn vcd_shapes_have_the_expected_edges() {
        assert_eq!(sample(WaveSigId::Clk, 0), 0);
        assert_eq!(sample(WaveSigId::Clk, 1), 1);
        assert_eq!(sample(WaveSigId::RstN, 0), 0);
        assert_eq!(sample(WaveSigId::RstN, 8), 1);
        assert_eq!(sample(WaveSigId::DivClk, 0), 0);
        assert_eq!(sample(WaveSigId::DivClk, 16), 1);
        assert_eq!(sample(WaveSigId::Counter, 0), 0);
        assert_eq!(sample(WaveSigId::Counter, 4), 2);
        assert_eq!(sample(WaveSigId::Done, 87), 0);
        assert_eq!(sample(WaveSigId::Done, 88), 1);
        assert_eq!(
            sample(WaveSigId::Clk, 9999),
            sample(WaveSigId::Clk, SAMPLES - 1)
        );
    }

    #[test]
    fn virtual_bus_packs_members_lsb_first() {
        // divclk rises at 16 while led[3:0] is 2 there: 0b1_0010.
        assert_eq!(sample(WaveSigId::Led, 16), 2);
        assert_eq!(sample(WaveSigId::DivClk, 16), 1);
        assert_eq!(sample(WaveSigId::VbLed, 16), 0b1_0010);
        assert_eq!(sample(WaveSigId::VbLed, 0), 0);
        for i in [0, 7, 16, 31, 48, 95] {
            let expect = sample(WaveSigId::Led, i) | (sample(WaveSigId::DivClk, i) << 4);
            assert_eq!(
                pack_virtual(&WaveSigId::VB_MEMBERS, i),
                expect,
                "sample {i}"
            );
        }
    }

    #[test]
    fn values_format_per_radix_without_mutating_samples() {
        assert_eq!(format_value(WaveSigId::Clk, 1, WaveRadix::Hex), "1");
        assert_eq!(format_value(WaveSigId::Led, 16, WaveRadix::Hex), "2");
        assert_eq!(format_value(WaveSigId::Led, 16, WaveRadix::Bin), "0010");
        assert_eq!(format_value(WaveSigId::Counter, 4, WaveRadix::Hex), "02");
        assert_eq!(
            format_value(WaveSigId::Counter, 4, WaveRadix::Bin),
            "00000010"
        );
        assert_eq!(format_value(WaveSigId::VbLed, 16, WaveRadix::Hex), "12");
    }

    #[test]
    fn bus_runs_cover_the_grid_without_gaps() {
        for id in [WaveSigId::Counter, WaveSigId::Led, WaveSigId::VbLed] {
            let runs = bus_runs(id);
            assert_eq!(runs.first().map(|r| r.0), Some(0));
            assert_eq!(runs.last().map(|r| r.1), Some(SAMPLES));
            for w in runs.windows(2) {
                assert_eq!(w[0].1, w[1].0);
                assert_ne!(w[0].2, w[1].2);
            }
        }
    }

    #[test]
    fn added_signals_mount_once_and_never_duplicate() {
        let mut w = WaveState::default();
        assert_eq!(w.visible().len(), WaveSigId::DEFAULT.len());
        assert!(w.add_signal(WaveSigId::Seg));
        assert!(!w.add_signal(WaveSigId::Seg));
        assert!(!w.add_signal(WaveSigId::Clk));
        assert!(w.visible().contains(&WaveSigId::Seg));
    }

    #[test]
    fn cursor_clamps_to_the_grid() {
        let mut w = WaveState::default();
        w.cursor = 0;
        w.step_cursor(-5);
        assert_eq!(w.cursor, 0);
        w.cursor = SAMPLES - 1;
        w.step_cursor(5);
        assert_eq!(w.cursor, SAMPLES - 1);
    }

    #[test]
    fn wave_tree_exposes_scopes_objects_names_canvas_and_readout() {
        let mut rt = harness_with(wave_state());
        rt.frame(0.0);
        for key in [
            "wave",
            "wave-toolbar",
            "wave-run",
            "wave-restart",
            "wave-step",
            "wave-zoom-out",
            "wave-zoom-label",
            "wave-zoom-in",
            "wave-zoom-fit",
            "wave-cursor-prev",
            "wave-cursor-next",
            "wave-set-a",
            "wave-set-b",
            "wave-add-marker",
            "wave-radix",
            "wave-scopes",
            "wave-scope-tb",
            "wave-scope-dut",
            "wave-scope-clkgen",
            "wave-objects",
            "wave-objects-head",
            "wave-object-row-clk",
            "wave-object-value-clk",
            "wave-object-add-done",
            "wave-names",
            "wave-names-head",
            "wave-name-clk",
            "wave-value-clk",
            "wave-name-counter",
            "wave-value-counter",
            "wave-name-vb-led",
            "wave-value-vb-led",
            "wave-ruler",
            "wave-canvas",
            "wave-lane-clk",
            "wave-lane-counter",
            "wave-cursor",
            "wave-readout",
            "wave-sel",
        ] {
            assert!(rt.find(key).is_some(), "missing wave key: {key}");
        }
        assert!(rt.find("wave-cursor-a").is_none(), "A mounts only once set");
        assert!(rt.find("wave-cursor-b").is_none(), "B mounts only once set");
        assert!(rt.find("wave-delta").is_none(), "delta needs both cursors");
        assert!(rt.find("wave-marker-0").is_none(), "no markers by default");
        assert!(
            rt.find("wave-name-seg").is_none(),
            "pool signals mount via add_wave"
        );
        assert!(
            rt.find("wave-object-add-clk").is_none(),
            "mounted signals offer no add"
        );
    }

    #[test]
    fn scope_switch_flips_the_objects_panel() {
        let mut rt = harness_with(wave_state());
        rt.frame(0.0);
        tap(&mut rt, "wave-scope-dut");
        assert_eq!(rt.state.wave.scope, SimScope::Dut);
        assert!(
            rt.find("wave-object-row-seg").is_some(),
            "dut scope shows seg"
        );
        assert!(
            rt.find("wave-object-add-seg").is_some(),
            "seg offers add_wave"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "select_scope dut"),
            "scope echoes its Tcl verb"
        );
    }

    #[test]
    fn add_wave_mounts_a_pool_signal_and_echoes_tcl() {
        let mut rt = harness_with(wave_state());
        rt.frame(0.0);
        tap(&mut rt, "wave-scope-dut");
        tap(&mut rt, "wave-object-add-seg");
        assert!(rt.state.wave.is_visible(WaveSigId::Seg));
        assert!(rt.find("wave-name-seg").is_some(), "seg name row mounts");
        assert!(rt.find("wave-lane-seg").is_some(), "seg lane mounts");
        assert!(
            rt.find("wave-object-add-seg").is_none(),
            "add leaves once mounted"
        );
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "add_wave {seg[7:0]}"),
            "add echoes its Tcl verb"
        );
    }

    #[test]
    fn trace_select_cross_probes_to_the_console() {
        let mut rt = harness_with(wave_state());
        rt.frame(0.0);
        tap(&mut rt, "wave-name-counter");
        assert_eq!(rt.state.wave.selected, Some(WaveSigId::Counter));
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "select_objects [get_waves {counter[7:0]}]"),
            "select echoes its Tcl verb"
        );
    }

    #[test]
    fn cursors_measure_delta_and_markers_jump() {
        let mut rt = harness_with(wave_state());
        rt.frame(0.0);
        tap(&mut rt, "wave-set-a");
        assert_eq!(rt.state.wave.cursor_a, Some(48));
        tap(&mut rt, "wave-cursor-next");
        tap(&mut rt, "wave-set-b");
        assert_eq!(rt.state.wave.cursor_b, Some(49));
        assert!(
            rt.find("wave-delta").is_some(),
            "delta mounts with both cursors"
        );
        tap(&mut rt, "wave-add-marker");
        assert_eq!(rt.state.wave.markers.len(), 1);
        assert!(rt.find("wave-marker-0").is_some(), "marker flag mounts");
        tap(&mut rt, "wave-cursor-prev");
        tap(&mut rt, "wave-marker-0");
        assert_eq!(rt.state.wave.cursor, 49, "marker jumps the cursor back");
        tap(&mut rt, "wave-radix");
        assert_eq!(rt.state.wave.radix, WaveRadix::Bin);
    }

    #[test]
    fn sim_tree_exposes_list_and_prompt_without_selection() {
        let mut rt = harness_with(sim_state());
        rt.frame(0.0);
        for key in [
            "sim",
            "sim-toolbar",
            "sim-discover",
            "sim-list",
            "sim-tb-0",
            "sim-tb-status-0",
            "sim-tb-1",
            "sim-tb-status-1",
            "sim-detail-prompt",
        ] {
            assert!(rt.find(key).is_some(), "missing sim key: {key}");
        }
        assert!(rt.find("sim-detail").is_none(), "detail needs a selection");
        assert!(rt.find("sim-detail-run").is_none(), "run needs a selection");
    }

    #[test]
    fn sim_select_runs_and_opens_the_waveform() {
        let mut rt = harness_with(sim_state());
        rt.frame(0.0);
        tap(&mut rt, "sim-tb-1");
        assert_eq!(rt.state.sim.selected, Some(1));
        for key in [
            "sim-detail",
            "sim-detail-name",
            "sim-detail-dut",
            "sim-detail-ports",
            "sim-detail-clocks",
            "sim-detail-decls",
            "sim-detail-run",
            "sim-detail-open-wave",
            "sim-detail-history",
        ] {
            assert!(rt.find(key).is_some(), "missing sim detail key: {key}");
        }
        tap(&mut rt, "sim-detail-run");
        assert_eq!(rt.state.sim.status(1), TbStatus::Running);
        assert_eq!(rt.state.flow.sim, RunStatus::Running);
        assert_eq!(rt.state.sim.history.len(), 1);
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "launch_simulation -simset sim_1 -tb project1_tb"),
            "run echoes its Tcl verb"
        );
        tap(&mut rt, "sim-detail-open-wave");
        assert_eq!(rt.state.center, crate::state::CenterPane::Wave);
        assert!(rt.find("wave").is_some(), "waveform opens");
    }

    const LIVE_VCD: &str = "\
$date\n  test\n$end\n$version\n  helion test\n$end\n$timescale 1ns $end\n\
$scope module top $end\n$var wire 1 ! clk $end\n$var wire 4 \" cnt $end\n\
$upscope $end\n$enddefinitions $end\n\
#0\n$dumpvars\n0!\nb0000 \"\n$end\n#5\n1!\n#10\n0!\nb0001 \"\n#15\n1!\n#20\n0!\nb0010 \"\n";

    fn seed_live(s: &mut AppState) {
        let mut live = import_vcd_text(LIVE_VCD).expect("seed VCD must parse");
        live.source = "seed (test)".into();
        s.vcd.timescale_ps = live.timescale_ps.max(1);
        s.vcd.duration_ps = live.duration_ps;
        s.vcd.signals = live.traces.iter().map(|t| t.name.clone()).collect();
        s.wave.live = Some(live);
        s.wave.clamp_to_grid();
    }

    fn live_state() -> AppState {
        let mut s = wave_state();
        seed_live(&mut s);
        s
    }

    #[test]
    fn live_import_parses_scalar_and_bus_on_the_viewer_grid() {
        let live = import_vcd_text(LIVE_VCD).expect("parse");
        assert_eq!(live.traces.len(), 2);
        assert_eq!(live.traces[0].name, "top.clk");
        assert_eq!(live.traces[0].width, 1);
        assert_eq!(live.traces[1].name, "top.cnt");
        assert_eq!(live.traces[1].width, 4);
        assert_eq!(live.timescale_ps, 1_000);
        assert_eq!(live.traces[0].samples.len(), 21);
        assert_eq!(live.traces[0].samples[5], 1);
        assert_eq!(live.traces[0].samples[10], 0);
        assert_eq!(live.traces[1].samples[10], 1);
        assert_eq!(live.traces[1].samples[20], 2);
    }

    #[test]
    fn live_import_rejects_malformed_loudly() {
        assert!(import_vcd_text("").is_err());
        assert!(import_vcd_text("not a vcd at all").is_err());
        assert!(import_vcd_text("$timescale 1ps $end\n").is_err());
        let backwards = "$timescale 1ps $end\n$scope module t $end\n$var wire 1 ! a $end\n\
            $upscope $end\n$enddefinitions $end\n#10\n1!\n#5\n0!\n";
        assert!(import_vcd_text(backwards).is_err());
    }

    #[test]
    fn live_import_maps_xz_to_zero() {
        let vcd = "$timescale 1ps $end\n$scope module t $end\n$var wire 1 ! a $end\n\
            $upscope $end\n$enddefinitions $end\n#0\nx!\n#10\nz!\n#20\n1!\n";
        let live = import_vcd_text(vcd).expect("parse");
        assert_eq!(live.traces[0].samples[0], 0);
        assert_eq!(live.traces[0].samples[10], 0);
        assert_eq!(live.traces[0].samples[20], 1);
    }

    #[test]
    fn live_bus_runs_cover_live_samples_without_gaps() {
        let live = import_vcd_text(LIVE_VCD).expect("parse");
        let runs = live_bus_runs(&live.traces[1]);
        assert_eq!(runs.first().map(|r| r.0), Some(0));
        assert_eq!(runs.last().map(|r| r.1), Some(21));
        for w in runs.windows(2) {
            assert_eq!(w[0].1, w[1].0);
            assert_ne!(w[0].2, w[1].2);
        }
    }

    #[test]
    fn engine_vcd_runs_the_helion_sim_counter() {
        let src =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
        let src = std::fs::read_to_string(&src).expect("counter.sv must read");
        let (live, lines) =
            run_engine_vcd(&src, "counter.sv", 16, LIVE_PERIOD_PS).expect("engine run");
        assert!(!live.traces.is_empty(), "engine must dump signals");
        assert!(
            live.traces.iter().any(|t| t.leaf() == "clk"),
            "dump must carry clk"
        );
        let clk = live.traces.iter().find(|t| t.leaf() == "clk").expect("clk");
        assert!(
            clk.samples.contains(&0) && clk.samples.contains(&1),
            "live clk must toggle from the kernel, not the demo shape"
        );
        assert!(
            lines.iter().any(|(_, l)| l.starts_with("synth_design ")),
            "stages must log"
        );
        assert!(
            lines.iter().any(|(_, l)| l.starts_with("dump_vcd ")),
            "import must log"
        );
    }

    #[test]
    fn sim_launch_streams_log_and_mounts_live() {
        let mut s = init();
        s.sim.selected = Some(0);
        launch_tb(&mut s, 0).expect("launch");
        assert_eq!(s.sim.status(0), TbStatus::Running);
        assert_eq!(s.flow.sim, RunStatus::Running);
        let start = std::time::Instant::now();
        while s.sim.is_active() {
            poll_sim(&mut s);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(120),
                "sim timed out"
            );
            if s.sim.is_active() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        poll_sim(&mut s);
        assert!(!s.sim.log.is_empty(), "engine must stream sim log lines");
        assert!(
            s.sim
                .log
                .iter()
                .any(|l| l.text.starts_with("synth_design ")),
            "stages must stream"
        );
        let live = s.wave.live.as_ref().expect("VCD import must mount");
        assert!(!live.traces.is_empty(), "live traces must load");
        assert!(live.cycles > 0, "live must record its engine cycles");
        assert!(!s.vcd.signals.is_empty(), "vcd dims must follow the import");
        assert_eq!(s.sim.status(0), TbStatus::Passed);
        assert_eq!(s.flow.sim, RunStatus::Complete);
    }

    #[test]
    fn wave_live_banner_mounts_and_live_lanes_replace_demo() {
        let mut rt = harness_with(live_state());
        rt.frame(0.0);
        assert!(
            rt.find("wave-live-banner").is_some(),
            "live source banner must mount"
        );
        assert!(
            rt.find("wave-live-name-0").is_some(),
            "live name row must mount"
        );
        assert!(
            rt.find("wave-live-value-0").is_some(),
            "live value must mount"
        );
        assert!(
            rt.find("wave-live-lane-0").is_some(),
            "live lane must mount"
        );
        assert!(
            rt.find("wave-name-clk").is_none(),
            "demo lanes must leave once live mounts"
        );
        assert!(
            rt.find("wave-cursor").is_some(),
            "cursor stays on the live grid"
        );
        assert!(
            rt.find("wave-readout").is_some(),
            "readout stays on the live grid"
        );
    }

    #[test]
    fn live_radix_reformats_live_values() {
        let mut rt = harness_with(live_state());
        rt.frame(0.0);
        // top.cnt is the bus (trace 1); cursor 10 holds value 1 there.
        rt.state.wave.cursor = 10;
        rt.state.wave.live_selected = Some(1);
        rt.frame(0.0);
        assert!(
            rt.find("wave-live-value-1").is_some(),
            "live bus value must mount"
        );
        tap(&mut rt, "wave-radix");
        assert_eq!(rt.state.wave.radix, WaveRadix::Bin);
        let trace = &rt.state.wave.live.as_ref().expect("live").traces[1];
        assert_eq!(format_live_value(trace, 10, WaveRadix::Bin), "0001");
        assert_eq!(format_live_value(trace, 10, WaveRadix::Hex), "1");
    }

    #[test]
    fn live_markers_clamp_to_the_live_grid_not_the_demo_grid() {
        let mut rt = harness_with(live_state());
        rt.frame(0.0);
        // The seed import spans 21 samples; SAMPLES is 96, so cursor 20 is
        // legal live but the demo clamp (95) must not apply either way: push
        // the cursor to the live end and add a marker there.
        rt.state.wave.cursor = 20;
        rt.frame(0.0);
        tap(&mut rt, "wave-add-marker");
        assert_eq!(rt.state.wave.markers.len(), 1);
        assert_eq!(rt.state.wave.markers[0].sample, 20);
        // A long engine dump exceeds the demo grid: markers must reach it.
        let src =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
        let src = std::fs::read_to_string(&src).expect("counter.sv must read");
        let (live, _) = run_engine_vcd(&src, "counter.sv", 64, LIVE_PERIOD_PS).expect("engine run");
        assert!(
            live.sample_len() > SAMPLES,
            "engine grid must exceed the demo grid"
        );
        rt.state.wave.live = Some(live);
        rt.state.wave.clamp_to_grid();
        rt.state.wave.cursor = 500;
        rt.state.wave.clamp_to_grid();
        rt.frame(0.0);
        tap(&mut rt, "wave-add-marker");
        assert_eq!(
            rt.state.wave.markers[1].sample, 500,
            "markers must reach the live grid"
        );
    }

    #[test]
    fn live_select_cross_probes_the_live_trace() {
        let mut rt = harness_with(live_state());
        rt.frame(0.0);
        tap(&mut rt, "wave-live-name-1");
        assert_eq!(rt.state.wave.live_selected, Some(1));
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "select_objects [get_waves {top.cnt}]"),
            "live select echoes its Tcl verb"
        );
    }
}
