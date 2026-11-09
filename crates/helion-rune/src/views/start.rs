//! Start page: vendor header plus Quick Start / Tasks / Learning banners.
//!
//! New Rune `El` code written from scratch for the Vivado brief
//! (`/tmp/vivado-ref/rd-96269aa3a*.png` start page): a white vendor header
//! ("HELION" over "FPGA DESIGN SUITE") over three
//! full-width banners — green "Quick Start", teal "Tasks", teal "Learning
//! Center" — each with a large light title and `label >` link rows. Banners
//! reflow: full width, padded content, no fixed-size cards. All colors come
//! from [`VivadoTheme`]; dispatch is typed dialog opens plus Tcl-verb console
//! echoes (no stringly status, no hardcoded hex, no manual geometry).
//!
//! Rune API citations (rev `a7f276c3630f85a06a0230295da4d2bd172d8d3b`):
//! - `crates/rune/src/el.rs`: `row()/col()/el()/text()`.
//! - `crate::theme::VivadoTheme::banner_hover`: translucent hover wash shared
//!   by both themes (banners are constant across modes).

use rune::{Color, El, ScrollSpec, Tx, col, el, row, text};

use crate::state::{AppState, ConsoleLine, MsgSev};
use crate::theme::VivadoTheme;
use crate::views::dialogs::DialogId;

/// Echo a Tcl verb to the console without flipping run state.
fn echo(s: &mut AppState, tcl: &'static str) {
    s.console.lines.push(ConsoleLine {
        text: tcl.into(),
        sev: MsgSev::Info,
    });
}

/// One `label >` banner link row dispatching a Tcl verb.
fn banner_link(key: String, label_text: String, tcl: &'static str, t: &VivadoTheme) -> El {
    el().key(key)
        .role("button")
        .label(label_text.clone())
        .focusable(true)
        .w_full()
        .h(24.0)
        .px(4.0)
        .center_y()
        .hover(t.banner_hover())
        .on_press(move |s: &mut AppState| echo(s, tcl))
        .child(
            row()
                .gap(6.0)
                .items_center()
                .child(
                    text(label_text)
                        .font(VivadoTheme::FONT_BANNER_LINK)
                        .color(t.banner_ink),
                )
                .child(
                    text(">")
                        .font(VivadoTheme::FONT_BANNER_LINK)
                        .color(t.banner_ink),
                ),
        )
}

/// One full-width banner: light display title over `label >` link rows.
fn banner(
    key: &'static str,
    title: &'static str,
    fill: Color,
    links: Vec<El>,
    t: &VivadoTheme,
) -> El {
    col()
        .key(key)
        .w_full()
        .fill(fill)
        .padding(18.0)
        .gap(4.0)
        .child(
            text(title)
                .font(VivadoTheme::FONT_DISPLAY)
                .color(t.banner_ink),
        )
        .child(col().gap(0.0).children(links))
}

/// "Create Project >" opens the New Project wizard instead of echoing Tcl.
fn create_project_link(t: &VivadoTheme) -> El {
    el().key("start-create-project")
        .role("button")
        .label("Create Project")
        .focusable(true)
        .w_full()
        .h(24.0)
        .px(4.0)
        .center_y()
        .hover(t.banner_hover())
        .on_press(|s: &mut AppState| s.dialogs.open(DialogId::NewProject))
        .child(
            row()
                .gap(6.0)
                .items_center()
                .child(
                    text("Create Project")
                        .font(VivadoTheme::FONT_BANNER_LINK)
                        .color(t.banner_ink),
                )
                .child(
                    text(">")
                        .font(VivadoTheme::FONT_BANNER_LINK)
                        .color(t.banner_ink),
                ),
        )
}

/// "Open Project >" raises the native Open dialog instead of echoing Tcl.
fn open_project_link(t: &VivadoTheme) -> El {
    el().key("start-open-project")
        .role("button")
        .label("Open Project")
        .focusable(true)
        .w_full()
        .h(24.0)
        .px(4.0)
        .center_y()
        .hover(t.banner_hover())
        .on_press_tx(|_: &mut AppState, tx: &mut Tx| crate::pickers::open_project(tx))
        .child(
            row()
                .gap(6.0)
                .items_center()
                .child(
                    text("Open Project")
                        .font(VivadoTheme::FONT_BANNER_LINK)
                        .color(t.banner_ink),
                )
                .child(
                    text(">")
                        .font(VivadoTheme::FONT_BANNER_LINK)
                        .color(t.banner_ink),
                ),
        )
}

/// White vendor header: HELION mark over the FPGA DESIGN SUITE subline.
fn vendor_header(t: &VivadoTheme) -> El {
    row()
        .key("start-header")
        .w_full()
        .h(64.0)
        .px(18.0)
        .items_center()
        .justify_start()
        .fill(t.panel_bg)
        .child(
            col()
                .gap(0.0)
                .child(text("HELION").font(VivadoTheme::FONT_DISPLAY).color(t.text))
                .child(
                    text("FPGA DESIGN SUITE")
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_dim),
                ),
        )
}

/// The start page center surface: vendor header over three reflowing banners.
pub fn start_pane(_s: &AppState, t: &VivadoTheme) -> El {
    col()
        .key("start-page")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(t.chrome_bg)
        .gap(10.0)
        .padding(12.0)
        .child(vendor_header(t))
        .child(
            col()
                .key("start-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .gap(10.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(banner(
                    "start-quick",
                    "Quick Start",
                    t.banner_quick,
                    vec![
                        create_project_link(t),
                        open_project_link(t),
                        banner_link(
                            "start-open-example".into(),
                            "Open Example Project".into(),
                            "open_example_project",
                            t,
                        ),
                    ],
                    t,
                ))
                .child(banner(
                    "start-tasks",
                    "Tasks",
                    t.banner_teal,
                    vec![
                        banner_link("start-manage-ip".into(), "Manage IP".into(), "manage_ip", t),
                        banner_link(
                            "start-hw-manager".into(),
                            "Open Hardware Manager".into(),
                            "open_hw_manager",
                            t,
                        ),
                        banner_link(
                            "start-tcl-store".into(),
                            "Helion Tcl Store".into(),
                            "tcl_store",
                            t,
                        ),
                    ],
                    t,
                ))
                .child(banner(
                    "start-learning",
                    "Learning Center",
                    t.banner_teal,
                    vec![
                        banner_link(
                            "start-docs".into(),
                            "Documentation and Tutorials".into(),
                            "show_help",
                            t,
                        ),
                        banner_link(
                            "start-videos".into(),
                            "Quick Take Videos".into(),
                            "show_videos",
                            t,
                        ),
                        banner_link(
                            "start-release-notes".into(),
                            "Release Notes Guide".into(),
                            "show_release_notes",
                            t,
                        ),
                    ],
                    t,
                )),
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

    fn start_state() -> AppState {
        let mut s = init();
        s.center = crate::state::CenterPane::Start;
        s
    }

    fn tap(rt: &mut Runtime<AppState>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("missing key: {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn start_tree_exposes_header_and_all_banners() {
        let mut rt = harness_with(start_state());
        rt.frame(0.0);
        for key in [
            "start-page",
            "start-header",
            "start-quick",
            "start-create-project",
            "start-open-project",
            "start-open-example",
            "start-tasks",
            "start-manage-ip",
            "start-hw-manager",
            "start-tcl-store",
            "start-learning",
            "start-docs",
            "start-videos",
            "start-release-notes",
        ] {
            assert!(rt.find(key).is_some(), "missing keyed region: {key}");
        }
    }

    #[test]
    fn create_project_opens_the_wizard() {
        let mut rt = harness_with(start_state());
        rt.frame(0.0);
        tap(&mut rt, "start-create-project");
        assert_eq!(rt.state.dialogs.open, Some(DialogId::NewProject));
    }

    #[test]
    fn banner_links_echo_tcl_verbs() {
        let mut rt = harness_with(start_state());
        rt.frame(0.0);
        tap(&mut rt, "start-hw-manager");
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == "open_hw_manager"),
            "banner link must echo its Tcl verb"
        );
    }

    #[test]
    fn open_project_link_raises_dialog_and_prj_reseeds_the_project() {
        use rune::FileDialogKind;
        let path =
            std::env::temp_dir().join(format!("helion-pick-{}-demo.prj", std::process::id()));
        std::fs::write(&path, "part xc7z007sclg400-1\ntop my_top\nread_sv top.v\n")
            .expect("fixture");
        let mut rt = harness_with(start_state());
        rt.frame(0.0);
        tap(&mut rt, "start-open-project");
        assert_eq!(rt.file_dialog().map(|(k, _)| k), Some(FileDialogKind::Open));
        rt.resolve_file_dialog(Some(path.clone()))
            .expect("answer the dialog");
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.project.part, "xc7z007sclg400-1",
            "part must come from the parsed .prj"
        );
        assert_eq!(rt.state.project.top.as_deref(), Some("my_top"));
        assert!(
            rt.state
                .console
                .lines
                .iter()
                .any(|l| l.text == format!("open_project {}", path.display())),
            "open must echo its Tcl verb"
        );
        assert_eq!(
            rt.state.center,
            crate::state::CenterPane::Summary,
            "open lands on the project summary"
        );
        let _ = std::fs::remove_file(&path);
    }
}
