//! Simulation waveform viewer (Vivado SIMULATION layout).
//!
//! New Rune `El` code written from scratch for the Vivado brief. The Wave
//! surface (`CenterPane::Wave`) is the Vivado simulator window: a Scope panel
//! over an Objects panel (signals of the scope with cursor values), a signal
//! Name/Value column, and a trace canvas with a time ruler, the main cursor,
//! A/B measurement cursors, and named markers. Run/restart/next-edge, zoom,
//! and the radix control (Bin/Hex/Dec/Signed, per selected trace or global)
//! live on the toolbar; the readout strip shows cursor/A/B/delta times.
//!
//! Only real data renders in production: before any simulation the pane is
//! an empty state with a real Run button. A run (testbench or free-running
//! design, orchestrated by [`crate::views::tb`]) produces VCD text that the
//! fresh [`import_vcd_text`] parser (IEEE 1364 subset, same grid semantics as
//! `vcd_wave.rs::import_vcd_waves`: uniform grid, zero-order hold,
//! `x`/`z`→0, 1/2/5 coarsening past [`MAX_LIVE_SAMPLES`]) lands in
//! [`WaveState::live`]. Scopes and objects derive from the imported
//! hierarchical names. The built-in demo shapes live in
//! [`crate::views::wave_demo`] and mount only when a scene or test seeds that
//! fixture ([`WaveState::demo`]).
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
//! - `crates/helion-gui/src/ide.rs:4273,4367`: `WaveMarker`/`Waveform` shapes.

use std::collections::HashMap;

use rune::tok::TextStyle;
use rune::{El, LineCap, ScrollSpec, Shape, canvas, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::sources::pane_header;
pub use crate::views::tb::{cancel_sim, launch, launch_default_tb, poll, poll_sim};
pub use crate::views::wave_demo::{
    SAMPLES, SimObject, SimScope, WaveSigId, bus_runs, format_value, objects, pack_virtual, sample,
};

/// Pixels per sample at 100% zoom: the full 96-sample sheet (384pt) fits the
/// trace viewport so every lane, cursor, and marker stays inside the shell.
const BASE_PPS: f32 = 4.0;
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

/// Pin/value type: smaller than CAD body text.
const FONT_PIN: TextStyle = TextStyle::new(10.0, 400.0);

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
    radix.format(trace.value_at(i), trace.width)
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
    /// Unsigned decimal.
    Dec,
    /// Two's-complement decimal over the trace width.
    Signed,
}

impl WaveRadix {
    /// Toolbar order.
    pub const ALL: [WaveRadix; 4] = [
        WaveRadix::Bin,
        WaveRadix::Hex,
        WaveRadix::Dec,
        WaveRadix::Signed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WaveRadix::Hex => "Hex",
            WaveRadix::Bin => "Bin",
            WaveRadix::Dec => "Dec",
            WaveRadix::Signed => "Signed",
        }
    }

    /// Next radix in toolbar order (Hex → Bin keeps the old toggle's
    /// first step; the cycle then visits Dec and Signed).
    pub fn toggle(self) -> Self {
        match self {
            WaveRadix::Hex => WaveRadix::Bin,
            WaveRadix::Bin => WaveRadix::Dec,
            WaveRadix::Dec => WaveRadix::Signed,
            WaveRadix::Signed => WaveRadix::Hex,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "hex" | "hexadecimal" => Some(WaveRadix::Hex),
            "bin" | "binary" => Some(WaveRadix::Bin),
            "dec" | "unsigned" | "decimal" => Some(WaveRadix::Dec),
            "signed" | "sdec" => Some(WaveRadix::Signed),
            _ => None,
        }
    }

    /// Format `v` (masked to `width` bits). Scalars always read `0`/`1`.
    pub fn format(self, v: u64, width: u8) -> String {
        let w = u32::from(width.clamp(1, 64));
        let v = if w >= 64 { v } else { v & ((1u64 << w) - 1) };
        if w == 1 {
            return if v == 0 { "0".into() } else { "1".into() };
        }
        let wu = w as usize;
        match self {
            WaveRadix::Hex => format!("{:0nib$X}", v, nib = wu.div_ceil(4)),
            WaveRadix::Bin => format!("{v:0wu$b}"),
            WaveRadix::Dec => v.to_string(),
            WaveRadix::Signed => {
                let signed = if w >= 64 {
                    v as i64
                } else if (v >> (w - 1)) & 1 == 1 {
                    (v as i64) - (1i64 << w)
                } else {
                    v as i64
                };
                signed.to_string()
            }
        }
    }
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
    /// Per-trace radix overrides by trace name; [`WaveState::radix`] is the
    /// default for traces without one.
    pub trace_radix: HashMap<String, WaveRadix>,
    /// Render the built-in demo fixture ([`crate::views::wave_demo`]) while
    /// no live import exists. Only scenes/tests set it; production starts
    /// on the empty state.
    pub demo: bool,
    /// Selected live scope (hierarchical prefix); `None` = first scope.
    pub live_scope: Option<String>,
    /// Live trace indices mounted as lanes, in lane order; empty = the
    /// first [`LIVE_SHOWN`] traces (unless [`WaveState::lanes_custom`]).
    pub live_lanes: Vec<usize>,
    /// The lane list was edited (add/remove/reorder): `live_lanes` is
    /// authoritative even when empty.
    pub lanes_custom: bool,
    /// Density, fixed-point/analog display, virtual buses, multi-select
    /// ([`crate::views::wave_style`]).
    pub style: crate::views::wave_style::WaveStyleState,
    /// Protocol decoders ([`crate::views::wave_decode`]).
    pub decode: crate::views::wave_decode::WaveDecodeState,
    /// Objects-panel signal filter (matches full names across scopes).
    pub filter: rune::TextEdit,
}

impl Default for WaveState {
    fn default() -> Self {
        Self {
            cursor: 0,
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
            trace_radix: HashMap::new(),
            demo: false,
            live_scope: None,
            live_lanes: Vec::new(),
            lanes_custom: false,
            style: Default::default(),
            decode: Default::default(),
            filter: rune::TextEdit::new(""),
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

    /// Grid length: live sample count once imported, the demo [`SAMPLES`]
    /// under the fixture, else a single empty sample.
    pub fn sample_len(&self) -> usize {
        match &self.live {
            Some(l) => l.sample_len().max(1),
            None if self.demo => SAMPLES,
            None => 1,
        }
    }

    /// Whether live engine traces are mounted (vs the demo fallback grid).
    pub fn has_live(&self) -> bool {
        self.live.as_ref().is_some_and(|l| !l.is_empty())
    }

    /// Whether the pane has anything to draw (live import or demo fixture).
    pub fn has_data(&self) -> bool {
        self.has_live() || self.demo
    }

    /// Radix for the trace named `name`: its override, else the default.
    pub fn radix_for(&self, name: &str) -> WaveRadix {
        self.trace_radix.get(name).copied().unwrap_or(self.radix)
    }

    /// Name of the selected trace (live selection first, then demo).
    pub fn selected_name(&self) -> Option<String> {
        if self.has_live() {
            let live = self.live.as_ref()?;
            return live.traces.get(self.live_selected?).map(|t| t.name.clone());
        }
        if self.demo {
            return self.selected.map(|id| id.name().to_string());
        }
        None
    }

    /// Radix shown by the toolbar: the selected trace's, else the default.
    pub fn active_radix(&self) -> WaveRadix {
        match self.selected_name() {
            Some(n) => self.radix_for(&n),
            None => self.radix,
        }
    }

    /// Apply `radix` to the selected trace, or to every trace without an
    /// override when nothing is selected. Returns the Tcl echo.
    pub fn set_radix(&mut self, radix: WaveRadix) -> String {
        match self.selected_name() {
            Some(name) => {
                self.trace_radix.insert(name.clone(), radix);
                if let Some(d) = self.style.display.get_mut(&name) {
                    d.fixed = None;
                }
                format!(
                    "set_property radix {} [get_waves {{{name}}}]",
                    radix.label().to_lowercase()
                )
            }
            None => {
                self.radix = radix;
                format!("set_radix {}", radix.label().to_lowercase())
            }
        }
    }

    /// Live traces shown as lanes (first [`LIVE_SHOWN`] by default; the rest
    /// stay in Objects until added, so the sheet stays compact).
    pub fn live_shown(&self) -> usize {
        self.lanes().len()
    }

    /// Mounted live lane trace indices, in lane order.
    pub fn lanes(&self) -> Vec<usize> {
        let n = self.live.as_ref().map(|l| l.traces.len()).unwrap_or(0);
        if self.live_lanes.is_empty() && !self.lanes_custom {
            (0..n.min(LIVE_SHOWN)).collect()
        } else {
            self.live_lanes.iter().copied().filter(|&i| i < n).collect()
        }
    }

    /// Mount live trace `idx` as a lane; `false` when already mounted.
    pub fn add_live_lane(&mut self, idx: usize) -> bool {
        let mut lanes = self.lanes();
        let n = self.live.as_ref().map(|l| l.traces.len()).unwrap_or(0);
        if idx >= n || lanes.contains(&idx) {
            return false;
        }
        lanes.push(idx);
        self.live_lanes = lanes;
        self.lanes_custom = true;
        true
    }

    /// Selected live scope, defaulting to the first scope of the import.
    pub fn current_live_scope(&self) -> Option<String> {
        let live = self.live.as_ref()?;
        let scopes = live_scopes(live);
        match &self.live_scope {
            Some(sel) if scopes.contains(sel) => Some(sel.clone()),
            _ => scopes.into_iter().next(),
        }
    }

    /// Replace the live import: lanes, scope, and selection reset and every
    /// cursor clamps onto the new grid (the cursor parks at the end).
    pub fn mount_live(&mut self, live: LiveWave) {
        self.live = Some(live);
        self.live_lanes.clear();
        self.lanes_custom = false;
        self.style.multi.clear();
        self.decode.pick = None;
        self.rebuild_virtual_buses();
        self.live_scope = None;
        self.live_selected = None;
        self.clamp_to_grid();
        self.cursor = self.sample_len().saturating_sub(1);
    }

    /// Move the cursor to the next transition of the selected trace (one
    /// sample when nothing is selected or no later edge exists).
    pub fn next_edge(&mut self) {
        let n = self.sample_len();
        let from = self.cursor;
        let value_at = |i: usize| -> Option<u64> {
            if let Some(live) = self.live.as_ref().filter(|l| !l.is_empty()) {
                let t = live.traces.get(self.live_selected?)?;
                Some(t.value_at(i))
            } else if self.demo {
                Some(sample(self.selected?, i))
            } else {
                None
            }
        };
        let next = value_at(from).and_then(|v0| (from + 1..n).find(|&i| value_at(i) != Some(v0)));
        match next {
            Some(i) => self.cursor = i,
            None => self.step_cursor(1),
        }
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

/// Hierarchical scope of a trace name: everything before the last `.`.
pub fn trace_scope(name: &str) -> String {
    match name.rfind('.') {
        Some(i) => name[..i].to_string(),
        None => String::new(),
    }
}

/// Distinct scopes of a live import, in first-appearance order.
pub fn live_scopes(live: &LiveWave) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in &live.traces {
        let sc = trace_scope(&t.name);
        if !out.contains(&sc) {
            out.push(sc);
        }
    }
    out
}

/// Add a live Objects row to the canvas, echoing the Vivado verb.
fn add_live_object(s: &mut AppState, idx: usize) {
    let Some(name) = s
        .wave
        .live
        .as_ref()
        .and_then(|l| l.traces.get(idx))
        .map(|t| t.name.clone())
    else {
        return;
    };
    if s.wave.add_live_lane(idx) {
        echo(s, format!("add_wave {{{name}}}"));
    }
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
    let lane_h = s.wave.style.density.lane_h();
    let w = s.wave.sheet_w();
    let mut shape = Shape::new();
    let mut g = 0;
    while g <= SAMPLES {
        let x = g as f32 * pps;
        shape = shape.line([x, 0.0], [x, lane_h], 1.0, LineCap::Butt, t.wave_grid);
        g += GRID_STEP;
    }
    let hi = 4.0;
    let lo = lane_h - 4.0;
    let mid = lane_h / 2.0;
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
fn live_lane_shape(s: &AppState, t: &VivadoTheme, trace: &LiveTrace, lane_h: f32) -> Shape {
    let pps = s.wave.pps();
    let n = sample_len(s);
    if trace.is_bus() && s.wave.display_of(&trace.name).analog.is_some() {
        return crate::views::wave_style::analog_shape(s, t, trace, lane_h, n);
    }
    let w = s.wave.sheet_w();
    let mut shape = Shape::new();
    let mut g = 0;
    while g <= n {
        let x = g as f32 * pps;
        shape = shape.line([x, 0.0], [x, lane_h], 1.0, LineCap::Butt, t.wave_grid);
        g += GRID_STEP;
    }
    let hi = 4.0;
    let lo = lane_h - 4.0;
    let mid = lane_h / 2.0;
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

/// Toolbar button that renders disabled (with the reason as its accessible
/// description and tooltip) when `enabled` is false.
fn tool_btn_state(key: &str, label: &str, t: &VivadoTheme, enabled: bool, why: &str) -> El {
    let btn = tool_btn(key, label, t);
    if enabled {
        btn
    } else {
        btn.disabled(true)
            .focusable(false)
            .description(why)
            .help(why)
            .layer_opacity(0.45)
    }
}

/// Toolbar Run: start the selected testbench (or the free-running design).
fn run_from_wave(s: &mut AppState) {
    echo(s, "run all".into());
    if let Err(e) = launch_default_tb(s) {
        s.console.lines.push(ConsoleLine {
            text: format!("ERROR: {e}"),
            sev: MsgSev::Error,
        });
        s.toasts.push(
            crate::state::ToastKind::Error,
            "Cannot start simulation",
            &e,
        );
    }
}

/// Toolbar Restart: rerun the last simulation from time zero.
fn restart_from_wave(s: &mut AppState) {
    echo(s, "restart".into());
    if let Err(e) = crate::views::tb::rerun_last(s) {
        s.console.lines.push(ConsoleLine {
            text: format!("ERROR: {e}"),
            sev: MsgSev::Error,
        });
        s.toasts.push(
            crate::state::ToastKind::Error,
            "Cannot restart simulation",
            &e,
        );
    }
}

/// Radix segments: apply to the selected trace, or to all traces when none
/// is selected (the leading caption says which).
fn radix_control(s: &AppState, t: &VivadoTheme) -> El {
    let active = s.wave.active_radix();
    // A fixed-point trace reads in its Q format: no radix segment is lit.
    let fixed = s
        .wave
        .selected_trace()
        .is_some_and(|tr| tr.is_bus() && s.wave.display_of(&tr.name).fixed.is_some());
    let target = match s.wave.selected_name() {
        Some(n) => n.rsplit('.').next().unwrap_or(&n).to_string(),
        None => "all".into(),
    };
    let mut segs = row()
        .key("wave-radix")
        .role("radiogroup")
        .label(format!("Radix for {target}"))
        .h(20.0)
        .items_center()
        .border(1.0, t.border);
    for r in WaveRadix::ALL {
        let on = r == active && !fixed;
        segs = segs.child(
            el().key(format!("wave-radix-{}", r.label().to_lowercase()))
                .role("radio")
                .label(r.label())
                .focusable(true)
                .focus_selected(on)
                .h_full()
                .px(6.0)
                .center()
                .fill(if on { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    let verb = s.wave.set_radix(r);
                    echo(s, verb);
                })
                .child(text(r.label()).font(VivadoTheme::FONT_UI).color(t.text)),
        );
    }
    row()
        .gap(4.0)
        .items_center()
        .child(
            el().max_w(96.0).child(
                pin_label(&format!("Radix ({target})"), t)
                    .truncate()
                    .help_when_truncated(format!("Radix ({target})")),
            ),
        )
        .child(segs)
}

/// Sim toolbar: run controls, zoom, cursor step, A/B + markers, radix.
fn wave_toolbar(s: &AppState, t: &VivadoTheme) -> El {
    let running = s.sim.is_active();
    let can_restart = !running && s.sim.last_run().is_some();
    let run_btn = if running {
        tool_btn("wave-run", "■ Stop", t)
            .help("Stop the running simulation")
            .on_press(|s: &mut AppState| cancel_sim(s))
    } else {
        tool_btn("wave-run", "▶ Run", t)
            .help("Run the selected testbench, or the design for RUNTIME_CYCLES when there is none")
            .on_press(run_from_wave)
    };
    row()
        .key("wave-toolbar")
        .w_full()
        .h(TOOL_H)
        .px(4.0)
        .gap(3.0)
        .items_center()
        .fill(t.header_bg)
        .child(run_btn)
        .child(
            tool_btn_state(
                "wave-restart",
                "↺ Restart",
                t,
                can_restart,
                if running {
                    "Stop the running simulation first"
                } else {
                    "No simulation has run yet"
                },
            )
            .on_press(restart_from_wave),
        )
        .child(
            tool_btn("wave-step", "Next edge", t)
                .help("Move the cursor to the next transition of the selected trace")
                .on_press(|s: &mut AppState| s.wave.next_edge()),
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
                let last = s.wave.sample_len().saturating_sub(1);
                s.wave.markers.push(WaveMarker {
                    name: name.clone(),
                    sample: c.min(last),
                });
                echo(s, format!("add_marker {name} {}", time_label(s, c)));
            }),
        )
        .child(el().grow(1.0))
        .child(radix_control(s, t))
}

/// Panel title row shared by the Scope/Objects panels.
fn panel_head(title: &str, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(ROW_H)
        .px(6.0)
        .items_center()
        .fill(t.header_bg)
        .child(text(title).font(VivadoTheme::FONT_TITLE).color(t.text))
}

/// Scope panel: the demo hierarchy under the fixture, else the scopes the
/// live VCD declares.
fn scope_panel(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col()
        .key("wave-scopes")
        .role("tree")
        .w_full()
        .h(SCOPE_H)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(panel_head("Scope", t));
    if let Some(live) = s.wave.live.as_ref().filter(|l| !l.is_empty()) {
        let current = s.wave.current_live_scope();
        for (k, scope) in live_scopes(live).into_iter().enumerate() {
            let selected = current.as_deref() == Some(scope.as_str());
            let shown = if scope.is_empty() {
                "(top)".to_string()
            } else {
                scope.clone()
            };
            let pick = scope.clone();
            body = body.child(
                row()
                    .key(format!("wave-live-scope-{k}"))
                    .role("treeitem")
                    .label(shown.clone())
                    .focus_selected(selected)
                    .focusable(true)
                    .w_full()
                    .h(ROW_H)
                    .items_center()
                    .gap(4.0)
                    .pl(6.0)
                    .pr(6.0)
                    .fill(if selected { t.selection } else { t.panel_bg })
                    .hover(t.selection)
                    .on_press(move |s: &mut AppState| {
                        s.wave.live_scope = Some(pick.clone());
                        echo(s, format!("select_scope {pick}"));
                    })
                    .child(
                        icon(&glyphs::TRIANGLE_RIGHT, 9.0)
                            .color(t.text_dim)
                            .a11y_hidden(),
                    )
                    .child(
                        ui_label(&shown, t)
                            .truncate()
                            .help_when_truncated(shown.clone()),
                    ),
            );
        }
        return body;
    }
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

/// Objects header row (Name / Value / Type).
fn objects_head(t: &VivadoTheme) -> El {
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
        .child(el().grow(1.0).child(pin_label("Type", t)))
}

/// Objects panel over the live import: traces of the selected scope with
/// cursor values; traces not yet on the canvas offer `add_wave`.
fn live_objects_panel(s: &AppState, t: &VivadoTheme, live: &LiveWave) -> El {
    let mut body = col().w_full();
    let scope = s.wave.current_live_scope().unwrap_or_default();
    let lanes = s.wave.lanes();
    let query = object_query(s);
    let mut shown = 0usize;
    for (idx, trace) in live.traces.iter().enumerate() {
        // A filter searches every scope by full name; otherwise the
        // selected scope's traces list.
        let hit = match &query {
            Some(q) => trace.name.to_lowercase().contains(q),
            None => trace_scope(&trace.name) == scope,
        };
        if !hit {
            continue;
        }
        shown += 1;
        let selected = s.wave.live_selected == Some(idx);
        let value = s.wave.trace_value(trace, s.wave.cursor);
        let dtype = if s.wave.is_virtual(&trace.name) {
            format!("vbus [{}:0]", trace.width - 1)
        } else if trace.is_bus() {
            format!("wire [{}:0]", trace.width - 1)
        } else {
            "wire".to_string()
        };
        let mut r = row()
            .key(format!("wave-live-object-{idx}"))
            .role("treeitem")
            .label(trace.name.clone())
            .focus_selected(selected)
            .focusable(true)
            .w_full()
            .h(ROW_H)
            .px(6.0)
            .gap(6.0)
            .items_center()
            .fill(if selected { t.selection } else { t.panel_bg })
            .hover(t.selection)
            .on_press(move |s: &mut AppState| select_live_trace(s, idx))
            .child(
                el().w(88.0).child(
                    ui_label(trace.leaf(), t)
                        .truncate()
                        .help_when_truncated(trace.name.clone()),
                ),
            )
            .child(
                el().w(56.0).child(
                    mono_value(&value, t)
                        .truncate()
                        .help_when_truncated(value.clone()),
                ),
            )
            .child(
                el().grow(1.0)
                    .min_w(0.0)
                    .child(pin_label(&dtype, t).truncate()),
            );
        if !lanes.contains(&idx) {
            r = r.child(
                tool_btn(&format!("wave-live-add-{idx}"), "+", t)
                    .label(format!("Add {} to wave", trace.name))
                    .help("Add to the waveform")
                    .on_press(move |s: &mut AppState| add_live_object(s, idx)),
            );
        }
        body = body.child(r);
    }
    objects_frame(s, t, body, shown)
}

/// Lowercased Objects filter, `None` when empty.
fn object_query(s: &AppState) -> Option<String> {
    let q = s.wave.filter.text.trim().to_lowercase();
    (!q.is_empty()).then_some(q)
}

/// Objects frame: filter field, header, the rows, and an honest
/// no-match line when the filter hides everything.
fn objects_frame(s: &AppState, t: &VivadoTheme, rows: El, shown: usize) -> El {
    let mut frame = col()
        .key("wave-objects")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            el().w_full().padding(4.0).child(crate::views::cad::search_input(
                "wave-objects-filter",
                &s.wave.filter,
                "Filter signals",
                t,
                |s: &mut AppState| &mut s.wave.filter,
            )),
        )
        .child(objects_head(t));
    if shown == 0 {
        if let Some(q) = object_query(s) {
            frame = frame.child(
                row()
                    .key("wave-objects-nomatch")
                    .w_full()
                    .h(ROW_H)
                    .px(6.0)
                    .items_center()
                    .child(dim_label(&format!("No signal matches \"{q}\""), t).truncate()),
            );
        }
    }
    frame.child(
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(rows),
    )
}

/// Objects panel: the demo scope's objects under the fixture, else the live
/// scope's traces.
fn objects_panel(s: &AppState, t: &VivadoTheme) -> El {
    if let Some(live) = s.wave.live.as_ref().filter(|l| !l.is_empty()) {
        return live_objects_panel(s, t, live);
    }
    let mut body = col().w_full();
    let query = object_query(s);
    let mut shown = 0usize;
    for obj in objects(s.wave.scope).iter().copied() {
        if query
            .as_ref()
            .is_some_and(|q| !obj.name.to_lowercase().contains(q))
        {
            continue;
        }
        shown += 1;
        let value = match (obj.fixed, obj.sig) {
            (Some(f), _) => f.into(),
            (None, Some(id)) => format_value(id, s.wave.cursor, s.wave.radix_for(id.name())),
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
    objects_frame(s, t, body, shown)
}

/// Source banner: names the engine import behind the lanes, or labels the
/// demo fixture so its traces are never mistaken for sim data.
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
        None if s.sim.is_active() => (
            "Simulating… the waveform loads when the run finishes".into(),
            t.link,
        ),
        None => ("Demo traces (fixture)".into(), t.text_faint),
    };
    row()
        .key("wave-live-banner")
        .w_full()
        .h(20.0)
        .px(8.0)
        .items_center()
        .fill(t.header_bg)
        .child(
            text(label.clone())
                .font(VivadoTheme::FONT_UI)
                .color(ink)
                .truncate()
                .help_when_truncated(label),
        )
}

/// Live Name/Value column: one row per mounted lane, selection shared with
/// the live lanes; each value uses that trace's radix.
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
    let lanes = s.wave.lanes();
    let sel = s.wave.selection();
    for (lrow, _, lane_h) in crate::views::wave_decode::live_rows(&s.wave) {
        let idx = match lrow {
            crate::views::wave_decode::LiveRow::Trace(idx) => idx,
            crate::views::wave_decode::LiveRow::Decoded { dec, title, lane } => {
                let key = format!("wave-dec-name-{dec}-{}", lane.name.to_lowercase());
                body = body.child(crate::views::wave_decode::decoded_name_row(
                    key, &title, &lane, lane_h, t,
                ));
                continue;
            }
        };
        let Some(trace) = live.traces.get(idx) else {
            continue;
        };
        let selected = sel.contains(&idx);
        let value = s.wave.trace_value(trace, s.wave.cursor);
        body = body.child(
            row()
                .key(format!("wave-live-name-{idx}"))
                .role("treeitem")
                .label(&trace.name)
                .focus_selected(selected)
                .focusable(true)
                .w_full()
                .h(lane_h)
                .px(6.0)
                .gap(6.0)
                .items_center()
                .fill(if selected { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press_tx(move |s: &mut AppState, tx: &mut rune::Tx| {
                    crate::views::wave_style::press_name(s, idx, tx)
                })
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
                .child(
                    el().w(104.0).child(
                        ui_label(trace.leaf(), t)
                            .truncate()
                            .help_when_truncated(trace.name.clone()),
                    ),
                )
                .child(
                    el().key(format!("wave-live-value-{idx}"))
                        .grow(1.0)
                        .min_w(0.0)
                        .child(
                            mono_value(&value, t)
                                .truncate()
                                .help_when_truncated(value.clone()),
                        ),
                ),
        );
    }
    let hidden = live.traces.len().saturating_sub(lanes.len());
    if hidden > 0 {
        body = body.child(
            row()
                .key("wave-live-more")
                .w_full()
                .h(ROW_H)
                .px(6.0)
                .items_center()
                .child(pin_label(&format!("+{hidden} more — add from Objects"), t)),
        );
    }
    body
}

/// Signal Name/Value column: live engine rows once imported, else the demo
/// fixture rows.
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
                .h(s.wave.style.density.lane_h())
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
                            &format_value(id, s.wave.cursor, s.wave.radix_for(id.name())),
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
    // Label every k-th tick so labels keep >= 56pt apart at any zoom.
    let stride = ((56.0 / (GRID_STEP as f32 * pps)).ceil() as usize).max(1) * GRID_STEP;
    let mut g = 0;
    while g <= n {
        // The end tick keeps its line but no label: the text would run off
        // the sheet edge.
        if g < n && g % stride == 0 {
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

/// One demo trace lane: the trace canvas plus bus value labels on wide runs.
fn lane(s: &AppState, t: &VivadoTheme, id: WaveSigId) -> El {
    let pps = s.wave.pps();
    let radix = s.wave.radix_for(id.name());
    let lane_h = s.wave.style.density.lane_h();
    let mut l = el()
        .key(id.lane_key())
        .w(s.wave.sheet_w())
        .h(lane_h)
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
                let label = format_value(id, a, radix);
                l = l.child(
                    text(label)
                        .font(FONT_PIN)
                        .color(t.text)
                        .absolute(a as f32 * pps + 4.0, (lane_h - 14.0) / 2.0),
                );
            }
        }
    }
    l
}

/// One live trace lane: the engine canvas plus bus value labels on wide runs.
fn live_lane(s: &AppState, t: &VivadoTheme, idx: usize, trace: &LiveTrace, lane_h: f32) -> El {
    let pps = s.wave.pps();
    let analog = s.wave.display_of(&trace.name).analog.is_some();
    let mut l = el()
        .key(format!("wave-live-lane-{idx}"))
        .w(s.wave.sheet_w())
        .h(lane_h)
        .fill(if s.wave.selection().contains(&idx) {
            t.selection
        } else {
            t.panel_bg
        })
        .child(canvas(live_lane_shape(s, t, trace, lane_h)).w_full().h_full());
    if trace.is_bus() && !analog {
        for (a, b, _v) in live_bus_runs(trace) {
            let px = (b - a) as f32 * pps;
            let label = s.wave.trace_value(trace, a);
            // Labels mount only where the run is wide enough to hold them.
            if px >= 12.0 + label.chars().count() as f32 * 6.0 {
                l = l.child(
                    text(label)
                        .font(FONT_PIN)
                        .color(t.text)
                        .absolute(a as f32 * pps + 4.0, (lane_h - 14.0) / 2.0),
                );
            }
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
    use crate::views::wave_decode::{LiveRow, decoded_lane, live_rows};
    let use_live = s.wave.has_live();
    let lane_h = s.wave.style.density.lane_h();
    let rows = if use_live { live_rows(&s.wave) } else { Vec::new() };
    let lanes_h = if use_live {
        rows.last().map(|(_, y, h)| y + h).unwrap_or(0.0)
    } else {
        s.wave.visible().len() as f32 * lane_h
    };
    let mut sheet = el()
        .key("wave-sheet")
        .w(s.wave.sheet_w())
        .h(lanes_h)
        .fill(t.panel_bg);
    if use_live {
        if let Some(live) = &s.wave.live {
            for (lrow, y, h) in rows {
                match lrow {
                    LiveRow::Trace(idx) => {
                        if let Some(trace) = live.traces.get(idx) {
                            sheet = sheet.child(live_lane(s, t, idx, trace, h).absolute(0.0, y));
                        }
                    }
                    LiveRow::Decoded { dec, lane, .. } => {
                        let key = format!("wave-dec-lane-{dec}-{}", lane.name.to_lowercase());
                        sheet = sheet.child(decoded_lane(s, key, &lane, h, t).absolute(0.0, y));
                    }
                }
            }
        }
        // Click parks the cursor; Shift-click places A, Alt-click B.
        sheet = sheet.on_drag(crate::views::wave_style::press_sheet);
    } else {
        for (row, id) in s.wave.visible().iter().enumerate() {
            sheet = sheet.child(lane(s, t, *id).absolute(0.0, row as f32 * lane_h));
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
        let d = (db - da).unsigned_abs();
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
                s.wave.trace_value(trace, s.wave.cursor),
                s.wave.format_tag(trace)
            ),
            None => "No signal selected".into(),
        }
    } else {
        match s.wave.selected {
            Some(id) => {
                let radix = s.wave.radix_for(id.name());
                format!(
                    "Selected: {} = {} ({})",
                    id.name(),
                    format_value(id, s.wave.cursor, radix),
                    radix.label().to_lowercase()
                )
            }
            None => "No signal selected".into(),
        }
    };
    r.child(
        el().key("wave-sel").grow(1.0).min_w(0.0).child(
            dim_label(&sel, t)
                .truncate()
                .help_when_truncated(sel.clone()),
        ),
    )
}

/// Empty state before any waveform exists: one sentence plus the one real
/// action (Run, or Stop while a run is in flight).
fn wave_empty(s: &AppState, t: &VivadoTheme) -> El {
    let (message, action) = if s.sim.is_active() {
        (
            format!(
                "Simulating {} — the waveform loads when it finishes.",
                s.sim.active_label()
            ),
            tool_btn("wave-empty-stop", "■ Stop", t).on_press(|s: &mut AppState| cancel_sim(s)),
        )
    } else {
        let ready = crate::views::tb::can_launch(s);
        let why = ready.clone().err().unwrap_or_default();
        (
            "No waveform yet — run a simulation to see its traces here.".to_string(),
            tool_btn_state("wave-empty-run", "▶ Run Simulation", t, ready.is_ok(), &why)
                .on_press(run_from_wave),
        )
    };
    col()
        .key("wave-empty")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .gap(10.0)
        .padding(16.0)
        .child(
            text(message.clone())
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .truncate()
                .help_when_truncated(message),
        )
        .child(action)
}

/// Wave center surface: scopes/objects left, names + trace canvas right; the
/// empty state before any waveform exists.
pub fn wave_pane(s: &AppState, t: &VivadoTheme) -> El {
    let pane = col()
        .key("wave")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Waveform - sim_1", "wave-pane", t));
    if !s.wave.has_data() {
        return pane.child(wave_empty(s, t));
    }
    pane.child(live_banner(s, t))
        .child(wave_toolbar(s, t))
        .child(crate::views::wave_style::trace_bar(s, t))
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
                        .child(objects_panel(s, t))
                        .children(
                            (s.wave.decode.panel_open && s.wave.has_live())
                                .then(|| crate::views::wave_decode::decoder_panel(s, t)),
                        ),
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
        crate::views::wave_demo::seed_demo_wave(&mut s);
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
        let mut w = WaveState {
            demo: true,
            ..WaveState::default()
        };
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
        tap(&mut rt, "wave-radix-bin");
        assert_eq!(
            rt.state.wave.radix,
            WaveRadix::Bin,
            "no selection sets the default"
        );
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
    fn design_run_streams_log_and_mounts_live() {
        let mut s = init();
        launch_default_tb(&mut s).expect("launch");
        assert!(s.sim.is_active());
        assert_eq!(s.flow.sim, crate::state::RunStatus::Running);
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
        assert!(
            s.sim.log.iter().any(|l| l.text.starts_with("elaborated ")),
            "stages must stream: {:?}",
            s.sim.log
        );
        let live = s.wave.live.as_ref().expect("VCD import must mount");
        assert!(!live.traces.is_empty(), "live traces must load");
        assert_eq!(
            live.cycles, s.debug.runtime_cycles,
            "RUNTIME_CYCLES drives it"
        );
        assert!(!s.vcd.signals.is_empty(), "vcd dims must follow the import");
        assert_eq!(s.flow.sim, crate::state::RunStatus::Complete);
    }

    #[test]
    fn production_starts_on_the_empty_state_with_a_real_run() {
        let mut s = init();
        s.center = crate::state::CenterPane::Wave;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("wave-empty").is_some(), "empty state mounts");
        assert!(rt.find("wave-empty-run").is_some(), "one real action");
        assert!(rt.find("wave-name-clk").is_none(), "no demo traces");
        assert!(rt.find("wave-toolbar").is_none());
        tap(&mut rt, "wave-empty-run");
        assert!(rt.state.sim.is_active(), "Run starts a real simulation");
        crate::views::tb::cancel_sim(&mut rt.state);
    }

    #[test]
    fn radix_formats_dec_and_signed_over_the_width() {
        assert_eq!(WaveRadix::Dec.format(0xF, 4), "15");
        assert_eq!(WaveRadix::Signed.format(0xF, 4), "-1");
        assert_eq!(WaveRadix::Signed.format(0x7, 4), "7");
        assert_eq!(WaveRadix::Signed.format(0x80, 8), "-128");
        assert_eq!(WaveRadix::Hex.format(0x1F, 4), "F", "masks to width");
        assert_eq!(WaveRadix::Bin.format(5, 4), "0101");
        assert_eq!(WaveRadix::Signed.format(1, 1), "1", "scalars stay 0/1");
        assert_eq!(WaveRadix::parse("signed"), Some(WaveRadix::Signed));
    }

    #[test]
    fn next_edge_jumps_to_the_selected_trace_transition() {
        let mut s = live_state();
        s.wave.cursor = 0;
        s.wave.live_selected = Some(1); // top.cnt changes every 10 ns
        s.wave.next_edge();
        let cnt = &s.wave.live.as_ref().unwrap().traces[1];
        let c = s.wave.cursor;
        assert!(
            c > 0 && cnt.value_at(c) != cnt.value_at(0),
            "cursor {c} lands on an edge"
        );
        s.wave.live_selected = None;
        let before = s.wave.cursor;
        s.wave.next_edge();
        assert_eq!(s.wave.cursor, before + 1, "no selection steps one sample");
    }

    #[test]
    fn live_scopes_and_objects_add_hidden_traces() {
        let mut s = live_state();
        let live = s.wave.live.clone().unwrap();
        assert_eq!(live_scopes(&live), vec!["top".to_string()]);
        // Only the first lane is mounted; the second is added from Objects.
        s.wave.live_lanes = vec![0];
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("wave-live-scope-0").is_some());
        assert!(rt.find("wave-live-object-1").is_some());
        assert!(rt.find("wave-live-lane-1").is_none());
        tap(&mut rt, "wave-live-add-1");
        assert!(
            rt.find("wave-live-lane-1").is_some(),
            "add_wave mounts the lane"
        );
        assert!(rt.find("wave-live-add-1").is_none());
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
        tap(&mut rt, "wave-radix-bin");
        assert_eq!(
            rt.state.wave.radix_for("top.cnt"),
            WaveRadix::Bin,
            "selected trace only"
        );
        assert_eq!(rt.state.wave.radix, WaveRadix::Hex, "the default stays");
        assert_eq!(rt.state.wave.radix_for("top.clk"), WaveRadix::Hex);
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
