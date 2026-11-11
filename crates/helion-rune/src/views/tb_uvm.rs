//! UVM surface of the testbench runner (inside the Sim pane detail).
//!
//! For class-based / UVM testbenches (`helion_verif::classtb`, engine in
//! `helion-svtb`): pick the test that runs (`+UVM_TESTNAME`, or the test
//! named by `run_test()`), the UVM report summary of the last run, and the
//! per-component UVM messages with severity; rows whose message carries a
//! source location (`` `uvm_error`` etc.) open the editor there.
//!
//! Tcl: `tb_uvm_test ?name|-default?` (selected testbench).

use std::path::Path;

use helion_verif::classtb::{UvmMsg, parse_uvm_message};
use rune::{El, ScrollSpec, col, el, row, text};

use crate::state::AppState;
use crate::theme::VivadoTheme;
use crate::views::cad::{self, ColSpec};
use crate::views::tb::{self, TbItem};
use crate::views::tb_view::section_head;

/// Messages listed in the table (newest kept).
const MSG_SHOWN: usize = 300;
const MSG_H: f32 = 62.0;
const ROW_H: f32 = 20.0;

const MSG_COLS: [ColSpec; 5] = [
    ColSpec::fixed("Severity", 86.0),
    ColSpec::num("Time (ns)", 72.0),
    ColSpec::grow("Component", 1.2),
    ColSpec::fixed("ID", 72.0),
    ColSpec::grow("Message", 2.0),
];

/// Plusargs for a run of `path` (`+UVM_TESTNAME=` when a test is chosen).
pub fn plusargs(s: &AppState, path: &Path) -> Vec<String> {
    s.sim.uvm_test.get(path).map(|t| vec![format!("+UVM_TESTNAME={t}")]).unwrap_or_default()
}

/// UVM tests discovered in testbench `i` (empty when not a UVM bench).
pub fn tests_of(item: &TbItem) -> Vec<String> {
    item.info.as_ref().ok().and_then(|i| i.class.as_ref()).map(|c| c.uvm_tests.clone()).unwrap_or_default()
}

fn default_test(item: &TbItem) -> Option<String> {
    item.info.as_ref().ok().and_then(|i| i.class.as_ref()).and_then(|c| c.default_test.clone())
}

/// Choose the UVM test for testbench `i` (`None` = the `run_test()` default).
pub fn select_test(s: &mut AppState, i: usize, test: Option<String>) -> Result<String, String> {
    let item = s.sim.tbs.get(i).ok_or_else(|| format!("tb_uvm_test: no testbench index {i}"))?;
    let path = item.file.path.clone();
    let name = item.name().to_string();
    match test {
        None => {
            s.sim.uvm_test.remove(&path);
            Ok(format!("tb_uvm_test {name}: default ({})", default_test(item).unwrap_or_else(|| "run_test() argument".into())))
        }
        Some(t) => {
            let tests = tests_of(item);
            if tests.is_empty() {
                return Err(format!("tb_uvm_test: {name} is not a UVM testbench"));
            }
            if !tests.contains(&t) {
                return Err(format!("tb_uvm_test: no UVM test `{t}` in {name} (tests: {})", tests.join(", ")));
            }
            s.sim.uvm_test.insert(path, t.clone());
            Ok(format!("tb_uvm_test {name}: +UVM_TESTNAME={t}"))
        }
    }
}

/// Tcl `tb_uvm_test ?name|-default?` on the selected testbench.
pub fn exec_uvm_test(s: &mut AppState, arg: &str) -> Result<String, String> {
    tb::ensure_scanned(s);
    let i = s.sim.selected.ok_or_else(|| "tb_uvm_test: select a testbench first".to_string())?;
    match arg {
        "" => {
            let item = &s.sim.tbs[i];
            let cur = s.sim.uvm_test.get(&item.file.path).cloned().or_else(|| default_test(item)).unwrap_or_else(|| "-".into());
            Ok(format!("tb_uvm_test {} (tests: {})", cur, tests_of(item).join(", ")))
        }
        "-default" => select_test(s, i, None),
        t => select_test(s, i, Some(t.to_string())),
    }
}

/// UVM report lines of the last run of `item`.
pub fn messages(item: &TbItem) -> Vec<UvmMsg> {
    match &item.last {
        Some(Ok(out)) => out.log.iter().filter_map(|l| parse_uvm_message(&l.text)).collect(),
        _ => Vec::new(),
    }
}

/// Open the source line of UVM message `k` of testbench `i`.
pub fn jump_to_message(s: &mut AppState, i: usize, k: usize) -> Result<String, String> {
    let m = s.sim.tbs.get(i).map(messages).unwrap_or_default().into_iter().nth(k).ok_or_else(|| format!("no UVM message {k}"))?;
    let file = m.file.ok_or_else(|| "this message has no source location".to_string())?;
    if file.starts_with("<helion>") {
        return Err("message comes from the built-in UVM library".into());
    }
    tb::open_in_editor(s, Path::new(&file), m.line)
}

fn sev_ink(sev: &str, t: &VivadoTheme) -> rune::Color {
    match sev {
        "UVM_ERROR" | "UVM_FATAL" => t.fail,
        "UVM_WARNING" => t.warn,
        _ => t.text_dim,
    }
}

fn test_picker(s: &AppState, i: usize, item: &TbItem, t: &VivadoTheme) -> El {
    let tests = tests_of(item);
    let chosen = s.sim.uvm_test.get(&item.file.path).cloned();
    let mut list = row()
        .key("sim-uvm-tests")
        .w_full()
        .h(ROW_H + 6.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .child(text("Test").font(VivadoTheme::FONT_UI).color(t.text_dim));
    let default_label = match default_test(item) {
        Some(d) => format!("Default ({d})"),
        None => "Default".to_string(),
    };
    let mut entries: Vec<(Option<String>, String)> = vec![(None, default_label)];
    entries.extend(tests.into_iter().map(|n| (Some(n.clone()), n)));
    for (k, (test, label)) in entries.into_iter().enumerate() {
        let selected = test == chosen;
        let help = match &test {
            Some(n) => format!("Run with +UVM_TESTNAME={n}"),
            None => "Run the test named in run_test() (no +UVM_TESTNAME)".into(),
        };
        list = list.child(
            row()
                .key(format!("sim-uvm-test-{k}"))
                .role("button")
                .label(format!("UVM test {label}"))
                .focus_selected(selected)
                .focusable(true)
                .h(ROW_H)
                .px(8.0)
                .min_w(0.0)
                .items_center()
                .radius(10.0)
                .fill(if selected { t.selection } else { t.panel_bg })
                .border(1.0, if selected { t.accent } else { t.border })
                .hover(t.selection)
                .help(help)
                .on_press(move |s: &mut AppState| {
                    let r = select_test(s, i, test.clone());
                    match r {
                        Ok(l) => s.console.lines.push(crate::state::ConsoleLine { text: l, sev: crate::state::MsgSev::Info }),
                        Err(e) => s.toasts.push(crate::state::ToastKind::Error, "Cannot select the UVM test", &e),
                    }
                })
                .child(cad::cut(label, VivadoTheme::FONT_MONO, if selected { t.text } else { t.text_dim })),
        );
    }
    list
}

fn summary_row(item: &TbItem, t: &VivadoTheme) -> Option<El> {
    let Some(Ok(out)) = &item.last else { return None };
    let u = out.uvm.as_ref()?;
    let ink = |n: u32, bad: rune::Color| if n > 0 { bad } else { t.text_dim };
    let count = |key: &str, label: &str, n: u32, color: rune::Color| {
        row()
            .key(key.to_string())
            .gap(4.0)
            .items_center()
            .child(text(label.to_string()).font(VivadoTheme::FONT_UI).color(t.text_dim))
            .child(cad::badge(n.to_string(), color, t))
    };
    let test = u.test.clone().unwrap_or_else(|| "-".into());
    let verdict = if out.passed { ("passed", t.pass) } else { ("failed", t.fail) };
    Some(
        row()
            .key("sim-uvm-summary")
            .w_full()
            .h(ROW_H + 4.0)
            .px(8.0)
            .gap(12.0)
            .items_center()
            .child(text("Last run").font(VivadoTheme::FONT_UI).color(t.text_dim))
            .child(el().key("sim-uvm-kpi-test").min_w(0.0).child(cad::cut(test, VivadoTheme::FONT_MONO, t.text)))
            .child(cad::badge(verdict.0, verdict.1, t))
            .child(el().grow(1.0))
            .child(count("sim-uvm-kpi-info", "Info", u.info, t.text_dim))
            .child(count("sim-uvm-kpi-warning", "Warning", u.warning, ink(u.warning, t.warn)))
            .child(count("sim-uvm-kpi-error", "Error", u.error, ink(u.error, t.fail)))
            .child(count("sim-uvm-kpi-fatal", "Fatal", u.fatal, ink(u.fatal, t.fail))),
    )
}

fn message_table(i: usize, item: &TbItem, t: &VivadoTheme) -> Option<El> {
    let msgs = messages(item);
    if msgs.is_empty() {
        return None;
    }
    let n = msgs.len();
    let skip = n.saturating_sub(MSG_SHOWN);
    let mut body = col().key("sim-uvm-msgs-body").w_full().h(MSG_H.min(20.0 * n as f32 + 2.0)).scroll(ScrollSpec { stick_bottom: true });
    for (k, m) in msgs.into_iter().enumerate().skip(skip) {
        let has_src = m.file.as_ref().is_some_and(|f| !f.starts_with("<helion>"));
        let label = format!("{} {} [{}] {}", m.severity, m.path, m.id, m.message);
        let mut r = cad::table_row(format!("sim-uvm-msg-{k}"), label, false, t);
        if has_src {
            let f = m.file.clone().unwrap_or_default();
            r = r.help(format!("Open {f} at line {}", m.line)).on_press(move |s: &mut AppState| {
                if let Err(e) = jump_to_message(s, i, k) {
                    s.toasts.push(crate::state::ToastKind::Error, "Cannot open the message source", &e);
                }
            });
        } else {
            r = r.description("Reported by the UVM library (no testbench source line)");
        }
        let msg_ink = if has_src { t.text } else { t.text_dim };
        body = body.child(
            r.child(cad::cell(&MSG_COLS[0], text(m.severity.clone()).font(VivadoTheme::FONT_MONO).color(sev_ink(&m.severity, t))))
                .child(cad::cell(&MSG_COLS[1], text(m.time.to_string()).font(VivadoTheme::FONT_MONO.mono()).color(t.text_dim)))
                .child(cad::cell(&MSG_COLS[2], cad::cut(m.path.clone(), VivadoTheme::FONT_MONO, t.text)))
                .child(cad::cell(&MSG_COLS[3], cad::cut(m.id.clone(), VivadoTheme::FONT_MONO, t.link)))
                .child(cad::cell(&MSG_COLS[4], cad::cut(m.message.clone(), VivadoTheme::FONT_UI, msg_ink))),
        );
    }
    Some(
        col()
            .key("sim-uvm-msgs")
            .w_full()
            .child(section_head("sim-uvm-msgs-head", &format!("UVM messages ({n})"), t))
            .child(cad::header("sim-uvm-msgs-cols", &MSG_COLS, t))
            .child(body),
    )
}

/// The UVM block of the testbench detail (`None` for non-UVM benches).
pub fn uvm_section(s: &AppState, i: usize, item: &TbItem, t: &VivadoTheme) -> Option<El> {
    let class = item.info.as_ref().ok()?.class.as_ref()?;
    if !class.uses_uvm {
        return None;
    }
    let head = format!("UVM tests ({})", class.uvm_tests.len());
    Some(
        col()
            .key("sim-uvm")
            .w_full()
            .child(section_head("sim-uvm-head", &head, t))
            .child(test_picker(s, i, item, t))
            .children(summary_row(item, t))
            .children(message_table(i, item, t)),
    )
}

/// Scene/test fixture: a temp copy of `examples/uvm_counter` with
/// `examples/counter.sv` (or a deliberately broken counter when `buggy`)
/// in the parent folder, discovered and run once.
pub fn seed_uvm_fixture(s: &mut AppState, buggy: bool) -> Result<std::path::PathBuf, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let ex = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let dir = std::env::temp_dir().join(format!("helion-rune-uvm-fixture-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    let tbdir = dir.join("uvm_counter");
    std::fs::create_dir_all(&tbdir).map_err(|e| e.to_string())?;
    for f in ["counter_if.sv", "counter_pkg.sv", "uvm_counter_tb.sv"] {
        std::fs::copy(ex.join("uvm_counter").join(f), tbdir.join(f)).map_err(|e| format!("{f}: {e}"))?;
    }
    if buggy {
        std::fs::write(
            dir.join("counter.sv"),
            "module counter (input logic clk, output logic led);\n  logic [3:0] cnt;\n  always_ff @(posedge clk) cnt <= cnt + 1;\n  assign led = cnt[2]; // bug: should be cnt[3]\nendmodule\n",
        )
        .map_err(|e| e.to_string())?;
    } else {
        std::fs::copy(ex.join("counter.sv"), dir.join("counter.sv")).map_err(|e| e.to_string())?;
    }
    s.sim.dir_override = Some(tbdir.clone());
    tb::rescan(s);
    s.sim.selected = Some(0);
    tb::run_testbench_now(s, 0)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init;

    #[test]
    fn uvm_fixture_runs_the_example_and_lists_tests_summary_and_messages() {
        let mut s = init();
        seed_uvm_fixture(&mut s, false).unwrap();
        let item = &s.sim.tbs[0];
        assert_eq!(item.name(), "uvm_counter_tb");
        assert_eq!(tests_of(item), ["counter_long_test", "counter_test"]);
        assert_eq!(default_test(item).as_deref(), Some("counter_test"));
        let out = item.last.as_ref().unwrap().as_ref().unwrap();
        assert!(out.passed, "{}", out.summary);
        let u = out.uvm.as_ref().unwrap();
        assert_eq!((u.test.as_deref(), u.error, u.fatal), (Some("counter_test"), 0, 0));
        let msgs = messages(item);
        assert!(msgs.iter().any(|m| m.path == "uvm_test_top.env.scb" && m.id == "SCB"));
        assert!(msgs.iter().any(|m| m.path == "reporter" && m.id == "RNTST"));
    }

    #[test]
    fn uvm_test_selection_sets_the_testname_plusarg() {
        let mut s = init();
        seed_uvm_fixture(&mut s, false).unwrap();
        let path = s.sim.tbs[0].file.path.clone();
        assert!(plusargs(&s, &path).is_empty());
        assert!(tb::exec_tb(&mut s, "tb_uvm_test", &["nope".into()]).unwrap_err().contains("no UVM test `nope`"));
        tb::exec_tb(&mut s, "tb_uvm_test", &["counter_long_test".into()]).unwrap();
        assert_eq!(plusargs(&s, &path), ["+UVM_TESTNAME=counter_long_test"]);
        tb::run_testbench_now(&mut s, 0).unwrap();
        let out = s.sim.tbs[0].last.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(out.uvm.as_ref().unwrap().test.as_deref(), Some("counter_long_test"));
        // The choice survives a rescan.
        tb::rescan(&mut s);
        assert_eq!(plusargs(&s, &path), ["+UVM_TESTNAME=counter_long_test"]);
        tb::exec_tb(&mut s, "tb_uvm_test", &["-default".into()]).unwrap();
        assert!(plusargs(&s, &path).is_empty());
    }

    #[test]
    fn failing_uvm_run_reports_errors_and_messages_jump_to_source() {
        let mut s = init();
        seed_uvm_fixture(&mut s, true).unwrap();
        let item = &s.sim.tbs[0];
        let out = item.last.as_ref().unwrap().as_ref().unwrap();
        assert!(!out.passed);
        assert!(out.uvm.as_ref().unwrap().error > 0);
        let k = messages(item).iter().position(|m| m.severity == "UVM_ERROR").unwrap();
        let line = messages(item)[k].line;
        jump_to_message(&mut s, 0, k).unwrap();
        assert_eq!(s.center, crate::state::CenterPane::Editor);
        let text = s.editor.lines.join("\n");
        assert!(text.contains("class counter_scoreboard"));
        assert!(s.editor.lines[line - 1].contains("uvm_error(\"SCB\""));
        // Library-origin messages refuse to jump.
        let lib = messages(&s.sim.tbs[0]).iter().position(|m| m.file.is_none()).unwrap();
        assert!(jump_to_message(&mut s, 0, lib).is_err());
    }
}
