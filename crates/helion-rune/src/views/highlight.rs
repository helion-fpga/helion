//! Editor syntax highlighting for the three languages a Helion project
//! holds: Verilog/SystemVerilog, VHDL, and Tcl-flavored constraints
//! (XDC/SDC).
//!
//! [`highlight`] lexes a whole buffer so constructs that span lines (SV
//! `/* … */` block comments) color correctly; [`highlight_line`] is the
//! single-line form used by tests and line-oriented callers. Spans are UTF-8
//! byte ranges whose boundaries always sit on ASCII bytes.

use rune::Span;

use crate::theme::VivadoTheme;

/// Source language of an editor buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Verilog,
    Vhdl,
    Tcl,
}

impl Dialect {
    /// Dialect for a tab/file name (constraints are Tcl; `.vhd(l)` is VHDL;
    /// everything else is Verilog/SystemVerilog).
    pub fn for_name(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        if n.ends_with(".xdc") || n.ends_with(".sdc") || n.ends_with(".tcl") {
            Dialect::Tcl
        } else if n.ends_with(".vhd") || n.ends_with(".vhdl") {
            Dialect::Vhdl
        } else {
            Dialect::Verilog
        }
    }

    /// Line-comment prefix used by the comment toggle.
    pub fn line_comment(self) -> &'static str {
        match self {
            Dialect::Verilog => "//",
            Dialect::Vhdl => "--",
            Dialect::Tcl => "#",
        }
    }
}

/// Verilog-2005 + SystemVerilog keywords.
const SV_KEYWORDS: &[&str] = &[
    "always", "always_comb", "always_ff", "always_latch", "and", "assert", "assign", "assume",
    "automatic", "begin", "break", "case", "casex", "casez", "class", "const", "continue",
    "cover", "default", "defparam", "disable", "do", "else", "end", "endcase", "endclass",
    "endfunction", "endgenerate", "endinterface", "endmodule", "endpackage", "endproperty",
    "endsequence", "endtask", "enum", "export", "extends", "final", "for", "foreach", "forever",
    "fork", "function", "generate", "genvar", "if", "iff", "import", "initial", "inout",
    "input", "inside", "interface", "join", "join_any", "join_none", "localparam", "modport",
    "module", "nand", "negedge", "new", "nor", "not", "null", "or", "output", "package",
    "packed", "parameter", "posedge", "priority", "property", "repeat", "return", "sequence",
    "signed", "static", "struct", "task", "this", "timeprecision", "timeunit", "typedef",
    "union", "unique", "unique0", "unsigned", "virtual", "wait", "while", "xnor", "xor",
];

/// Verilog/SV net and variable types (painted in the type ink).
const SV_TYPES: &[&str] = &[
    "bit", "byte", "event", "int", "integer", "logic", "longint", "real", "realtime", "reg",
    "shortint", "string", "supply0", "supply1", "time", "tri", "tri0", "tri1", "uwire", "void",
    "wand", "wire", "wor",
];

/// VHDL keywords (matched case-insensitively).
const VHDL_KEYWORDS: &[&str] = &[
    "abs", "after", "alias", "all", "and", "architecture", "array", "assert", "attribute",
    "begin", "block", "body", "buffer", "case", "component", "configuration", "constant",
    "downto", "else", "elsif", "end", "entity", "exit", "file", "for", "function", "generate",
    "generic", "if", "in", "inout", "is", "library", "loop", "map", "mod", "nand", "next",
    "nor", "not", "null", "of", "on", "open", "or", "others", "out", "package", "port",
    "procedure", "process", "range", "record", "rem", "report", "return", "select", "severity",
    "signal", "subtype", "then", "to", "type", "until", "use", "variable", "wait", "when",
    "while", "with", "xnor", "xor",
];

/// VHDL standard types and common functions (type ink).
const VHDL_TYPES: &[&str] = &[
    "bit", "bit_vector", "boolean", "character", "falling_edge", "integer", "natural",
    "positive", "real", "resize", "rising_edge", "signed", "std_logic", "std_logic_vector",
    "std_ulogic", "std_ulogic_vector", "string", "time", "to_integer", "to_signed",
    "to_unsigned", "unsigned",
];

/// SDC/XDC and core Tcl command words.
const TCL_KEYWORDS: &[&str] = &[
    "add_cells_to_pblock", "all_clocks", "all_inputs", "all_outputs", "all_registers",
    "create_clock", "create_generated_clock", "create_pblock", "current_design", "else",
    "foreach", "get_cells", "get_clocks", "get_iobanks", "get_nets", "get_pins", "get_ports",
    "if", "proc", "puts", "resize_pblock", "return", "set", "set_case_analysis",
    "set_clock_groups", "set_clock_latency", "set_clock_uncertainty", "set_disable_timing",
    "set_false_path", "set_input_delay", "set_input_jitter", "set_load", "set_max_delay",
    "set_min_delay", "set_multicycle_path", "set_output_delay", "set_property",
];

fn is_word_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn is_number_char(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' | b'x' | b'X' | b'z' | b'Z' | b'_' | b'\'' | b'.')
}

/// Whole-buffer spans (byte offsets into `text`).
pub fn highlight(text: &str, dialect: Dialect, t: &VivadoTheme) -> Vec<Span> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    let comment = |a: usize, b: usize| Span::new(a, b).color(t.code_comment).italic();
    let line_end = |from: usize| {
        bytes[from..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(bytes.len(), |p| from + p)
    };
    while i < bytes.len() {
        let b = bytes[i];
        let next = bytes.get(i + 1).copied();
        // Comments.
        match dialect {
            Dialect::Verilog if b == b'/' && next == Some(b'/') => {
                let e = line_end(i);
                spans.push(comment(i, e));
                i = e;
                continue;
            }
            Dialect::Verilog if b == b'/' && next == Some(b'*') => {
                let e = text[i + 2..].find("*/").map_or(bytes.len(), |p| i + 2 + p + 2);
                spans.push(comment(i, e));
                i = e;
                continue;
            }
            Dialect::Vhdl if b == b'-' && next == Some(b'-') => {
                let e = line_end(i);
                spans.push(comment(i, e));
                i = e;
                continue;
            }
            Dialect::Tcl
                if b == b'#'
                    && (i == 0 || bytes[i - 1].is_ascii_whitespace() || bytes[i - 1] == b';') =>
            {
                let e = line_end(i);
                spans.push(comment(i, e));
                i = e;
                continue;
            }
            _ => {}
        }
        // Strings (no newline crossing; an unterminated string ends the line).
        if b == b'"' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' && bytes[j] != b'\n' {
                if bytes[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            let e = (j + 1).min(bytes.len());
            spans.push(Span::new(i, e).color(t.code_string));
            i = e;
            continue;
        }
        // Verilog compiler directives (`timescale) and system tasks ($display).
        if dialect == Dialect::Verilog && (b == b'`' || b == b'$') {
            let mut j = i + 1;
            while j < bytes.len() && is_word_char(bytes[j]) {
                j += 1;
            }
            if j > i + 1 {
                let ink = if b == b'`' { t.code_keyword } else { t.code_type };
                spans.push(Span::new(i, j).color(ink));
            }
            i = j.max(i + 1);
            continue;
        }
        // Tcl options (-period, -name): dim like Vivado's Tcl editor.
        if dialect == Dialect::Tcl
            && b == b'-'
            && next.is_some_and(|c| c.is_ascii_alphabetic())
            && (i == 0 || bytes[i - 1].is_ascii_whitespace())
        {
            let mut j = i + 1;
            while j < bytes.len() && is_word_char(bytes[j]) {
                j += 1;
            }
            spans.push(Span::new(i, j).color(t.code_type));
            i = j;
            continue;
        }
        // Words.
        if is_word_start(b) {
            let mut j = i + 1;
            while j < bytes.len() && is_word_char(bytes[j]) {
                j += 1;
            }
            let word = &text[i..j];
            let ink = match dialect {
                Dialect::Verilog => {
                    if SV_KEYWORDS.contains(&word) {
                        Some(t.code_keyword)
                    } else if SV_TYPES.contains(&word) {
                        Some(t.code_type)
                    } else {
                        None
                    }
                }
                Dialect::Vhdl => {
                    let lower = word.to_ascii_lowercase();
                    if VHDL_KEYWORDS.contains(&lower.as_str()) {
                        Some(t.code_keyword)
                    } else if VHDL_TYPES.contains(&lower.as_str()) {
                        Some(t.code_type)
                    } else {
                        None
                    }
                }
                Dialect::Tcl => TCL_KEYWORDS.contains(&word).then_some(t.code_keyword),
            };
            if let Some(ink) = ink {
                spans.push(Span::new(i, j).color(ink));
            }
            i = j;
            continue;
        }
        // Numbers: plain, sized (24'd499_999), based (8'hFF) and decimals.
        if b.is_ascii_digit() || (b == b'\'' && next.is_some_and(|c| c.is_ascii_alphabetic())) {
            let mut j = i + 1;
            while j < bytes.len() && is_number_char(bytes[j]) {
                j += 1;
            }
            spans.push(Span::new(i, j).color(t.code_number));
            i = j;
            continue;
        }
        i += 1;
    }
    spans
}

/// One line's spans (offsets relative to the line).
pub fn highlight_line(line: &str, dialect: Dialect, t: &VivadoTheme) -> Vec<Span> {
    highlight(line, dialect, t)
}

/// One line's spans when the line may start inside an SV `/* … */` block
/// comment (`in_block`, from [`block_open_after`] over the lines above).
/// This is what lets a virtualized editor lex only the rows it shows.
pub fn highlight_line_from(line: &str, dialect: Dialect, in_block: bool, t: &VivadoTheme) -> Vec<Span> {
    if dialect != Dialect::Verilog || !in_block {
        return highlight(line, dialect, t);
    }
    let comment = |a: usize, b: usize| Span::new(a, b).color(t.code_comment).italic();
    match line.find("*/") {
        None => vec![comment(0, line.len())],
        Some(p) => {
            let e = p + 2;
            let mut out = vec![comment(0, e)];
            for sp in highlight(&line[e..], dialect, t) {
                let mut sp = sp;
                sp.start += e as u32;
                sp.end += e as u32;
                out.push(sp);
            }
            out
        }
    }
}

/// Whether an SV block comment is still open at the end of `line`, given
/// whether one was open at its start. Strings and `//` comments are skipped
/// the same way [`highlight`] skips them. Other dialects never carry state.
pub fn block_open_after(line: &str, dialect: Dialect, in_block: bool) -> bool {
    if dialect != Dialect::Verilog {
        return false;
    }
    let b = line.as_bytes();
    let mut open = in_block;
    let mut i = 0;
    while i < b.len() {
        if open {
            match line[i..].find("*/") {
                Some(p) => {
                    open = false;
                    i += p + 2;
                }
                None => return true,
            }
            continue;
        }
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => return false,
            b'/' if b.get(i + 1) == Some(&b'*') => {
                open = true;
                i += 2;
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;
    fn theme() -> VivadoTheme {
        VivadoTheme::light()
    }

    fn painted(text: &str, d: Dialect) -> Vec<(String, rune::Color)> {
        let t = theme();
        highlight(text, d, &t)
            .into_iter()
            .map(|s| (text[s.start as usize..s.end as usize].to_string(), s.color.unwrap()))
            .collect()
    }

    #[test]
    fn dialect_follows_the_extension() {
        assert_eq!(Dialect::for_name("top.sv"), Dialect::Verilog);
        assert_eq!(Dialect::for_name("pins.XDC"), Dialect::Tcl);
        assert_eq!(Dialect::for_name("clk.sdc"), Dialect::Tcl);
        assert_eq!(Dialect::for_name("alu.vhd"), Dialect::Vhdl);
        assert_eq!(Dialect::Vhdl.line_comment(), "--");
    }

    #[test]
    fn systemverilog_keywords_types_strings_and_system_tasks() {
        let t = theme();
        let p = painted(
            "always_ff @(posedge clk) logic x; $display(\"hi %d\", x);",
            Dialect::Verilog,
        );
        assert!(p.contains(&("always_ff".into(), t.code_keyword)));
        assert!(p.contains(&("posedge".into(), t.code_keyword)));
        assert!(p.contains(&("logic".into(), t.code_type)));
        assert!(p.contains(&("$display".into(), t.code_type)));
        assert!(p.contains(&("\"hi %d\"".into(), t.code_string)));
    }

    #[test]
    fn block_comments_span_lines() {
        let t = theme();
        let text = "a /* one\ntwo */ module";
        let p = painted(text, Dialect::Verilog);
        assert_eq!(p[0], ("/* one\ntwo */".into(), t.code_comment));
        assert!(p.contains(&("module".into(), t.code_keyword)));
    }

    #[test]
    fn vhdl_is_case_insensitive_with_dash_comments() {
        let t = theme();
        let p = painted("ENTITY blinky IS -- top\n signal s : std_logic;", Dialect::Vhdl);
        assert!(p.contains(&("ENTITY".into(), t.code_keyword)));
        assert!(p.contains(&("-- top".into(), t.code_comment)));
        assert!(p.contains(&("std_logic".into(), t.code_type)));
    }

    #[test]
    fn tcl_commands_options_and_comments() {
        let t = theme();
        let p = painted(
            "create_clock -period 10 [get_ports clk] # main",
            Dialect::Tcl,
        );
        assert!(p.contains(&("create_clock".into(), t.code_keyword)));
        assert!(p.contains(&("-period".into(), t.code_type)));
        assert!(p.contains(&("10".into(), t.code_number)));
        assert!(p.contains(&("# main".into(), t.code_comment)));
    }

    #[test]
    fn per_line_lexing_matches_the_whole_buffer() {
        let t = theme();
        let src = "wire a; /* open\n still comment\n done */ reg b; // x /*\nlogic c;";
        let whole = highlight(src, Dialect::Verilog, &t);
        let mut in_block = false;
        let mut off = 0u32;
        let mut per_line = Vec::new();
        for line in src.split('\n') {
            for mut sp in highlight_line_from(line, Dialect::Verilog, in_block, &t) {
                sp.start += off;
                sp.end += off;
                per_line.push((sp.start, sp.end, sp.color));
            }
            in_block = block_open_after(line, Dialect::Verilog, in_block);
            off += line.len() as u32 + 1;
        }
        // Every whole-buffer span is covered by the per-line spans (block
        // comments split at line ends, everything else identical).
        for sp in &whole {
            let covered = per_line
                .iter()
                .filter(|(a, b, c)| *a >= sp.start && *b <= sp.end && *c == sp.color)
                .map(|(a, b, _)| b - a)
                .sum::<u32>();
            let newlines = src[sp.start as usize..sp.end as usize].matches('\n').count() as u32;
            assert_eq!(covered + newlines, sp.end - sp.start, "span {:?}", (sp.start, sp.end));
        }
        assert!(!block_open_after("x /* a */ y", Dialect::Verilog, false));
        assert!(block_open_after("x /* a", Dialect::Verilog, false));
        assert!(!block_open_after("\"/*\" y", Dialect::Verilog, false));
        assert!(!block_open_after("-- /*", Dialect::Vhdl, false));
    }

    #[test]
    fn sized_literals_stay_one_number() {
        let t = theme();
        let p = painted("cnt <= 24'd499_999;", Dialect::Verilog);
        assert!(p.contains(&("24'd499_999".into(), t.code_number)));
    }
}
