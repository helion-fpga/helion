//! JTAG Chain card on the Hardware Manager target node: on the Native FTDI
//! cable it scans the chain, picks TCK and lists raw scans; on the other
//! cables it shows the TAP the cable reaches. Model: [`crate::views::hw_native`].

use helion_hw::CableKind;
use rune::{El, col, el, row};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::hw::CableChoice;
use crate::views::hw_debug_view::{segment, segments};
use crate::views::hw_native::{self as nat, TCK_CHOICES};

const LABEL_W: f32 = 96.0;

const CHAIN_COLS: [ColSpec; 5] = [
    ColSpec::num("#", 24.0),
    ColSpec::fixed("IDCODE", 92.0),
    ColSpec::num("IR", 28.0),
    ColSpec::grow("Device", 1.0),
    ColSpec::fixed("Role", 64.0),
];

const SCAN_COLS: [ColSpec; 4] = [
    ColSpec::fixed("Reg", 28.0),
    ColSpec::num("Bits", 40.0),
    ColSpec::grow("TDI", 1.0),
    ColSpec::grow("TDO", 1.0),
];

fn act(s: &mut AppState, r: Result<String, String>) {
    if let Err(e) = r {
        s.console.lines.push(crate::state::ConsoleLine {
            text: format!("ERROR: {e}"),
            sev: crate::state::MsgSev::Error,
        });
        s.program.native.last_error = Some(e);
    }
}

fn tck_label(hz: u32) -> String {
    if hz >= 1_000_000 {
        format!("{} MHz", hz / 1_000_000)
    } else {
        format!("{} kHz", hz / 1_000)
    }
}

/// Badge + one line saying how far the cable's path is verified.
pub fn verification_line(kind: CableKind, key: &'static str, t: &VivadoTheme) -> El {
    let (ink, note) = match kind {
        CableKind::Sim => (t.text_dim, "In-process Helion TAP — the verified path."),
        CableKind::NativeMock => (
            t.text_dim,
            "MPSSE engine driving a simulated FTDI + Helion TAP (fixture board).",
        ),
        CableKind::NativeUsb => (t.warn, nat::NATIVE_NOTE),
    };
    row()
        .key(key)
        .w_full()
        .h(cad::ROW_H)
        .items_center()
        .gap(8.0)
        .child(cad::badge(kind.label(), ink, t))
        .child(
            el().grow(1.0)
                .min_w(0.0)
                .child(cad::cut(note, VivadoTheme::FONT_UI, t.text_dim)),
        )
}

fn chain_table(s: &AppState, t: &VivadoTheme) -> El {
    let Some(c) = &s.program.native.chain else {
        return cad::empty_state(
            "hw-jtag-empty",
            "Not scanned. Scan Chain lists the devices on the cable.",
            None,
            t,
        );
    };
    let mut rows = col().w_full();
    for (i, d) in c.devices.iter().enumerate() {
        let target = i == c.target;
        let id = d
            .idcode
            .map(|v| format!("{v:#010x}"))
            .unwrap_or_else(|| "BYPASS".into());
        rows = rows.child(
            cad::table_row(format!("hw-jtag-dev-{i}"), d.name.clone(), target, t)
                .child(cad::cell(&CHAIN_COLS[0], cad::mono(i.to_string(), t)))
                .child(cad::cell(&CHAIN_COLS[1], cad::mono(id, t)))
                .child(cad::cell(
                    &CHAIN_COLS[2],
                    cad::mono(d.ir_len.to_string(), t),
                ))
                .child(cad::cell(
                    &CHAIN_COLS[3],
                    cad::cut(d.name.clone(), VivadoTheme::FONT_UI, t.text),
                ))
                .child(cad::cell(
                    &CHAIN_COLS[4],
                    cad::dim(if target { "target" } else { "bypass" }, t),
                )),
        );
    }
    col()
        .w_full()
        .gap(6.0)
        .child(cad::kv(
            "Port",
            cad::cut(c.port.clone(), cad::mono_style(), t.text).key("hw-jtag-port"),
            LABEL_W,
            t,
        ))
        .child(cad::kv(
            "TCK",
            cad::mono(
                format!(
                    "{:.3} MHz · divisor {} · TDO nearest device first",
                    f64::from(c.tck_hz) / 1e6,
                    c.divisor
                ),
                t,
            )
            .key("hw-jtag-tck-real"),
            LABEL_W,
            t,
        ))
        .child(
            cad::table_frame("hw-jtag-chain", t)
                .child(cad::header("hw-jtag-chain-head", &CHAIN_COLS, t))
                .child(rows),
        )
}

fn scan_log(s: &AppState, t: &VivadoTheme) -> Option<El> {
    let log = &s.program.native.scans;
    if log.is_empty() {
        return None;
    }
    let mut rows = col().w_full();
    for (i, r) in log.iter().take(8).enumerate() {
        let (tdo, ink) = match &r.tdo {
            Ok(v) => (v.clone(), t.text),
            Err(e) => (e.clone(), t.fail),
        };
        let reg = if r.ir { "IR" } else { "DR" };
        rows = rows.child(
            cad::table_row(
                format!("hw-jtag-scan-{i}"),
                format!("{reg} {}", r.bits),
                false,
                t,
            )
            .child(cad::cell(&SCAN_COLS[0], cad::mono(reg, t)))
            .child(cad::cell(&SCAN_COLS[1], cad::mono(r.bits.to_string(), t)))
            .child(cad::cell(
                &SCAN_COLS[2],
                cad::cut(r.tdi.clone(), cad::mono_style(), t.text),
            ))
            .child(cad::cell(
                &SCAN_COLS[3],
                cad::cut(tdo, cad::mono_style(), ink),
            )),
        );
    }
    Some(
        cad::table_frame("hw-jtag-scans", t)
            .child(cad::header("hw-jtag-scans-head", &SCAN_COLS, t))
            .child(rows),
    )
}

/// The JTAG Chain card for the target node.
pub fn chain_card(s: &AppState, t: &VivadoTheme) -> El {
    if s.program.cable != CableChoice::Native {
        let part = crate::views::program::program_part(s);
        let chain = match &s.program.stat {
            Some(st) => format!("{} · IDCODE {:#010x} · IR 6 bits", st.part, st.idcode),
            None => format!("{part} · IDCODE not read (Refresh STAT)"),
        };
        let mut body = col()
            .w_full()
            .gap(4.0)
            .child(cad::cut(chain, cad::mono_style(), t.text).key("program-chain-device"));
        if s.program.cable == CableChoice::Sim {
            body = body.child(verification_line(CableKind::Sim, "hw-jtag-verify", t));
        } else {
            body = body.child(
                cad::cut(
                    "openFPGALoader owns this cable: it programs and reads SPI flash but cannot \
                     scan the chain or shift data registers. Select Native FTDI for JTAG access.",
                    VivadoTheme::FONT_UI,
                    t.text_dim,
                )
                .key("hw-jtag-ofl"),
            );
        }
        return cad::card("program-chain", "JTAG Chain", body, t);
    }
    let n = &s.program.native;
    let why = nat::scan_block(s);
    let busy = n.busy();
    let mut tck = Vec::new();
    for hz in TCK_CHOICES {
        tck.push(segment(
            format!("hw-jtag-tck-{hz}"),
            &tck_label(hz),
            n.tck_hz == hz,
            t,
            !busy,
            move |s: &mut AppState| {
                let r = nat::set_tck(s, hz);
                act(s, r)
            },
        ));
    }
    let mut tools = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .child(cad::primary_btn(
            "hw-jtag-scan",
            "Scan Chain",
            t,
            why.as_deref(),
            |s: &mut AppState| {
                let r = nat::scan(s);
                act(s, r)
            },
        ))
        .child(cad::dim("TCK", t))
        .child(segments("hw-jtag-tck", "TCK frequency", tck, t));
    if nat::held_by_debug(s) {
        tools = tools.child(cad::btn(
            "hw-jtag-close",
            "Close Target",
            t,
            s.program
                .debug
                .busy()
                .map(|_| "Busy: a debug job is running"),
            |s: &mut AppState| {
                let r = nat::close_target(s);
                act(s, r)
            },
        ));
    }
    if busy {
        tools = tools.child(cad::dim("Scanning…", t).key("hw-jtag-busy"));
    }
    let mut body = col()
        .w_full()
        .gap(6.0)
        .child(tools)
        .child(verification_line(n.kind(), "hw-jtag-verify", t));
    if let Some(w) = why.filter(|w| !w.starts_with("Busy")) {
        body = body.child(cad::cut(w, VivadoTheme::FONT_UI, t.warn).key("hw-jtag-why"));
    }
    if let Some(e) = &n.last_error {
        body = body.child(cad::cut(e.clone(), VivadoTheme::FONT_UI, t.fail).key("hw-jtag-error"));
    }
    body = body.child(chain_table(s, t));
    if let Some(log) = scan_log(s, t) {
        body = body.child(cad::caps("Raw scans", t)).child(log);
    }
    cad::card("program-chain", "JTAG Chain", body, t)
}
