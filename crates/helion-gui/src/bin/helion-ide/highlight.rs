//! Syntax highlighting for the Helion IDE text editor (P1-7).
//!
//! Hand-rolled single-pass tokenizer for SystemVerilog, VHDL, and Tcl: no
//! regex, no `.unwrap()`, O(n) in the input length. Spans are emitted
//! left-to-right and never overlap; gaps between spans paint the editor
//! default (`theme.text`).

use std::ops::Range;

use eframe::egui::Color32;

use super::theme::Theme;

/// Language detected from a file path / extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    SystemVerilog,
    Vhdl,
    Tcl,
    Unknown,
}

/// Classify by file extension.
///
/// `.sv` / `.svh` -> SystemVerilog, `.vhd` / `.vhdl` -> VHDL,
/// `.tcl` / `.xdc` / `.sdc` -> Tcl, everything else -> [`Lang::Unknown`]
/// (the editor then paints plain `theme.text`).
pub fn detect_lang(path: &str) -> Lang {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "sv" | "svh" => Lang::SystemVerilog,
        "vhd" | "vhdl" => Lang::Vhdl,
        "tcl" | "xdc" | "sdc" => Lang::Tcl,
        _ => Lang::Unknown,
    }
}

/// Highlight `text`, returning `(byte_range, color)` spans in order.
///
/// Token classes: keywords (per-language accent), `//` `--` `#` and `/* */`
/// comments, strings (incl. Tcl `{...}` groups), numbers (incl. `'h`/`'b`
/// literals), and preprocessor/compiler directives (`` ` `` in SV, `'attr` in
/// VHDL). [`Lang::Unknown`] (or empty input) returns an empty vec.
/// Complexity: O(n) in the input length; keyword lookup is a linear scan over
/// a small table per word (bounded, no allocation).
/// One highlighted span. `italic` is set for comments (dim italic per the
/// editor spec); everything else renders upright.
pub struct HlSpan {
    pub range: Range<usize>,
    pub color: Color32,
    pub italic: bool,
}

pub fn highlight(lang: Lang, text: &str, theme: &Theme) -> Vec<HlSpan> {
    if lang == Lang::Unknown || text.is_empty() {
        return Vec::new();
    }
    // Keyword hue is fixed per language so a file never mixes accents.
    let kw = match lang {
        Lang::SystemVerilog | Lang::Tcl => theme.accent_dim,
        Lang::Vhdl => theme.violet,
        Lang::Unknown => theme.text,
    };
    let mut s = Scan {
        b: text.as_bytes(),
        i: 0,
        out: Vec::new(),
        lang,
        kw,
        comment: theme.faint,
        string: theme.ok,
        number: theme.warn,
        directive: theme.info,
        plain: theme.text,
    };
    s.run();
    s.out
}

const SV_KEYWORDS: &[&str] = &[
    "module",
    "endmodule",
    "always",
    "always_ff",
    "always_comb",
    "always_latch",
    "logic",
    "reg",
    "wire",
    "tri",
    "input",
    "output",
    "inout",
    "parameter",
    "localparam",
    "specparam",
    "defparam",
    "assign",
    "deassign",
    "force",
    "release",
    "if",
    "else",
    "case",
    "endcase",
    "casex",
    "casez",
    "default",
    "for",
    "while",
    "do",
    "foreach",
    "repeat",
    "forever",
    "generate",
    "endgenerate",
    "genvar",
    "posedge",
    "negedge",
    "edge",
    "function",
    "endfunction",
    "task",
    "endtask",
    "begin",
    "end",
    "fork",
    "join",
    "join_any",
    "join_none",
    "initial",
    "import",
    "export",
    "package",
    "endpackage",
    "interface",
    "endinterface",
    "modport",
    "clocking",
    "endclocking",
    "typedef",
    "enum",
    "struct",
    "union",
    "class",
    "endclass",
    "extends",
    "virtual",
    "new",
    "null",
    "this",
    "super",
    "rand",
    "randc",
    "constraint",
    "covergroup",
    "endgroup",
    "property",
    "endproperty",
    "assert",
    "assume",
    "cover",
    "restrict",
    "int",
    "integer",
    "bit",
    "byte",
    "shortint",
    "longint",
    "time",
    "realtime",
    "real",
    "shortreal",
    "string",
    "event",
    "void",
    "const",
    "automatic",
    "static",
    "ref",
    "signed",
    "unsigned",
    "unique",
    "unique0",
    "priority",
    "inside",
    "with",
    "matches",
    "dist",
    "break",
    "continue",
    "return",
    "wait",
    "disable",
    "iff",
];

const VHDL_KEYWORDS: &[&str] = &[
    "entity",
    "architecture",
    "begin",
    "end",
    "signal",
    "process",
    "port",
    "map",
    "library",
    "use",
    "is",
    "of",
    "component",
    "generic",
    "constant",
    "variable",
    "type",
    "subtype",
    "array",
    "record",
    "file",
    "access",
    "function",
    "procedure",
    "return",
    "package",
    "body",
    "if",
    "then",
    "elsif",
    "else",
    "case",
    "when",
    "others",
    "for",
    "loop",
    "while",
    "generate",
    "and",
    "or",
    "not",
    "nand",
    "nor",
    "xor",
    "xnor",
    "in",
    "out",
    "inout",
    "buffer",
    "linkage",
    "downto",
    "to",
    "range",
    "reverse",
    "std_logic",
    "std_logic_vector",
    "std_ulogic",
    "std_ulogic_vector",
    "integer",
    "natural",
    "positive",
    "boolean",
    "bit",
    "bit_vector",
    "character",
    "string",
    "time",
    "real",
    "severity",
    "true",
    "false",
    "null",
    "wait",
    "until",
    "after",
    "transport",
    "inertial",
    "reject",
    "assert",
    "report",
    "note",
    "warning",
    "error",
    "failure",
    "next",
    "exit",
    "postponed",
    "pure",
    "impure",
    "shared",
    "protected",
    "new",
    "alias",
    "attribute",
    "abs",
    "mod",
    "rem",
    "sll",
    "srl",
    "sla",
    "sra",
    "rol",
    "ror",
    "select",
    "unaffected",
    "with",
    "on",
    "open",
    "configuration",
    "context",
    "block",
    "guarded",
];

const TCL_KEYWORDS: &[&str] = &[
    "proc",
    "set",
    "puts",
    "foreach",
    "if",
    "return",
    "else",
    "elseif",
    "while",
    "for",
    "break",
    "continue",
    "switch",
    "expr",
    "list",
    "lappend",
    "lindex",
    "llength",
    "lrange",
    "lsort",
    "lsearch",
    "lreplace",
    "linsert",
    "lassign",
    "lmap",
    "dict",
    "string",
    "namespace",
    "variable",
    "global",
    "upvar",
    "uplevel",
    "eval",
    "source",
    "package",
    "require",
    "catch",
    "try",
    "throw",
    "finally",
    "error",
    "incr",
    "append",
    "unset",
    "format",
    "subst",
    "regsub",
    "regexp",
    "split",
    "join",
    "concat",
    "after",
    "update",
    "vwait",
    "trace",
    "rename",
    "info",
    "array",
    "apply",
    "lambda",
    "coroutine",
    "yield",
    "tailcall",
    "on",
    "exit",
];

/// Byte-scanner state for one [`highlight`] call.
struct Scan<'a> {
    b: &'a [u8],
    i: usize,
    out: Vec<HlSpan>,
    lang: Lang,
    kw: Color32,
    comment: Color32,
    string: Color32,
    number: Color32,
    directive: Color32,
    plain: Color32,
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// ASCII case-insensitive table hit (VHDL keywords are case-insensitive).
fn eq_fold(word: &[u8], kw: &str) -> bool {
    word.len() == kw.len()
        && word
            .iter()
            .zip(kw.bytes())
            .all(|(a, b)| a.to_ascii_lowercase() == b)
}

impl Scan<'_> {
    fn push(&mut self, from: usize, to: usize, color: Color32, italic: bool) {
        if to > from {
            self.out.push(HlSpan {
                range: from..to,
                color,
                italic,
            });
        }
    }

    fn peek(&self, off: usize) -> u8 {
        self.b.get(self.i + off).copied().unwrap_or(0)
    }

    /// Full pass. Every branch advances `i`, so this terminates in O(n).
    fn run(&mut self) {
        while self.i < self.b.len() {
            self.step();
        }
    }

    /// One token; returns false only at EOF.
    fn step(&mut self) -> bool {
        if self.i >= self.b.len() {
            return false;
        }
        let c = self.b[self.i];
        if self.try_line_comment() || self.try_block_comment() {
            return true;
        }
        match c {
            b'"' => self.scan_string(),
            b'{' if self.lang == Lang::Tcl => self.scan_brace(),
            b'`' if self.lang == Lang::SystemVerilog => self.scan_directive(),
            b'\'' => self.scan_tick(),
            _ => {
                if c.is_ascii_digit() {
                    self.scan_number();
                } else if is_word_byte(c) {
                    self.scan_word();
                } else {
                    self.scan_punct();
                }
            }
        }
        true
    }

    fn try_line_comment(&mut self) -> bool {
        let (c0, c1) = (self.peek(0), self.peek(1));
        let is_comment = match self.lang {
            Lang::SystemVerilog => c0 == b'/' && c1 == b'/',
            Lang::Vhdl => c0 == b'-' && c1 == b'-',
            // `#` is a comment only where a command word can start.
            Lang::Tcl => {
                c0 == b'#' && {
                    let p = self.b.get(self.i.wrapping_sub(1)).copied().unwrap_or(b'\n');
                    p == b'\n' || p.is_ascii_whitespace() || matches!(p, b';' | b'{' | b'[')
                }
            }
            Lang::Unknown => false,
        };
        if !is_comment {
            return false;
        }
        let from = self.i;
        while self.i < self.b.len() && self.b[self.i] != b'\n' {
            self.i += 1;
        }
        self.push(from, self.i, self.comment, true);
        true
    }

    fn try_block_comment(&mut self) -> bool {
        if self.lang != Lang::SystemVerilog || self.peek(0) != b'/' || self.peek(1) != b'*' {
            return false;
        }
        let from = self.i;
        self.i += 2;
        while self.i + 1 < self.b.len() && !(self.b[self.i] == b'*' && self.b[self.i + 1] == b'/') {
            self.i += 1;
        }
        self.i = (self.i + 2).min(self.b.len());
        self.push(from, self.i, self.comment, true);
        true
    }

    fn scan_string(&mut self) {
        let from = self.i;
        self.i += 1;
        // Both SV `\"` escapes and VHDL `""` doubling terminate correctly.
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b'\\' {
                self.i += 2;
            } else if c == b'"' && self.peek(1) == b'"' {
                self.i += 2;
            } else if c == b'"' || c == b'\n' {
                self.i += usize::from(c == b'"');
                break;
            } else {
                self.i += 1;
            }
        }
        self.push(from, self.i.min(self.b.len()), self.string, false);
    }

    /// Tcl `{...}` group: braces nest; the group is lexed recursively so
    /// keywords inside proc bodies still highlight.
    fn scan_brace(&mut self) {
        self.push(self.i, self.i + 1, self.plain, false);
        self.i += 1;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == b'{' {
                self.scan_brace();
            } else if c == b'}' {
                self.push(self.i, self.i + 1, self.plain, false);
                self.i += 1;
                return;
            } else if c == b'\\' {
                self.i = (self.i + 2).min(self.b.len());
            } else if !self.step() {
                return;
            }
        }
    }

    /// SV `` `timescale `` / `` `define `` compiler directives.
    fn scan_directive(&mut self) {
        let from = self.i;
        self.i += 1;
        while self.i < self.b.len() && is_word_byte(self.b[self.i]) {
            self.i += 1;
        }
        self.push(from, self.i, self.directive, false);
    }

    /// `'` starts an SV base literal (`'hFF`), a VHDL attribute (`'event`),
    /// or plain punctuation.
    fn scan_tick(&mut self) {
        let n = self.peek(1);
        if self.lang == Lang::SystemVerilog
            && matches!(n, b'b' | b'B' | b'h' | b'H' | b'd' | b'D' | b'o' | b'O')
        {
            self.scan_based_number();
        } else if self.lang == Lang::Vhdl && n.is_ascii_alphabetic() {
            let from = self.i;
            self.i += 1;
            while self.i < self.b.len() && is_word_byte(self.b[self.i]) {
                self.i += 1;
            }
            self.push(from, self.i, self.directive, false);
        } else {
            self.push(self.i, self.i + 1, self.plain, false);
            self.i += 1;
        }
    }

    /// `'hFF`, `'b1010`, `'d42`: base letter then digits/`x`/`z`/`?`/`_`.
    fn scan_based_number(&mut self) {
        let from = self.i;
        self.i += 2;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c.is_ascii_hexdigit() || matches!(c, b'x' | b'X' | b'z' | b'Z' | b'?' | b'_') {
                self.i += 1;
            } else {
                break;
            }
        }
        self.push(from, self.i, self.number, false);
    }

    fn scan_number(&mut self) {
        let from = self.i;
        if self.peek(0) == b'0' && matches!(self.peek(1), b'x' | b'X') {
            self.i += 2;
        }
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c.is_ascii_hexdigit() || c == b'_' {
                self.i += 1;
            } else if c == b'.' && self.peek(1).is_ascii_digit() {
                self.i += 2;
                while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                    self.i += 1;
                }
            } else if matches!(c, b'e' | b'E')
                && (self.peek(1).is_ascii_digit() || matches!(self.peek(1), b'+' | b'-'))
            {
                self.i += 1;
                if matches!(self.peek(0), b'+' | b'-') {
                    self.i += 1;
                }
                while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
        self.push(from, self.i, self.number, false);
    }

    fn scan_word(&mut self) {
        let from = self.i;
        while self.i < self.b.len() && is_word_byte(self.b[self.i]) {
            self.i += 1;
        }
        let word = &self.b[from..self.i];
        let table = match self.lang {
            Lang::SystemVerilog => SV_KEYWORDS,
            Lang::Vhdl => VHDL_KEYWORDS,
            Lang::Tcl => TCL_KEYWORDS,
            Lang::Unknown => &[],
        };
        // Linear table scan per word; tables are small and words are short.
        let hit = table.iter().any(|k| {
            if self.lang == Lang::Vhdl {
                eq_fold(word, k)
            } else {
                word == k.as_bytes()
            }
        });
        if hit {
            self.push(from, self.i, self.kw, false);
        }
    }

    /// Maximal run of non-token bytes (whitespace, operators, `;`, ...).
    /// Never swallows a comment opener or token starter, and never stalls:
    /// a stray Tcl `}` at top level is emitted as its own span.
    fn scan_punct(&mut self) {
        if self.lang == Lang::Tcl && self.peek(0) == b'}' {
            self.push(self.i, self.i + 1, self.plain, false);
            self.i += 1;
            return;
        }
        let from = self.i;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if is_word_byte(c) || c.is_ascii_digit() || matches!(c, b'"' | b'\'' | b'`' | b'{') {
                break;
            }
            let (d0, d1) = (c, self.peek(1));
            let comment_opens =
                (d0 == b'/' && (d1 == b'/' || d1 == b'*')) || (d0 == b'-' && d1 == b'-');
            if comment_opens {
                break;
            }
            if c == b'}' && self.lang == Lang::Tcl {
                break;
            }
            self.i += 1;
        }
        self.push(from, self.i, self.plain, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::dark()
    }

    #[test]
    fn detect_by_extension() {
        assert_eq!(detect_lang("top.sv"), Lang::SystemVerilog);
        assert_eq!(detect_lang("pkg.svh"), Lang::SystemVerilog);
        assert_eq!(detect_lang("alu.VHD"), Lang::Vhdl);
        assert_eq!(detect_lang("board.vhdl"), Lang::Vhdl);
        assert_eq!(detect_lang("flow.tcl"), Lang::Tcl);
        assert_eq!(detect_lang("timing.xdc"), Lang::Tcl);
        assert_eq!(detect_lang("clk.sdc"), Lang::Tcl);
        assert_eq!(detect_lang("notes.txt"), Lang::Unknown);
        assert_eq!(detect_lang("Makefile"), Lang::Unknown);
    }

    #[test]
    fn sv_keywords_comments_strings_numbers() {
        let t = theme();
        let src =
            "module top; // comment\n  logic [7:0] cnt = 'hFF;\n  `timescale 1ns/1ps\nendmodule";
        let spans = highlight(Lang::SystemVerilog, src, &t);
        let at = |pos: usize| {
            spans.iter().find(|s| s.range.contains(&pos)).map(|s| s.color)
        };
        assert_eq!(
            at(src.find("module").unwrap_or(usize::MAX)),
            Some(t.accent_dim)
        );
        assert_eq!(at(src.find("//").unwrap_or(usize::MAX)), Some(t.faint));
        let italic_at = |pos: usize| {
            spans
                .iter()
                .find(|s| s.range.contains(&pos))
                .map(|s| s.italic)
        };
        assert_eq!(italic_at(src.find("//").unwrap_or(usize::MAX)), Some(true));
        assert_eq!(
            italic_at(src.find("module").unwrap_or(usize::MAX)),
            Some(false)
        );
        assert_eq!(at(src.find("'h").unwrap_or(usize::MAX)), Some(t.warn));
        assert_eq!(at(src.find('`').unwrap_or(usize::MAX)), Some(t.info));
        assert_eq!(
            at(src.find("endmodule").unwrap_or(usize::MAX)),
            Some(t.accent_dim)
        );
        // Spans are ordered and non-overlapping.
        for w in spans.windows(2) {
            assert!(w[0].range.end <= w[1].range.start);
        }
    }

    #[test]
    fn vhdl_case_insensitive_and_attributes() {
        let t = theme();
        let src = "ENTITY alu IS\n  PORT (clk : IN std_logic);\nEND alu;\n-- done\np: PROCESS\nBEGIN\n  IF rising_edge(clk) THEN\n  END IF;\nEND PROCESS;";
        let spans = highlight(Lang::Vhdl, src, &t);
        let color_at = |needle: &str| {
            let pos = src.find(needle).unwrap_or(usize::MAX);
            spans.iter().find(|s| s.range.contains(&pos)).map(|s| s.color)
        };
        assert_eq!(color_at("ENTITY"), Some(t.violet));
        assert_eq!(color_at("std_logic"), Some(t.violet));
        assert_eq!(color_at("--"), Some(t.faint));
        assert_eq!(color_at("PROCESS"), Some(t.violet));
    }

    #[test]
    fn tcl_hash_comments_and_braces() {
        let t = theme();
        let src = "# comment\nproc add {a b} {\n  return [expr {$a + $b}]\n}\nputs \"hi\"";
        let spans = highlight(Lang::Tcl, src, &t);
        let color_at = |needle: &str| {
            let pos = src.find(needle).unwrap_or(usize::MAX);
            spans.iter().find(|s| s.range.contains(&pos)).map(|s| s.color)
        };
        assert_eq!(color_at("#"), Some(t.faint));
        assert_eq!(color_at("proc"), Some(t.accent_dim));
        assert_eq!(color_at("return"), Some(t.accent_dim));
        assert_eq!(color_at("\"hi\""), Some(t.ok));
        for w in spans.windows(2) {
            assert!(w[0].range.end <= w[1].range.start);
        }
    }

    #[test]
    fn unknown_is_empty() {
        let t = theme();
        assert!(highlight(Lang::Unknown, "module x;", &t).is_empty());
        assert!(highlight(Lang::SystemVerilog, "", &t).is_empty());
    }
}
