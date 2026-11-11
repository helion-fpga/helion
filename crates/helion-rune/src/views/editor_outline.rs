//! Editor symbol outline and go-to-definition.
//!
//! [`symbols`] scans the buffer for the structure an RTL engineer jumps
//! between (old `C/editor.rs:696-791` outline): modules/interfaces/packages,
//! ports, always/initial blocks, functions and tasks; for VHDL, entities,
//! architectures, ports and processes. The outline strip lists them beside
//! the code (virtual rows, so a 27k-line file's thousands of symbols cost
//! only the visible rows) and a click jumps the caret there.
//!
//! Go to Definition (F12) resolves the word under the caret: a module or
//! entity declared by any project file (`ProjectFiles::defining`) opens
//! that file at its declaration; otherwise a symbol of the open file.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use rune::{Color, El, GrowthSpec, ScrollSpec, Tx, col, el, icon, row, text};

use crate::state::{AppState, ToastKind};
use crate::theme::VivadoTheme;
use crate::views::highlight::{Dialect, block_open_after};

/// Outline strip width.
pub const OUTLINE_W: f32 = 232.0;
const SYM_ROW_H: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymKind {
    Module,
    Port,
    Block,
    Func,
    Entity,
    Arch,
    Process,
}

impl SymKind {
    pub fn badge(self) -> &'static str {
        match self {
            SymKind::Module => "MOD",
            SymKind::Port => "PORT",
            SymKind::Block => "BLK",
            SymKind::Func => "FN",
            SymKind::Entity => "ENT",
            SymKind::Arch => "ARCH",
            SymKind::Process => "PROC",
        }
    }

    fn ink(self, t: &VivadoTheme) -> Color {
        match self {
            SymKind::Module | SymKind::Entity | SymKind::Arch => t.accent,
            SymKind::Port => t.link,
            SymKind::Block | SymKind::Process => t.warn,
            SymKind::Func => t.code_type,
        }
    }

    /// Definition preference when a name resolves to several symbols.
    fn rank(self) -> u8 {
        match self {
            SymKind::Module | SymKind::Entity => 0,
            SymKind::Func => 1,
            SymKind::Port => 2,
            SymKind::Arch | SymKind::Process | SymKind::Block => 3,
        }
    }
}

/// One outline entry on a 0-based line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub kind: SymKind,
    pub name: String,
    pub line: usize,
    /// Direction, keyword or sensitivity shown dim beside the name.
    pub detail: String,
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

fn idents(code: &str) -> Vec<&str> {
    code.split(|c: char| !is_ident_char(c))
        .filter(|w| !w.is_empty() && !w.starts_with(|c: char| c.is_ascii_digit()))
        .collect()
}

/// Drop `[...]` ranges and `= value` defaults from a declaration segment.
fn strip_ranges(seg: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in seg.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split('=').next().unwrap_or("").to_string()
}

const SV_DIRS: &[&str] = &["input", "output", "inout", "ref"];

/// Port names declared on one SV code line (ANSI header, one-line header
/// or a non-ANSI `input clk;`): every comma segment after a direction keyword
/// contributes its last identifier.
fn sv_ports(code: &str, line: usize, out: &mut Vec<Symbol>) {
    let words = idents(code);
    if !words.iter().any(|w| SV_DIRS.contains(w)) {
        return;
    }
    let mut dir: Option<&'static str> = None;
    // Walk comma/semicolon/paren separated segments in order.
    for seg in code.split([',', ';', '(', ')']) {
        let clean = strip_ranges(seg);
        let ids = idents(&clean);
        if let Some(d) = ids
            .iter()
            .find_map(|w| SV_DIRS.iter().copied().find(|x| x == w))
        {
            dir = Some(d);
        }
        let Some(d) = dir else { continue };
        if let Some(name) = ids.last()
            && !SV_DIRS.contains(name)
            && !matches!(
                *name,
                "wire" | "reg" | "logic" | "signed" | "unsigned" | "var" | "bit" | "module"
            )
        {
            out.push(Symbol {
                kind: SymKind::Port,
                name: name.to_string(),
                line,
                detail: d.to_string(),
            });
        }
    }
}

fn sv_symbols(text: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    let mut in_block = false;
    for (n, raw) in text.split('\n').enumerate() {
        let started_in = in_block;
        in_block = block_open_after(raw, Dialect::Verilog, started_in);
        // Code outside comments, roughly: skip a leading block-comment tail
        // and anything after `//` or `/*`.
        let mut code = raw;
        if started_in {
            match code.find("*/") {
                Some(p) => code = &code[p + 2..],
                None => continue,
            }
        }
        let cut = [code.find("//"), code.find("/*")]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(code.len());
        let code = code[..cut].trim();
        if code.is_empty() {
            continue;
        }
        let words = idents(code);
        let Some(&first) = words.first() else {
            continue;
        };
        match first {
            "module" | "macromodule" | "interface" | "package" | "program" => {
                if let Some(name) = words
                    .iter()
                    .skip(1)
                    .find(|w| !matches!(**w, "automatic" | "static"))
                {
                    out.push(Symbol {
                        kind: SymKind::Module,
                        name: name.to_string(),
                        line: n,
                        detail: first.to_string(),
                    });
                }
                if let Some(p) = code.find('(') {
                    sv_ports(&code[p..], n, &mut out);
                }
            }
            "always" | "always_ff" | "always_comb" | "always_latch" | "initial" | "final" => {
                let label = code
                    .split_once("begin")
                    .and_then(|(_, rest)| rest.trim().strip_prefix(':'))
                    .map(|l| l.trim().split_whitespace().next().unwrap_or("").to_string())
                    .filter(|l| !l.is_empty());
                let head = code.split("begin").next().unwrap_or(code).trim();
                let head: String = head.chars().take(48).collect();
                out.push(Symbol {
                    kind: SymKind::Block,
                    name: label.unwrap_or_else(|| head.clone()),
                    line: n,
                    detail: first.to_string(),
                });
            }
            "function" | "task" => {
                let upto = code.split(['(', ';']).next().unwrap_or(code);
                if let Some(name) = idents(&strip_ranges(upto)).last()
                    && *name != first
                {
                    out.push(Symbol {
                        kind: SymKind::Func,
                        name: name.to_string(),
                        line: n,
                        detail: first.to_string(),
                    });
                }
            }
            _ => sv_ports(code, n, &mut out),
        }
    }
    out
}

fn vhdl_symbols(text: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    for (n, raw) in text.split('\n').enumerate() {
        let code = raw.split("--").next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let lower = code.to_ascii_lowercase();
        let words = idents(&lower);
        let orig = idents(code);
        let at = |w: &str| words.iter().position(|x| *x == w);
        if words.first() == Some(&"entity") && words.get(2) == Some(&"is") {
            out.push(Symbol {
                kind: SymKind::Entity,
                name: orig[1].to_string(),
                line: n,
                detail: "entity".into(),
            });
            continue;
        }
        if words.first() == Some(&"architecture")
            && let (Some(a), Some(e)) = (orig.get(1), orig.get(3))
        {
            out.push(Symbol {
                kind: SymKind::Arch,
                name: format!("{a} of {e}"),
                line: n,
                detail: "architecture".into(),
            });
            continue;
        }
        if let Some(p) = at("process")
            && (p == 0 || words[p - 1] != "end")
        {
            let label = (p >= 1 && lower.contains(':'))
                .then(|| orig[0].to_string())
                .filter(|l| !l.eq_ignore_ascii_case("process"));
            let sens = code
                .find('(')
                .map(|i| code[i..].split(')').next().unwrap_or("").to_string() + ")")
                .unwrap_or_default();
            out.push(Symbol {
                kind: SymKind::Process,
                name: label.unwrap_or_else(|| format!("process{sens}")),
                line: n,
                detail: "process".into(),
            });
            continue;
        }
        if matches!(
            words.first(),
            Some(&"function" | &"procedure" | &"impure" | &"pure")
        ) {
            let kw = at("function").or(at("procedure")).unwrap_or(0);
            if let Some(name) = orig.get(kw + 1) {
                out.push(Symbol {
                    kind: SymKind::Func,
                    name: name.to_string(),
                    line: n,
                    detail: words[kw].to_string(),
                });
            }
            continue;
        }
        // `a, b : in std_logic` inside a port clause.
        if let Some((names, rest)) = code.split_once(':') {
            let rest_l = rest.trim_start().to_ascii_lowercase();
            let dir = ["inout", "in", "out", "buffer"]
                .into_iter()
                .find(|d| rest_l.starts_with(&format!("{d} ")));
            if let Some(dir) = dir {
                let names = names.trim_start_matches(|c: char| c == '(' || c.is_whitespace());
                let names = names.rsplit_once('(').map(|(_, r)| r).unwrap_or(names);
                for name in names.split(',').map(str::trim).filter(|x| !x.is_empty()) {
                    if name.chars().all(is_ident_char) && !name.eq_ignore_ascii_case("port") {
                        out.push(Symbol {
                            kind: SymKind::Port,
                            name: name.to_string(),
                            line: n,
                            detail: dir.to_string(),
                        });
                    }
                }
            }
        }
    }
    out
}

/// Outline symbols of `text` in `dialect` (constraints have none).
pub fn symbols(text: &str, dialect: Dialect) -> Vec<Symbol> {
    match dialect {
        Dialect::Verilog => sv_symbols(text),
        Dialect::Vhdl => vhdl_symbols(text),
        Dialect::Tcl => Vec::new(),
    }
}

thread_local! {
    static CACHE: RefCell<Option<(u64, Dialect, Rc<Vec<Symbol>>)>> = const { RefCell::new(None) };
}

/// [`symbols`] of the live field, cached by text fingerprint.
pub fn cached(s: &AppState) -> Rc<Vec<Symbol>> {
    let dialect = crate::views::editor::active_dialect(s);
    let text = &s.editor.edit.text;
    let fp = crate::views::editor_scale::fingerprint(text);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((f, d, syms)) = c.as_ref()
            && *f == fp
            && *d == dialect
        {
            return syms.clone();
        }
        let syms = Rc::new(symbols(text, dialect));
        *c = Some((fp, dialect, syms.clone()));
        syms
    })
}

// ---- go to definition ----

/// The identifier under (or just before) the caret.
pub fn word_at_caret(s: &AppState) -> Option<String> {
    let text = &s.editor.edit.text;
    let at = s.editor.edit.focus.min(text.len());
    let start = text[..at]
        .rfind(|c: char| !is_ident_char(c))
        .map(|i| i + 1)
        .unwrap_or(0);
    let end = text[at..]
        .find(|c: char| !is_ident_char(c))
        .map(|i| at + i)
        .unwrap_or(text.len());
    let w = &text[start..end];
    (!w.is_empty() && !w.starts_with(|c: char| c.is_ascii_digit())).then(|| w.to_string())
}

fn declaration_line(s: &AppState, name: &str) -> Option<usize> {
    let vhdl = crate::views::editor::active_dialect(s) == Dialect::Vhdl;
    let same = |a: &str| {
        if vhdl {
            a.eq_ignore_ascii_case(name)
        } else {
            a == name
        }
    };
    let syms = cached(s);
    syms.iter()
        .filter(|x| same(&x.name))
        .min_by_key(|x| (x.kind.rank(), x.line))
        .map(|x| x.line)
}

/// Jump to the definition of the word under the caret. Returns where it
/// went (`file:line`) or why it could not.
pub fn goto_definition(s: &mut AppState, tx: Option<&mut Tx>) -> Result<String, String> {
    crate::views::editor::sync_edit(s);
    let word = word_at_caret(s).ok_or("Put the caret on a module or symbol name.")?;
    let here = s.editor.tabs.get(s.editor.active).cloned();
    // A module/entity some project file declares: open it there.
    if let Some(i) = s.project_files.defining(&word) {
        let file = s.project_files.files[i].name.clone();
        if here.as_deref() != Some(file.as_str()) {
            crate::project::open_index(s, i);
        }
        if let Some(line) = declaration_line(s, &word) {
            jump(s, line, tx);
            return Ok(format!("{file}:{}", line + 1));
        }
    }
    match declaration_line(s, &word) {
        Some(line) => {
            jump(s, line, tx);
            Ok(format!("{}:{}", here.unwrap_or_default(), line + 1))
        }
        None => Err(format!("No definition found for `{word}`.")),
    }
}

/// F12 / toolbar entry: jump or toast why not.
pub fn goto_definition_ui(s: &mut AppState, tx: &mut Tx) {
    match goto_definition(s, Some(tx)) {
        Ok(at) => s.console.lines.push(crate::state::ConsoleLine {
            text: format!("goto_definition {at}"),
            sev: crate::state::MsgSev::Info,
        }),
        Err(e) => s.toasts.push(ToastKind::Info, "Go to Definition", &e),
    }
}

/// Park the caret on `line` and scroll it into view.
pub fn jump(s: &mut AppState, line: usize, tx: Option<&mut Tx>) {
    crate::views::editor::move_cursor_to(s, line);
    crate::views::editor_scale::park_line_edit(s);
    if let Some(tx) = tx {
        tx.scroll_into_view(crate::views::editor_scale::line_key(s, line));
    }
}

// ---- view ----

/// The outline side strip (header + virtual symbol rows).
pub fn outline_strip(s: &AppState, t: &VivadoTheme) -> El {
    let syms = cached(s);
    let caret = s.editor.cursor_line;
    // The symbol the caret sits in: the last one starting at or above it.
    let current = syms.iter().rposition(|x| x.line <= caret);
    let n = syms.len();
    let header = row()
        .key("editor-outline-head")
        .w_full()
        .h(24.0)
        .items_center()
        .gap(6.0)
        .px(8.0)
        .fill(t.header_bg)
        .child(
            text("Outline")
                .font(VivadoTheme::FONT_TITLE)
                .color(t.text)
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            text(format!("{n}"))
                .key("editor-outline-count")
                .font(VivadoTheme::FONT_MONO.mono())
                .color(t.text_dim),
        )
        .child(
            el().key("editor-outline-close")
                .role("button")
                .label("Close outline")
                .focusable(true)
                .size(18.0, 18.0)
                .center()
                .radius(3.0)
                .hover(t.selection)
                .on_press(|s: &mut AppState| s.editor.outline_open = false)
                .child(
                    icon(&rune_kit::glyphs::CLOSE, 9.0)
                        .color(t.text_dim)
                        .a11y_hidden(),
                ),
        );
    let body = if n == 0 {
        col()
            .key("editor-outline-empty")
            .w_full()
            .px(8.0)
            .py(8.0)
            .child(
                text(match crate::views::editor::active_dialect(s) {
                    Dialect::Tcl => "Constraint files have no outline.",
                    _ => "No modules, ports or blocks found.",
                })
                .font(VivadoTheme::FONT_UI)
                .color(t.text_dim)
                .wrap(),
            )
    } else {
        let tt = t.clone();
        let rows_syms = syms.clone();
        col()
            .key("editor-outline-scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .key("editor-outline-rows")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        n,
                        SYM_ROW_H,
                        |_: &AppState, i| Cow::Owned(format!("editor-sym-{i}")),
                        move |_: &AppState, i| sym_row(&rows_syms[i], i, current == Some(i), &tt),
                    )
                    .virtual_rows_version(0u8),
            )
    };
    col()
        .key("editor-outline")
        .w(OUTLINE_W)
        .h_full()
        .min_h(0.0)
        .fill(t.panel_bg)
        .border_sides([0.0, 0.0, 0.0, 1.0], t.border)
        .child(header)
        .child(body)
}

fn sym_row(sym: &Symbol, i: usize, current: bool, t: &VivadoTheme) -> El {
    let line = sym.line;
    let tip = format!(
        "{} {} ({}) · line {}",
        sym.kind.badge(),
        sym.name,
        sym.detail,
        line + 1
    );
    let indent = if matches!(sym.kind, SymKind::Module | SymKind::Entity | SymKind::Arch) {
        0.0
    } else {
        10.0
    };
    row()
        .key(format!("editor-sym-{i}"))
        .role("button")
        .label(format!("Go to {} line {}", sym.name, line + 1))
        .focus_selected(current)
        .focusable(true)
        .w_full()
        .h(SYM_ROW_H)
        .items_center()
        .gap(6.0)
        .pl(6.0 + indent)
        .pr(8.0)
        .fill(if current { t.selection } else { t.panel_bg })
        .hover(t.selection)
        .help(tip)
        .on_press_tx(move |s: &mut AppState, tx: &mut Tx| jump(s, line, Some(tx)))
        .child(
            el().w(34.0).child(
                text(sym.kind.badge())
                    .font(crate::views::cad::FONT_PIN.mono())
                    .color(sym.kind.ink(t)),
            ),
        )
        .child(
            text(sym.name.clone())
                .font(VivadoTheme::FONT_MONO.mono())
                .color(if current { t.text } else { t.text_dim })
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        )
        .child(
            text(format!("{}", line + 1))
                .font(crate::views::cad::FONT_PIN.mono())
                .color(t.text_faint),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(syms: &[Symbol]) -> Vec<(SymKind, &str, usize)> {
        syms.iter()
            .map(|x| (x.kind, x.name.as_str(), x.line))
            .collect()
    }

    #[test]
    fn sv_outline_finds_modules_ports_blocks_and_functions() {
        let src = "// module fake\n\
module top #(parameter W = 4) (\n\
    input  logic clk, rst,\n\
    input  logic [W-1:0] d,\n\
    output logic [3:0] q\n\
);\n\
/* always_ff ignored\n   inside a comment */\n\
  always_ff @(posedge clk) begin : regs\n\
    q <= d;\n\
  end\n\
  always_comb x = 1;\n\
  function automatic logic [3:0] inc(input logic [3:0] a);\n\
    return a + 1;\n\
  endfunction\n\
  task pulse;\n\
  endtask\n\
endmodule\n\
module leaf(input a, output b); assign b = a; endmodule\n";
        let syms = symbols(src, Dialect::Verilog);
        let k = kinds(&syms);
        assert!(k.contains(&(SymKind::Module, "top", 1)), "{k:?}");
        for (name, line) in [("clk", 2), ("rst", 2), ("d", 3), ("q", 4)] {
            assert!(k.contains(&(SymKind::Port, name, line)), "{name}: {k:?}");
        }
        assert!(k.contains(&(SymKind::Block, "regs", 8)), "{k:?}");
        assert!(
            k.iter().any(|(kd, n, l)| *kd == SymKind::Block
                && n.starts_with("always_comb")
                && *l == 11)
        );
        assert!(k.contains(&(SymKind::Func, "inc", 12)), "{k:?}");
        assert!(k.contains(&(SymKind::Func, "pulse", 15)), "{k:?}");
        assert!(k.contains(&(SymKind::Module, "leaf", 18)), "{k:?}");
        assert!(k.contains(&(SymKind::Port, "a", 18)) && k.contains(&(SymKind::Port, "b", 18)));
        assert!(
            !k.iter().any(|(_, n, _)| *n == "fake"),
            "comments are skipped"
        );
        assert!(
            !k.iter().any(|(_, _, l)| *l == 6 || *l == 7),
            "block comment skipped"
        );
    }

    #[test]
    fn vhdl_outline_finds_entities_ports_architectures_and_processes() {
        let src = "library ieee;\n\
entity blinky is\n\
  port ( clk, rst : in std_logic;\n\
         led : out std_logic );\n\
end entity;\n\
architecture rtl of blinky is\n\
  signal s : std_logic;\n\
begin\n\
  tick : process (clk)\n\
  begin\n\
  end process;\n\
  process (rst) begin end process;\n\
end rtl;\n";
        let k = kinds(&symbols(src, Dialect::Vhdl))
            .into_iter()
            .map(|(a, b, c)| (a, b.to_string(), c))
            .collect::<Vec<_>>();
        assert!(k.contains(&(SymKind::Entity, "blinky".into(), 1)), "{k:?}");
        assert!(k.contains(&(SymKind::Port, "clk".into(), 2)), "{k:?}");
        assert!(k.contains(&(SymKind::Port, "rst".into(), 2)), "{k:?}");
        assert!(k.contains(&(SymKind::Port, "led".into(), 3)), "{k:?}");
        assert!(
            k.contains(&(SymKind::Arch, "rtl of blinky".into(), 5)),
            "{k:?}"
        );
        assert!(k.contains(&(SymKind::Process, "tick".into(), 8)), "{k:?}");
        assert!(
            k.iter()
                .any(|(kd, _, l)| *kd == SymKind::Process && *l == 11),
            "{k:?}"
        );
        assert!(!k.iter().any(|(_, n, _)| n == "s"), "signals are not ports");
    }

    #[test]
    fn go_to_definition_opens_the_declaring_project_file() {
        let mut s = crate::init();
        // A second in-memory design file declaring `leaf`, used by the top.
        let leaf = "module leaf(input a, output b);\n  assign b = a;\nendmodule\n";
        s.project_files.add(crate::project::ProjectFile::in_memory(
            "leaf.v",
            crate::project::FileSet::Design,
            leaf,
        ));
        s.editor.parked.insert(
            "leaf.v".into(),
            crate::state::ParkedBuffer {
                lines: leaf.lines().map(str::to_string).collect(),
                ..Default::default()
            },
        );
        crate::views::editor::sync_edit(&mut s);
        let mut text = s.editor.edit.text.clone();
        text.push_str("\nleaf u_leaf(.a(clk), .b());\n");
        s.editor.edit = rune::TextEdit::new(text.clone());
        let at = text.find("leaf u_leaf").unwrap() + 2;
        s.editor.edit.set_caret(at);
        assert_eq!(word_at_caret(&s).as_deref(), Some("leaf"));
        let went = goto_definition(&mut s, None).expect("definition");
        assert_eq!(went, "leaf.v:1");
        assert_eq!(s.editor.tabs[s.editor.active], "leaf.v");
        assert_eq!(s.editor.cursor_line, 0);
        // A word with no declaration is an honest miss.
        s.editor
            .edit
            .set_caret(s.editor.edit.text.find("assign").unwrap() + 1);
        assert!(goto_definition(&mut s, None).is_err());
        // A port of the open file resolves in place.
        s.editor
            .edit
            .set_caret(s.editor.edit.text.find("b = a").unwrap());
        assert_eq!(goto_definition(&mut s, None).unwrap(), "leaf.v:1");
    }
}
