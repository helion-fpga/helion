//! Showcase scenes for the README and website screenshots: a real project
//! (the full ysyx Ibex RV32 core by default) taken through the IDE's own run
//! worker — synthesis, placement, routing, STA — before the pane renders.
//! Nothing here is a fixture; the pictures show what a user gets.
//!
//! Enabled with `HELION_SHOWCASE=1` so the test scene catalog stays fast:
//!
//! ```sh
//! HELION_SHOWCASE=1 helion-ide --snap docs/screenshots/ide-device.png --scene showcase-device
//! ```
//!
//! `HELION_SHOWCASE_PRJ` picks another project (default `examples/ibex_core.prj`).

use crate::state::{AppState, CenterPane};
use crate::{runs, views};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const SCENES: &[(&str, fn(&mut AppState))] = &[
    ("showcase-summary", |s| {
        real(s);
        s.navigate(CenterPane::Summary);
    }),
    ("showcase-device", |s| {
        real(s);
        s.navigate(CenterPane::Device);
        s.device.layers.nets = true;
        s.device.zoom = 0.5;
    }),
    ("showcase-schematic", |s| {
        real(s);
        s.navigate(CenterPane::Schematic);
    }),
    ("showcase-timing", |s| {
        real(s);
        s.navigate(CenterPane::Timing);
        s.timing_view.selected = views::timing::TimingNav::IntraClock;
    }),
    ("showcase-editor", |s| {
        real(s);
        s.navigate(CenterPane::Editor);
    }),
    ("showcase-console", |s| {
        real(s);
        for verb in ["report_utilization", "report_timing -max_paths 5"] {
            crate::tcl::eval_command(s, verb);
        }
        s.navigate(CenterPane::Summary);
    }),
];

fn project() -> PathBuf {
    std::env::var_os("HELION_SHOWCASE_PRJ")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ibex_core.prj")
        })
}

/// Open the project and run implementation to completion on the real worker.
fn real(s: &mut AppState) {
    let prj = project().canonicalize().unwrap_or_else(|_| project());
    crate::pickers::open_path(s, &prj);
    runs::launch(s, runs::RunKind::Impl);
    let deadline = Instant::now() + Duration::from_secs(1800);
    loop {
        let busy = runs::poll_runs(s) | runs::pump_queue(s);
        if !s.runs.is_active() && !busy {
            break;
        }
        if Instant::now() > deadline {
            eprintln!("showcase: run did not finish within 30 minutes");
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Scratch checkpoint paths are machine-specific; keep them out of
    // published screenshots.
    let tmp = std::env::temp_dir().to_string_lossy().into_owned();
    s.console
        .lines
        .retain(|l| !l.text.contains(tmp.trim_end_matches('/')));
}

/// The scene list `app_spec` uses: the test catalog, plus these when
/// `HELION_SHOWCASE` is set.
pub fn scenes() -> &'static [(&'static str, fn(&mut AppState))] {
    if std::env::var_os("HELION_SHOWCASE").is_none() {
        return crate::SCENES;
    }
    let all: Vec<_> = crate::SCENES.iter().chain(SCENES).copied().collect();
    Box::leak(all.into_boxed_slice())
}
