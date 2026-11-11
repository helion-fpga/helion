//! Waveform trace styling and lane editing for live (engine) traces.
//!
//! Owns the per-trace display beyond the radix (fixed-point `Q`/`UQ` and
//! analog step/linear rendering), the Comfortable/Compact lane density,
//! virtual buses packed from 1-bit traces, multi-selection, lane reorder /
//! removal, and the trace-bar strip that drives them. The math lives in
//! [`helion_verif::wave`]; this module is state + UI glue only.
//!
//! Old-IDE reference: `helion-gui/src/bin/helion-ide.rs` 11676-11696
//! (Cursor A/B, Virtual Bus, Density), 12043-12117 (per-trace radix,
//! style, protocol, and display-mode menu).

use std::collections::HashMap;

use helion_verif::wave::{FixedPoint, analog_y, pack_bits, sign_extend};
use rune::{Drag, El, LineCap, Phase, Shape, Tx, el, row, text};

use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::wave::{LiveTrace, LiveWave, WaveRadix, WaveState, time_label, trace_scope};

/// Lane density (GTKWave-style Comfortable/Compact).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

impl Density {
    pub const ALL: [Density; 2] = [Density::Comfortable, Density::Compact];

    /// Digital lane height in points.
    pub fn lane_h(self) -> f32 {
        match self {
            Density::Comfortable => 24.0,
            Density::Compact => 18.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Density::Comfortable => "Comfortable",
            Density::Compact => "Compact",
        }
    }
}

/// Analog rendering of a multi-bit trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Analog {
    /// Zero-order hold staircase.
    Step,
    /// Straight segments between value changes.
    Linear,
}

/// Display overrides of one trace beyond its radix.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TraceDisplay {
    pub fixed: Option<FixedPoint>,
    pub analog: Option<Analog>,
}

/// A virtual bus: 1-bit member traces packed LSB-first (member 0 = bit 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualBus {
    pub name: String,
    pub members: Vec<String>,
}

/// Style slice of [`WaveState`].
#[derive(Clone, Debug, Default)]
pub struct WaveStyleState {
    pub density: Density,
    /// Per-trace overrides by full trace name.
    pub display: HashMap<String, TraceDisplay>,
    /// Virtual buses, re-packed onto every new import by member name.
    pub buses: Vec<VirtualBus>,
    /// Extra selected live traces (Cmd/Shift-click in the Name column);
    /// [`WaveState::live_selected`] stays the primary selection.
    pub multi: Vec<usize>,
}

/// Default fractional bits when a trace first switches to fixed point.
const DEFAULT_FRAC: u8 = 4;

impl WaveState {
    pub fn display_of(&self, name: &str) -> TraceDisplay {
        self.style.display.get(name).copied().unwrap_or_default()
    }

    /// Value text of a live trace at grid index `i`: fixed point when the
    /// trace has a `Q` format, else its radix.
    pub fn trace_value(&self, trace: &LiveTrace, i: usize) -> String {
        match self.display_of(&trace.name).fixed {
            Some(fp) if trace.is_bus() => fp.format(trace.value_at(i), trace.width),
            _ => self
                .radix_for(&trace.name)
                .format(trace.value_at(i), trace.width),
        }
    }

    /// Short format tag for the readout (`hex`, `Q3.4`).
    pub fn format_tag(&self, trace: &LiveTrace) -> String {
        match self.display_of(&trace.name).fixed {
            Some(fp) if trace.is_bus() => fp.label_for(trace.width),
            _ => self.radix_for(&trace.name).label().to_lowercase(),
        }
    }

    /// Lane height of a live trace: analog lanes are twice the density.
    pub fn live_lane_h(&self, trace: &LiveTrace) -> f32 {
        let base = self.style.density.lane_h();
        if trace.is_bus() && self.display_of(&trace.name).analog.is_some() {
            base * 2.0
        } else {
            base
        }
    }

    /// Selected live traces: the primary plus the multi-selection, in
    /// trace order.
    pub fn selection(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .live_selected
            .into_iter()
            .chain(self.style.multi.iter().copied())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The trace the bar edits (the primary live selection).
    pub fn selected_trace(&self) -> Option<&LiveTrace> {
        self.live.as_ref()?.traces.get(self.live_selected?)
    }

    /// Move a mounted lane `delta` rows (negative = up). `false` at an end
    /// or when `idx` is not mounted.
    pub fn move_lane(&mut self, idx: usize, delta: i32) -> bool {
        let mut lanes = self.lanes();
        let Some(pos) = lanes.iter().position(|&l| l == idx) else {
            return false;
        };
        let to = pos as i32 + delta;
        if to < 0 || to as usize >= lanes.len() {
            return false;
        }
        lanes.swap(pos, to as usize);
        self.live_lanes = lanes;
        self.lanes_custom = true;
        true
    }

    /// Unmount lane `idx` (the trace stays in Objects). Clears it from
    /// the selection.
    pub fn remove_lane(&mut self, idx: usize) -> bool {
        let mut lanes = self.lanes();
        let before = lanes.len();
        lanes.retain(|&l| l != idx);
        if lanes.len() == before {
            return false;
        }
        self.live_lanes = lanes;
        self.lanes_custom = true;
        if self.live_selected == Some(idx) {
            self.live_selected = None;
        }
        self.style.multi.retain(|&m| m != idx);
        true
    }

    /// Why a virtual bus cannot be built from the selection (`None` = it can).
    pub fn bus_blocker(&self) -> Option<&'static str> {
        let live = self.live.as_ref().filter(|l| !l.is_empty())?;
        let sel = self.selection();
        if sel.len() < 2 {
            return Some("Select two or more 1-bit traces (Cmd-click names to add)");
        }
        if sel
            .iter()
            .any(|&i| live.traces.get(i).is_none_or(|t| t.width != 1))
        {
            return Some("Virtual buses pack 1-bit traces only");
        }
        if sel.len() > 64 {
            return Some("A virtual bus holds at most 64 bits");
        }
        None
    }

    /// Pack the selected 1-bit traces (trace order = LSB first) into a new
    /// virtual bus lane. Returns the bus name.
    pub fn create_virtual_bus(&mut self) -> Result<String, String> {
        if self.live.as_ref().is_none_or(|l| l.is_empty()) {
            return Err("Run a simulation first".into());
        }
        if let Some(why) = self.bus_blocker() {
            return Err(why.into());
        }
        let live = self.live.as_ref().expect("checked above");
        let members: Vec<String> = self
            .selection()
            .iter()
            .map(|&i| live.traces[i].name.clone())
            .collect();
        let name = bus_name(live, &members, self.style.buses.len());
        let bus = VirtualBus {
            name: name.clone(),
            members,
        };
        let idx = append_bus(self.live.as_mut().expect("checked above"), &bus)
            .ok_or("a member trace is missing")?;
        self.style.buses.push(bus);
        let mut lanes = self.lanes();
        lanes.push(idx);
        self.live_lanes = lanes;
        self.lanes_custom = true;
        self.live_selected = Some(idx);
        self.style.multi.clear();
        Ok(name)
    }

    /// Re-pack every virtual bus onto a fresh import (members matched by
    /// name; buses whose members vanished are skipped, not faked).
    pub fn rebuild_virtual_buses(&mut self) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        for bus in &self.style.buses {
            if live.traces.iter().all(|t| t.name != bus.name) {
                let _ = append_bus(live, bus);
            }
        }
    }

    pub fn is_virtual(&self, name: &str) -> bool {
        self.style.buses.iter().any(|b| b.name == name)
    }
}

/// Bus name: the members' shared scope + common stem (`counter.cnt`),
/// else `vbus<N>`; never collides with an existing trace.
fn bus_name(live: &LiveWave, members: &[String], n: usize) -> String {
    let scope = trace_scope(&members[0]);
    let leaves: Vec<&str> = members
        .iter()
        .map(|m| m.rsplit('.').next().unwrap_or(m))
        .collect();
    let mut stem = leaves[0].to_string();
    for l in &leaves[1..] {
        let k = stem
            .chars()
            .zip(l.chars())
            .take_while(|(a, b)| a == b)
            .count();
        stem.truncate(
            stem.char_indices()
                .nth(k)
                .map(|(i, _)| i)
                .unwrap_or(stem.len()),
        );
    }
    let stem = stem.trim_end_matches(['_', '[']).to_string();
    let mut leaf = if stem.is_empty() {
        format!("vbus{n}")
    } else {
        stem
    };
    let full = |leaf: &str| {
        if scope.is_empty() {
            leaf.to_string()
        } else {
            format!("{scope}.{leaf}")
        }
    };
    while live.traces.iter().any(|t| t.name == full(&leaf)) {
        leaf.push_str("_vb");
    }
    full(&leaf)
}

/// Append the packed bus as a live trace; returns its index.
fn append_bus(live: &mut LiveWave, bus: &VirtualBus) -> Option<usize> {
    let mut cols: Vec<&[u64]> = Vec::new();
    for m in &bus.members {
        cols.push(&live.traces.iter().find(|t| &t.name == m)?.samples);
    }
    let samples = pack_bits(&cols);
    live.traces.push(LiveTrace {
        name: bus.name.clone(),
        width: bus.members.len().min(64) as u8,
        samples,
    });
    Some(live.traces.len() - 1)
}

fn echo(s: &mut AppState, text: String) {
    s.console.lines.push(ConsoleLine {
        text,
        sev: MsgSev::Info,
    });
}

/// Name-column press: plain selects one trace; Cmd- or Shift-click toggles
/// it in the multi-selection (for virtual buses).
pub fn press_name(s: &mut AppState, idx: usize, tx: &Tx) {
    let m = tx.mods();
    if m.cmd || m.shift {
        if s.wave.live_selected == Some(idx) {
            // Demote the primary: the next multi pick (if any) leads.
            s.wave.live_selected = s.wave.style.multi.first().copied();
            s.wave
                .style
                .multi
                .retain(|&i| Some(i) != s.wave.live_selected);
        } else if s.wave.style.multi.contains(&idx) {
            s.wave.style.multi.retain(|&i| i != idx);
        } else if s.wave.live_selected.is_none() {
            s.wave.live_selected = Some(idx);
        } else {
            s.wave.style.multi.push(idx);
        }
        return;
    }
    s.wave.style.multi.clear();
    let Some(name) = s
        .wave
        .live
        .as_ref()
        .and_then(|l| l.traces.get(idx))
        .map(|t| t.name.clone())
    else {
        return;
    };
    s.wave.live_selected = Some(idx);
    echo(s, format!("select_objects [get_waves {{{name}}}]"));
}

/// Press on the trace sheet: plain click parks the main cursor, Shift-click
/// places cursor A, Alt-click cursor B (old IDE convention). The press also
/// selects the trace lane under the pointer.
pub fn press_sheet(s: &mut AppState, d: &mut Drag, tx: &mut Tx) {
    if d.phase != Phase::Down {
        return;
    }
    let pps = s.wave.pps().max(0.01);
    let last = s.wave.sample_len().saturating_sub(1);
    let i = ((d.x / pps).round().max(0.0) as usize).min(last);
    let m = tx.mods();
    if m.shift {
        s.wave.cursor_a = Some(i);
        echo(s, format!("set_cursor A {}", time_label(s, i)));
    } else if m.alt {
        s.wave.cursor_b = Some(i);
        echo(s, format!("set_cursor B {}", time_label(s, i)));
    } else {
        s.wave.cursor = i;
        if let Some(idx) = crate::views::wave_decode::trace_at_y(s, d.y) {
            s.wave.live_selected = Some(idx);
            s.wave.style.multi.clear();
        }
    }
}

/// Analog trace shape (`h` tall): staircase or linear segments between
/// value changes, scaled over the trace's own min..max.
pub fn analog_shape(s: &AppState, t: &VivadoTheme, trace: &LiveTrace, h: f32, n: usize) -> Shape {
    let pps = s.wave.pps();
    let disp = s.wave.display_of(&trace.name);
    let radix = s.wave.radix_for(&trace.name);
    let real = |v: u64| -> f64 {
        match disp.fixed {
            Some(fp) => fp.value(v, trace.width),
            None if radix == WaveRadix::Signed => sign_extend(v, trace.width) as f64,
            None => (v & helion_verif::wave::mask(trace.width)) as f64,
        }
    };
    // Change points only: long grids stay a few hundred vertices.
    let mut idx = vec![0usize];
    for i in 1..n {
        if trace.value_at(i) != trace.value_at(i - 1) {
            idx.push(i);
        }
    }
    let vals: Vec<f64> = idx.iter().map(|&i| real(trace.value_at(i))).collect();
    let ys = analog_y(&vals, 3.0, h - 3.0);
    let w = n as f32 * pps;
    let mut pts = Vec::with_capacity(idx.len() * 2 + 1);
    match disp.analog.unwrap_or(Analog::Step) {
        Analog::Step => {
            for (k, &i) in idx.iter().enumerate() {
                let x = i as f32 * pps;
                if k > 0 {
                    pts.push([x, ys[k - 1]]);
                }
                pts.push([x, ys[k]]);
            }
            pts.push([w, *ys.last().unwrap_or(&h)]);
        }
        Analog::Linear => {
            for (k, &i) in idx.iter().enumerate() {
                pts.push([i as f32 * pps, ys[k]]);
            }
            pts.push([w, *ys.last().unwrap_or(&h)]);
        }
    }
    let mut shape = Shape::new();
    let mut g = 0;
    while g <= n {
        let x = g as f32 * pps;
        shape = shape.line([x, 0.0], [x, h], 1.0, LineCap::Butt, t.wave_grid);
        g += 16;
    }
    shape.stroke(pts, 1.5, t.wave_trace)
}

// ---- trace bar -------------------------------------------------------------

fn bar_btn(
    key: &str,
    label: &str,
    t: &VivadoTheme,
    reason: Option<&str>,
    f: impl Fn(&mut AppState) + 'static,
) -> El {
    let b = row()
        .key(key.to_string())
        .role("button")
        .label(label.to_string())
        .h(20.0)
        .px(6.0)
        .items_center()
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            text(label.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.text),
        );
    match reason {
        None => b.focusable(true).hover(t.selection).on_press(f),
        Some(why) => b
            .disabled(true)
            .focusable(false)
            .description(why.to_string())
            .help(why.to_string())
            .layer_opacity(0.45),
    }
}

/// One segment of a segmented control.
fn seg(
    key: &str,
    label: &str,
    on: bool,
    t: &VivadoTheme,
    reason: Option<&str>,
    f: impl Fn(&mut AppState) + 'static,
) -> El {
    let e = el()
        .key(key.to_string())
        .role("radio")
        .label(label.to_string())
        .focus_selected(on)
        .h_full()
        .px(6.0)
        .center()
        .fill(if on { t.selection } else { t.panel_bg })
        .child(
            text(label.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.text),
        );
    match reason {
        None => e.focusable(true).hover(t.selection).on_press(f),
        Some(why) => e
            .disabled(true)
            .focusable(false)
            .description(why.to_string())
            .help(why.to_string())
            .layer_opacity(0.45),
    }
}

fn seg_group(key: &str, label: &str, t: &VivadoTheme) -> El {
    row()
        .key(key.to_string())
        .role("radiogroup")
        .label(label.to_string())
        .h(20.0)
        .items_center()
        .border(1.0, t.border)
}

fn caption(content: &str, t: &VivadoTheme) -> El {
    text(content.to_string())
        .font(crate::views::cad::FONT_PIN)
        .color(t.text_dim)
}

fn sep(t: &VivadoTheme) -> El {
    el().w(1.0).h(14.0).fill(t.border).a11y_hidden()
}

/// Edit the selected trace's display (no-op without a selection).
fn edit_display(s: &mut AppState, f: impl Fn(&mut TraceDisplay)) -> Option<String> {
    let name = s.wave.selected_trace()?.name.clone();
    let d = s.wave.style.display.entry(name.clone()).or_default();
    f(d);
    if *d == TraceDisplay::default() {
        s.wave.style.display.remove(&name);
    }
    Some(name)
}

/// Trace strip under the sim toolbar: lane order/removal, virtual bus,
/// fixed-point and analog display of the selected bus, decoder panel
/// toggle, and lane density. Unavailable actions say why.
pub fn trace_bar(s: &AppState, t: &VivadoTheme) -> El {
    let live = s.wave.has_live();
    let sel = s.wave.selected_trace();
    let lanes = s.wave.lanes();
    let pos = s
        .wave
        .live_selected
        .and_then(|i| lanes.iter().position(|&l| l == i));
    let no_live = (!live).then_some("Run a simulation first — demo traces are read-only");
    let lane_why = no_live.or(if pos.is_none() {
        Some("Select a trace lane first")
    } else {
        None
    });
    let up_why = lane_why.or((pos == Some(0)).then_some("Already the top lane"));
    let down_why = lane_why
        .or((pos == Some(lanes.len().saturating_sub(1))).then_some("Already the bottom lane"));
    let bus_why = no_live.or(s.wave.bus_blocker());
    let bus_sel = sel.filter(|t| t.is_bus());
    let fmt_why = no_live.or(match sel {
        None => Some("Select a multi-bit trace first"),
        Some(t) if !t.is_bus() => Some("Fixed point and analog apply to multi-bit traces"),
        _ => None,
    });
    let disp = bus_sel
        .map(|t| s.wave.display_of(&t.name))
        .unwrap_or_default();
    let n_sel = s.wave.selection().len();
    let target = match (sel, n_sel) {
        (_, n) if n > 1 => format!("{n} traces selected"),
        (Some(tr), _) => tr.leaf().to_string(),
        (None, _) => "No trace selected".into(),
    };

    let mut fixed = seg_group("wave-fixed", "Fixed point", t)
        .child(seg(
            "wave-fixed-off",
            "Off",
            disp.fixed.is_none(),
            t,
            fmt_why,
            |s: &mut AppState| {
                if let Some(n) = edit_display(s, |d| d.fixed = None) {
                    echo(
                        s,
                        format!("set_property display_mode default [get_waves {{{n}}}]"),
                    );
                }
            },
        ))
        .child(seg(
            "wave-fixed-q",
            "Q",
            disp.fixed.is_some_and(|f| f.signed),
            t,
            fmt_why,
            |s: &mut AppState| set_fixed(s, Some(true), 0),
        ))
        .child(seg(
            "wave-fixed-uq",
            "UQ",
            disp.fixed.is_some_and(|f| !f.signed),
            t,
            fmt_why,
            |s: &mut AppState| set_fixed(s, Some(false), 0),
        ));
    let frac_why = fmt_why.or(disp.fixed.is_none().then_some("Choose Q or UQ first"));
    let frac_label = match (disp.fixed, bus_sel) {
        (Some(fp), Some(tr)) => fp.label_for(tr.width),
        _ => "—".into(),
    };
    fixed = fixed.child(
        el().key("wave-fixed-label")
            .w(54.0)
            .h_full()
            .center()
            .child(caption(&frac_label, t)),
    );
    let fixed = row()
        .gap(2.0)
        .items_center()
        .child(fixed)
        .child(bar_btn(
            "wave-frac-dec",
            "−",
            t,
            frac_why,
            |s: &mut AppState| set_fixed(s, None, -1),
        ))
        .child(bar_btn(
            "wave-frac-inc",
            "+",
            t,
            frac_why,
            |s: &mut AppState| set_fixed(s, None, 1),
        ));

    let mut analog = seg_group("wave-analog", "Trace style", t);
    for (key, label, mode) in [
        ("wave-style-digital", "Digital", None),
        ("wave-style-step", "Step", Some(Analog::Step)),
        ("wave-style-linear", "Linear", Some(Analog::Linear)),
    ] {
        analog = analog.child(seg(key, label, disp.analog == mode, t, fmt_why, move |s| {
            if let Some(n) = edit_display(s, |d| d.analog = mode) {
                let m = match mode {
                    None => "digital",
                    Some(Analog::Step) => "analog_step",
                    Some(Analog::Linear) => "analog_linear",
                };
                echo(
                    s,
                    format!("set_property wave_style {m} [get_waves {{{n}}}]"),
                );
            }
        }));
    }

    let mut density = seg_group("wave-density", "Row density", t);
    for d in Density::ALL {
        density = density.child(seg(
            &format!("wave-density-{}", d.label().to_lowercase()),
            d.label(),
            s.wave.style.density == d,
            t,
            None,
            move |s: &mut AppState| s.wave.style.density = d,
        ));
    }

    let n_dec = s.wave.decode.decoders.len();
    let dec_label = if n_dec == 0 {
        "Decoders".to_string()
    } else {
        format!("Decoders ({n_dec})")
    };
    let dec_btn = bar_btn(
        "wave-decoders-toggle",
        &dec_label,
        t,
        no_live,
        |s: &mut AppState| s.wave.decode.panel_open = !s.wave.decode.panel_open,
    )
    .fill(if s.wave.decode.panel_open {
        t.selection
    } else {
        t.panel_bg
    });

    row()
        .key("wave-trace-bar")
        .role("toolbar")
        .w_full()
        .h(26.0)
        .px(4.0)
        .gap(4.0)
        .items_center()
        .fill(t.panel_bg)
        .border(1.0, t.border_light)
        .child(
            el().key("wave-trace-target").w(92.0).child(
                text(target.clone())
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim)
                    .truncate()
                    .help_when_truncated(target),
            ),
        )
        .child(bar_btn("wave-trace-up", "▲", t, up_why, move_selected(-1)).label("Move trace up"))
        .child(
            bar_btn("wave-trace-down", "▼", t, down_why, move_selected(1)).label("Move trace down"),
        )
        .child(bar_btn(
            "wave-trace-remove",
            "Remove",
            t,
            lane_why,
            |s: &mut AppState| {
                if let Some(idx) = s.wave.live_selected {
                    let name = s.wave.selected_trace().map(|t| t.name.clone());
                    if s.wave.remove_lane(idx) {
                        if let Some(n) = name {
                            echo(s, format!("remove_wave [get_waves {{{n}}}]"));
                        }
                    }
                }
            },
        ))
        .child(bar_btn(
            "wave-make-bus",
            "Make Bus",
            t,
            bus_why,
            |s: &mut AppState| match s.wave.create_virtual_bus() {
                Ok(name) => {
                    let members = s
                        .wave
                        .style
                        .buses
                        .last()
                        .map(|b| b.members.join(" "))
                        .unwrap_or_default();
                    echo(s, format!("add_wave_virtual_bus {name} {{{members}}}"));
                }
                Err(e) => s.toasts.push(
                    crate::state::ToastKind::Error,
                    "Cannot create virtual bus",
                    &e,
                ),
            },
        ))
        .child(sep(t))
        .child(fixed)
        .child(analog)
        .child(el().grow(1.0))
        .child(dec_btn)
        .child(density)
}

fn move_selected(delta: i32) -> impl Fn(&mut AppState) + 'static {
    move |s: &mut AppState| {
        if let Some(idx) = s.wave.live_selected {
            if s.wave.move_lane(idx, delta) {
                let name = s
                    .wave
                    .selected_trace()
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                echo(
                    s,
                    format!(
                        "move_wave {} [get_waves {{{name}}}]",
                        if delta < 0 { "up" } else { "down" }
                    ),
                );
            }
        }
    }
}

/// Set fixed point on the selected bus: `signed = Some(..)` switches
/// Q/UQ (keeping the fraction), `step` nudges the fractional bit count.
pub fn set_fixed(s: &mut AppState, signed: Option<bool>, step: i32) {
    let Some(width) = s.wave.selected_trace().map(|t| t.width) else {
        return;
    };
    let max = width.saturating_sub(1).max(0);
    let res = edit_display(s, |d| {
        let cur = d.fixed.unwrap_or(FixedPoint {
            frac_bits: DEFAULT_FRAC.min(max),
            signed: true,
        });
        let frac = (i32::from(cur.frac_bits) + step).clamp(0, i32::from(max)) as u8;
        d.fixed = Some(FixedPoint {
            frac_bits: frac,
            signed: signed.unwrap_or(cur.signed),
        });
    });
    if let Some(n) = res {
        if let Some(fp) = s.wave.display_of(&n).fixed {
            echo(
                s,
                format!(
                    "set_property display_mode {} [get_waves {{{n}}}]",
                    fp.label_for(width).to_lowercase()
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::wave_demo::seed_protocol_fixture;
    use crate::{app_spec, init};
    use rune::{Env, Mods, PhysicalInput, Runtime};

    fn fixture() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Wave;
        seed_protocol_fixture(&mut s);
        s
    }

    fn idx(s: &AppState, leaf: &str) -> usize {
        s.wave
            .live
            .as_ref()
            .unwrap()
            .traces
            .iter()
            .position(|t| t.leaf() == leaf)
            .unwrap_or_else(|| panic!("no trace {leaf}"))
    }

    fn rt(s: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        let env = Env {
            system_dark: false,
            scale: 2.0,
        };
        let mut rt = Runtime::new(s, crate::view, spec.size, env);
        rt.frame(1.0 / 60.0);
        rt
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn rect(rt: &Runtime<AppState>, key: &str) -> [f32; 4] {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .map(|e| e.rect)
            .unwrap_or_else(|| panic!("missing {key}"))
    }

    fn mods(rt: &mut Runtime<AppState>, m: Mods) {
        rt.physical_input(PhysicalInput::Modifiers(m));
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn lanes_reorder_and_remove_down_to_empty() {
        let mut w = fixture().wave;
        let first = w.lanes();
        assert!(w.move_lane(first[1], -1));
        assert_eq!(w.lanes()[0], first[1]);
        assert!(!w.move_lane(first[1], -1), "already the top lane");
        w.live_selected = Some(first[0]);
        for l in first.clone() {
            assert!(w.remove_lane(l));
        }
        assert!(
            w.lanes().is_empty(),
            "removing every lane must not restore defaults"
        );
        assert_eq!(w.live_selected, None);
        assert!(w.add_live_lane(first[3]));
        assert_eq!(w.lanes(), vec![first[3]]);
    }

    #[test]
    fn virtual_bus_packs_counter_bits_and_survives_reimport() {
        let mut s = fixture();
        let bits: Vec<usize> = (0..4).map(|b| idx(&s, &format!("cnt_{b}"))).collect();
        s.wave.live_selected = Some(bits[2]);
        s.wave.style.multi = vec![bits[0], bits[3], bits[1]];
        assert_eq!(s.wave.bus_blocker(), None);
        let name = s.wave.create_virtual_bus().unwrap();
        assert_eq!(name, "bus_tb.cnt");
        let bus = s.wave.selected_trace().unwrap().clone();
        assert_eq!(bus.width, 4);
        for i in [0usize, 8, 37, 200] {
            assert_eq!(bus.value_at(i), ((i / 8) % 16) as u64, "sample {i}");
        }
        assert!(s.wave.lanes().contains(&s.wave.live_selected.unwrap()));
        // A new import re-packs the bus from its members by name.
        seed_protocol_fixture(&mut s);
        let again = s.wave.live.as_ref().unwrap();
        assert!(again.traces.iter().any(|t| t.name == "bus_tb.cnt"));
        // Mixed widths are refused with a reason.
        s.wave.live_selected = Some(idx(&s, "state"));
        s.wave.style.multi = vec![bits[0]];
        assert_eq!(
            s.wave.bus_blocker(),
            Some("Virtual buses pack 1-bit traces only")
        );
    }

    #[test]
    fn fixed_point_reformats_and_radix_clears_it() {
        let mut s = fixture();
        s.wave.live_selected = Some(idx(&s, "dac"));
        set_fixed(&mut s, Some(true), 0);
        set_fixed(&mut s, None, 2);
        let tr = s.wave.selected_trace().unwrap().clone();
        assert_eq!(s.wave.format_tag(&tr), "Q1.6");
        // sample 30 is the sine peak: 64 / 2^6 = 1.0.
        assert_eq!(s.wave.trace_value(&tr, 30), "1.0");
        assert_eq!(s.wave.trace_value(&tr, 90), "-1.0");
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.contains("display_mode q1.6"))
        );
        s.wave.set_radix(WaveRadix::Signed);
        assert_eq!(s.wave.trace_value(&tr, 90), "-64");
        assert_eq!(s.wave.display_of(&tr.name).fixed, None);
    }

    #[test]
    fn analog_lanes_are_taller_and_density_shrinks_rows() {
        let mut s = fixture();
        let dac = idx(&s, "dac");
        s.wave.live_lanes = vec![dac, idx(&s, "clk")];
        s.wave.lanes_custom = true;
        s.wave.live_selected = Some(dac);
        let mut r = rt(s);
        let h = |r: &Runtime<AppState>, k: &str| rect(r, k)[3];
        let key = format!("wave-live-lane-{dac}");
        // Layout changes animate: advance virtual time until they settle.
        let mut now = 0.1;
        let mut step = |r: &mut Runtime<AppState>, k: &str| {
            tap(r, k);
            for _ in 0..60 {
                now += 1.0 / 30.0;
                r.frame(now);
            }
        };
        assert_eq!(h(&r, &key), 24.0);
        step(&mut r, "wave-style-step");
        assert_eq!(
            r.state.wave.display_of("bus_tb.dac").analog,
            Some(Analog::Step)
        );
        assert_eq!(h(&r, &key), 48.0);
        step(&mut r, "wave-density-compact");
        assert_eq!(h(&r, &key), 36.0);
        step(&mut r, "wave-style-digital");
        assert_eq!(h(&r, &key), 18.0);
    }

    #[test]
    fn shift_and_alt_click_place_cursors_a_and_b() {
        let mut r = rt(fixture());
        let sheet = rect(&r, "wave-sheet");
        let pps = r.state.wave.pps();
        let x = sheet[0] + 40.0 * pps;
        let y = sheet[1] + 30.0;
        mods(
            &mut r,
            Mods {
                shift: true,
                ..Mods::default()
            },
        );
        r.pointer_move(x, y);
        r.pointer_down(x, y);
        r.pointer_up(x, y);
        r.frame(1.0 / 60.0);
        assert_eq!(r.state.wave.cursor_a, Some(40));
        let x2 = sheet[0] + 70.0 * pps;
        mods(
            &mut r,
            Mods {
                alt: true,
                ..Mods::default()
            },
        );
        r.pointer_move(x2, y);
        r.pointer_down(x2, y);
        r.pointer_up(x2, y);
        r.frame(1.0 / 60.0);
        assert_eq!(r.state.wave.cursor_b, Some(70));
        mods(&mut r, Mods::default());
        let x3 = sheet[0] + 10.0 * pps;
        r.pointer_move(x3, y);
        r.pointer_down(x3, y);
        r.pointer_up(x3, y);
        r.frame(1.0 / 60.0);
        assert_eq!(r.state.wave.cursor, 10);
        // y = 30 sits in the second lane (24pt rows).
        assert_eq!(r.state.wave.live_selected, Some(r.state.wave.lanes()[1]));
        assert!(
            r.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("set_cursor A"))
        );
    }

    #[test]
    fn cmd_click_multiselects_and_make_bus_builds_the_lane() {
        let mut s = fixture();
        let bits: Vec<usize> = (0..4).map(|b| idx(&s, &format!("cnt_{b}"))).collect();
        s.wave.live_lanes = bits.clone();
        s.wave.lanes_custom = true;
        let mut r = rt(s);
        tap(&mut r, &format!("wave-live-name-{}", bits[0]));
        mods(
            &mut r,
            Mods {
                cmd: true,
                ..Mods::default()
            },
        );
        for b in &bits[1..] {
            tap(&mut r, &format!("wave-live-name-{b}"));
        }
        mods(&mut r, Mods::default());
        assert_eq!(r.state.wave.selection(), bits);
        tap(&mut r, "wave-make-bus");
        let lanes = r.state.wave.lanes();
        assert_eq!(lanes.len(), 5);
        assert!(r.state.wave.is_virtual("bus_tb.cnt"));
        assert!(r.find(&format!("wave-live-lane-{}", lanes[4])).is_some());
        assert!(
            r.state
                .console
                .lines
                .iter()
                .any(|l| l.text.starts_with("add_wave_virtual_bus bus_tb.cnt"))
        );
        // Up / Remove act on the new bus lane.
        tap(&mut r, "wave-trace-up");
        assert_eq!(r.state.wave.lanes()[3], lanes[4]);
        tap(&mut r, "wave-trace-remove");
        assert_eq!(r.state.wave.lanes().len(), 4);
    }

    #[test]
    fn trace_bar_disables_with_reasons_on_the_demo_fixture() {
        let mut s = init();
        s.center = crate::state::CenterPane::Wave;
        crate::views::wave_demo::seed_demo_wave(&mut s);
        let r = rt(s);
        for key in [
            "wave-trace-up",
            "wave-trace-remove",
            "wave-make-bus",
            "wave-fixed-q",
            "wave-decoders-toggle",
        ] {
            let e = r
                .elements()
                .into_iter()
                .find(|e| e.key.as_deref() == Some(key))
                .unwrap_or_else(|| panic!("missing {key}"));
            assert!(
                !e.focusable && e.help.as_deref().is_some_and(|h| h.contains("simulation")),
                "{key} must be disabled with a reason on demo traces"
            );
        }
        // Density still drives the demo lanes.
        assert!(r.find("wave-density-compact").is_some());
    }

    #[test]
    fn objects_filter_searches_all_scopes() {
        let mut s = fixture();
        s.wave.filter = rune::TextEdit::new("awv");
        let r = rt(s);
        let rows: Vec<String> = r
            .elements()
            .into_iter()
            .filter_map(|e| e.key)
            .filter(|k| k.starts_with("wave-live-object-"))
            .collect();
        assert_eq!(rows.len(), 1, "{rows:?}");
        let mut s = fixture();
        s.wave.filter = rune::TextEdit::new("zzz");
        let r = rt(s);
        assert!(r.find("wave-objects-nomatch").is_some());
    }
}
