//! Simulation-based ILA (integrated logic analyzer).
//!
//! Vivado-ILA-equivalent flow on top of [`helion_sim`]: pick nets as probes,
//! configure a trigger (edge or value match on any probe), arm a background
//! capture that samples the event kernel once per posedge, keep a pre/post
//! window around the trigger, and view it in a GTKWave-grade waveform panel.
//! Captures round-trip through JSON session files.
//!
//! The capture algorithm is two pure, unit-tested pieces:
//! [`find_trigger`] (trigger search on one probe's trace) and
//! [`apply_window`] (pre/post windowing); [`run_sim_ila_capture`] implements
//! the same semantics online in a single sim pass.

use helion_ir::Design;
use helion_sim::Sim;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};

/// Session file schema version.
pub const SESSION_VERSION: u32 = 1;
/// Hard caps keep captures bounded in memory and the viewer interactive.
pub const MAX_PROBES: usize = 32;
pub const MAX_DEPTH: usize = 65536;
pub const MAX_WIDTH: usize = 64;
pub const MAX_CYCLES: u32 = 2_000_000;

/// Value radix for one ILA probe (bin/hex/dec, GTKWave-style per-trace radix).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IlaRadix {
    Bin,
    Hex,
    Dec,
}

impl IlaRadix {
    pub fn label(self) -> &'static str {
        match self {
            IlaRadix::Bin => "Bin",
            IlaRadix::Hex => "Hex",
            IlaRadix::Dec => "Dec",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bin" | "binary" => Ok(IlaRadix::Bin),
            "hex" | "hexadecimal" => Ok(IlaRadix::Hex),
            "dec" | "decimal" | "unsigned" => Ok(IlaRadix::Dec),
            other => Err(format!("ila radix: unknown {other}")),
        }
    }
}

/// Format one packed probe sample at `width` bits.
pub fn format_value(v: u64, width: usize, radix: IlaRadix) -> String {
    let w = width.max(1);
    match radix {
        IlaRadix::Bin => format!("{v:0w$b}"),
        IlaRadix::Hex => {
            let digits = w.div_ceil(4).max(1);
            format!("0x{v:0digits$X}")
        }
        IlaRadix::Dec => format!("{v}"),
    }
}

/// One ILA probe: an ordered net list sampled as a packed word, MSB first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IlaProbeCfg {
    pub name: String,
    pub nets: Vec<String>,
    pub radix: IlaRadix,
}

impl IlaProbeCfg {
    pub fn scalar(name: &str, net: &str) -> Self {
        Self {
            name: name.into(),
            nets: vec![net.into()],
            radix: IlaRadix::Bin,
        }
    }

    pub fn width(&self) -> usize {
        self.nets.len()
    }

    pub fn is_bus(&self) -> bool {
        self.nets.len() > 1
    }
}

/// Trigger condition on one probe's samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IlaTriggerKind {
    /// Fire on the first sample (no search).
    Immediate,
    /// LSB 0 → 1.
    Rising,
    /// LSB 1 → 0.
    Falling,
    /// `(sample & mask) == (value & mask)`.
    ValueMatch { value: u64, mask: u64 },
}

impl IlaTriggerKind {
    pub fn label(self) -> &'static str {
        match self {
            IlaTriggerKind::Immediate => "Immediate",
            IlaTriggerKind::Rising => "Rising edge",
            IlaTriggerKind::Falling => "Falling edge",
            IlaTriggerKind::ValueMatch { .. } => "Value match",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "immediate" | "any" | "now" => Ok(IlaTriggerKind::Immediate),
            "rising" | "rise" | "posedge" => Ok(IlaTriggerKind::Rising),
            "falling" | "fall" | "negedge" => Ok(IlaTriggerKind::Falling),
            "value" | "match" | "valuematch" => Ok(IlaTriggerKind::ValueMatch {
                value: 0,
                mask: u64::MAX,
            }),
            other => Err(format!("ila trigger: unknown {other}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IlaTrigger {
    /// Index into [`IlaConfig::probes`].
    pub probe: usize,
    pub kind: IlaTriggerKind,
}

/// Full ILA setup: probes, trigger, depth, pre/post split, clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IlaConfig {
    pub probes: Vec<IlaProbeCfg>,
    /// Total samples kept per probe.
    pub depth: usize,
    /// Fraction of `depth` kept before the trigger sample (0.0..=1.0).
    pub pre_ratio: f32,
    pub clock_net: String,
    pub timescale_ps: u64,
    pub trigger: IlaTrigger,
    /// Give up the trigger search after this many posedges.
    pub max_cycles: u32,
}

impl Default for IlaConfig {
    fn default() -> Self {
        Self {
            probes: Vec::new(),
            depth: 1024,
            pre_ratio: 0.25,
            clock_net: "clk".into(),
            timescale_ps: 10_000,
            trigger: IlaTrigger {
                probe: 0,
                kind: IlaTriggerKind::Immediate,
            },
            max_cycles: 8192,
        }
    }
}

impl IlaConfig {
    /// Samples kept before the trigger sample.
    pub fn pre(&self) -> usize {
        ((self.depth as f32 * self.pre_ratio).round() as usize).min(self.depth)
    }

    /// Samples kept after the trigger sample.
    pub fn post(&self) -> usize {
        self.depth.saturating_sub(self.pre()).saturating_sub(1)
    }

    fn probe_net_known(&self, design: &Design, net: &str) -> bool {
        net.eq_ignore_ascii_case("led")
            || net.eq_ignore_ascii_case("clk")
            || design.net(net).is_some()
    }

    /// Validate probe setup against a design. Fails loud, never half-arms.
    pub fn validate(&self, design: &Design) -> Result<(), String> {
        if self.probes.is_empty() {
            return Err("ila: no probes (add nets first)".into());
        }
        if self.probes.len() > MAX_PROBES {
            return Err(format!("ila: too many probes ({}/{MAX_PROBES})", self.probes.len()));
        }
        for (i, p) in self.probes.iter().enumerate() {
            if p.name.trim().is_empty() {
                return Err(format!("ila: probe {i} has no name"));
            }
            if p.nets.is_empty() {
                return Err(format!("ila: probe '{}' has no nets", p.name));
            }
            if p.nets.len() > MAX_WIDTH {
                return Err(format!(
                    "ila: probe '{}' width {} > {MAX_WIDTH}",
                    p.name,
                    p.nets.len()
                ));
            }
            for n in &p.nets {
                if !self.probe_net_known(design, n) {
                    return Err(format!("ila: probe '{}': no net {n}", p.name));
                }
            }
        }
        if self.depth == 0 || self.depth > MAX_DEPTH {
            return Err(format!("ila: depth must be 1..={MAX_DEPTH}"));
        }
        if !(0.0..=1.0).contains(&self.pre_ratio) {
            return Err("ila: pre_ratio must be 0.0..=1.0".into());
        }
        if self.trigger.probe >= self.probes.len() {
            return Err(format!(
                "ila: trigger probe {} out of range ({} probes)",
                self.trigger.probe,
                self.probes.len()
            ));
        }
        if let IlaTriggerKind::ValueMatch { value, mask } = self.trigger.kind {
            let w = self.probes[self.trigger.probe].width();
            if w < 64 && (value >> w != 0 || mask >> w != 0) {
                return Err(format!(
                    "ila: trigger value/mask exceed probe width {w}"
                ));
            }
        }
        if !self.probe_net_known(design, &self.clock_net) {
            return Err(format!("ila: no clock net {}", self.clock_net));
        }
        if self.timescale_ps == 0 {
            return Err("ila: timescale_ps must be >= 1".into());
        }
        if self.max_cycles < self.depth as u32 {
            return Err(format!(
                "ila: max_cycles ({}) < depth ({})",
                self.max_cycles, self.depth
            ));
        }
        if self.max_cycles > MAX_CYCLES {
            return Err(format!("ila: max_cycles > {MAX_CYCLES}"));
        }
        Ok(())
    }
}

/// One captured run: per-probe sample columns plus trigger position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IlaCapture {
    pub probes: Vec<IlaProbeCfg>,
    /// samples[probe][i], packed MSB-first.
    pub samples: Vec<Vec<u64>>,
    /// valid[probe][i]: false renders as X (undriven net / loop).
    pub valid: Vec<Vec<bool>>,
    pub timescale_ps: u64,
    pub clock_net: String,
    pub trigger_sample: Option<usize>,
    pub cycles_run: u32,
    pub found_trigger: bool,
}

impl IlaCapture {
    pub fn len(&self) -> usize {
        self.samples.first().map(|s| s.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn value_at(&self, probe: usize, sample: usize) -> Option<u64> {
        self.samples
            .get(probe)
            .and_then(|s| s.get(sample))
            .copied()
            .filter(|_| self.valid.get(probe).and_then(|v| v.get(sample)).copied().unwrap_or(false))
    }

    pub fn time_ps(&self, sample: usize) -> u64 {
        sample as u64 * self.timescale_ps.max(1)
    }

    pub fn formatted_at(&self, probe: usize, sample: usize) -> String {
        let p = &self.probes[probe];
        match self.value_at(probe, sample) {
            Some(v) => format_value(v, p.width(), p.radix),
            None => "X".into(),
        }
    }
}

/// Saved ILA session: setup plus the last capture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IlaSession {
    pub version: u32,
    pub config: IlaConfig,
    pub capture: Option<IlaCapture>,
}

impl IlaSession {
    pub fn new(config: IlaConfig, capture: Option<IlaCapture>) -> Self {
        Self {
            version: SESSION_VERSION,
            config,
            capture,
        }
    }
}

pub fn save_session(path: &Path, session: &IlaSession) -> Result<(), String> {
    let text = serde_json::to_string_pretty(session).map_err(|e| format!("ila save: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("ila save {}: {e}", path.display()))
}

pub fn load_session(path: &Path) -> Result<IlaSession, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("ila load {}: {e}", path.display()))?;
    let s: IlaSession =
        serde_json::from_str(&text).map_err(|e| format!("ila load {}: {e}", path.display()))?;
    if s.version != SESSION_VERSION {
        return Err(format!(
            "ila load: session version {} != {SESSION_VERSION}",
            s.version
        ));
    }
    Ok(s)
}

/// Does the trigger fire on the step `prev -> cur`? (`None` prev = first sample.)
pub fn trigger_fires(kind: &IlaTriggerKind, prev: Option<u64>, cur: u64) -> bool {
    match *kind {
        IlaTriggerKind::Immediate => true,
        IlaTriggerKind::Rising => prev.is_some_and(|p| p & 1 == 0 && cur & 1 == 1),
        IlaTriggerKind::Falling => prev.is_some_and(|p| p & 1 == 1 && cur & 1 == 0),
        IlaTriggerKind::ValueMatch { value, mask } => (cur & mask) == (value & mask),
    }
}

/// First sample index where the trigger fires on one probe's trace.
pub fn find_trigger(values: &[u64], kind: &IlaTriggerKind) -> Option<usize> {
    let mut prev = None;
    for (i, &v) in values.iter().enumerate() {
        if trigger_fires(kind, prev, v) {
            return Some(i);
        }
        prev = Some(v);
    }
    None
}

/// Keep `pre` samples before and `post` after the trigger; returns the window
/// and the trigger's new index inside it.
pub fn apply_window<T: Clone>(trace: &[T], trig: usize, pre: usize, post: usize) -> (Vec<T>, usize) {
    let trig = trig.min(trace.len().saturating_sub(1));
    let start = trig.saturating_sub(pre);
    let end = (trig + post + 1).min(trace.len());
    (trace[start..end].to_vec(), trig - start)
}

/// Sample one probe: pack nets MSB-first. `false` valid if any net reads X.
pub fn pack_probe(nets: &[String], sim: &Sim) -> (u64, bool) {
    let mut v = 0u64;
    let mut ok = true;
    for n in nets {
        v <<= 1;
        match sim.net_value(n) {
            Some(true) => v |= 1,
            Some(false) => {}
            None => ok = false,
        }
    }
    (v, ok)
}

/// Run the event kernel, sampling every probe once per posedge, and keep a
/// `depth`-sample window around the trigger (`pre` before, `post` after).
/// Single pass: a bounded ring holds pre-trigger history; the search breaks
/// early on the first firing sample. `progress(cycles_done)` streams progress
/// and returns `true` to cancel (the partial ring is returned as a missed
/// trigger, honestly marked).
pub fn run_sim_ila_capture(
    design: &Design,
    config: &IlaConfig,
    progress: &dyn Fn(u32) -> bool,
) -> Result<IlaCapture, String> {
    config.validate(design)?;
    let depth = config.depth;
    let pre = config.pre();
    let post = config.post();
    let tp = config.trigger.probe;
    let ts = config.timescale_ps.max(1);

    let mut sim = Sim::new(design);
    // Ring of full pre-trigger history, bounded by depth.
    let mut ring: VecDeque<(Vec<u64>, Vec<bool>)> = VecDeque::new();
    let mut prev_trig: Option<u64> = None;
    let mut cycles_run: u32 = 0;
    for cycle in 0..config.max_cycles {
        sim.step_posedge(ts);
        cycles_run = cycle + 1;
        let mut vals = Vec::with_capacity(config.probes.len());
        let mut oks = Vec::with_capacity(config.probes.len());
        for p in &config.probes {
            let (v, ok) = pack_probe(&p.nets, &sim);
            vals.push(v);
            oks.push(ok);
        }
        let cur_trig = vals[tp];
        let fired = trigger_fires(&config.trigger.kind, prev_trig, cur_trig);
        prev_trig = Some(cur_trig);
        if fired {
            // Window: last `pre` from the ring + this sample + `post` more.
            let mut samples: Vec<Vec<u64>> =
                config.probes.iter().map(|_| Vec::with_capacity(depth)).collect();
            let mut valids: Vec<Vec<bool>> =
                config.probes.iter().map(|_| Vec::with_capacity(depth)).collect();
            let take = pre.min(ring.len());
            let skip = ring.len() - take;
            for (v, o) in ring.iter().skip(skip) {
                for (pi, (s, vv)) in samples.iter_mut().zip(valids.iter_mut()).enumerate() {
                    s.push(v[pi]);
                    vv.push(o[pi]);
                }
            }
            for (pi, (s, vv)) in samples.iter_mut().zip(valids.iter_mut()).enumerate() {
                s.push(vals[pi]);
                vv.push(oks[pi]);
            }
            let trigger_sample = Some(take);
            for _ in 0..post {
                sim.step_posedge(ts);
                cycles_run += 1;
                for (pi, p) in config.probes.iter().enumerate() {
                    let (v, ok) = pack_probe(&p.nets, &sim);
                    samples[pi].push(v);
                    valids[pi].push(ok);
                }
            }
            progress(cycles_run);
            return Ok(IlaCapture {
                probes: config.probes.clone(),
                samples,
                valid: valids,
                timescale_ps: ts,
                clock_net: config.clock_net.clone(),
                trigger_sample,
                cycles_run,
                found_trigger: true,
            });
        }
        ring.push_back((vals, oks));
        while ring.len() > depth {
            ring.pop_front();
        }
        if cycle % 512 == 0 && progress(cycles_run) {
            break; // cancelled: fall through to the missed-trigger return
        }
    }
    // No trigger: hand back the last `depth` samples honestly marked.
    let mut samples: Vec<Vec<u64>> = config.probes.iter().map(|_| Vec::new()).collect();
    let mut valids: Vec<Vec<bool>> = config.probes.iter().map(|_| Vec::new()).collect();
    for (v, o) in ring {
        for (pi, (s, vv)) in samples.iter_mut().zip(valids.iter_mut()).enumerate() {
            s.push(v[pi]);
            vv.push(o[pi]);
        }
    }
    progress(cycles_run);
    Ok(IlaCapture {
        probes: config.probes.clone(),
        samples,
        valid: valids,
        timescale_ps: ts,
        clock_net: config.clock_net.clone(),
        trigger_sample: None,
        cycles_run,
        found_trigger: false,
    })
}

/// Background capture: design + config move to a `helion-ila` thread; the UI
/// polls the receiver and reads progress without blocking the egui frame.
#[derive(Debug)]
pub struct IlaRunProgress {
    pub cycles: AtomicU32,
    pub done: AtomicU32,
    /// Set by the UI to stop a capture in flight.
    pub cancel: std::sync::atomic::AtomicBool,
}

pub struct IlaRunHandle {
    pub rx: mpsc::Receiver<Result<IlaCapture, String>>,
    pub progress: Arc<IlaRunProgress>,
}

pub fn spawn_sim_ila_capture(design: Design, config: IlaConfig) -> IlaRunHandle {
    let (tx, rx) = mpsc::channel();
    let progress = Arc::new(IlaRunProgress {
        cycles: AtomicU32::new(0),
        done: AtomicU32::new(0),
        cancel: std::sync::atomic::AtomicBool::new(false),
    });
    let p = Arc::clone(&progress);
    std::thread::Builder::new()
        .name("helion-ila".into())
        .spawn(move || {
            let r = run_sim_ila_capture(&design, &config, &|c| {
                p.cycles.store(c, Ordering::Relaxed);
                p.cancel.load(Ordering::Relaxed)
            });
            p.done.store(1, Ordering::Relaxed);
            let _ = tx.send(r);
        })
        .expect("ila: spawn capture thread");
    IlaRunHandle { rx, progress }
}

/// Zoom/pan/cursor state for the ILA waveform viewer (pure logic; egui-free).
#[derive(Clone, Debug)]
pub struct IlaViewer {
    /// Fractional sample index at the left edge.
    pub first: f64,
    /// Samples per pixel.
    pub spp: f64,
    pub cursor: Option<usize>,
    pub cursor_a: Option<usize>,
    pub cursor_b: Option<usize>,
    /// Last canvas width in px (for toolbar zoom anchoring).
    pub view_w: f32,
}

impl IlaViewer {
    pub fn fit(n: usize, width_px: f32) -> Self {
        let n = n.max(1);
        Self {
            first: 0.0,
            spp: n as f64 / width_px.max(1.0) as f64,
            cursor: None,
            cursor_a: None,
            cursor_b: None,
            view_w: width_px.max(1.0),
        }
    }

    /// Zoom around the current view center (toolbar buttons).
    pub fn zoom_center(&mut self, factor: f64, n: usize) {
        let center = self.first + self.view_w as f64 * self.spp / 2.0;
        self.zoom(factor, center, n);
    }

    pub fn clamp(&mut self, n: usize) {
        let n = n.max(1) as f64;
        self.spp = self.spp.clamp(0.02, n);
        let max_first = (n - 1.0).max(0.0);
        self.first = self.first.clamp(0.0, max_first);
        for c in [&mut self.cursor, &mut self.cursor_a, &mut self.cursor_b] {
            if let Some(v) = c {
                if *v as f64 >= n {
                    *c = Some(n as usize - 1);
                }
            }
        }
    }

    /// Zoom by `factor` (< 1 zooms in) around `anchor` (fractional sample).
    pub fn zoom(&mut self, factor: f64, anchor: f64, n: usize) {
        self.first = anchor - (anchor - self.first) * factor;
        self.spp *= factor;
        self.clamp(n);
    }

    pub fn pan_samples(&mut self, delta: f64, n: usize) {
        self.first += delta;
        self.clamp(n);
    }

    /// Fractional sample index under horizontal pixel `x` (canvas at `x0`).
    pub fn sample_at(&self, x: f32, x0: f32) -> f64 {
        self.first + (x - x0) as f64 * self.spp
    }

    /// Pixel x of a fractional sample.
    pub fn x_of(&self, sample: f64, x0: f32) -> f32 {
        x0 + ((sample - self.first) / self.spp) as f32
    }

    /// (a, b, |b - a|) in samples when both A/B are placed.
    pub fn delta_samples(&self) -> Option<(usize, usize, usize)> {
        match (self.cursor_a, self.cursor_b) {
            (Some(a), Some(b)) => Some((a, b, a.abs_diff(b))),
            _ => None,
        }
    }
}

/// Edge/pattern at one sample on one probe: Rising/Falling for scalar
/// probes, else ValueMatch on the current value masked to the probe width.
/// `None` when the sample is X (invalid) or out of range.
pub fn edge_kind_at(cap: &IlaCapture, probe: usize, sample: usize) -> Option<IlaTriggerKind> {
    let p = cap.probes.get(probe)?;
    let vals = cap.samples.get(probe)?;
    let cur = *vals.get(sample)?;
    if !cap.valid.get(probe)?.get(sample).copied().unwrap_or(false) {
        return None;
    }
    if p.width() == 1 && sample > 0 {
        let prev_ok = cap.valid[probe].get(sample - 1).copied().unwrap_or(false);
        if prev_ok {
            let prev = vals[sample - 1];
            if prev & 1 == 0 && cur & 1 == 1 {
                return Some(IlaTriggerKind::Rising);
            }
            if prev & 1 == 1 && cur & 1 == 0 {
                return Some(IlaTriggerKind::Falling);
            }
        }
    }
    let w = p.width();
    let mask = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
    Some(IlaTriggerKind::ValueMatch {
        value: cur & mask,
        mask,
    })
}

/// First probe (in order) with a usable trigger suggestion at `sample`.
/// Scalars prefer their edge kind; buses fall back to a value match.
pub fn suggest_trigger_at(cap: &IlaCapture, sample: usize) -> Option<(usize, IlaTriggerKind)> {
    for pi in 0..cap.probes.len() {
        if let Some(k) = edge_kind_at(cap, pi, sample) {
            // Prefer a genuine scalar edge over a bus value match.
            if cap.probes[pi].width() == 1
                && matches!(k, IlaTriggerKind::Rising | IlaTriggerKind::Falling)
            {
                return Some((pi, k));
            }
        }
    }
    for pi in 0..cap.probes.len() {
        if let Some(k) = edge_kind_at(cap, pi, sample) {
            return Some((pi, k));
        }
    }
    None
}

/// VCD identifier: base-94 over printable ASCII `!`..=`~`.
fn vcd_id(mut k: usize) -> String {
    let mut s = String::new();
    loop {
        s.push((b'!' + (k % 94) as u8) as char);
        k /= 94;
        if k == 0 {
            break;
        }
    }
    s
}

/// Sanitize a net name for `$var` (no whitespace / control chars).
fn vcd_safe_name(n: &str) -> String {
    n.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_.$:[]<>".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Largest standard VCD timescale unit dividing `timescale_ps` evenly.
fn vcd_timescale(timescale_ps: u64) -> (u64, &'static str) {
    let ts = timescale_ps.max(1);
    for (unit, ps) in [
        ("s", 1_000_000_000_000u64),
        ("ms", 1_000_000_000),
        ("us", 1_000_000),
        ("ns", 1_000),
        ("ps", 1),
    ] {
        if ts % ps == 0 {
            return (ts / ps, unit);
        }
    }
    (ts, "ps")
}

/// Write the capture as a VCD file readable by GTKWave/surfer.
///
/// One scalar `$var` per probed net (first-seen order), `x` for invalid
/// samples, change-only value dumping, and a `$comment` marking the trigger
/// sample. Pure writer; the only failure modes are empty captures and IO.
pub fn export_vcd(path: &Path, cap: &IlaCapture) -> Result<(), String> {
    if cap.is_empty() {
        return Err("ila: no samples to export".into());
    }
    // Unique nets, first-seen order.
    let mut nets: Vec<&str> = Vec::new();
    for p in &cap.probes {
        for n in &p.nets {
            if !nets.iter().any(|x| *x == n) {
                nets.push(n);
            }
        }
    }
    if nets.is_empty() {
        return Err("ila: no nets to export".into());
    }
    // Map each net to (probe, lsb index) of its packed column.
    let mut cols: Vec<(usize, usize)> = Vec::with_capacity(nets.len());
    for net in &nets {
        let mut found = None;
        for (pi, p) in cap.probes.iter().enumerate() {
            if let Some(j) = p.nets.iter().position(|x| x == net) {
                found = Some((pi, p.width() - 1 - j));
                break;
            }
        }
        let (pi, lsb) =
            found.ok_or_else(|| format!("ila vcd: net {net} has no probe column"))?;
        cols.push((pi, lsb));
    }
    let bit_at = |net_i: usize, s: usize| -> char {
        let (pi, lsb) = cols[net_i];
        let ok = cap
            .valid
            .get(pi)
            .and_then(|v| v.get(s))
            .copied()
            .unwrap_or(false);
        if !ok {
            return 'x';
        }
        match cap.samples.get(pi).and_then(|v| v.get(s)) {
            Some(v) if (v >> lsb) & 1 == 1 => '1',
            Some(_) => '0',
            None => 'x',
        }
    };

    let (ts_num, ts_unit) = vcd_timescale(cap.timescale_ps);
    let n = cap.len();
    let mut out = String::with_capacity(n * nets.len() * 4);
    out.push_str("$date\n   helion ILA export\n$end\n");
    out.push_str("$version\n   helion\n$end\n");
    out.push_str(&format!("$timescale {ts_num} {ts_unit} $end\n"));
    out.push_str("$scope module ila $end\n");
    for (i, net) in nets.iter().enumerate() {
        out.push_str(&format!(
            "$var wire 1 {} {} $end\n",
            vcd_id(i),
            vcd_safe_name(net)
        ));
    }
    out.push_str("$upscope $end\n$enddefinitions $end\n");
    if let Some(ti) = cap.trigger_sample.filter(|_| cap.found_trigger) {
        out.push_str(&format!("$comment trigger sample {ti} of {n} $end\n"));
    }
    // Initial values, then change-only dumps.
    out.push_str("#0\n$dumpvars\n");
    let mut prev: Vec<char> = Vec::with_capacity(nets.len());
    for i in 0..nets.len() {
        let b = bit_at(i, 0);
        out.push(b);
        out.push_str(&vcd_id(i));
        out.push('\n');
        prev.push(b);
    }
    out.push_str("$end\n");
    for s in 1..n {
        let t = s as u64 * ts_num;
        let mut changed = false;
        let mut body = String::new();
        for i in 0..nets.len() {
            let b = bit_at(i, s);
            if b != prev[i] {
                body.push(b);
                body.push_str(&vcd_id(i));
                body.push('\n');
                prev[i] = b;
                changed = true;
            }
        }
        if changed {
            out.push('#');
            out.push_str(&t.to_string());
            out.push('\n');
            out.push_str(&body);
        }
    }
    std::fs::write(path, out).map_err(|e| format!("ila vcd {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::CellKind;

    fn trace(v: &[u64]) -> Vec<u64> {
        v.to_vec()
    }

    #[test]
    fn trigger_fires_edge_and_value() {
        let k = IlaTriggerKind::Rising;
        assert_eq!(find_trigger(&trace(&[0, 0, 1, 1]), &k), Some(2));
        assert_eq!(find_trigger(&trace(&[0, 0, 0]), &k), None);
        // Edge is on the LSB for buses.
        assert_eq!(find_trigger(&trace(&[0b100, 0b101]), &k), Some(1));
        assert_eq!(find_trigger(&trace(&[0b101, 0b100]), &k), None);
        let kf = IlaTriggerKind::Falling;
        assert_eq!(find_trigger(&trace(&[1, 1, 0]), &kf), Some(2));
        let kv = IlaTriggerKind::ValueMatch {
            value: 0xA,
            mask: 0xF,
        };
        assert_eq!(find_trigger(&trace(&[0x1B, 0x2B, 0x3A]), &kv), Some(2));
        assert_eq!(find_trigger(&trace(&[0x1B, 0x2B]), &kv), None);
        // Mask ignores the top nibble.
        let km = IlaTriggerKind::ValueMatch {
            value: 0xA0,
            mask: 0xF0,
        };
        assert_eq!(find_trigger(&trace(&[0xAB]), &km), Some(0));
        let ki = IlaTriggerKind::Immediate;
        assert_eq!(find_trigger(&trace(&[7, 8]), &ki), Some(0));
        assert_eq!(find_trigger(&trace(&[]), &ki), None);
    }

    #[test]
    fn window_centers_trigger_with_pre_post() {
        let t: Vec<u64> = (0..100).collect();
        let (w, ti) = apply_window(&t, 50, 10, 20);
        assert_eq!(w.len(), 31);
        assert_eq!(ti, 10);
        assert_eq!(w[ti], 50);
        // Clamped at the edges.
        let (w2, ti2) = apply_window(&t, 3, 10, 20);
        assert_eq!(ti2, 3);
        assert_eq!(w2[0], 0);
        let (w3, ti3) = apply_window(&t, 95, 10, 20);
        assert_eq!(w3.len(), 15);
        assert_eq!(ti3, 10);
        assert_eq!(w3[ti3], 95);
    }

    #[test]
    fn config_validation_fails_loud() {
        let d = Design::structural_counter();
        let mut cfg = IlaConfig::default();
        assert!(cfg.validate(&d).is_err(), "no probes must fail");
        cfg.probes.push(IlaProbeCfg::scalar("q0", "q0"));
        assert!(cfg.validate(&d).is_ok());
        cfg.probes.push(IlaProbeCfg::scalar("bogus", "no_net"));
        assert!(cfg.validate(&d).is_err());
        cfg.probes.pop();
        cfg.depth = 0;
        assert!(cfg.validate(&d).is_err());
        cfg.depth = MAX_DEPTH + 1;
        assert!(cfg.validate(&d).is_err());
        cfg.depth = 64;
        cfg.pre_ratio = 1.5;
        assert!(cfg.validate(&d).is_err());
        cfg.pre_ratio = 0.25;
        cfg.trigger.probe = 7;
        assert!(cfg.validate(&d).is_err());
        cfg.trigger.probe = 0;
        cfg.trigger.kind = IlaTriggerKind::ValueMatch {
            value: 0x1FF,
            mask: 0x1FF,
        };
        assert!(cfg.validate(&d).is_err(), "9-bit value on 1-bit probe");
        cfg.trigger.kind = IlaTriggerKind::Rising;
        cfg.clock_net = "no_clk".into();
        assert!(cfg.validate(&d).is_err());
    }

    #[test]
    fn capture_finds_rising_edge_with_pre_post_window() {
        let d = Design::structural_counter();
        let mut cfg = IlaConfig::default();
        cfg.probes.push(IlaProbeCfg::scalar("q3", "q3"));
        cfg.probes.push(IlaProbeCfg {
            name: "cnt".into(),
            nets: vec!["q3".into(), "q2".into(), "q1".into(), "q0".into()],
            radix: IlaRadix::Hex,
        });
        cfg.depth = 32;
        cfg.pre_ratio = 0.25; // pre=8, post=23
        cfg.trigger = IlaTrigger {
            probe: 0,
            kind: IlaTriggerKind::Rising,
        };
        cfg.max_cycles = 1024;
        let cap = run_sim_ila_capture(&d, &cfg, &|_| false).expect("capture");
        assert!(cap.found_trigger, "q3 rises at count 8");
        assert_eq!(cap.samples.len(), 2);
        let ti = cap.trigger_sample.expect("trigger idx");
        // q3 first rises at sample 7 (count 8); only 7 pre samples exist.
        assert_eq!(ti, 7, "trigger index = min(pre, trig_cycle)");
        assert_eq!(cap.len(), ti + 1 + cfg.post());
        // q3 rises when the counter wraps 7->8; sample before is 0, at is 1.
        assert_eq!(cap.samples[0][ti - 1], 0);
        assert_eq!(cap.samples[0][ti], 1);
        // Bus probe counts 0..15 across the window; counter at trigger = 8.
        assert_eq!(cap.samples[1][ti], 8);
        assert!(cap.valid.iter().all(|v| v.iter().all(|&b| b)));
        assert_eq!(cap.time_ps(ti), ti as u64 * 10_000);
    }

    #[test]
    fn capture_value_match_and_missed_trigger() {
        let d = Design::structural_counter();
        let mut cfg = IlaConfig::default();
        cfg.probes.push(IlaProbeCfg {
            name: "cnt".into(),
            nets: vec!["q3".into(), "q2".into(), "q1".into(), "q0".into()],
            radix: IlaRadix::Dec,
        });
        cfg.depth = 16;
        cfg.pre_ratio = 0.5;
        cfg.trigger = IlaTrigger {
            probe: 0,
            kind: IlaTriggerKind::ValueMatch {
                value: 5,
                mask: 0xF,
            },
        };
        let cap = run_sim_ila_capture(&d, &cfg, &|_| false).expect("capture");
        assert!(cap.found_trigger);
        let ti = cap.trigger_sample.unwrap();
        assert_eq!(cap.samples[0][ti], 5);
        // Trigger at sample 4 < pre=8: only 4 pre samples exist.
        assert_eq!(cap.len(), ti + 1 + cfg.post());
        // Missed trigger: clk reads 1 at every posedge sample, so value 0
        // never fires - the capture honestly reports the last depth samples.
        cfg.probes.push(IlaProbeCfg::scalar("clk", "clk"));
        cfg.trigger = IlaTrigger {
            probe: 1,
            kind: IlaTriggerKind::ValueMatch { value: 0, mask: 1 },
        };
        cfg.max_cycles = 40;
        let miss = run_sim_ila_capture(&d, &cfg, &|_| false).expect("capture");
        assert!(!miss.found_trigger);
        assert_eq!(miss.trigger_sample, None);
        assert_eq!(miss.len(), 16, "last depth samples kept");
        assert_eq!(miss.cycles_run, 40);
    }

    #[test]
    fn session_round_trip() {
        let d = Design::structural_counter();
        let mut cfg = IlaConfig::default();
        cfg.probes.push(IlaProbeCfg::scalar("q0", "q0"));
        cfg.depth = 16;
        cfg.pre_ratio = 0.0; // immediate trigger at cycle 0 keeps all 16
        let cap = run_sim_ila_capture(&d, &cfg, &|_| false).expect("capture");
        let s = IlaSession::new(cfg, Some(cap));
        let dir = std::env::temp_dir().join("helion-ila-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.json");
        save_session(&path, &s).expect("save");
        let back = load_session(&path).expect("load");
        assert_eq!(back, s);
        assert_eq!(back.capture.unwrap().len(), 16);
        let bad = load_session(&dir.join("missing.json"));
        assert!(bad.is_err());
        std::fs::write(dir.join("badver.json"), r#"{"version":99,"config":{},"capture":null}"#).unwrap();
        assert!(load_session(&dir.join("badver.json")).is_err());
    }

    #[test]
    fn viewer_zoom_pan_cursor_math() {
        let mut v = IlaViewer::fit(1000, 500.0);
        assert!((v.spp - 2.0).abs() < 1e-9);
        assert!((v.sample_at(250.0, 0.0) - 500.0).abs() < 1e-9);
        v.zoom(0.5, 500.0, 1000);
        assert!((v.spp - 1.0).abs() < 1e-9);
        assert!((v.first - 250.0).abs() < 1e-9);
        v.pan_samples(100.0, 1000);
        assert!((v.first - 350.0).abs() < 1e-9);
        // Clamp at edges.
        v.pan_samples(10000.0, 1000);
        assert!(v.first <= 999.0);
        v.cursor_a = Some(100);
        v.cursor_b = Some(350);
        assert_eq!(v.delta_samples(), Some((100, 350, 250)));
        assert!((v.x_of(500.0, 10.0) - (10.0 + (500.0 - v.first) / v.spp) as f32).abs() < 1e-3);
    }

    #[test]
    fn format_value_radices() {
        assert_eq!(format_value(0b1010, 4, IlaRadix::Bin), "1010");
        assert_eq!(format_value(0xAB, 8, IlaRadix::Hex), "0xAB");
        assert_eq!(format_value(0xAB, 12, IlaRadix::Hex), "0x0AB");
        assert_eq!(format_value(42, 8, IlaRadix::Dec), "42");
    }

    #[test]
    fn chained_lut_probe_captures() {
        // 2-level combo net probes through the whole capture path.
        let mut d = Design::structural_counter();
        d.add_cell("u_and", CellKind::Lut6 { init: 0x8888_8888_8888_8888 });
        d.connect("d0", "u_and", "I0");
        d.connect("q1", "u_and", "I1");
        d.connect("e", "u_and", "O");
        let mut cfg = IlaConfig::default();
        cfg.probes.push(IlaProbeCfg::scalar("e", "e"));
        cfg.depth = 8;
        cfg.pre_ratio = 0.0; // immediate trigger at cycle 0: no pre samples
        let cap = run_sim_ila_capture(&d, &cfg, &|_| false).expect("capture");
        // e = d0 & q1 with d0 = ~q0 combinational on the CURRENT count:
        // counts 1..8 → 0,1,0,0,0,1,0,0.
        let vals: Vec<u64> = cap.samples[0].clone();
        assert_eq!(vals, vec![0, 1, 0, 0, 0, 1, 0, 0]);
        assert!(cap.valid[0].iter().all(|&b| b));
    }

    #[test]
    fn spawn_thread_capture_matches_sync() {
        let d = Design::structural_counter();
        let mut cfg = IlaConfig::default();
        cfg.probes.push(IlaProbeCfg::scalar("q3", "q3"));
        cfg.depth = 32;
        cfg.trigger = IlaTrigger {
            probe: 0,
            kind: IlaTriggerKind::Rising,
        };
        let h = spawn_sim_ila_capture(d, cfg.clone());
        let cap = h.rx.recv_timeout(std::time::Duration::from_secs(30)).expect("done").expect("ok");
        assert!(cap.found_trigger);
        let ti = cap.trigger_sample.expect("trigger idx");
        assert_eq!(cap.len(), ti + 1 + cfg.post());
        assert_eq!(h.progress.done.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn capture_cancel_returns_partial_ring_honestly() {
        let d = Design::structural_counter();
        let mut cfg = IlaConfig::default();
        cfg.probes.push(IlaProbeCfg::scalar("q0", "q0"));
        cfg.depth = 64;
        cfg.max_cycles = 100_000;
        cfg.trigger = IlaTrigger {
            probe: 0,
            kind: IlaTriggerKind::Rising,
        };
        // Rising can't fire on the very first sample (no previous edge), so
        // the immediate cancel wins the race deterministically.
        let cap = run_sim_ila_capture(&d, &cfg, &|c| c >= 1).expect("capture");
        assert!(!cap.found_trigger);
        assert_eq!(cap.trigger_sample, None);
        assert!(cap.cycles_run < 100_000, "stopped early at {}", cap.cycles_run);
        assert!(cap.len() <= 64);
    }

    #[test]
    fn probe_struct_helpers() {
        let p = IlaProbeCfg::scalar("q0", "q0");
        assert_eq!(p.width(), 1);
        assert!(!p.is_bus());
        assert_eq!(IlaRadix::parse("hex").unwrap(), IlaRadix::Hex);
        assert!(IlaRadix::parse("oct").is_err());
        assert_eq!(IlaTriggerKind::parse("posedge").unwrap(), IlaTriggerKind::Rising);
        assert!(IlaTriggerKind::parse("banana").is_err());
        let c = IlaConfig::default();
        assert_eq!(c.pre(), 256);
        assert_eq!(c.post(), 1024 - 256 - 1);
    }

    fn demo_capture() -> IlaCapture {
        let d = Design::structural_counter();
        let mut cfg = IlaConfig::default();
        cfg.probes.push(IlaProbeCfg::scalar("q0", "q0"));
        cfg.probes.push(IlaProbeCfg {
            name: "cnt".into(),
            nets: vec!["q3".into(), "q2".into(), "q1".into(), "q0".into()],
            radix: IlaRadix::Hex,
        });
        cfg.depth = 32;
        cfg.pre_ratio = 0.25;
        cfg.trigger = IlaTrigger {
            probe: 0,
            kind: IlaTriggerKind::Rising,
        };
        run_sim_ila_capture(&d, &cfg, &|_| false).expect("capture")
    }

    #[test]
    fn edge_kind_at_detects_edges_and_bus_value() {
        let cap = demo_capture();
        let ti = cap.trigger_sample.expect("trigger");
        // q0 rises at the trigger sample.
        assert_eq!(edge_kind_at(&cap, 0, ti), Some(IlaTriggerKind::Rising));
        // q0 toggles every cycle, so the next sample is a falling edge.
        assert_eq!(edge_kind_at(&cap, 0, ti + 1), Some(IlaTriggerKind::Falling));
        // Bus probe gives a value match with the current value.
        match edge_kind_at(&cap, 1, ti) {
            Some(IlaTriggerKind::ValueMatch { value, mask }) => {
                assert_eq!(mask, 0xF);
                assert_eq!(value, cap.samples[1][ti] & 0xF);
            }
            other => panic!("expected value match, got {other:?}"),
        }
        // Out of range / X-free design: sample past the end is None.
        assert_eq!(edge_kind_at(&cap, 0, cap.len()), None);
        assert_eq!(edge_kind_at(&cap, 9, 0), None);
    }

    #[test]
    fn suggest_trigger_at_prefers_scalar_edge() {
        let cap = demo_capture();
        let ti = cap.trigger_sample.expect("trigger");
        // q0 (probe 0) rises at ti: suggestion is the scalar edge.
        assert_eq!(
            suggest_trigger_at(&cap, ti),
            Some((0, IlaTriggerKind::Rising))
        );
        // q0 falls at ti+1.
        assert_eq!(
            suggest_trigger_at(&cap, ti + 1),
            Some((0, IlaTriggerKind::Falling))
        );
    }

    #[test]
    fn export_vcd_writes_gtkwave_structure() {
        let cap = demo_capture();
        let dir = std::env::temp_dir().join("helion-ila-vcd-structure");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cap.vcd");
        export_vcd(&path, &cap).expect("export");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("$timescale 10 ns $end"), "{}", &text[..400]);
        assert!(text.contains("$var wire 1 ! q0 $end"));
        assert!(text.contains("$var wire 1 \" q3 $end"));
        assert!(text.contains("$enddefinitions $end"));
        let ti = cap.trigger_sample.unwrap();
        assert!(text.contains(&format!("$comment trigger sample {ti}")));
        // Values: q0's id is '!' - both 0! and 1! transitions must appear.
        assert!(text.contains("0!\n"));
        assert!(text.contains("1!\n"));
        // Timestamps: timescale is "10 ns", so sample s lands on #(10*s).
        assert!(text.contains("#10\n"));
        // Empty capture fails loud.
        let empty = IlaCapture {
            probes: vec![],
            samples: vec![],
            valid: vec![],
            timescale_ps: 10_000,
            clock_net: "clk".into(),
            trigger_sample: None,
            cycles_run: 0,
            found_trigger: false,
        };
        assert!(export_vcd(&dir.join("empty.vcd"), &empty).is_err());
    }

    #[test]
    fn export_vcd_marks_invalid_as_x() {
        let mut cap = demo_capture();
        cap.valid[0][3] = false;
        let dir = std::env::temp_dir().join("helion-ila-vcd-invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.vcd");
        export_vcd(&path, &cap).expect("export");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("x!\n"), "invalid sample must dump as x");
    }
}
