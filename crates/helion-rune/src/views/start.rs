//! Start page: the launcher shown when no project is open.
//!
//! Three cards on a quiet background: **Start** (create, open, the bundled
//! example), **Recent Projects** (real paths from prefs; a missing file is
//! shown as missing, not hidden), and **Tasks / Learn** (Hardware Manager,
//! Settings, Keyboard Shortcuts, Documentation). Every row performs its
//! action; nothing only prints a Tcl verb.

use rune::{El, ScrollSpec, Tx, col, el, row, text};

use crate::state::{AppState, CenterPane};
use crate::theme::VivadoTheme;
use crate::views::dialogs::DialogId;

/// Card max width so lines stay readable on wide windows.
const CARD_W: f32 = 360.0;

/// One launcher row: title (link ink) over a one-line description.
fn action_row(
    key: &'static str,
    title: &'static str,
    detail: &str,
    t: &VivadoTheme,
    enabled: bool,
) -> El {
    let mut r = col()
        .key(key)
        .role("button")
        .label(title)
        .focusable(enabled)
        .disabled(!enabled)
        .w_full()
        .py(6.0)
        .px(10.0)
        .gap(2.0)
        .radius(4.0)
        .child(
            text(title)
                .font(VivadoTheme::FONT_TITLE)
                .color(if enabled { t.link } else { t.text_faint }),
        )
        .child(
            text(detail.to_string())
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .truncate(),
        );
    if enabled {
        r = r.hover(t.selection);
    } else {
        r = r.layer_opacity(0.6);
    }
    r
}

/// A titled card with caps heading.
fn card(key: &'static str, title: &'static str, body: El, t: &VivadoTheme) -> El {
    col()
        .key(key)
        .w(CARD_W)
        .max_w(CARD_W)
        .shrink()
        .min_w(0.0)
        .fill(t.panel_bg)
        .border(1.0, t.border)
        .radius(6.0)
        .padding(8.0)
        .gap(4.0)
        .child(
            el().px(10.0).pt(4.0).pb(2.0).child(
                text(title)
                    .font(VivadoTheme::FONT_CAPS)
                    .color(t.text_faint),
            ),
        )
        .child(body)
}

/// Start card: create, open, example.
fn start_card(t: &VivadoTheme) -> El {
    let example = crate::views::chrome::example_project();
    let body = col()
        .w_full()
        .child(
            action_row(
                "start-create-project",
                "Create Project…",
                "New project from sources, a part and constraints",
                t,
                true,
            )
            .on_press(|s: &mut AppState| s.dialogs.open(DialogId::NewProject)),
        )
        .child(
            action_row(
                "start-open-project",
                "Open Project…",
                "Open a .prj project or a single HDL file",
                t,
                true,
            )
            .on_press_tx(|_: &mut AppState, tx: &mut Tx| crate::pickers::open_project(tx)),
        )
        .child({
            let r = action_row(
                "start-open-example",
                "Open Example Project",
                if example.is_some() {
                    "counter.prj — a 4-bit counter with an SDC clock"
                } else {
                    "Examples are not installed with this build"
                },
                t,
                example.is_some(),
            );
            match example {
                Some(path) => {
                    r.on_press(move |s: &mut AppState| crate::pickers::open_path(s, &path))
                }
                None => r,
            }
        });
    card("start-quick", "START", body, t)
}

/// Recent projects card: newest first; missing files say so.
fn recent_card(s: &AppState, t: &VivadoTheme) -> El {
    let mut body = col().key("start-recent-list").w_full();
    if s.recent.is_empty() {
        body = body.child(
            el().key("start-recent-empty").px(10.0).py(8.0).child(
                text("Projects you open appear here.")
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_dim),
            ),
        );
    }
    for (i, path) in s.recent.iter().enumerate() {
        let p = std::path::PathBuf::from(path);
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let dir = p
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let exists = p.exists();
        let mut r = col()
            .key(format!("start-recent-{i}"))
            .role("button")
            .label(format!("Open {name}"))
            .help(path.clone())
            .focusable(exists)
            .disabled(!exists)
            .w_full()
            .py(5.0)
            .px(10.0)
            .gap(1.0)
            .radius(4.0)
            .child(
                row()
                    .gap(6.0)
                    .items_center()
                    .child(
                        text(name)
                            .font(VivadoTheme::FONT_TITLE)
                            .color(if exists { t.text } else { t.text_faint }),
                    )
                    .child(if exists {
                        el()
                    } else {
                        text("missing")
                            .font(VivadoTheme::FONT_CAPS)
                            .color(t.warn)
                    }),
            )
            .child(
                text(dir)
                    .font(VivadoTheme::FONT_UI)
                    .color(t.text_faint)
                    .truncate_middle(),
            );
        if exists {
            r = r
                .hover(t.selection)
                .on_press(move |s: &mut AppState| crate::pickers::open_path(s, &p));
        }
        body = body.child(r);
    }
    card("start-recent", "RECENT PROJECTS", body, t)
}

/// Tasks + learning card.
fn tasks_card(t: &VivadoTheme) -> El {
    let body = col()
        .w_full()
        .child(
            action_row(
                "start-hw-manager",
                "Open Hardware Manager",
                "Detect a board, program a bitstream, read STAT",
                t,
                true,
            )
            .on_press(|s: &mut AppState| s.navigate(CenterPane::Program)),
        )
        .child(
            action_row(
                "start-settings",
                "Settings…",
                "Synthesis and implementation strategies, editor, theme",
                t,
                true,
            )
            .on_press(|s: &mut AppState| s.dialogs.open(DialogId::Settings)),
        )
        .child(
            action_row(
                "start-shortcuts",
                "Keyboard Shortcuts",
                "⌘P files · ⇧⌘P commands · ⌘1/2/3 views",
                t,
                true,
            )
            .on_press(crate::views::palette::open_cheatsheet),
        );
    let learn = col().key("start-learning").w_full().child(
        action_row(
            "start-docs",
            "Documentation",
            "Guides and reference on the Helion repository",
            t,
            true,
        )
        .on_press_tx(|s: &mut AppState, tx: &mut Tx| {
            let url = crate::views::chrome::DOCS_URL;
            if tx.open_url(url).is_err() {
                s.toasts.push(
                    crate::state::ToastKind::Error,
                    "Couldn't open the documentation",
                    url,
                );
            }
        }),
    );
    card(
        "start-tasks",
        "TASKS",
        col().w_full().child(body).child(
            el().w_full().px(10.0).pt(8.0).pb(2.0).child(
                text("LEARN")
                    .font(VivadoTheme::FONT_CAPS)
                    .color(t.text_faint),
            ),
        )
        .child(learn),
        t,
    )
}

/// Product header: wordmark, tagline and version.
fn vendor_header(t: &VivadoTheme) -> El {
    row()
        .key("start-header")
        .w_full()
        .items_end()
        .justify_between()
        .px(4.0)
        .child(
            col()
                .gap(2.0)
                .child(text("Helion").font(VivadoTheme::FONT_DISPLAY).color(t.text))
                .child(
                    text("FPGA design suite — synthesis to bitstream")
                        .font(VivadoTheme::FONT_UI)
                        .color(t.text_dim),
                ),
        )
        .child(
            text(format!("v{}", env!("CARGO_PKG_VERSION")))
                .font(VivadoTheme::FONT_UI)
                .color(t.text_faint),
        )
}

/// The start page center surface.
pub fn start_pane(s: &AppState, t: &VivadoTheme) -> El {
    col()
        .key("start-page")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(t.chrome_bg)
        .border(1.0, t.border)
        .child(
            col()
                .key("start-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(28.0)
                .gap(20.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(vendor_header(t))
                .child(
                    row()
                        .w_full()
                        .gap(16.0)
                        .items_start()
                        .flex_wrap()
                        .child(start_card(t))
                        .child(recent_card(s, t))
                        .child(tasks_card(t)),
                ),
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
            "start-recent",
            "start-recent-empty",
            "start-tasks",
            "start-hw-manager",
            "start-settings",
            "start-shortcuts",
            "start-learning",
            "start-docs",
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
    fn hardware_manager_link_navigates() {
        let mut rt = harness_with(start_state());
        rt.frame(0.0);
        tap(&mut rt, "start-hw-manager");
        assert_eq!(rt.state.center, crate::state::CenterPane::Program);
    }

    #[test]
    fn example_project_opens_real_files_and_lands_in_recents() {
        let mut rt = harness_with(start_state());
        rt.frame(0.0);
        tap(&mut rt, "start-open-example");
        assert_eq!(rt.state.project.name, "counter");
        assert!(
            rt.state
                .runs
                .source
                .as_ref()
                .is_some_and(|p| p.ends_with("counter.prj")),
            "the .prj becomes the run input"
        );
        assert!(rt.state.recent[0].ends_with("counter.prj"));
        assert!(rt.state.project_files.files.iter().all(|f| f.path.is_some()));
    }

    #[test]
    fn recent_rows_open_and_missing_files_are_inert() {
        let mut s = start_state();
        s.recent = vec!["/definitely/missing/x.prj".into()];
        let mut rt = harness_with(s);
        rt.frame(0.0);
        assert!(rt.find("start-recent-0").is_some());
        tap(&mut rt, "start-recent-0");
        assert_eq!(rt.state.center, crate::state::CenterPane::Start, "missing file does nothing");
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
