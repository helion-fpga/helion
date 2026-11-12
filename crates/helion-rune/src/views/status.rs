//! Status bar and toast stack: run verb, part, WNS, message counts, layout.
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/rd-1b93965a*.png` main window): a 24px chrome status bar
//! with the typed [`StatusVerb`], project part, worst negative slack from the
//! typed [`TimingState`], console message counts computed from [`MsgSev`]
//! (never stringly), and a right-aligned "Default Layout" label. Toasts are
//! kit [`Toast`](rune_kit::Toast) cards in a
//! [`ToastRegion`](rune_kit::ToastRegion) overlay pinned 48px above the bar by
//! the kit (no manual `-36` anchor math, no ASCII close glyphs — the kit
//! paints a real close icon).
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/el()/text()`.
//! - `crates/rune-kit/src/toast.rs`: `Toast::new/description/lead/on_close`,
//!   `ToastRegion::new/toast/build` (mount via `rune_kit::root` overlays).

use rune::{El, el, row, text};

use crate::state::{AppState, MsgSev, StatusVerb, ToastKind};
use crate::theme::VivadoTheme;

/// Status bar height (Vivado-density chrome strip).
const BAR_H: f32 = 24.0;

/// Format picoseconds as `2.061 ns` for the WNS readout.
fn format_ns(ps: i64) -> String {
    format!("{:.3} ns", ps as f64 / 1000.0)
}

fn verb_ink(verb: StatusVerb, t: &VivadoTheme) -> rune::Color {
    match verb {
        StatusVerb::Ready => t.text,
        StatusVerb::SynthesisRunning | StatusVerb::ImplementationRunning => t.link,
        StatusVerb::SynthesisComplete | StatusVerb::ImplementationComplete => t.pass,
        StatusVerb::Failed => t.fail,
    }
}

fn toast_ink(kind: ToastKind, t: &VivadoTheme) -> rune::Color {
    match kind {
        ToastKind::Info => t.link,
        ToastKind::Success => t.pass,
        ToastKind::Warning => t.warn,
        ToastKind::Error => t.fail,
    }
}

/// Diagnostics summary from typed severities. Info lines are the Tcl
/// journal, not diagnostics, so the bar counts what needs attention:
/// `No issues`, or `1 Error · 2 Warnings` (critical warnings called out).
fn message_counts(s: &AppState) -> String {
    let (mut warn, mut crit, mut err) = (0, 0, 0);
    for line in &s.console.lines {
        match line.sev {
            MsgSev::Info => {}
            MsgSev::Warn => warn += 1,
            MsgSev::Critical => crit += 1,
            MsgSev::Error => err += 1,
        }
    }
    let plural = |n: usize, one: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {one}s")
        }
    };
    let mut parts = Vec::new();
    if err > 0 {
        parts.push(plural(err, "Error"));
    }
    if crit > 0 {
        parts.push(plural(crit, "Critical Warning"));
    }
    if warn > 0 {
        parts.push(plural(warn, "Warning"));
    }
    if parts.is_empty() {
        "No issues".into()
    } else {
        parts.join(" · ")
    }
}

/// Worst severity present, for the messages segment ink.
fn worst(s: &AppState) -> Option<MsgSev> {
    let mut worst = None;
    for line in &s.console.lines {
        worst = match (worst, line.sev) {
            (_, MsgSev::Error) | (Some(MsgSev::Error), _) => Some(MsgSev::Error),
            (_, MsgSev::Critical) | (Some(MsgSev::Critical), _) => Some(MsgSev::Critical),
            (_, MsgSev::Warn) | (Some(MsgSev::Warn), _) => Some(MsgSev::Warn),
            (w, MsgSev::Info) => w,
        };
    }
    worst
}

/// A clickable status segment: hover wash, tooltip, one action.
fn segment(
    key: &'static str,
    content: String,
    ink: rune::Color,
    help: &'static str,
    t: &VivadoTheme,
    on: impl Fn(&mut AppState) + 'static,
) -> El {
    row()
        .key(key)
        .role("button")
        .label(content.clone())
        .help(help)
        .focusable(true)
        .h(BAR_H - 4.0)
        .px(6.0)
        .radius(3.0)
        .items_center()
        .hover(t.selection)
        .on_press(on)
        .child(text(content).font(VivadoTheme::FONT_UI).color(ink))
}

/// The 24px chrome status bar: run verb, then clickable part (Device), WNS
/// (Timing) and diagnostics (Messages) segments; the layout segment on the
/// right opens the Layout menu.
pub fn status_bar(s: &AppState, t: &VivadoTheme) -> El {
    use crate::state::CenterPane;
    let wns = match s.timing.wns_ps {
        Some(ps) => format!("WNS {}", format_ns(ps)),
        None => "WNS --".into(),
    };
    let wns_ink = match s.timing.wns_ps {
        Some(ps) if ps < 0 => t.fail,
        Some(_) => t.pass,
        None => t.text_dim,
    };
    let msg_ink = match worst(s) {
        Some(MsgSev::Error) | Some(MsgSev::Critical) => t.fail,
        Some(MsgSev::Warn) => t.warn,
        _ => t.text_dim,
    };
    let mut verb = row().key("status-verb").gap(4.0).items_center().pr(6.0).child(
        text(s.status.label())
            .font(VivadoTheme::FONT_UI)
            .color(verb_ink(s.status, t)),
    );
    match s.status {
        StatusVerb::SynthesisComplete | StatusVerb::ImplementationComplete => {
            verb = verb.child(text("✓").font(VivadoTheme::FONT_UI).color(t.pass));
        }
        StatusVerb::Failed => {
            verb = verb.child(text("✗").font(VivadoTheme::FONT_UI).color(t.fail));
        }
        StatusVerb::Ready | StatusVerb::SynthesisRunning | StatusVerb::ImplementationRunning => {}
    }
    let sep = || el().w(1.0).h(12.0).fill(t.border).a11y_hidden();
    let mut left = row().gap(4.0).items_center().child(verb);
    if !s.project.part.is_empty() {
        left = left.child(sep()).child(segment(
            "status-part",
            s.project.part.clone(),
            t.text_dim,
            "Target part — open the Device view",
            t,
            |s: &mut AppState| s.navigate(CenterPane::Device),
        ));
    }
    left = left
        .child(sep())
        .child(segment(
            "status-wns",
            wns,
            wns_ink,
            "Worst negative slack — open Timing",
            t,
            |s: &mut AppState| s.navigate(CenterPane::Timing),
        ))
        .child(sep())
        .child(segment(
            "status-msgs",
            message_counts(s),
            msg_ink,
            "Show Messages",
            t,
            |s: &mut AppState| {
                crate::layout::set_console_visible(s, true);
                s.console.tab = crate::views::console::ConsoleTab::Messages;
            },
        ));
    row()
        .key("statusbar")
        .w_full()
        .h(BAR_H)
        .px(6.0)
        .items_center()
        .justify_between()
        .fill(t.chrome_bg)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.border)
        .child(left)
        .child(segment(
            "status-layout",
            s.layout.label(),
            t.text_dim,
            "Layout presets",
            t,
            |s: &mut AppState| s.chrome.open_menu = Some(crate::views::chrome::MenuId::Layout),
        ))
}

/// Push the toast stack overlay when there are notifications to show.
pub fn push_toasts(layer: &mut Vec<El>, s: &AppState, t: &VivadoTheme) {
    if s.toasts.items.is_empty() {
        return;
    }
    let kit = t.kit();
    let mut region = rune_kit::ToastRegion::new();
    for item in &s.toasts.items {
        let id = item.id;
        let dot = el()
            .size(10.0, 10.0)
            .radius(5.0)
            .fill(toast_ink(item.kind, t));
        region = region.toast(
            rune_kit::Toast::new(format!("toast-{id}"), item.title.clone())
                .description(item.detail.clone())
                .lead(dot)
                .on_close(move |st: &mut AppState| st.toasts.dismiss(id))
                .build(&kit),
        );
    }
    layer.push(region.build(&kit));
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

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn status_tree_exposes_verb_part_wns_counts_layout() {
        let mut rt = harness_with(init());
        rt.frame(0.0);
        for key in [
            "statusbar",
            "status-verb",
            "status-part",
            "status-wns",
            "status-msgs",
            "status-layout",
        ] {
            assert!(rt.find(key).is_some(), "missing keyed region: {key}");
        }
    }

    #[test]
    fn toast_region_mounts_and_close_dismisses_by_id() {
        let mut s = init();
        s.toasts.push(
            crate::state::ToastKind::Success,
            "Synthesis complete",
            "synth_1 finished with 0 Errors",
        );
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("toast-region").is_some(), "toast region must mount");
        assert!(rt.find("toast-0").is_some(), "toast card must mount");
        // Tap the kit close button; the handler dismisses by id and unmounts.
        tap(&mut rt, "close");
        assert!(
            rt.state.toasts.items.is_empty(),
            "close must dismiss the toast"
        );
        assert!(rt.find("toast-0").is_none(), "dismissed toast must unmount");
    }

    #[test]
    fn message_counts_come_from_typed_severities() {
        let s = init();
        assert_eq!(
            message_counts(&s),
            "No issues",
            "info lines are the Tcl journal, not diagnostics"
        );
        let mut s = init();
        for sev in [MsgSev::Warn, MsgSev::Warn, MsgSev::Error] {
            s.console.lines.push(crate::state::ConsoleLine {
                text: "x".into(),
                sev,
            });
        }
        assert_eq!(message_counts(&s), "1 Error · 2 Warnings");
        assert_eq!(worst(&s), Some(MsgSev::Error));
    }

    #[test]
    fn wns_readout_formats_picoseconds() {
        assert_eq!(format_ns(2061), "2.061 ns");
        assert_eq!(format_ns(-1545), "-1.545 ns");
    }
}
