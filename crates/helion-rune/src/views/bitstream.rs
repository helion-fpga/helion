//! Bitstream surface: Generate/Report Bitstream, KPI cards, and the
//! FAR-addressed configuration frame table.
//!
//! Ports the old IDE's Bitstream workspace (`helion-ide.rs:8184
//! paint_bitstream`, model `ide.rs:7578 bitstream_report` /
//! `select_bitstream_frame`) onto the real artifact the implementation run
//! keeps in [`crate::state::BitstreamState`]: the `helion_bits::Bitstream`
//! frames + packet stream, never counts alone. Generate Bitstream launches
//! the real `impl_1` run through `write_bitstream`
//! ([`crate::runs::launch_verb`]); the FAR table is virtualized
//! (`El::virtual_rows`) so large frame maps stay cheap.
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el/virtual_rows.rs`: `virtual_rows` + `GrowthSpec`.
//! - `crates/rune/src/el/scroll.rs`: `.scroll(ScrollSpec)` viewport.

use std::sync::OnceLock;

use rune::{El, GrowthSpec, ScrollSpec, col, el, row};

use crate::runs::RunKind;
use crate::state::{AppState, BitstreamState, CenterPane, FarRow};
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::sources::pane_header;

/// FAR table columns (old `bitstream_far_table`).
const COLS: [ColSpec; 7] = [
    ColSpec::fixed("FAR", 104.0),
    ColSpec::fixed("Block", 86.0),
    ColSpec::num("Die", 28.0),
    ColSpec::num("Major", 44.0),
    ColSpec::num("Minor", 40.0),
    ColSpec::num("Ones", 36.0),
    ColSpec::grow("Frame Word", 1.0),
];

/// The Tcl verb Generate Bitstream runs (the impl worker ends with the real
/// `write_bitstream` stage).
pub const GENERATE_VERB: &str = "launch_runs impl_1 -to_step write_bitstream";

/// Block ink for the FAR swatch (old `bitstream_block_color`).
pub fn block_color(block: &str, t: &VivadoTheme) -> rune::Color {
    match block {
        "CLB_IO_CLK" => t.pass,
        "DSP" => t.wave_marker,
        "BRAM" => t.warn,
        "IOB" => t.link,
        _ => t.text_dim,
    }
}

/// Why Generate Bitstream is unavailable (`None` = enabled).
pub fn generate_block_reason(s: &AppState) -> Option<String> {
    s.runs
        .active_kind()
        .map(|k| format!("{} is running; wait for it to finish.", k.run_name()))
}

/// Generate Bitstream: the real implementation run through write_bitstream.
pub fn generate(s: &mut AppState) {
    if generate_block_reason(s).is_some() {
        return;
    }
    crate::runs::launch_verb(s, RunKind::Impl, GENERATE_VERB);
}

/// Adopt the Tcl session's bitstream (`write_bitstream` verb) when no run
/// produced one, so the console flow fills the same surface.
pub fn adopt_session_bitstream(s: &mut AppState) {
    if let Some(b) = s.tcl.session.bitstream.as_ref() {
        let hash = s.tcl.session.blinky_hash().unwrap_or(0);
        let part = s.project.part.clone();
        s.bitstream = BitstreamState::from_bits(b, hash, &part, "Tcl session");
    }
}

/// `report_bitstream` text (old `BitstreamReport::text`).
pub fn report_text(b: &BitstreamState) -> String {
    if !b.has_run {
        return "no bitstream - run Generate Bitstream".into();
    }
    let mut s = format!(
        "bitstream idcode={:#010x} hash={:#010x} bytes={} frames={} configured={}",
        b.idcode, b.hash, b.bytes, b.frames, b.configured
    );
    for (i, r) in b.rows.iter().enumerate() {
        s.push_str(&format!(
            "\n{i} FAR={} BLOCK={} DIE={} MAJOR={} MINOR={} ONES={} WORD={}",
            r.far_hex(),
            r.block_name(),
            r.die,
            r.major,
            r.minor,
            r.ones(),
            r.word_hex()
        ));
    }
    s
}

/// Tcl `report_bitstream`: open the surface and dump the FAR table.
pub fn report_bitstream(s: &mut AppState) -> Result<String, String> {
    s.navigate(CenterPane::Bitstream);
    if !s.bitstream.has_run {
        adopt_session_bitstream(s);
    }
    if !s.bitstream.has_run {
        return Err(format!(
            "report_bitstream: no bitstream (run Generate Bitstream: {GENERATE_VERB})"
        ));
    }
    Ok(report_text(&s.bitstream))
}

/// Resolve a frame spec: row index, FAR hex, block name, or
/// `block major minor` (old `BitstreamReport::frame`).
pub fn find_frame<'a>(b: &'a BitstreamState, spec: &str) -> Option<&'a FarRow> {
    let spec = spec.trim();
    if spec.is_empty() {
        return b.rows.first();
    }
    let hex = spec.strip_prefix("0x").or_else(|| spec.strip_prefix("0X"));
    if let Some(h) = hex {
        let far = u32::from_str_radix(h, 16).ok()?;
        return b.rows.iter().find(|r| r.far == far);
    }
    if let Ok(i) = spec.parse::<usize>() {
        return b.rows.get(i);
    }
    let up = spec.to_ascii_uppercase();
    if let Some(r) = b.rows.iter().find(|r| r.block_name() == up) {
        return Some(r);
    }
    let mut p = spec.split_whitespace();
    if let (Some(bk), Some(maj), Some(min)) = (p.next(), p.next(), p.next()) {
        let block = match bk.to_ascii_uppercase().as_str() {
            "CLB_IO_CLK" | "CLB" => helion_device::Far::CLB_IO_CLK,
            "DSP" => helion_device::Far::DSP,
            "BRAM" => helion_device::Far::BRAM,
            "IOB" => helion_device::Far::IOB,
            other => other.parse().ok()?,
        };
        let major: u16 = maj.parse().ok()?;
        let minor: u8 = min.parse().ok()?;
        return b
            .rows
            .iter()
            .find(|r| r.block == block && r.major == major && r.minor == minor);
    }
    None
}

fn frame_text(r: &FarRow) -> String {
    format!(
        "bitstream FAR={} BLOCK={} DIE={} MAJOR={} MINOR={} ONES={} WORD={}",
        r.far_hex(),
        r.block_name(),
        r.die,
        r.major,
        r.minor,
        r.ones(),
        r.word_hex()
    )
}

/// Tcl `select_bitstream_frame` / row click.
pub fn select_frame(s: &mut AppState, spec: &str) -> Result<String, String> {
    if s.bitstream.rows.is_empty() {
        return Err("select_bitstream_frame: no bitstream".into());
    }
    let row = *find_frame(&s.bitstream, spec)
        .ok_or_else(|| format!("select_bitstream_frame: no FAR {spec}"))?;
    s.bitstream.selected = Some(row.far);
    s.navigate(CenterPane::Bitstream);
    Ok(frame_text(&row))
}

/// 128-bit word as four 32-bit groups (`0x00000000 00000000 ...`).
pub fn word_groups(word: u128) -> String {
    let raw = format!("{word:032x}");
    let parts: Vec<&str> = (0..4).map(|i| &raw[i * 8..i * 8 + 8]).collect();
    format!("0x{}", parts.join(" "))
}

fn kpis(b: &BitstreamState, t: &VivadoTheme) -> El {
    let configured = b.configured > 0;
    row()
        .key("bitstream-kpis")
        .w_full()
        .gap(8.0)
        .child(cad::kpi(
            "bitstream-kpi-status",
            "Configuration Status",
            if configured {
                "Configured"
            } else {
                "Unconfigured"
            },
            if configured { t.pass } else { t.warn },
            format!(
                "{} of {} frames non-zero",
                cad::grouped(b.configured),
                cad::grouped(b.frames)
            ),
            t,
        ))
        .child(cad::kpi(
            "bitstream-kpi-frames",
            "Frames",
            cad::grouped(b.frames),
            t.text,
            "128-bit FAR frames",
            t,
        ))
        .child(cad::kpi(
            "bitstream-kpi-bytes",
            "Payload Size",
            format!("{} B", cad::grouped(b.bytes)),
            t.text,
            format!("hash {:#010x}", b.hash),
            t,
        ))
        .child(cad::kpi(
            "bitstream-kpi-idcode",
            "IDCODE",
            format!("{:#010x}", b.idcode),
            t.text,
            if b.part.is_empty() {
                b.source.clone()
            } else {
                format!("{} · {}", b.part, b.source)
            },
            t,
        ))
}

/// Per-block legend with real frame counts.
fn legend(b: &BitstreamState, t: &VivadoTheme) -> El {
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    for r in &b.rows {
        match counts.iter_mut().find(|(n, _)| *n == r.block_name()) {
            Some((_, c)) => *c += 1,
            None => counts.push((r.block_name(), 1)),
        }
    }
    let mut line = row()
        .key("bitstream-legend")
        .w_full()
        .h(cad::ROW_H)
        .items_center()
        .gap(12.0)
        .child(cad::title("Configuration Frames", t));
    for (name, n) in counts {
        line = line.child(
            row()
                .items_center()
                .gap(4.0)
                .child(swatch(block_color(name, t)))
                .child(cad::dim(format!("{name} {}", cad::grouped(n)), t)),
        );
    }
    line
}

fn swatch(c: rune::Color) -> El {
    el().size(8.0, 8.0).radius(2.0).fill(c).a11y_hidden()
}

fn far_row(s: &AppState, t: &VivadoTheme, i: usize) -> El {
    let Some(r) = s.bitstream.rows.get(i).copied() else {
        return el().h(cad::ROW_H);
    };
    let selected = s.bitstream.selected == Some(r.far);
    let far = r.far;
    cad::table_row(
        format!("bitstream-row-{i}"),
        format!("Frame {} {}", r.far_hex(), r.block_name()),
        selected,
        t,
    )
    .on_press(move |s: &mut AppState| s.bitstream.selected = Some(far))
    .child(cad::cell(
        &COLS[0],
        row()
            .items_center()
            .gap(6.0)
            .child(swatch(block_color(r.block_name(), t)))
            .child(cad::mono(r.far_hex(), t)),
    ))
    .child(cad::cell(&COLS[1], cad::mono(r.block_name(), t).truncate()))
    .child(cad::cell(&COLS[2], cad::mono(r.die.to_string(), t)))
    .child(cad::cell(&COLS[3], cad::mono(r.major.to_string(), t)))
    .child(cad::cell(&COLS[4], cad::mono(r.minor.to_string(), t)))
    .child(cad::cell(&COLS[5], cad::mono(r.ones().to_string(), t)))
    .child(cad::cell(
        &COLS[6],
        cad::cut(r.word_hex(), cad::mono_style(), t.text_dim),
    ))
}

fn far_table(s: &AppState, t: &VivadoTheme) -> El {
    let theme = *t;
    let count = s.bitstream.rows.len();
    let rows = col()
        .key("bitstream-rows")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            cad::ROW_H,
            |_: &AppState, i| format!("bitstream-row-{i}").into(),
            move |s: &AppState, i| far_row(s, &theme, i),
        );
    cad::table_frame("bitstream-table", t)
        .grow(1.0)
        .min_h(0.0)
        .child(cad::header("bitstream-head", &COLS, t))
        .child(
            col()
                .key("bitstream-viewport")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(rows),
        )
}

fn detail(s: &AppState, t: &VivadoTheme) -> El {
    let strip = col()
        .key("bitstream-detail")
        .w_full()
        .gap(2.0)
        .px(10.0)
        .py(6.0)
        .radius(4.0)
        .fill(t.header_bg)
        .border(1.0, t.border_light);
    match s.bitstream.selected_row() {
        None => strip.child(cad::dim("Select a frame to inspect its 128-bit word.", t)),
        Some(r) => strip
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .child(swatch(block_color(r.block_name(), t)))
                    .child(
                        cad::title(format!("Frame {}", r.far_hex()), t).key("bitstream-detail-far"),
                    )
                    .child(
                        cad::dim(
                            format!(
                                "{} · die {} · major {} · minor {} · {} ones",
                                r.block_name(),
                                r.die,
                                r.major,
                                r.minor,
                                r.ones()
                            ),
                            t,
                        )
                        .truncate(),
                    ),
            )
            .child(
                cad::cut(word_groups(r.word), cad::mono_style(), t.text)
                    .key("bitstream-detail-word"),
            ),
    }
}

/// The center Bitstream pane wired into the shell.
pub fn bitstream_pane(s: &AppState, t: &VivadoTheme) -> El {
    let gen_reason = generate_block_reason(s);
    let report_reason = (!s.bitstream.has_run && s.tcl.session.bitstream.is_none())
        .then_some("No bitstream to report yet. Generate Bitstream first.");
    let mut tools = cad::toolbar("bitstream-toolbar", t)
        .child(cad::btn(
            "bitstream-generate",
            "Generate Bitstream",
            t,
            gen_reason.as_deref(),
            generate,
        ))
        .child(cad::btn(
            "bitstream-report",
            "Report Bitstream",
            t,
            report_reason,
            |s: &mut AppState| {
                s.console.tab = crate::views::console::ConsoleTab::Tcl;
                crate::tcl::eval_command(s, "report_bitstream");
            },
        ))
        .child(el().grow(1.0));
    if let Some(k) = s.runs.active_kind() {
        tools =
            tools.child(cad::dim(format!("{} running…", k.run_name()), t).key("bitstream-running"));
    }
    let body =
        if s.bitstream.has_run {
            let b = &s.bitstream;
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(8.0)
                .padding(8.0)
                .child(kpis(b, t))
                .child(legend(b, t))
                .child(far_table(s, t))
                .child(detail(s, t))
        } else {
            let cta = match gen_reason {
                None => Some(cad::primary_btn(
                    "bitstream-empty-generate",
                    "Generate Bitstream",
                    t,
                    None,
                    generate,
                )),
                Some(_) => None,
            };
            col().w_full().grow(1.0).padding(8.0).child(cad::empty_state(
            "bitstream-empty",
            "No bitstream yet. Generate Bitstream runs implementation through write_bitstream.",
            cta,
            t,
        ))
        };
    col()
        .key("bitstream")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .child(pane_header("Bitstream", "bitstream", t))
        .child(tools)
        .child(body)
}

type FixtureBits = Result<(helion_bits::Bitstream, u32), String>;

/// Real bitstream for scenes/tests: synth + place + route + write_bitstream
/// of [`crate::live::FIXTURE_SRC`] on the fixture part (cached per process).
pub fn seed_bitstream_fixture(s: &mut AppState) -> Result<(), String> {
    static FIXTURE: OnceLock<FixtureBits> = OnceLock::new();
    let built = FIXTURE.get_or_init(|| {
        let design = helion_sv::synth_sv(crate::live::FIXTURE_SRC, "fixture.sv")?;
        let dev = helion_device::Device::load_part(crate::live::FIXTURE_PART)?;
        let mut session = helion_proj::Session::new(helion_proj::Mode::Project);
        session.synth_design(design);
        session.place_design(&dev)?;
        session.route_design(&dev)?;
        let bits = session.write_bitstream(&dev)?.clone();
        let hash = session.blinky_hash().unwrap_or(0);
        Ok((bits, hash))
    });
    let (bits, hash) = built.as_ref().map_err(|e| e.clone())?;
    s.bitstream = BitstreamState::from_bits(bits, *hash, crate::live::FIXTURE_PART, "impl_1");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_spec, init};
    use rune::{Env, Runtime};

    fn harness_with(state: AppState) -> Runtime<AppState> {
        let spec = app_spec();
        Runtime::new(
            state,
            crate::view,
            spec.size,
            Env {
                system_dark: false,
                scale: 2.0,
            },
        )
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn seeded() -> AppState {
        let mut s = init();
        s.center = CenterPane::Bitstream;
        seed_bitstream_fixture(&mut s).expect("fixture bitstream");
        s
    }

    #[test]
    fn state_keeps_the_real_frames_and_packets() {
        let s = seeded();
        let b = &s.bitstream;
        let bits = b.bits.as_ref().expect("frames + packets kept");
        assert_eq!(b.frames, bits.frames.len());
        assert_eq!(b.bytes, bits.packets.len());
        assert!(b.configured > 0, "fixture configures frames");
        assert_eq!(b.rows.len(), b.configured);
        assert!(
            b.rows.windows(2).all(
                |w| (w[0].block, w[0].major, w[0].minor) < (w[1].block, w[1].major, w[1].minor)
            )
        );
        let r = b.rows[0];
        let decoded = helion_device::Far::decode(r.far);
        assert_eq!(
            (decoded.block_type, decoded.major, decoded.minor),
            (r.block, r.major, r.minor)
        );
    }

    #[test]
    fn empty_state_offers_generate_and_disables_report() {
        let mut s = init();
        s.center = CenterPane::Bitstream;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("bitstream-empty").is_some());
        assert!(rt.find("bitstream-empty-generate").is_some());
        assert!(rt.find("bitstream-table").is_none());
        tap(&mut rt, "bitstream-report");
        assert!(
            !rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "report_bitstream"),
            "disabled Report must not run"
        );
    }

    #[test]
    fn generate_launches_the_real_impl_run() {
        let mut s = init();
        s.center = CenterPane::Bitstream;
        let mut rt = harness_with(s);
        rt.frame(0.0);
        tap(&mut rt, "bitstream-generate");
        assert_eq!(rt.state.runs.active_kind(), Some(RunKind::Impl));
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == GENERATE_VERB)
        );
        assert!(
            generate_block_reason(&rt.state).is_some(),
            "busy run disables Generate"
        );
        crate::runs::cancel_run(&mut rt.state);
    }

    #[test]
    fn far_table_selects_rows_and_shows_detail() {
        let mut rt = harness_with(seeded());
        rt.frame(0.0);
        for key in [
            "bitstream-kpis",
            "bitstream-kpi-status",
            "bitstream-kpi-idcode",
            "bitstream-legend",
            "bitstream-head",
            "bitstream-row-0",
            "bitstream-detail",
        ] {
            assert!(rt.find(key).is_some(), "missing key: {key}");
        }
        tap(&mut rt, "bitstream-row-0");
        let first = rt.state.bitstream.rows[0];
        assert_eq!(rt.state.bitstream.selected, Some(first.far));
        assert!(rt.find("bitstream-detail-word").is_some());
    }

    #[test]
    fn tcl_report_and_select_frame() {
        let mut s = seeded();
        s.center = CenterPane::Summary;
        crate::tcl::eval_command(&mut s, "report_bitstream");
        assert_eq!(s.center, CenterPane::Bitstream);
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text.starts_with("bitstream idcode="))
        );
        let first = s.bitstream.rows[0];
        crate::tcl::eval_command(
            &mut s,
            &format!("select_bitstream_frame {}", first.far_hex()),
        );
        assert_eq!(s.bitstream.selected, Some(first.far));
        crate::tcl::eval_command(&mut s, "select_bitstream_frame 0xdeadbeef");
        assert!(s.console.lines.last().unwrap().text.contains("no FAR"));
        let last = s.bitstream.rows.len() - 1;
        assert_eq!(
            find_frame(&s.bitstream, &last.to_string()).map(|r| r.far),
            s.bitstream.rows.last().map(|r| r.far)
        );
        assert_eq!(word_groups(0x103), "0x00000000 00000000 00000000 00000103");
    }

    #[test]
    fn report_without_bitstream_errors_honestly() {
        let mut s = init();
        let err = report_bitstream(&mut s).unwrap_err();
        assert!(err.contains("no bitstream"), "{err}");
    }
}
