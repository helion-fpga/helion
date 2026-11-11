//! Editor gutter diagnostics and bookmarks.
//!
//! Diagnostics come from a real parse of the active buffer, never from a
//! lint of our own (old `C/editor.rs:1047-1183` painted methodology/DRC
//! ticks; here the parsers speak):
//! - Verilog/SystemVerilog: `helion_sv::parse_sv` (sv-parser) reports the
//!   byte where parsing stopped, mapped to its line as an error; a file that
//!   parses but that `helion_sv::list_sv_modules` cannot elaborate gets a
//!   warning on its module line.
//! - VHDL: `helion_vhdl::map_vhdl` errors, plus its soft diagnostics
//!   (`helion_ir::SoftDiag` with source lines) as warnings.
//! - XDC/SDC: the Constraints pane's per-line `helion_sta` parse errors.
//!
//! Parsing a 27k-line file takes a while, so it runs on a worker, debounced
//! after typing stops ([`poll`]); the gutter shows the last result for the
//! tab. Bookmarks are per tab and toggled from the gutter tick column or
//! the toolbar.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, mpsc};

use crate::state::AppState;
use crate::views::highlight::Dialect;

/// Quiet period after the last edit before a re-parse starts.
const DEBOUNCE_S: f64 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagSev {
    Error,
    Warning,
}

/// One diagnostic on a 0-based line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub line: usize,
    pub sev: DiagSev,
    pub msg: String,
}

type Key = (String, u64);

/// Diagnostics of the active tab plus the in-flight parse.
#[derive(Clone, Default)]
pub struct DiagState {
    /// `(tab, text fingerprint)` the items describe.
    pub key: Option<Key>,
    pub items: Vec<Diag>,
    pending: Option<(Key, Arc<Mutex<mpsc::Receiver<Vec<Diag>>>>)>,
    due: Option<(Key, f64)>,
}

impl std::fmt::Debug for DiagState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagState")
            .field("key", &self.key)
            .field("items", &self.items)
            .field("pending", &self.pending.is_some())
            .finish()
    }
}

impl DiagState {
    pub fn is_parsing(&self) -> bool {
        self.pending.is_some()
    }
}

/// Byte offset → 0-based line.
fn line_of(text: &str, byte: usize) -> usize {
    let b = byte.min(text.len());
    text.as_bytes()[..b].iter().filter(|&&c| c == b'\n').count()
}

/// `line N` / `:N:` style 1-based line in an engine message, if any.
fn line_in_msg(msg: &str) -> Option<usize> {
    let lower = msg.to_ascii_lowercase();
    let at = lower.find("line ")?;
    let digits: String = lower[at + 5..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .map(|n| n - 1)
}

/// The `N` of sv-parser's `Some(("origin", N))` error tail.
fn sv_offset(msg: &str) -> Option<usize> {
    let tail = msg.rsplit(", ").next()?;
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The token at `byte` (what the parser choked on), for the message.
fn token_at(text: &str, byte: usize) -> String {
    let b = byte.min(text.len());
    let rest = text[b..].trim_start();
    let tok: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace())
        .take(24)
        .collect();
    if tok.is_empty() {
        "end of file".into()
    } else {
        format!("`{tok}`")
    }
}

fn first_line_with(text: &str, word: &str) -> usize {
    text.lines()
        .position(|l| {
            l.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|w| w.eq_ignore_ascii_case(word))
        })
        .unwrap_or(0)
}

/// Parse `text` (named `name`) with the real frontend for its language.
/// Pure: this is the worker body.
pub fn compute(text: &str, name: &str) -> Vec<Diag> {
    let mut out = Vec::new();
    match Dialect::for_name(name) {
        Dialect::Verilog => match helion_sv::parse_sv(text, name) {
            Err(e) => {
                let byte = sv_offset(&e).unwrap_or(0);
                out.push(Diag {
                    line: line_of(text, byte),
                    sev: DiagSev::Error,
                    msg: format!("Syntax error near {} (sv-parser)", token_at(text, byte)),
                });
            }
            Ok(_) => {
                if let Err(e) = helion_sv::list_sv_modules(text) {
                    out.push(Diag {
                        line: line_in_msg(&e).unwrap_or_else(|| first_line_with(text, "module")),
                        sev: DiagSev::Warning,
                        msg: format!("helion-sv cannot elaborate: {e}"),
                    });
                }
            }
        },
        Dialect::Vhdl => match helion_vhdl::map_vhdl(text) {
            Err(e) => out.push(Diag {
                line: line_in_msg(&e).unwrap_or_else(|| first_line_with(text, "entity")),
                sev: DiagSev::Error,
                msg: e,
            }),
            Ok(m) => {
                for d in &m.softs {
                    let Some(line) = d.span.line.filter(|l| *l > 0) else {
                        continue;
                    };
                    let what = helion_ir::resolve(d.name).to_string();
                    let msg = match &d.detail {
                        Some(detail) => format!("{what}: {detail}"),
                        None => what,
                    };
                    out.push(Diag {
                        line: line as usize - 1,
                        sev: DiagSev::Warning,
                        msg,
                    });
                }
            }
        },
        Dialect::Tcl => {
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            for (line, msg) in crate::views::constraints::parse_constraints(&lines).errors {
                out.push(Diag {
                    line: line.saturating_sub(1),
                    sev: DiagSev::Error,
                    msg,
                });
            }
        }
    }
    out.sort_by(|a, b| (a.line, a.sev).cmp(&(b.line, b.sev)));
    out
}

/// The active tab's diagnostics when they describe that tab (the last
/// result stays up while a re-parse runs, so ticks never flicker).
pub fn for_view(s: &AppState) -> &[Diag] {
    let tab = s.editor.tabs.get(s.editor.active);
    match &s.editor.diag.key {
        Some((name, _)) if Some(name) == tab => &s.editor.diag.items,
        _ => &[],
    }
}

/// Diagnostics on 0-based `line`.
pub fn on_line(items: &[Diag], line: usize) -> Vec<&Diag> {
    items.iter().filter(|d| d.line == line).collect()
}

/// Debounced worker pump. Returns (state changed, seconds until the next
/// wake is useful).
pub fn pump(s: &mut AppState, now: f64) -> (bool, Option<f64>) {
    let mut changed = false;
    if let Some((key, rx)) = s.editor.diag.pending.clone() {
        let next = rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
        match next {
            Ok(items) => {
                s.editor.diag.items = items;
                s.editor.diag.key = Some(key);
                s.editor.diag.pending = None;
                changed = true;
            }
            Err(mpsc::TryRecvError::Empty) => return (false, Some(0.1)),
            Err(mpsc::TryRecvError::Disconnected) => s.editor.diag.pending = None,
        }
    }
    let Some(tab) = s
        .editor
        .edit_tab
        .filter(|i| *i == s.editor.active)
        .and_then(|i| s.editor.tabs.get(i).cloned())
    else {
        return (changed, None);
    };
    let want: Key = (
        tab,
        crate::views::editor_scale::fingerprint(&s.editor.edit.text),
    );
    if s.editor.diag.key.as_ref() == Some(&want) {
        s.editor.diag.due = None;
        return (changed, None);
    }
    match &s.editor.diag.due {
        Some((k, at)) if *k == want => {
            if now < *at {
                return (changed, Some(at - now));
            }
        }
        _ => {
            s.editor.diag.due = Some((want, now + DEBOUNCE_S));
            return (changed, Some(DEBOUNCE_S));
        }
    }
    s.editor.diag.due = None;
    let text = s.editor.edit.text.clone();
    let name = want.0.clone();
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("helion-editor-diag".into())
        .spawn(move || {
            let _ = tx.send(compute(&text, &name));
        });
    if spawned.is_ok() {
        s.editor.diag.pending = Some((want, Arc::new(Mutex::new(rx))));
    }
    (changed, Some(0.1))
}

/// `AppSpec::poll` wiring.
pub fn poll(s: &mut AppState, now: f64, tx: &mut rune::Tx) -> bool {
    let (changed, wake) = pump(s, now);
    if let Some(secs) = wake {
        tx.wake_after(std::time::Duration::from_secs_f64(secs.max(0.02)));
    }
    changed
}

// ---- bookmarks ----

/// Bookmarked lines of the active tab.
pub fn bookmarks(s: &AppState) -> Option<&BTreeSet<usize>> {
    let tab = s.editor.tabs.get(s.editor.active)?;
    s.editor.bookmarks.get(tab).filter(|b| !b.is_empty())
}

/// Toggle a bookmark on 0-based `line` of the active tab.
pub fn toggle_bookmark(s: &mut AppState, line: usize) {
    let Some(tab) = s.editor.tabs.get(s.editor.active).cloned() else {
        return;
    };
    let set = s.editor.bookmarks.entry(tab).or_default();
    if !set.remove(&line) {
        set.insert(line);
    }
}

/// The next bookmark after `from` (wrapping), if the tab has any.
pub fn next_bookmark(s: &AppState, from: usize) -> Option<usize> {
    let set = bookmarks(s)?;
    set.range(from + 1..)
        .next()
        .or_else(|| set.iter().next())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sv_syntax_errors_land_on_their_line() {
        let src = "module a(input clk);\n  wire x;\n  assign x = ;\nendmodule\n";
        let d = compute(src, "a.sv");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!((d[0].line, d[0].sev), (2, DiagSev::Error));
        assert!(d[0].msg.contains("`;`"), "{}", d[0].msg);
        assert!(compute("module a(input clk);\nendmodule\n", "a.sv").is_empty());
    }

    #[test]
    fn xdc_and_vhdl_use_their_own_parsers() {
        let d = compute(
            "create_clock -name clk\nset_property X Y [get_ports a]\n",
            "p.xdc",
        );
        assert!(
            d.iter().any(|x| x.line == 0 && x.sev == DiagSev::Error),
            "{d:?}"
        );
        let v = compute("architecture r of nope is\nbegin\nend r;\n", "x.vhd");
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].sev, DiagSev::Error);
    }

    #[test]
    fn the_worker_parses_after_the_debounce_and_tracks_the_tab() {
        let mut s = crate::init();
        crate::views::editor::sync_edit(&mut s);
        s.editor.edit =
            rune::TextEdit::new("module a(input clk);\n  wire x\n  assign x = 1;\nendmodule");
        let (_, wake) = pump(&mut s, 0.0);
        assert_eq!(wake, Some(DEBOUNCE_S), "first change arms the debounce");
        assert!(!s.editor.diag.is_parsing());
        pump(&mut s, 1.0);
        assert!(s.editor.diag.is_parsing(), "parse starts once quiet");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while s.editor.diag.is_parsing() && std::time::Instant::now() < deadline {
            pump(&mut s, 2.0);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let items = for_view(&s);
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].line, 2);
        // Same text again: nothing to do.
        assert_eq!(pump(&mut s, 3.0), (false, None));
    }

    #[test]
    fn bookmarks_toggle_and_wrap() {
        let mut s = crate::init();
        toggle_bookmark(&mut s, 4);
        toggle_bookmark(&mut s, 9);
        assert_eq!(next_bookmark(&s, 4), Some(9));
        assert_eq!(next_bookmark(&s, 9), Some(4), "wraps");
        toggle_bookmark(&mut s, 4);
        assert_eq!(next_bookmark(&s, 0), Some(9));
    }
}
