//! Protocol decoders for the live waveform: AXI-Lite, SPI, UART, and FSM
//! state names, rendered as decoded lanes under the traces.
//!
//! Each decoder maps protocol roles (SCLK, MOSI, AWVALID, ...) to live
//! traces. Roles auto-assign by signal name when a decoder is added and
//! re-map through a per-role picker popover. Decoding itself is
//! [`helion_verif::wave`] (`decode_spi`/`decode_uart`/`decode_axi_lite`/
//! `decode_fsm`); FSM state names come from `localparam` / `enum` groups in
//! the open HDL buffers (`fsm_name_groups`). Nothing is decoded until a
//! simulation imports real traces.
//!
//! Old-IDE reference: `helion-gui/src/ide.rs` `WaveProtocol` and
//! `WaveTrace::decode_transactions`; `helion-ide.rs` 12091-12104 (the
//! per-trace "Protocol:" menu).

use helion_verif::wave::{
    AxiLiteSignals, DecodedLane, SpanKind, SpiConfig, best_name_group, decode_axi_lite, decode_fsm,
    decode_spi, decode_uart, estimate_bit_period, fsm_name_groups,
};
use rune::{El, ScrollSpec, Shape, canvas, col, el, icon, row, text};
use rune_kit::glyphs;

use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::wave::{LiveTrace, WaveState};

/// Protocol a decoder speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecoderKind {
    AxiLite,
    Spi,
    Uart,
    Fsm,
}

impl DecoderKind {
    pub const ALL: [DecoderKind; 4] = [
        DecoderKind::AxiLite,
        DecoderKind::Spi,
        DecoderKind::Uart,
        DecoderKind::Fsm,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DecoderKind::AxiLite => "AXI-Lite",
            DecoderKind::Spi => "SPI",
            DecoderKind::Uart => "UART",
            DecoderKind::Fsm => "FSM",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            DecoderKind::AxiLite => "axi",
            DecoderKind::Spi => "spi",
            DecoderKind::Uart => "uart",
            DecoderKind::Fsm => "fsm",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "axi" | "axilite" | "axi-lite" | "axi4-lite" => Some(DecoderKind::AxiLite),
            "spi" => Some(DecoderKind::Spi),
            "uart" => Some(DecoderKind::Uart),
            "fsm" | "state" | "counter" => Some(DecoderKind::Fsm),
            _ => None,
        }
    }

    pub fn roles(self) -> &'static [Role] {
        match self {
            DecoderKind::AxiLite => AXI_ROLES,
            DecoderKind::Spi => SPI_ROLES,
            DecoderKind::Uart => UART_ROLES,
            DecoderKind::Fsm => FSM_ROLES,
        }
    }
}

/// One decoder input. `bit` = 1-bit only; otherwise any width.
#[derive(Clone, Copy, Debug)]
pub struct Role {
    pub label: &'static str,
    pub bit: bool,
    /// Leaf names (or `_`-suffixes) that auto-assign this role.
    pub aliases: &'static [&'static str],
}

const fn r(label: &'static str, bit: bool, aliases: &'static [&'static str]) -> Role {
    Role {
        label,
        bit,
        aliases,
    }
}

const SPI_ROLES: &[Role] = &[
    r("SCLK", true, &["sclk", "sck", "spi_clk"]),
    r("MOSI", true, &["mosi", "copi", "sdi"]),
    r("MISO", true, &["miso", "cipo", "sdo"]),
    r("CS_N", true, &["cs_n", "csn", "cs", "ss_n", "ss", "ssn"]),
];
const UART_ROLES: &[Role] = &[r(
    "LINE",
    true,
    &["tx", "rx", "txd", "rxd", "uart_tx", "uart_rx"],
)];
const FSM_ROLES: &[Role] = &[r(
    "STATE",
    false,
    &["state", "fsm", "state_q", "cur_state", "st"],
)];
const AXI_ROLES: &[Role] = &[
    r("ACLK", true, &["aclk"]),
    r("AWVALID", true, &["awvalid"]),
    r("AWREADY", true, &["awready"]),
    r("AWADDR", false, &["awaddr"]),
    r("WVALID", true, &["wvalid"]),
    r("WREADY", true, &["wready"]),
    r("WDATA", false, &["wdata"]),
    r("BVALID", true, &["bvalid"]),
    r("BREADY", true, &["bready"]),
    r("BRESP", false, &["bresp"]),
    r("ARVALID", true, &["arvalid"]),
    r("ARREADY", true, &["arready"]),
    r("ARADDR", false, &["araddr"]),
    r("RVALID", true, &["rvalid"]),
    r("RREADY", true, &["rready"]),
    r("RDATA", false, &["rdata"]),
    r("RRESP", false, &["rresp"]),
];

/// One configured decoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoder {
    /// Stable id for element keys (survives removal of other decoders).
    pub id: usize,
    pub kind: DecoderKind,
    /// Trace name per role (parallel to [`DecoderKind::roles`]).
    pub inputs: Vec<Option<String>>,
    pub spi_mode: u8,
    pub spi_bits: u8,
    /// UART bit period in samples (`None` = auto-baud).
    pub uart_period: Option<usize>,
    /// FSM encoding → state name (from the HDL; empty = old counter wording).
    pub fsm_names: Vec<(u64, String)>,
}

/// Decoder slice of [`WaveState`].
#[derive(Clone, Debug, Default)]
pub struct WaveDecodeState {
    pub decoders: Vec<Decoder>,
    pub panel_open: bool,
    pub next_id: usize,
    /// Open role picker: (decoder id, role index).
    pub pick: Option<(usize, usize)>,
}

fn echo(s: &mut AppState, text: String) {
    s.console.lines.push(ConsoleLine {
        text,
        sev: MsgSev::Info,
    });
}

fn alias_match(leaf: &str, alias: &str) -> bool {
    let l = leaf.to_ascii_lowercase();
    l == alias || l.ends_with(&format!("_{alias}"))
}

fn fits(role: &Role, t: &LiveTrace) -> bool {
    !role.bit || t.width == 1
}

impl WaveState {
    fn trace_named(&self, name: &str) -> Option<&LiveTrace> {
        self.live.as_ref()?.traces.iter().find(|t| t.name == name)
    }

    /// Add a decoder with roles auto-assigned by signal name (FSM falls
    /// back to the selected multi-bit trace). Returns its id.
    pub fn add_decoder(&mut self, kind: DecoderKind, hdl: &str) -> usize {
        let id = self.decode.next_id;
        self.decode.next_id += 1;
        let traces: Vec<LiveTrace> = self
            .live
            .as_ref()
            .map(|l| l.traces.clone())
            .unwrap_or_default();
        let mut inputs: Vec<Option<String>> = kind
            .roles()
            .iter()
            .map(|role| {
                role.aliases.iter().find_map(|a| {
                    traces
                        .iter()
                        .find(|t| fits(role, t) && alias_match(t.leaf(), a))
                        .map(|t| t.name.clone())
                })
            })
            .collect();
        if kind == DecoderKind::Fsm && inputs[0].is_none() {
            inputs[0] = self
                .selected_trace()
                .filter(|t| t.is_bus())
                .map(|t| t.name.clone());
        }
        let mut d = Decoder {
            id,
            kind,
            inputs,
            spi_mode: 0,
            spi_bits: 8,
            uart_period: None,
            fsm_names: Vec::new(),
        };
        self.refresh_fsm_names(&mut d, hdl);
        self.decode.decoders.push(d);
        self.decode.panel_open = true;
        id
    }

    /// Re-derive FSM state names for the mapped state trace from `hdl`.
    fn refresh_fsm_names(&self, d: &mut Decoder, hdl: &str) {
        if d.kind != DecoderKind::Fsm {
            return;
        }
        d.fsm_names.clear();
        let Some(t) = d.inputs[0].as_deref().and_then(|n| self.trace_named(n)) else {
            return;
        };
        let mut seen: Vec<u64> = Vec::new();
        for &v in &t.samples {
            if !seen.contains(&v) {
                seen.push(v);
                if seen.len() > 64 {
                    break;
                }
            }
        }
        if let Some(g) = best_name_group(&fsm_name_groups(hdl), &seen) {
            d.fsm_names = g;
        }
    }

    /// Map role `role` of decoder `id` to `trace` (`None` clears it).
    pub fn assign_role(&mut self, id: usize, role: usize, trace: Option<String>, hdl: &str) {
        let Some(pos) = self.decode.decoders.iter().position(|d| d.id == id) else {
            return;
        };
        let mut d = self.decode.decoders[pos].clone();
        if let Some(slot) = d.inputs.get_mut(role) {
            *slot = trace;
        }
        self.refresh_fsm_names(&mut d, hdl);
        self.decode.decoders[pos] = d;
    }

    /// Decode one decoder over the live import. `Err` names what to map.
    pub fn run_decoder(&self, d: &Decoder) -> Result<Vec<DecodedLane>, String> {
        let get = |i: usize| -> Option<&[u64]> {
            d.inputs
                .get(i)?
                .as_deref()
                .and_then(|n| self.trace_named(n))
                .map(|t| t.samples.as_slice())
        };
        match d.kind {
            DecoderKind::Spi => {
                let sclk = get(0).ok_or("Map SCLK to decode")?;
                if get(1).is_none() && get(2).is_none() {
                    return Err("Map MOSI or MISO to decode".into());
                }
                Ok(decode_spi(
                    sclk,
                    get(1),
                    get(2),
                    get(3),
                    SpiConfig {
                        mode: d.spi_mode,
                        bits: d.spi_bits,
                        msb_first: true,
                    },
                ))
            }
            DecoderKind::Uart => {
                let line = get(0).ok_or("Map LINE to decode")?;
                let mut lane = decode_uart(line, d.uart_period);
                lane.name = "UART".into();
                Ok(vec![lane])
            }
            DecoderKind::Fsm => {
                let name = d.inputs[0].as_deref().ok_or("Map STATE to decode")?;
                let t = self
                    .trace_named(name)
                    .ok_or("STATE trace is not in this run")?;
                Ok(vec![decode_fsm(&t.samples, t.width, &d.fsm_names)])
            }
            DecoderKind::AxiLite => {
                let sig = AxiLiteSignals {
                    aclk: get(0),
                    awvalid: get(1),
                    awready: get(2),
                    awaddr: get(3),
                    wvalid: get(4),
                    wready: get(5),
                    wdata: get(6),
                    bvalid: get(7),
                    bready: get(8),
                    bresp: get(9),
                    arvalid: get(10),
                    arready: get(11),
                    araddr: get(12),
                    rvalid: get(13),
                    rready: get(14),
                    rdata: get(15),
                    rresp: get(16),
                };
                let lanes = decode_axi_lite(&sig);
                if lanes.is_empty() {
                    return Err("Map AW+W (valid/ready/addr/data) or AR channel signals".into());
                }
                Ok(lanes)
            }
        }
    }
}

/// One row of the live sheet: a trace lane or a decoded lane.
#[derive(Clone, Debug)]
pub enum LiveRow {
    Trace(usize),
    Decoded {
        dec: usize,
        title: String,
        lane: DecodedLane,
    },
}

/// Live rows top to bottom with their y offset and height: mounted trace
/// lanes, then every decoder's lanes.
pub fn live_rows(w: &WaveState) -> Vec<(LiveRow, f32, f32)> {
    let mut out = Vec::new();
    let mut y = 0.0;
    let Some(live) = w.live.as_ref().filter(|l| !l.is_empty()) else {
        return out;
    };
    for idx in w.lanes() {
        if let Some(t) = live.traces.get(idx) {
            let h = w.live_lane_h(t);
            out.push((LiveRow::Trace(idx), y, h));
            y += h;
        }
    }
    let h = w.style.density.lane_h();
    for (k, d) in w.decode.decoders.iter().enumerate() {
        if let Ok(lanes) = w.run_decoder(d) {
            for lane in lanes {
                let title = format!("{} #{} · {}", d.kind.label(), k + 1, lane.name);
                out.push((
                    LiveRow::Decoded {
                        dec: d.id,
                        title,
                        lane,
                    },
                    y,
                    h,
                ));
                y += h;
            }
        }
    }
    out
}

/// Trace lane under sheet y (decoded lanes select nothing).
pub fn trace_at_y(s: &AppState, y: f32) -> Option<usize> {
    live_rows(&s.wave)
        .into_iter()
        .find(|(_, top, h)| y >= *top && y < top + h)
        .and_then(|(r, _, _)| match r {
            LiveRow::Trace(i) => Some(i),
            LiveRow::Decoded { .. } => None,
        })
}

fn span_color(kind: SpanKind, t: &VivadoTheme) -> rune::Color {
    match kind {
        SpanKind::Read => t.link,
        SpanKind::Write => t.warn,
        SpanKind::Data => t.pass,
        SpanKind::State => t.accent,
        SpanKind::Error => t.fail,
    }
}

/// Name-column row of a decoded lane (`SPI #1 · MOSI`, span count).
pub fn decoded_name_row(
    key: String,
    title: &str,
    lane: &DecodedLane,
    h: f32,
    t: &VivadoTheme,
) -> El {
    let count = lane.spans.len().to_string();
    row()
        .key(key)
        .w_full()
        .h(h)
        .px(6.0)
        .gap(6.0)
        .items_center()
        .fill(t.header_bg)
        .child(
            icon(&glyphs::TRIANGLE_RIGHT, 9.0)
                .color(t.header_bg)
                .a11y_hidden(),
        )
        .child(
            el().w(104.0).child(
                text(title.to_string())
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim)
                    .truncate()
                    .help_when_truncated(title.to_string()),
            ),
        )
        .child(
            el().grow(1.0).min_w(0.0).child(
                text(if count == "1" {
                    "1 span".to_string()
                } else {
                    format!("{count} spans")
                })
                .font(crate::views::cad::FONT_PIN)
                .color(t.text_faint)
                .truncate(),
            ),
        )
}

/// Decoded lane on the sheet: hexagon spans (one canvas) plus labels on
/// spans wide enough to hold them; narrow spans carry no label element.
pub fn decoded_lane(s: &AppState, key: String, lane: &DecodedLane, h: f32, t: &VivadoTheme) -> El {
    let pps = s.wave.pps();
    let mut shape = Shape::new();
    let (top, bot, mid) = (3.0, h - 3.0, h / 2.0);
    let mut labels = Vec::new();
    for (k, sp) in lane.spans.iter().enumerate() {
        let x0 = sp.start as f32 * pps;
        let x1 = (sp.end.max(sp.start + 1)) as f32 * pps;
        let c = span_color(sp.kind, t);
        let p = 2.0f32.min((x1 - x0) / 2.0);
        let pts = vec![
            [x0, mid],
            [x0 + p, top],
            [x1 - p, top],
            [x1, mid],
            [x1 - p, bot],
            [x0 + p, bot],
            [x0, mid],
        ];
        shape = shape.fill(pts.clone(), c.alpha(0.16)).stroke(pts, 1.0, c);
        // Spans wide enough for a few glyphs get a (truncating) label;
        // the full text is the tooltip.
        if x1 - x0 >= 22.0 && labels.len() < 400 {
            labels.push(
                el().key(format!("{key}-span-{k}"))
                    .absolute(x0 + 4.0, (h - 12.0) / 2.0)
                    .w(x1 - x0 - 8.0)
                    .child(
                        text(sp.label.clone())
                            .font(crate::views::cad::FONT_PIN)
                            .color(t.text)
                            .truncate()
                            .help_when_truncated(sp.label.clone()),
                    ),
            );
        }
    }
    let mut l = el()
        .key(key)
        .role("img")
        .label(format!(
            "{} decoded lane, {} spans",
            lane.name,
            lane.spans.len()
        ))
        .w(s.wave.sheet_w())
        .h(h)
        .fill(t.panel_bg)
        .child(canvas(shape).w_full().h_full());
    for lab in labels {
        l = l.child(lab);
    }
    l
}

/// HDL text the FSM name scan reads: every open buffer.
pub fn hdl_text(s: &AppState) -> String {
    let mut out = s.editor.lines.join("\n");
    out.push('\n');
    out.push_str(&s.editor.tb_lines.join("\n"));
    for b in s.editor.parked.values() {
        out.push('\n');
        out.push_str(&b.lines.join("\n"));
    }
    out
}

/// Add a decoder from the panel (or the `wave_decoder` verb).
pub fn add_decoder(s: &mut AppState, kind: DecoderKind) -> usize {
    let hdl = hdl_text(s);
    let id = s.wave.add_decoder(kind, &hdl);
    echo(s, format!("add_wave_decoder {}", kind.slug()));
    id
}

fn remove_decoder(s: &mut AppState, id: usize) {
    s.wave.decode.decoders.retain(|d| d.id != id);
    if s.wave.decode.pick.is_some_and(|(d, _)| d == id) {
        s.wave.decode.pick = None;
    }
    echo(s, format!("remove_wave_decoder {id}"));
}

fn edit(s: &mut AppState, id: usize, f: impl Fn(&mut Decoder)) {
    if let Some(d) = s.wave.decode.decoders.iter_mut().find(|d| d.id == id) {
        f(d);
    }
}

fn small_btn(
    key: String,
    label: &str,
    t: &VivadoTheme,
    reason: Option<&str>,
    f: impl Fn(&mut AppState) + 'static,
) -> El {
    crate::views::cad::btn(key, label, t, reason, f)
        .h(20.0)
        .px(6.0)
}

fn role_trigger(d: &Decoder, k: usize, role: &Role, t: &VivadoTheme) -> El {
    let id = d.id;
    let current = d.inputs[k]
        .as_deref()
        .map(|n| n.rsplit('.').next().unwrap_or(n).to_string())
        .unwrap_or_else(|| "— none —".into());
    let full = d.inputs[k].clone().unwrap_or_default();
    el().key(format!("wave-dec-{id}-role-{k}"))
        .role("button")
        .label(format!("{} signal, current {current}", role.label))
        .focusable(true)
        .grow(1.0)
        .min_w(0.0)
        .h(20.0)
        .px(6.0)
        .center_y()
        .radius(3.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .hover(t.selection)
        .on_press(move |s: &mut AppState| {
            s.wave.decode.pick = if s.wave.decode.pick == Some((id, k)) {
                None
            } else {
                Some((id, k))
            };
        })
        .child(
            row()
                .w_full()
                .items_center()
                .gap(4.0)
                .child(
                    text(current.clone())
                        .font(VivadoTheme::FONT_MONO)
                        .color(if d.inputs[k].is_some() {
                            t.text
                        } else {
                            t.text_faint
                        })
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate()
                        .help_when_truncated(full),
                )
                .child(
                    icon(&glyphs::CHEVRON_DOWN, 10.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        )
}

fn opt_row(k: &str, label: &str, t: &VivadoTheme) -> El {
    row()
        .w_full()
        .h(22.0)
        .items_center()
        .gap(4.0)
        .child(
            el().w(64.0).child(
                text(label.to_string())
                    .font(crate::views::cad::FONT_PIN)
                    .color(t.text_dim),
            ),
        )
        .key(k.to_string())
}

/// One decoder card in the panel: title + status, role pickers, options.
fn decoder_card(s: &AppState, k: usize, d: &Decoder, t: &VivadoTheme) -> El {
    let id = d.id;
    let result = s.wave.run_decoder(d);
    let status = match &result {
        Ok(lanes) => {
            let n: usize = lanes.iter().map(|l| l.spans.len()).sum();
            (
                if n == 1 {
                    "1 span".into()
                } else {
                    format!("{n} spans")
                },
                t.pass,
            )
        }
        Err(e) => (e.clone(), t.warn),
    };
    let mut card = col()
        .key(format!("wave-dec-{id}"))
        .w_full()
        .gap(2.0)
        .padding(6.0)
        .radius(4.0)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .h(20.0)
                .gap(6.0)
                .items_center()
                .child(
                    text(format!("{} #{}", d.kind.label(), k + 1))
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text),
                )
                .child(
                    el().grow(1.0).min_w(0.0).child(
                        text(status.0.clone())
                            .key(format!("wave-dec-{id}-status"))
                            .font(crate::views::cad::FONT_PIN)
                            .color(status.1)
                            .truncate()
                            .help_when_truncated(status.0),
                    ),
                )
                .child(
                    small_btn(format!("wave-dec-{id}-remove"), "✕", t, None, move |s| {
                        remove_decoder(s, id)
                    })
                    .label(format!("Remove {} decoder", d.kind.label())),
                ),
        );
    for (r, role) in d.kind.roles().iter().enumerate() {
        card = card.child(
            opt_row(&format!("wave-dec-{id}-row-{r}"), role.label, t)
                .child(role_trigger(d, r, role, t)),
        );
    }
    match d.kind {
        DecoderKind::Spi => {
            let mut modes = row()
                .key(format!("wave-dec-{id}-mode"))
                .role("radiogroup")
                .label("SPI mode")
                .h(20.0)
                .border(1.0, t.border);
            for m in 0..4u8 {
                let on = d.spi_mode == m;
                modes = modes.child(
                    el().key(format!("wave-dec-{id}-mode-{m}"))
                        .role("radio")
                        .label(format!("Mode {m}"))
                        .focusable(true)
                        .focus_selected(on)
                        .h_full()
                        .px(6.0)
                        .center()
                        .fill(if on { t.selection } else { t.panel_bg })
                        .hover(t.selection)
                        .on_press(move |s: &mut AppState| edit(s, id, |d| d.spi_mode = m))
                        .child(text(m.to_string()).font(VivadoTheme::FONT_UI).color(t.text)),
                );
            }
            let bits = d.spi_bits;
            card = card
                .child(opt_row(&format!("wave-dec-{id}-opt-mode"), "Mode", t).child(modes))
                .child(
                    opt_row(&format!("wave-dec-{id}-opt-bits"), "Bits", t)
                        .child(small_btn(
                            format!("wave-dec-{id}-bits-dec"),
                            "−",
                            t,
                            (bits <= 1).then_some("At least 1 bit"),
                            move |s| {
                                edit(s, id, |d| d.spi_bits = d.spi_bits.saturating_sub(1).max(1))
                            },
                        ))
                        .child(
                            el().w(28.0).center().child(
                                text(bits.to_string())
                                    .font(VivadoTheme::FONT_MONO)
                                    .color(t.text),
                            ),
                        )
                        .child(small_btn(
                            format!("wave-dec-{id}-bits-inc"),
                            "+",
                            t,
                            (bits >= 32).then_some("At most 32 bits"),
                            move |s| edit(s, id, |d| d.spi_bits = (d.spi_bits + 1).min(32)),
                        )),
                );
        }
        DecoderKind::Uart => {
            let auto = d.inputs[0]
                .as_deref()
                .and_then(|n| s.wave.trace_named(n))
                .and_then(|t| estimate_bit_period(&t.samples));
            let shown = match (d.uart_period, auto) {
                (Some(p), _) => format!("{p} samples"),
                (None, Some(p)) => format!("auto · {p}"),
                (None, None) => "auto".into(),
            };
            let cur = d.uart_period.or(auto);
            card = card.child(
                opt_row(&format!("wave-dec-{id}-opt-period"), "Bit period", t)
                    .child(small_btn(
                        format!("wave-dec-{id}-period-dec"),
                        "−",
                        t,
                        cur.is_none_or(|p| p <= 1)
                            .then_some("No shorter bit period"),
                        move |s| {
                            edit(s, id, move |d| {
                                d.uart_period = cur.map(|p| p.saturating_sub(1).max(1))
                            })
                        },
                    ))
                    .child(
                        el().key(format!("wave-dec-{id}-period"))
                            .w(72.0)
                            .center()
                            .child(
                                text(shown)
                                    .font(VivadoTheme::FONT_MONO)
                                    .color(t.text)
                                    .truncate(),
                            ),
                    )
                    .child(small_btn(
                        format!("wave-dec-{id}-period-inc"),
                        "+",
                        t,
                        cur.is_none().then_some("Map LINE first"),
                        move |s| edit(s, id, move |d| d.uart_period = cur.map(|p| p + 1)),
                    ))
                    .child(small_btn(
                        format!("wave-dec-{id}-period-auto"),
                        "Auto",
                        t,
                        d.uart_period.is_none().then_some("Already auto-baud"),
                        move |s| edit(s, id, |d| d.uart_period = None),
                    )),
            );
        }
        DecoderKind::Fsm => {
            let names = if d.fsm_names.is_empty() {
                "none found — counter wording".to_string()
            } else {
                d.fsm_names
                    .iter()
                    .map(|(_, n)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            card = card.child(
                opt_row(&format!("wave-dec-{id}-opt-names"), "Names", t).child(
                    el().grow(1.0).min_w(0.0).child(
                        text(names.clone())
                            .key(format!("wave-dec-{id}-names"))
                            .font(crate::views::cad::FONT_PIN)
                            .color(t.text)
                            .truncate()
                            .help_when_truncated(names),
                    ),
                ),
            );
        }
        DecoderKind::AxiLite => {}
    }
    card
}

/// Decoder panel (left column): add buttons plus one card per decoder.
pub fn decoder_panel(s: &AppState, t: &VivadoTheme) -> El {
    let no_live = (!s.wave.has_live()).then_some("Run a simulation first");
    let mut adds = row().key("wave-dec-add").w_full().gap(4.0).items_center();
    for kind in DecoderKind::ALL {
        adds = adds.child(small_btn(
            format!("wave-dec-add-{}", kind.slug()),
            &format!("+ {}", kind.label()),
            t,
            no_live,
            move |s: &mut AppState| {
                add_decoder(s, kind);
            },
        ));
    }
    let body = col()
        .key("wave-decoders")
        .w_full()
        .grow(1.6)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(
            row()
                .w_full()
                .h(20.0)
                .px(6.0)
                .items_center()
                .fill(t.header_bg)
                .child(
                    text("Protocol Decoders")
                        .font(VivadoTheme::FONT_TITLE)
                        .color(t.text),
                )
                .child(el().grow(1.0))
                .child(
                    el().key("wave-dec-close")
                        .role("button")
                        .label("Close decoders")
                        .focusable(true)
                        .size(18.0, 18.0)
                        .center()
                        .hover(t.selection)
                        .on_press(|s: &mut AppState| s.wave.decode.panel_open = false)
                        .child(icon(&glyphs::CLOSE, 10.0).color(t.text_dim).a11y_hidden()),
                ),
        );
    let mut list = col().w_full().gap(6.0).padding(6.0).child(adds);
    if s.wave.decode.decoders.is_empty() {
        list = list.child(
            text("Add a decoder; roles map from signal names and can be changed per role.")
                .key("wave-dec-empty")
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .wrap(),
        );
    }
    for (k, d) in s.wave.decode.decoders.iter().enumerate() {
        list = list.child(decoder_card(s, k, d, t));
    }
    body.child(
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(list),
    )
}

/// Push the open role picker popover (overlay layer from the root view).
pub fn push_wave_pops(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    let Some((id, k)) = s.wave.decode.pick else {
        return;
    };
    if s.center != crate::state::CenterPane::Wave || !s.wave.decode.panel_open {
        return;
    }
    let Some(d) = s.wave.decode.decoders.iter().find(|d| d.id == id) else {
        return;
    };
    let Some(role) = d.kind.roles().get(k).copied() else {
        return;
    };
    let Some(live) = s.wave.live.as_ref() else {
        return;
    };
    let kit = t.kit();
    let current = d.inputs[k].clone();
    let mut opts = col().w_full().gap(0.0);
    let mut choices: Vec<Option<String>> = vec![None];
    choices.extend(
        live.traces
            .iter()
            .filter(|tr| fits(&role, tr))
            .map(|tr| Some(tr.name.clone())),
    );
    for (j, choice) in choices.into_iter().enumerate() {
        let label = choice.clone().unwrap_or_else(|| "— none —".into());
        let on = choice == current;
        let pick = choice.clone();
        opts = opts.child(
            el().key(format!("wave-dec-pick-{j}"))
                .role("button")
                .label(format!("Choose {label}"))
                .focusable(true)
                .w_full()
                .h(22.0)
                .px(8.0)
                .center_y()
                .fill(if on { t.selection } else { t.panel_bg })
                .hover(t.selection)
                .on_press(move |s: &mut AppState| {
                    let hdl = hdl_text(s);
                    s.wave.assign_role(id, k, pick.clone(), &hdl);
                    s.wave.decode.pick = None;
                    let sig = pick.clone().unwrap_or_else(|| "{}".into());
                    echo(
                        s,
                        format!(
                            "set_wave_decoder_role {id} {} {sig}",
                            role.label.to_lowercase()
                        ),
                    );
                })
                .child(
                    text(label.clone())
                        .font(VivadoTheme::FONT_MONO)
                        .color(t.text)
                        .truncate()
                        .help_when_truncated(label),
                ),
        );
    }
    let anchor = format!("wave-dec-{id}-role-{k}");
    let content = col()
        .key("wave-dec-pick-content")
        .role("dialog")
        .label(format!("{} signal", role.label))
        .focus_return(&anchor)
        .min_w(200.0)
        .max_w(320.0)
        .child(
            col()
                .key("body")
                .padding(4.0)
                .shrink()
                .min_h(0.0)
                .child(rune_kit::popover::fit_scroll(opts)),
        );
    layer.push(rune_kit::scrim(
        "wave-dec-pick-scrim",
        |s: &mut AppState| s.wave.decode.pick = None,
    ));
    layer.push(rune_kit::popover::anchored(
        &kit,
        rune_kit::popover::raised(&kit, content, kit.radius.md),
        &anchor,
        rune_kit::Placement::BottomStart,
        4.0,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::wave_demo::{PROTOCOL_FIXTURE_HDL, seed_protocol_fixture};
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn fixture() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Wave;
        seed_protocol_fixture(&mut s);
        s
    }

    fn labels(w: &WaveState, d: &Decoder) -> Vec<Vec<String>> {
        w.run_decoder(d)
            .unwrap()
            .iter()
            .map(|l| l.spans.iter().map(|s| s.label.clone()).collect())
            .collect()
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

    #[test]
    fn roles_auto_assign_and_every_protocol_decodes_the_fixture() {
        let mut w = fixture().wave;
        for k in DecoderKind::ALL {
            w.add_decoder(k, PROTOCOL_FIXTURE_HDL);
        }
        let d = w.decode.decoders.clone();
        assert_eq!(d[0].inputs[1].as_deref(), Some("bus_tb.s_axi_awvalid"));
        assert_eq!(d[0].inputs[0], None, "no aclk alias in the fixture");
        assert_eq!(d[1].inputs[3].as_deref(), Some("bus_tb.cs_n"));
        assert_eq!(
            labels(&w, &d[0]),
            vec![
                vec!["WR 0x0010 ← 0x0000BEEF OKAY".to_string()],
                vec!["RD 0x0014 → 0x00001234".to_string()]
            ]
        );
        assert_eq!(
            labels(&w, &d[1]),
            vec![vec!["0xA5", "0x3C"], vec!["0x5A", "0xC3"]]
        );
        assert_eq!(labels(&w, &d[2]), vec![vec!["0x4F 'O'", "0x4B 'K'"]]);
        assert_eq!(
            labels(&w, &d[3]),
            vec![vec!["IDLE", "LOAD", "SHIFT", "DONE", "IDLE"]]
        );
        // Decoded lanes follow the trace lanes on the sheet.
        let rows = live_rows(&w);
        let decoded = rows
            .iter()
            .filter(|(r, _, _)| matches!(r, LiveRow::Decoded { .. }))
            .count();
        assert_eq!(decoded, 6);
    }

    #[test]
    fn missing_roles_name_what_to_map_and_fsm_without_names_uses_counter_wording() {
        let mut w = fixture().wave;
        let id = w.add_decoder(DecoderKind::Spi, "");
        w.assign_role(id, 0, None, "");
        let d = w.decode.decoders[0].clone();
        assert_eq!(w.run_decoder(&d), Err("Map SCLK to decode".into()));
        let id = w.add_decoder(DecoderKind::Fsm, "");
        let d = w
            .decode
            .decoders
            .iter()
            .find(|d| d.id == id)
            .unwrap()
            .clone();
        assert!(d.fsm_names.is_empty());
        assert_eq!(labels(&w, &d)[0][1], "INC 0x1");
    }

    #[test]
    fn panel_adds_decoders_and_the_role_picker_remaps() {
        let mut s = fixture();
        s.wave.decode.panel_open = true;
        let mut r = rt(s);
        assert!(r.find("wave-dec-empty").is_some());
        tap(&mut r, "wave-dec-add-uart");
        assert_eq!(r.state.wave.decode.decoders.len(), 1);
        assert!(r.find("wave-dec-lane-0-uart").is_some());
        assert!(
            r.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "add_wave_decoder uart")
        );
        // Re-map LINE to `mosi` through the popover.
        tap(&mut r, "wave-dec-0-role-0");
        assert!(r.find("wave-dec-pick-content").is_some());
        let mosi = r
            .state
            .wave
            .live
            .as_ref()
            .unwrap()
            .traces
            .iter()
            .filter(|t| t.width == 1)
            .position(|t| t.leaf() == "mosi")
            .unwrap();
        tap(&mut r, &format!("wave-dec-pick-{}", mosi + 1));
        assert_eq!(r.state.wave.decode.pick, None);
        assert_eq!(
            r.state.wave.decode.decoders[0].inputs[0].as_deref(),
            Some("bus_tb.mosi")
        );
        tap(&mut r, "wave-dec-0-remove");
        assert!(r.state.wave.decode.decoders.is_empty());
        assert!(r.find("wave-dec-lane-0-uart").is_none());
    }

    #[test]
    fn spi_mode_and_uart_period_controls_change_the_decode() {
        let mut s = fixture();
        s.wave.add_decoder(DecoderKind::Spi, "");
        let mut r = rt(s);
        tap(&mut r, "wave-dec-0-mode-1");
        assert_eq!(r.state.wave.decode.decoders[0].spi_mode, 1);
        let d = r.state.wave.decode.decoders[0].clone();
        assert_ne!(labels(&r.state.wave, &d)[0], vec!["0xA5", "0x3C"]);
        tap(&mut r, "wave-dec-0-remove");
        tap(&mut r, "wave-dec-add-uart");
        tap(&mut r, "wave-dec-1-period-inc");
        assert_eq!(r.state.wave.decode.decoders[0].uart_period, Some(7));
        tap(&mut r, "wave-dec-1-period-auto");
        assert_eq!(r.state.wave.decode.decoders[0].uart_period, None);
    }
}
