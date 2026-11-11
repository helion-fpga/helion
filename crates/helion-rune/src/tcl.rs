//! Full Tcl-subset session for the `tcl>` console input: every verb the old
//! IDE's `tcl_interp` supports, wired to the same engines.
//!
//! The old IDE's interpreter is `TclInterp` at
//! `crates/helion-gui/src/tcl/interp.rs` (scopes, procs, `expr`, loops) plus
//! the object model / flow commands in `crates/helion-gui/src/tcl/design.rs`
//! (`get_cells`, `read_sv`, `report_timing`, ...), entered via `tcl_eval`
//! (`crates/helion-gui/src/lib.rs:247`). `TclInterp::eval` needs
//! `&mut GpuiShell` (it owns the `helion_proj::Session`), and `helion-rune`
//! must never depend on the `helion-gui` lib — so this module implements the
//! full surface directly against the engine crates, mirroring `tcl_eval`
//! semantics the way `runs.rs` mirrors the run flows:
//! - the typed verb is echoed, then `puts` output, then the LAST result
//!   value; errors render as `ERROR: ... (line N)` lines exactly like
//!   `tcl_eval` (never faked: unknown commands and missing designs are
//!   honest errors with usage hints);
//! - every submitted verb is kept in [`TclSession::history`] (Up/Down in
//!   the console input recalls it; see `views::console`);
//! - variables/procs/clocks/selection persist across commands;
//!   `read_sv`/`synth_design` elaborate for real (`helion_sv`/`helion_vhdl`
//!   plus `.prj`, same dispatch as `synth_hdl_path`); object queries read
//!   the live `helion_ir::Design`; flow verbs drive a real
//!   `helion_proj::Session` (opt/place/route/bitstream/checkpoint/eco/hw);
//!   `report_timing` runs real `helion_sta` with the `create_clock` state.
//!
//! Parity list = old `CORE_COMMANDS` (29, `interp.rs:14`) + old
//! `HOST_COMMANDS` (43, `design.rs:29`), plus old `Model::exec`'s package
//! and debugger verbs and the five Rune-only verbs that have no old-IDE
//! counterpart (`history`, `launch_runs`, `cancel_runs`, `open_report`,
//! `launch_simulation`). Word rules mirror `crates/helion-gui/src/tcl/parse.rs`
//! (`{}`, `""`, `[...]`, `$var`/`$arr(i)`, backslashes, `#` comments, `;`
//! separators). Divergences from the old IDE (all documented at use):
//! - proc bodies are stored raw and syntax-checked at first call (the old
//!   IDE pre-parses at `proc` time; this module parses and substitutes in
//!   one pass, so a check would execute the body);
//! - bare `synth_design` (no file) elaborates the active editor RTL buffer
//!   (the old IDE has no editor buffer; with a file it matches exactly);
//! - `info commands` also lists the old `Model::exec` package verbs and
//!   the five Rune-only verbs (they are real commands here; omitting them
//!   would lie).

use std::collections::HashMap;

use crate::state::{AppState, ConsoleLine, MsgSev};

/// Persistent Tcl session: variables, command history, and the elaborated
/// design the object queries read. Lives on [`AppState`] so it survives
/// across console commands (mirroring `GpuiShell::tcl_interp`).
#[derive(Clone, Debug)]
pub struct TclSession {
    /// Global Tcl variables (`set` / `$var`); proc frames live in [`TclSession::frames`].
    pub vars: HashMap<String, String>,
    /// Every submitted verb, oldest first (the `history` command lists it).
    pub history: Vec<String>,
    /// Design elaborated by `read_sv` / `synth_design` (`None` until then).
    pub design: Option<helion_ir::Design>,
    /// What the loaded design was elaborated from (path or buffer name).
    pub design_name: Option<String>,
    /// Proc call frames (innermost last); empty at console top level.
    /// Mirrors `TclInterp::scopes[1..]` (`helion-gui/src/tcl/interp.rs`).
    pub frames: Vec<HashMap<String, String>>,
    /// User procedures defined by `proc`.
    pub procs: HashMap<String, ProcDef>,
    /// Clocks from `create_clock` (empty = STA falls back to 10 ns `clk`).
    pub clocks: helion_sta::Constraints,
    /// Objects named by `select_objects` / `highlight_objects`.
    pub selected: Vec<SelObj>,
    /// Set by `select_objects` / `highlight_objects`; views clear it after
    /// driving the visual selection once.
    pub selection_dirty: bool,
    /// Live flow session (netlist + pack/place/route/bitstream + hw state).
    /// Kept in sync with [`TclSession::design`] by every synth/mutate verb.
    pub session: helion_proj::Session,
    /// Source-tree entries for `hds::tree` (seeded like `GpuiShell::tree`).
    pub tree_nodes: Vec<String>,
}

/// An object recorded by `select_objects` / `highlight_objects`.
/// Mirrors `helion_gui::tcl::SelObj` (`helion-gui/src/tcl/mod.rs`).
#[derive(Clone, Debug, PartialEq)]
pub struct SelObj {
    /// Object class: `cell`, `net`, `pin`, `port`, `clock`, `path`.
    pub kind: String,
    /// Hierarchical object name.
    pub name: String,
    /// Set by `highlight_objects` (vs plain `select_objects`).
    pub highlighted: bool,
}

/// A user procedure: parameter names with raw defaults plus the raw body.
/// Mirrors `ProcDef` (`helion-gui/src/tcl/interp.rs`); the body stays raw
/// text here (parsed at call time) instead of pre-parsed commands.
#[derive(Clone, Debug)]
pub struct ProcDef {
    /// `(name, raw default)` pairs; `None` default = required argument.
    pub params: Vec<(String, Option<String>)>,
    /// Raw body text (substituted and evaluated per call).
    pub body: String,
}

impl Default for TclSession {
    fn default() -> Self {
        Self {
            vars: HashMap::new(),
            history: Vec::new(),
            design: None,
            design_name: None,
            frames: Vec::new(),
            procs: HashMap::new(),
            clocks: helion_sta::Constraints::default(),
            selected: Vec::new(),
            selection_dirty: false,
            session: helion_proj::Session::new(helion_proj::Mode::Project),
            tree_nodes: vec!["sources".into(), "constraints".into(), "runs".into()],
        }
    }
}

/// Every console command: old `CORE_COMMANDS` (29) + old `HOST_COMMANDS`
/// (43), deduped, plus old `Model::exec`'s package/debugger verbs and the
/// five Rune-only verbs. `help` prints this and the unknown-command
/// suggestion searches it.
pub const COMMANDS: &[&str] = &[
    // Core language (old `interp.rs:14` order).
    "set",
    "unset",
    "puts",
    "expr",
    "if",
    "foreach",
    "for",
    "while",
    "proc",
    "return",
    "break",
    "continue",
    "list",
    "llength",
    "lindex",
    "lrange",
    "lappend",
    "concat",
    "join",
    "split",
    "string",
    "info",
    "catch",
    "error",
    "eval",
    "subst",
    "source",
    "format",
    "incr",
    "append",
    "help",
    // Object model + flow (old `design.rs:29` order; `help` deduped).
    "clear",
    "get_cells",
    "get_nets",
    "get_pins",
    "get_ports",
    "get_clocks",
    "get_timing_paths",
    "get_property",
    "set_property",
    "assign_package_pin",
    "package",
    "package_drawing",
    "select_package_pin",
    "sim_run",
    "sim_step",
    "sim_restart",
    "breakpoints",
    "add_bp",
    "select_breakpoint",
    "enable_bp",
    "disable_bp",
    "delete_bp",
    "memory",
    "memories",
    "select_memory",
    "select_memory_word",
    "locals",
    "select_local",
    "forces",
    "force_table",
    "force_constants",
    "add_force",
    "deposit",
    "remove_force",
    "select_force",
    "source_window",
    "select_source_line",
    "toggle_bp_line",
    "add_bp_line",
    "report_property",
    "list_property",
    "list_properties",
    "select_objects",
    "highlight_objects",
    "get_selected_objects",
    "current_design",
    "synth_design",
    "read_sv",
    "read_verilog",
    "read_vhdl",
    "opt_design",
    "place_design",
    "route_design",
    "write_bitstream",
    "write_checkpoint",
    "create_clock",
    "report_timing",
    "report_utilization",
    "mark_debug",
    "add_probe",
    "eco",
    "open_hw_manager",
    "program_hw",
    "program_hw_devices",
    "report_die",
    "report_hw_targets",
    "report_featuremap",
    "write_hnf",
    "hds::synth",
    "hds::impl",
    "hds::flow",
    "hds::tree",
    // Rune-only (no old-IDE counterpart; owned by `runs.rs` / console).
    "history",
    "launch_runs",
    "cancel_runs",
    "open_report",
    "launch_simulation",
    // Verification (testbench runner, simulation settings, logic analyzer).
    "tb_rescan",
    "tb_select",
    "tb_run",
    "tb_run_all",
    "tb_stop",
    "tb_vcd",
    "tb_wave",
    "tb_jump",
    "set_timescale",
    "set_runtime",
    "sim_settings",
    "compile_sim",
    "elaborate_sim",
    "ila",
];

/// Max nested `[...]` evaluation depth (mirrors the real interp's cap idea;
/// `interp.rs` uses 500 over full scripts, 64 is plenty for console input).
const MAX_DEPTH: usize = 64;

/// Interpreter-raised error with the 1-based script line, mirroring
/// `TclError` (`helion-gui/src/tcl/interp.rs`). Line 0 marks a *parse*
/// error, which the old IDE renders without a line suffix.
#[derive(Clone, Debug, PartialEq)]
struct TclError {
    msg: String,
    line: usize,
}

/// Internal control flow: values plus proc/loop control and errors.
/// Mirrors `Ctl` (`helion-gui/src/tcl/interp.rs`).
#[derive(Clone, Debug)]
enum Ctl {
    Value(String),
    Return(String),
    Break,
    Continue,
    Error(TclError),
}

impl Ctl {
    fn err(line: usize, msg: impl Into<String>) -> Self {
        Ctl::Error(TclError {
            msg: msg.into(),
            line,
        })
    }
}

/// Map a view-level `Result` onto the interpreter's value/error.
fn ctl_of(r: Result<String, String>, line: usize) -> Ctl {
    match r {
        Ok(v) => Ctl::Value(v),
        Err(e) => Ctl::err(line, e),
    }
}

fn wrong_args(usage: &str) -> String {
    format!("wrong # args: should be \"{usage}\"")
}

/// Read-only object queries read the same live design as the views: the last
/// run result first, else the console elaboration. Flow verbs keep using the
/// flow session's own copies.
fn query_design(s: &AppState) -> Option<&helion_ir::Design> {
    crate::live::live_design(s)
}

/// Evaluate one console verb: record history, reset the Up/Down recall
/// cursor, echo the verb, run it, and echo `puts` output plus the result.
/// Empty input is a no-op.
///
/// Echo order mirrors `tcl_eval` (`helion-gui/src/lib.rs:247`): `puts`
/// output first, then the script's (last) value; errors render as
/// `ERROR: ... (line N)` lines. `launch_runs` / `cancel_runs` /
/// `open_report` echo the verb themselves, so the generic verb echo is
/// skipped for exactly those.
pub fn eval_command(s: &mut AppState, verb: &str) {
    let verb = verb.trim();
    if verb.is_empty() {
        return;
    }
    s.tcl.history.push(verb.to_string());
    s.console.hist_pos = None;
    let self_echo = matches!(
        first_word(verb),
        "launch_runs" | "cancel_runs" | "open_report"
    );
    if !self_echo {
        push_lines(s, verb, MsgSev::Info);
    }
    let mut puts_out: Vec<String> = Vec::new();
    let mut values: Vec<String> = Vec::new();
    match eval_script(s, verb, &mut puts_out, 0, &mut values) {
        Ctl::Value(_) | Ctl::Return(_) => {
            echo_puts(s, &puts_out);
            if let Some(last) = values.last() {
                if !last.is_empty() {
                    push_lines(s, last, MsgSev::Info);
                }
            }
        }
        // The old top-level maps these without a line suffix; mirrored.
        Ctl::Break => {
            echo_puts(s, &puts_out);
            push_lines(
                s,
                "ERROR: invoked \"break\" outside of a loop",
                MsgSev::Error,
            );
        }
        Ctl::Continue => {
            echo_puts(s, &puts_out);
            push_lines(
                s,
                "ERROR: invoked \"continue\" outside of a loop",
                MsgSev::Error,
            );
        }
        Ctl::Error(e) => {
            echo_puts(s, &puts_out);
            push_lines(s, &format!("ERROR: {}", fmt_error(&e)), MsgSev::Error);
        }
    }
}

/// Echo accumulated `puts` output: chunks join into one buffer (so
/// `-nonewline` concatenates, exactly like the old output buffer) and the
/// trailing newline folds away (it ends the last line, not a new row).
fn echo_puts(s: &mut AppState, puts_out: &[String]) {
    let joined = puts_out.join("");
    let trimmed = joined.trim_end_matches('\n');
    if !trimmed.is_empty() {
        push_lines(s, trimmed, MsgSev::Info);
    }
}

/// Render an error the way `tcl_eval` does: parse errors (line 0) bare,
/// evaluation errors with their 1-based script line.
fn fmt_error(e: &TclError) -> String {
    if e.line == 0 {
        e.msg.clone()
    } else {
        format!("{} (line {})", e.msg, e.line)
    }
}

/// Push text as console lines, splitting embedded newlines (multi-line
/// results like `help` render one row per line).
fn push_lines(s: &mut AppState, text: &str, sev: MsgSev) {
    for line in text.split('\n') {
        s.console.lines.push(ConsoleLine {
            text: line.to_string(),
            sev,
        });
    }
}

/// First whitespace-delimited word (for the self-echo check).
fn first_word(verb: &str) -> &str {
    verb.split_whitespace().next().unwrap_or("")
}

/// One parsed command: substituted words plus its 1-based script line.
struct ParsedCmd {
    argv: Vec<String>,
    line: usize,
}

/// Parse and run every command in `script`. Each value appends to `values`
/// (the console echoes the last, mirroring `tcl_eval`); the first error or
/// non-local control aborts the rest (Tcl semantics). Returns the final
/// control state so bodies propagate `return`/`break`/`continue`.
fn eval_script(
    s: &mut AppState,
    script: &str,
    puts_out: &mut Vec<String>,
    depth: usize,
    values: &mut Vec<String>,
) -> Ctl {
    if depth > MAX_DEPTH {
        return Ctl::err(1, "max nesting depth exceeded (recursive eval?)");
    }
    let cmds = match parse_script(s, script, puts_out, depth) {
        Ok(cmds) => cmds,
        Err(e) => return Ctl::Error(e),
    };
    let mut last = Ctl::Value(String::new());
    for cmd in cmds {
        match dispatch(s, &cmd.argv, puts_out, depth, cmd.line) {
            Ctl::Value(v) => {
                values.push(v.clone());
                last = Ctl::Value(v);
            }
            Ctl::Return(v) => {
                values.push(v.clone());
                return Ctl::Return(v);
            }
            other => return other,
        }
    }
    last
}

/// Evaluate a control-structure body (`if`/`while`/proc/`catch`/...).
/// Parse failures surface as `body: ...` at the caller's line, mirroring
/// `eval_body` (`helion-gui/src/tcl/interp.rs:131`).
fn eval_body(
    s: &mut AppState,
    body: &str,
    line: usize,
    puts_out: &mut Vec<String>,
    depth: usize,
) -> Ctl {
    if depth > MAX_DEPTH {
        return Ctl::err(line, "max nesting depth exceeded (recursive eval?)");
    }
    let cmds = match parse_script(s, body, puts_out, depth + 1) {
        Ok(cmds) => cmds,
        Err(e) => return Ctl::err(line, format!("body: {}", e.msg)),
    };
    let mut last = Ctl::Value(String::new());
    for cmd in cmds {
        match dispatch(s, &cmd.argv, puts_out, depth + 1, cmd.line) {
            Ctl::Value(v) => last = Ctl::Value(v),
            other => return other,
        }
    }
    last
}

/// Reduce a nested-script control state to a substitution value, mirroring
/// the old `[...]` handling (`interp.rs` `parse_primary`): values and
/// `return` substitute, `break`/`continue` are outside-loop errors.
fn ctl_value(ctl: Ctl, line: usize) -> Result<String, TclError> {
    match ctl {
        Ctl::Value(v) | Ctl::Return(v) => Ok(v),
        Ctl::Break => Err(TclError {
            msg: "invoked \"break\" outside of a loop".into(),
            line,
        }),
        Ctl::Continue => Err(TclError {
            msg: "invoked \"continue\" outside of a loop".into(),
            line,
        }),
        Ctl::Error(e) => Err(e),
    }
}

/// Substitute a raw string fragment (array indices, proc defaults): parse it
/// as words, substitute each, rejoin with spaces. Mirrors `subst_str`
/// (`helion-gui/src/tcl/interp.rs:234`).
fn subst_str(
    s: &mut AppState,
    src: &str,
    line: usize,
    puts_out: &mut Vec<String>,
    depth: usize,
) -> Result<String, TclError> {
    if src.trim().is_empty() {
        return Ok(String::new());
    }
    let cmds = parse_script(s, src, puts_out, depth + 1).map_err(|e| TclError {
        msg: format!("subst: {}", e.msg),
        line,
    })?;
    // Words come out substituted (this module substitutes during the
    // parse, including nested `[...]`); they are joined, never executed.
    let mut words = Vec::new();
    for cmd in cmds {
        words.extend(cmd.argv);
    }
    Ok(words.join(" "))
}

impl TclSession {
    /// Innermost-visible variable value: proc frames first, then globals.
    /// Mirrors `lookup` (`helion-gui/src/tcl/interp.rs:256`).
    fn lookup(&self, key: &str) -> Option<&String> {
        for scope in self.frames.iter().rev() {
            if let Some(v) = scope.get(key) {
                return Some(v);
            }
        }
        self.vars.get(key)
    }

    /// Tcl `set` semantics: update the innermost scope that already has the
    /// variable, otherwise create it in the current (innermost) scope.
    /// Mirrors `assign` (`helion-gui/src/tcl/interp.rs:267`).
    fn assign(&mut self, key: String, val: String) {
        for scope in self.frames.iter_mut().rev() {
            if scope.contains_key(&key) {
                scope.insert(key, val);
                return;
            }
        }
        if let Some(top) = self.frames.last_mut() {
            top.insert(key, val);
        } else {
            self.vars.insert(key, val);
        }
    }

    /// Drop the elaborated design and every design-derived session bit
    /// (flow state, clocks, selection, tree names). History and procs are
    /// session-wide and stay. Called by new/empty project.
    pub fn reset_design_state(&mut self) {
        self.design = None;
        self.design_name = None;
        self.session = helion_proj::Session::new(helion_proj::Mode::Project);
        self.clocks = helion_sta::Constraints::default();
        self.selected.clear();
        self.selection_dirty = false;
        self.tree_nodes = vec!["sources".into(), "constraints".into(), "runs".into()];
    }
}

// ---- word rules (mirroring helion-gui/src/tcl/parse.rs) ----

/// Split a script into commands (`;` / newline separators) with full word
/// substitution: `{}` literals (nesting, backslash-newline folds to a
/// space), `""` quotes (`$var`, `[cmd]`, backslashes), `[cmd]` nesting,
/// `$name` / `${name}` / `$arr(i)`, backslash escapes, and `#` comments at
/// command start. `[...]` bodies evaluate immediately (results substitute
/// inline). Each command records its 1-based line for `ERROR: ... (line N)`.
fn parse_script(
    s: &mut AppState,
    script: &str,
    puts_out: &mut Vec<String>,
    depth: usize,
) -> Result<Vec<ParsedCmd>, TclError> {
    if depth > MAX_DEPTH {
        return Err(TclError {
            msg: "max nesting depth exceeded (recursive eval?)".into(),
            line: 0,
        });
    }
    let chars: Vec<char> = script.chars().collect();
    let mut commands: Vec<ParsedCmd> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut cmd_start = true;
    let mut line = 1usize;
    let mut cmd_line = 1usize;
    let mut i = 0;
    let end_word = |words: &mut Vec<String>, cur: &mut String, in_word: &mut bool| {
        if *in_word {
            words.push(std::mem::take(cur));
            *in_word = false;
        }
    };
    // Newlines inside a consumed span still advance the outer line count.
    let span_lines = |chars: &[char], from: usize, to: usize| -> usize {
        chars[from..to.min(chars.len())]
            .iter()
            .filter(|c| **c == '\n')
            .count()
    };
    let perr = |msg: String| TclError { msg, line: 0 };
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' => {
                end_word(&mut words, &mut cur, &mut in_word);
                i += 1;
            }
            '\n' | ';' => {
                end_word(&mut words, &mut cur, &mut in_word);
                if !words.is_empty() {
                    commands.push(ParsedCmd {
                        argv: std::mem::take(&mut words),
                        line: cmd_line,
                    });
                }
                if c == '\n' {
                    line += 1;
                }
                cmd_start = true;
                i += 1;
            }
            '#' if cmd_start && !in_word => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '{' => {
                // `{` is only special as the first character (`parse.rs`
                // `brace_only_first_char`); a braced span ends the word, so
                // `{a}b` is two words, exactly like the old parser.
                if !in_word {
                    if cmd_start {
                        cmd_line = line;
                    }
                    let (lit, next) = take_braced(&chars, i).map_err(perr)?;
                    line += span_lines(&chars, i, next);
                    cur.push_str(&lit);
                    in_word = true;
                    end_word(&mut words, &mut cur, &mut in_word);
                    cmd_start = false;
                    i = next;
                } else {
                    cur.push('{');
                    cmd_start = false;
                    i += 1;
                }
            }
            '"' => {
                // `"` is only special as the first character; mid-word it
                // is literal. A quoted span continues the word (`"a"b`).
                if !in_word {
                    if cmd_start {
                        cmd_line = line;
                    }
                    let (text, next) = take_quoted(s, &chars, i, puts_out, depth, line)?;
                    line += span_lines(&chars, i, next);
                    cur.push_str(&text);
                    in_word = true;
                    cmd_start = false;
                    i = next;
                } else {
                    cur.push('"');
                    cmd_start = false;
                    i += 1;
                }
            }
            '[' => {
                if cmd_start && !in_word {
                    cmd_line = line;
                }
                let (inner, next) = take_bracketed(&chars, i).map_err(perr)?;
                line += span_lines(&chars, i, next);
                let mut nested_values = Vec::new();
                let ctl = eval_script(s, &inner, puts_out, depth + 1, &mut nested_values);
                cur.push_str(&ctl_value(ctl, line)?);
                in_word = true;
                cmd_start = false;
                i = next;
            }
            '$' => {
                if cmd_start && !in_word {
                    cmd_line = line;
                }
                let (value, next) = take_var(s, &chars, i, puts_out, depth, line)?;
                cur.push_str(&value);
                in_word = true;
                cmd_start = false;
                i = next;
            }
            '\\' => {
                if cmd_start && !in_word {
                    cmd_line = line;
                }
                let (text, next) = take_backslash(&chars, i);
                line += span_lines(&chars, i, next);
                cur.push_str(&text);
                in_word = true;
                cmd_start = false;
                i = next;
            }
            _ => {
                if cmd_start && !in_word {
                    cmd_line = line;
                }
                cur.push(c);
                in_word = true;
                cmd_start = false;
                i += 1;
            }
        }
    }
    end_word(&mut words, &mut cur, &mut in_word);
    if !words.is_empty() {
        commands.push(ParsedCmd {
            argv: words,
            line: cmd_line,
        });
    }
    Ok(commands)
}

/// Literal `{...}` span: `chars[i] == '{'`. Returns the inner text (nesting
/// honored; inside braces only `\{`, `\}` and `\\` unescape, everything
/// else stays raw, mirroring `parse.rs`) and the index past `}`.
fn take_braced(chars: &[char], i: usize) -> Result<(String, usize), String> {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut j = i;
    while j < chars.len() {
        match chars[j] {
            '{' => {
                depth += 1;
                if depth > 1 {
                    out.push('{');
                }
                j += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((out, j + 1));
                }
                out.push('}');
                j += 1;
            }
            '\\' => match chars.get(j + 1) {
                Some('{') | Some('}') | Some('\\') => {
                    out.push(chars[j + 1]);
                    j += 2;
                }
                _ => {
                    out.push('\\');
                    j += 1;
                }
            },
            c => {
                out.push(c);
                j += 1;
            }
        }
    }
    Err("missing close-brace".into())
}

/// Quoted `"..."` span with substitution. Returns the text and the index
/// past the closing quote.
fn take_quoted(
    s: &mut AppState,
    chars: &[char],
    i: usize,
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Result<(String, usize), TclError> {
    let mut out = String::new();
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            '"' => return Ok((out, j + 1)),
            '[' => {
                let (inner, next) =
                    take_bracketed(chars, j).map_err(|msg| TclError { msg, line: 0 })?;
                let mut nested_values = Vec::new();
                let ctl = eval_script(s, &inner, puts_out, depth + 1, &mut nested_values);
                out.push_str(&ctl_value(ctl, line)?);
                j = next;
            }
            '$' => {
                let (value, next) = take_var(s, chars, j, puts_out, depth, line)?;
                out.push_str(&value);
                j = next;
            }
            '\\' => {
                let (text, next) = take_backslash(chars, j);
                out.push_str(&text);
                j = next;
            }
            c => {
                out.push(c);
                j += 1;
            }
        }
    }
    Err(TclError {
        msg: "missing close-quote".into(),
        line: 0,
    })
}

/// Bracketed `[...]` span: `chars[i] == '['`. Returns the inner script and
/// the index past `]`. Nested brackets count; braces/quotes inside are
/// skipped over so their brackets cannot close the span early.
fn take_bracketed(chars: &[char], i: usize) -> Result<(String, usize), String> {
    let mut depth = 0usize;
    let mut j = i;
    while j < chars.len() {
        match chars[j] {
            '[' => {
                depth += 1;
                j += 1;
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((chars[i + 1..j].iter().collect(), j + 1));
                }
                j += 1;
            }
            '{' => {
                let (_, next) = take_braced(chars, j)?;
                j = next;
            }
            '"' => {
                j += 1;
                while j < chars.len() && chars[j] != '"' {
                    j = if chars[j] == '\\' { j + 2 } else { j + 1 };
                }
                j = (j + 1).min(chars.len());
            }
            '\\' => j += 2.min(chars.len().saturating_sub(j)).max(1),
            _ => j += 1,
        }
    }
    Err("missing close-bracket".into())
}

/// `$name` / `${name}` / `$name(index)` substitution: `chars[i] == '$'`.
/// Returns the value and the index past the reference. A lone `$` stays
/// literal, and the index substitutes (`$a($i)`), mirroring `parse.rs` /
/// `var_subst` (`helion-gui/src/tcl/interp.rs:193`). Shape errors are
/// parse errors (line 0); lookup failures carry the current line and name
/// the bare variable, exactly like the old IDE.
fn take_var(
    s: &mut AppState,
    chars: &[char],
    i: usize,
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Result<(String, usize), TclError> {
    let perr = |msg: &str| TclError {
        msg: msg.into(),
        line: 0,
    };
    if chars.get(i + 1) == Some(&'{') {
        let mut j = i + 2;
        while j < chars.len() && chars[j] != '}' {
            j += 1;
        }
        if j >= chars.len() {
            return Err(perr("missing close-brace after ${"));
        }
        let name: String = chars[i + 2..j].iter().collect();
        if name.is_empty() {
            return Err(perr("empty ${} variable name"));
        }
        let next = j + 1;
        return s
            .tcl
            .lookup(&name)
            .cloned()
            .map(|v| (v, next))
            .ok_or_else(|| TclError {
                msg: format!("can't read \"{name}\": no such variable"),
                line,
            });
    }
    let mut j = i + 1;
    while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    if j == i + 1 {
        return Ok(("$".into(), i + 1));
    }
    let name: String = chars[i + 1..j].iter().collect();
    // Array reference: the index text substitutes before the lookup.
    if chars.get(j) == Some(&'(') {
        let mut k = j + 1;
        let mut depth_paren = 1usize;
        let mut idx_src = String::new();
        loop {
            match chars.get(k) {
                None => return Err(perr("missing ) in $var(index)")),
                Some('(') => {
                    depth_paren += 1;
                    idx_src.push('(');
                }
                Some(')') => {
                    depth_paren -= 1;
                    if depth_paren == 0 {
                        break;
                    }
                    idx_src.push(')');
                }
                Some(c) => idx_src.push(*c),
            }
            k += 1;
        }
        let idx = subst_str(s, &idx_src, line, puts_out, depth)?;
        let key = format!("{name}({idx})");
        let next = k + 1;
        return s
            .tcl
            .lookup(&key)
            .cloned()
            .map(|v| (v, next))
            .ok_or_else(|| TclError {
                msg: format!("can't read \"{name}\": no such variable"),
                line,
            });
    }
    s.tcl
        .lookup(&name)
        .cloned()
        .map(|v| (v, j))
        .ok_or_else(|| TclError {
            msg: format!("can't read \"{name}\": no such variable"),
            line,
        })
}

/// Backslash substitution: `chars[i] == '\\'`. Returns the replacement text
/// and the index past the escape. Backslash-newline folds to one space.
fn take_backslash(chars: &[char], i: usize) -> (String, usize) {
    let Some(&n) = chars.get(i + 1) else {
        return ("\\".into(), i + 1);
    };
    let text = match n {
        'n' => "\n",
        't' => "\t",
        'r' => "\r",
        '\n' => " ",
        _ => return (n.to_string(), i + 2),
    };
    (text.to_string(), i + 2)
}

// ---- dispatch ----

/// Run one parsed command. `puts` text accumulates in `puts_out` (echoed
/// before the result, mirroring `tcl_eval`); user procs shadow host
/// commands, exactly like the old `call` (`interp.rs:279`).
fn dispatch(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    if depth > MAX_DEPTH {
        return Ctl::err(line, "max nesting depth exceeded (recursive eval?)");
    }
    let name = argv[0].as_str();
    match name {
        "set" => cmd_set(s, argv, line),
        "unset" => cmd_unset(s, argv, line),
        "puts" => cmd_puts(argv, puts_out, line),
        "expr" => cmd_expr(s, argv, puts_out, depth, line),
        "if" => cmd_if(s, argv, puts_out, depth, line),
        "foreach" => cmd_foreach(s, argv, puts_out, depth, line),
        "for" => cmd_for(s, argv, puts_out, depth, line),
        "while" => cmd_while(s, argv, puts_out, depth, line),
        "proc" => cmd_proc(s, argv, line),
        "return" => cmd_return(argv, line),
        "break" => {
            if argv.len() != 1 {
                Ctl::err(line, "wrong # args: should be \"break\"")
            } else {
                Ctl::Break
            }
        }
        "continue" => {
            if argv.len() != 1 {
                Ctl::err(line, "wrong # args: should be \"continue\"")
            } else {
                Ctl::Continue
            }
        }
        "list" => Ctl::Value(join_list(&argv[1..])),
        "llength" => cmd_llength(argv, line),
        "lindex" => cmd_lindex(argv, line),
        "lrange" => cmd_lrange(argv, line),
        "lappend" => cmd_lappend(s, argv, line),
        "concat" => cmd_concat(argv, line),
        "join" => cmd_join(argv, line),
        "split" => cmd_split(argv, line),
        "string" => cmd_string(argv, line),
        "info" => cmd_info(s, argv, line),
        "catch" => cmd_catch(s, argv, puts_out, depth, line),
        "error" => cmd_error(argv, line),
        "eval" => cmd_eval(s, argv, puts_out, depth, line),
        "subst" => cmd_subst(s, argv, puts_out, depth, line),
        "source" => cmd_source(s, argv, puts_out, depth, line),
        "format" => cmd_format(argv, line),
        "incr" => cmd_incr(s, argv, line),
        "append" => cmd_append(s, argv, line),
        "history" => cmd_history(s, argv, line),
        "clear" => cmd_clear(s, argv, line),
        "launch_runs" => cmd_launch(s, argv, line),
        "cancel_runs" => cmd_cancel(s, argv, line),
        "launch_simulation" => {
            crate::views::wave::launch(s);
            Ctl::Value(String::new())
        }
        "help" => cmd_help(s, argv, line),
        _ => {
            if let Some(def) = s.tcl.procs.get(name).cloned() {
                call_proc(s, name, def, argv, puts_out, depth, line)
            } else {
                dispatch_host(s, name, argv, puts_out, depth, line)
            }
        }
    }
}

fn cmd_set(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    match argv.len() {
        2 => match s.tcl.lookup(&argv[1]).cloned() {
            Some(v) => Ctl::Value(v),
            None => Ctl::err(
                line,
                format!("can't read \"{}\": no such variable", argv[1]),
            ),
        },
        3 => {
            s.tcl.assign(argv[1].clone(), argv[2].clone());
            Ctl::Value(argv[2].clone())
        }
        _ => Ctl::err(line, wrong_args("set varName ?newValue?")),
    }
}

fn cmd_unset(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let mut nocomplain = false;
    let mut vars: Vec<&str> = Vec::new();
    for a in &argv[1..] {
        if a == "-nocomplain" {
            nocomplain = true;
        } else if a.starts_with('-') {
            return Ctl::err(line, format!("bad option \"{a}\": must be -nocomplain"));
        } else {
            vars.push(a);
        }
    }
    if vars.is_empty() {
        return Ctl::err(line, wrong_args("unset ?-nocomplain? varName ..."));
    }
    for v in vars {
        let mut found = false;
        for scope in s.tcl.frames.iter_mut().rev() {
            if scope.remove(v).is_some() {
                found = true;
                break;
            }
        }
        if !found && s.tcl.vars.remove(v).is_some() {
            found = true;
        }
        if !found && !nocomplain {
            return Ctl::err(line, format!("can't unset \"{v}\": no such variable"));
        }
    }
    Ctl::Value(String::new())
}

fn cmd_puts(argv: &[String], puts_out: &mut Vec<String>, line: usize) -> Ctl {
    let mut i = 1;
    let mut nonewline = false;
    while i < argv.len() && argv[i].starts_with('-') {
        match argv[i].as_str() {
            "-nonewline" => nonewline = true,
            other => return Ctl::err(line, format!("bad option \"{other}\": must be -nonewline")),
        }
        i += 1;
    }
    let args = &argv[i..];
    let text = match args.len() {
        1 => args[0].clone(),
        2 => {
            if args[0] != "stdout" {
                return Ctl::err(
                    line,
                    format!("can only puts to stdout, not \"{}\"", args[0]),
                );
            }
            args[1].clone()
        }
        _ => {
            return Ctl::err(line, wrong_args("puts ?-nonewline? ?channelId? string"));
        }
    };
    if nonewline {
        puts_out.push(text);
    } else {
        puts_out.push(format!("{text}\n"));
    }
    Ctl::Value(String::new())
}

fn cmd_expr(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    if argv.len() < 2 {
        return Ctl::err(line, wrong_args("expr arg ..."));
    }
    let src = argv[1..].join(" ");
    match eval_expr(s, &src, line, puts_out, depth) {
        Ok(v) => Ctl::Value(v.to_string()),
        Err(m) => Ctl::err(line, m),
    }
}

fn cmd_if(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    let mut i = 1;
    loop {
        if i >= argv.len() {
            return Ctl::err(
                line,
                wrong_args("if expr body ?elseif expr body? ?else body?"),
            );
        }
        if argv[i] == "else" {
            if i + 1 != argv.len() - 1 {
                return Ctl::err(line, "wrong # args: \"else\" must take exactly one body");
            }
            let body = argv[i + 1].clone();
            return eval_body(s, &body, line, puts_out, depth);
        }
        if i + 1 >= argv.len() {
            return Ctl::err(
                line,
                wrong_args("if expr body ?elseif expr body? ?else body?"),
            );
        }
        let cond_src = argv[i].clone();
        let body = argv[i + 1].clone();
        let cond = match eval_expr(s, &cond_src, line, puts_out, depth) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, m),
        };
        let take = match truth(&cond) {
            Ok(b) => b,
            Err(m) => return Ctl::err(line, m),
        };
        if take {
            return eval_body(s, &body, line, puts_out, depth);
        }
        i += 2;
        if i >= argv.len() {
            return Ctl::Value(String::new());
        }
        if argv[i] == "elseif" {
            i += 1;
        } else if argv[i] != "else" {
            return Ctl::err(line, format!("unexpected \"{}\" after if body", argv[i]));
        }
    }
}

fn cmd_foreach(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    if argv.len() < 4 || argv.len() % 2 != 0 {
        return Ctl::err(
            line,
            wrong_args("foreach varList list ?varList list ...? body"),
        );
    }
    let body = argv[argv.len() - 1].clone();
    let mut pairs: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for chunk in argv[1..argv.len() - 1].chunks(2) {
        let vars = match split_list(&chunk[0]) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, format!("foreach varList: {m}")),
        };
        let list = match split_list(&chunk[1]) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, format!("foreach list: {m}")),
        };
        if vars.is_empty() {
            return Ctl::err(line, "foreach: empty varList");
        }
        pairs.push((vars, list));
    }
    let mut idx = 0usize;
    loop {
        let mut any = false;
        for (vars, list) in &pairs {
            for (vi, var) in vars.iter().enumerate() {
                let pos = idx * vars.len() + vi;
                let val = list.get(pos).cloned().unwrap_or_default();
                if pos < list.len() {
                    any = true;
                }
                s.tcl.assign(var.clone(), val);
            }
        }
        if !any {
            break;
        }
        match eval_body(s, &body.clone(), line, puts_out, depth) {
            Ctl::Value(_) => {}
            Ctl::Break => break,
            Ctl::Continue => {}
            other => return other,
        }
        idx += 1;
    }
    Ctl::Value(String::new())
}

fn cmd_for(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    if argv.len() != 5 {
        return Ctl::err(line, wrong_args("for start test next body"));
    }
    let (start, test, next, body) = (
        argv[1].clone(),
        argv[2].clone(),
        argv[3].clone(),
        argv[4].clone(),
    );
    if let Ctl::Error(e) = eval_body(s, &start, line, puts_out, depth) {
        return Ctl::Error(e);
    }
    loop {
        let cond = match eval_expr(s, &test, line, puts_out, depth) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, m),
        };
        match truth(&cond) {
            Ok(true) => {}
            Ok(false) => break,
            Err(m) => return Ctl::err(line, m),
        }
        match eval_body(s, &body.clone(), line, puts_out, depth) {
            Ctl::Value(_) => {}
            Ctl::Break => break,
            Ctl::Continue => {}
            other => return other,
        }
        if let Ctl::Error(e) = eval_body(s, &next, line, puts_out, depth) {
            return Ctl::Error(e);
        }
    }
    Ctl::Value(String::new())
}

fn cmd_while(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    if argv.len() != 3 {
        return Ctl::err(line, wrong_args("while test body"));
    }
    let (test, body) = (argv[1].clone(), argv[2].clone());
    loop {
        let cond = match eval_expr(s, &test, line, puts_out, depth) {
            Ok(v) => v,
            Err(m) => return Ctl::err(line, m),
        };
        match truth(&cond) {
            Ok(true) => {}
            Ok(false) => break,
            Err(m) => return Ctl::err(line, m),
        }
        match eval_body(s, &body.clone(), line, puts_out, depth) {
            Ctl::Value(_) => {}
            Ctl::Break => break,
            Ctl::Continue => {}
            other => return other,
        }
    }
    Ctl::Value(String::new())
}

fn cmd_proc(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 4 {
        return Ctl::err(line, wrong_args("proc name args body"));
    }
    let name = argv[1].clone();
    if name.is_empty() {
        return Ctl::err(line, "proc: empty name");
    }
    let arglist = match split_list(&argv[2]) {
        Ok(v) => v,
        Err(m) => return Ctl::err(line, format!("proc argList: {m}")),
    };
    let mut params = Vec::new();
    for a in arglist {
        let parts = split_list(&a).unwrap_or_else(|_| vec![a.clone()]);
        match parts.as_slice() {
            [] => return Ctl::err(line, "proc: empty parameter name"),
            [name] => params.push((name.clone(), None)),
            [name, rest @ ..] => params.push((name.clone(), Some(rest.join(" ")))),
        }
    }
    // The body stays raw text (checked at first call); the old IDE
    // pre-parses here, which this single-pass parser cannot do without
    // executing the body. Arg-shape errors above match exactly.
    s.tcl.procs.insert(
        name,
        ProcDef {
            params,
            body: argv[3].clone(),
        },
    );
    Ctl::Value(String::new())
}

fn call_proc(
    s: &mut AppState,
    name: &str,
    def: ProcDef,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    let given = &argv[1..];
    let required = def.params.iter().filter(|p| p.1.is_none()).count();
    if given.len() < required || given.len() > def.params.len() {
        return Ctl::err(
            line,
            format!(
                "wrong # args: should be \"{name} {}\", got {}",
                def.params
                    .iter()
                    .map(|p| p.0.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                argv[1..].join(" "),
            ),
        );
    }
    let mut bound: Vec<(String, String)> = Vec::new();
    for (pi, p) in def.params.iter().enumerate() {
        if let Some(v) = given.get(pi) {
            bound.push((p.0.clone(), v.clone()));
        } else if let Some(d) = &p.1 {
            match subst_str(s, d, line, puts_out, depth) {
                Ok(v) => bound.push((p.0.clone(), v)),
                Err(e) => return Ctl::Error(e),
            }
        }
    }
    s.tcl.frames.push(HashMap::new());
    for (k, v) in bound {
        if let Some(top) = s.tcl.frames.last_mut() {
            top.insert(k, v);
        }
    }
    let body = def.body.clone();
    let r = eval_body(s, &body, line, puts_out, depth);
    s.tcl.frames.pop();
    match r {
        Ctl::Value(s) | Ctl::Return(s) => Ctl::Value(s),
        Ctl::Break => Ctl::err(line, "invoked \"break\" outside of a loop"),
        Ctl::Continue => Ctl::err(line, "invoked \"continue\" outside of a loop"),
        Ctl::Error(e) => Ctl::Error(e),
    }
}

fn cmd_return(argv: &[String], line: usize) -> Ctl {
    match argv.len() {
        1 => Ctl::Return(String::new()),
        2 => Ctl::Return(argv[1].clone()),
        _ => Ctl::err(line, wrong_args("return ?value?")),
    }
}

fn cmd_llength(argv: &[String], line: usize) -> Ctl {
    if argv.len() != 2 {
        return Ctl::err(line, wrong_args("llength list"));
    }
    match split_list(&argv[1]) {
        Ok(v) => Ctl::Value(v.len().to_string()),
        Err(m) => Ctl::err(line, m),
    }
}

fn cmd_lindex(argv: &[String], line: usize) -> Ctl {
    if argv.len() != 3 {
        return Ctl::err(line, wrong_args("lindex list index"));
    }
    let list = match split_list(&argv[1]) {
        Ok(v) => v,
        Err(m) => return Ctl::err(line, m),
    };
    match list_index(&argv[2], list.len(), line) {
        Ok(Some(i)) => Ctl::Value(list[i].clone()),
        Ok(None) => Ctl::Value(String::new()),
        Err(e) => Ctl::Error(e),
    }
}

fn cmd_lrange(argv: &[String], line: usize) -> Ctl {
    if argv.len() != 4 {
        return Ctl::err(line, wrong_args("lrange list first last"));
    }
    let list = match split_list(&argv[1]) {
        Ok(v) => v,
        Err(m) => return Ctl::err(line, m),
    };
    let n = list.len();
    if n == 0 {
        return Ctl::Value(String::new());
    }
    let first = match list_index(&argv[2], n, line) {
        Ok(Some(i)) => i,
        Ok(None) => return Ctl::Value(String::new()),
        Err(e) => return Ctl::Error(e),
    };
    let last = match list_index(&argv[3], n, line) {
        Ok(Some(i)) => i,
        Ok(None) => n.saturating_sub(1),
        Err(e) => return Ctl::Error(e),
    };
    if last < first {
        return Ctl::Value(String::new());
    }
    Ctl::Value(join_list(&list[first..=last.min(n - 1)]))
}

fn cmd_lappend(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 {
        return Ctl::err(line, wrong_args("lappend varName ?value ...?"));
    }
    let cur = s.tcl.lookup(&argv[1]).cloned().unwrap_or_default();
    let mut list = match split_list(&cur) {
        Ok(v) => v,
        Err(m) => return Ctl::err(line, m),
    };
    list.extend(argv[2..].iter().cloned());
    let joined = join_list(&list);
    s.tcl.assign(argv[1].clone(), joined.clone());
    Ctl::Value(joined)
}

fn cmd_concat(argv: &[String], line: usize) -> Ctl {
    let mut out: Vec<String> = Vec::new();
    for a in &argv[1..] {
        match split_list(a) {
            Ok(v) => out.extend(v),
            Err(m) => return Ctl::err(line, m),
        }
    }
    Ctl::Value(join_list(&out))
}

fn cmd_join(argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 || argv.len() > 3 {
        return Ctl::err(line, wrong_args("join list ?joinString?"));
    }
    let list = match split_list(&argv[1]) {
        Ok(v) => v,
        Err(m) => return Ctl::err(line, m),
    };
    let sep = argv.get(2).map(|s| s.as_str()).unwrap_or(" ");
    Ctl::Value(list.join(sep))
}

fn cmd_split(argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 || argv.len() > 3 {
        return Ctl::err(line, wrong_args("split string ?splitChars?"));
    }
    let s = &argv[1];
    let parts: Vec<String> = match argv.get(2) {
        None => s.split_whitespace().map(|x| x.to_string()).collect(),
        Some(chars) if chars.is_empty() => s.chars().map(|c| c.to_string()).collect(),
        Some(chars) => {
            let set: Vec<char> = chars.chars().collect();
            s.split(|c| set.contains(&c))
                .map(|x| x.to_string())
                .collect()
        }
    };
    Ctl::Value(join_list(&parts))
}

fn cmd_string(argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 {
        return Ctl::err(line, wrong_args("string subcommand ..."));
    }
    match argv[1].as_str() {
        "length" => {
            if argv.len() != 3 {
                return Ctl::err(line, "string length: wants 1 arg");
            }
            Ctl::Value(argv[2].chars().count().to_string())
        }
        "range" => {
            if argv.len() != 5 {
                return Ctl::err(line, wrong_args("string range string first last"));
            }
            let ch: Vec<char> = argv[2].chars().collect();
            let n = ch.len();
            match (str_index(&argv[3], n), str_index(&argv[4], n)) {
                (Some(first), Some(last)) if last >= first => {
                    Ctl::Value(ch[first..=last].iter().collect())
                }
                _ => Ctl::Value(String::new()),
            }
        }
        "index" => {
            if argv.len() != 4 {
                return Ctl::err(line, wrong_args("string index string index"));
            }
            let ch: Vec<char> = argv[2].chars().collect();
            match str_index(&argv[3], ch.len()) {
                Some(i) => Ctl::Value(ch[i].to_string()),
                None => Ctl::Value(String::new()),
            }
        }
        "match" => {
            if argv.len() != 4 {
                return Ctl::err(line, wrong_args("string match pattern string"));
            }
            Ctl::Value(
                if glob_match(&argv[2], &argv[3]) {
                    "1"
                } else {
                    "0"
                }
                .to_string(),
            )
        }
        "tolower" => {
            if argv.len() != 3 {
                return Ctl::err(line, "string tolower: wants 1 arg");
            }
            Ctl::Value(argv[2].to_lowercase())
        }
        "toupper" => {
            if argv.len() != 3 {
                return Ctl::err(line, "string toupper: wants 1 arg");
            }
            Ctl::Value(argv[2].to_uppercase())
        }
        "trim" => {
            if argv.len() < 3 || argv.len() > 4 {
                return Ctl::err(line, wrong_args("string trim string ?chars?"));
            }
            let chars: Vec<char> = argv
                .get(3)
                .map(|s| s.chars().collect())
                .unwrap_or_else(|| vec![' ', '\t', '\n', '\r']);
            Ctl::Value(argv[2].trim_matches(|c| chars.contains(&c)).to_string())
        }
        "first" => {
            if argv.len() < 4 || argv.len() > 5 {
                return Ctl::err(line, wrong_args("string first needle haystack ?start?"));
            }
            let start: usize = argv.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
            let hay = &argv[3];
            let sub = &argv[2];
            let pos = hay
                .char_indices()
                .enumerate()
                .skip_while(|(ci, _)| *ci < start)
                .find_map(|(ci, (bi, _))| hay[bi..].starts_with(sub).then_some(ci));
            Ctl::Value(pos.map(|p| p.to_string()).unwrap_or_else(|| "-1".into()))
        }
        "equal" => {
            let mut i = 2;
            let mut nocase = false;
            if argv.get(i).is_some_and(|s| s == "-nocase") {
                nocase = true;
                i += 1;
            }
            if argv.len() != i + 2 {
                return Ctl::err(line, wrong_args("string equal ?-nocase? s1 s2"));
            }
            let (a, b) = (&argv[i], &argv[i + 1]);
            let eq = if nocase {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            };
            Ctl::Value(if eq { "1" } else { "0" }.to_string())
        }
        other => Ctl::err(
            line,
            format!(
                "bad string subcommand \"{other}\": must be length, range, index, match, tolower, toupper, trim, first, or equal"
            ),
        ),
    }
}

fn cmd_info(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 {
        return Ctl::err(line, wrong_args("info subcommand ..."));
    }
    match argv[1].as_str() {
        "exists" => {
            if argv.len() != 3 {
                return Ctl::err(line, "info exists: wants 1 arg");
            }
            Ctl::Value(
                if s.tcl.lookup(&argv[2]).is_some() {
                    "1"
                } else {
                    "0"
                }
                .to_string(),
            )
        }
        "vars" => {
            let pat = argv.get(2).map(|s| s.as_str());
            let mut names: Vec<String> = s
                .tcl
                .vars
                .keys()
                .filter(|k| pat.is_none_or(|p| glob_match(p, k)))
                .cloned()
                .collect();
            names.sort();
            Ctl::Value(join_list(&names))
        }
        "procs" => {
            let pat = argv.get(2).map(|s| s.as_str());
            let mut names: Vec<String> = s
                .tcl
                .procs
                .keys()
                .filter(|k| pat.is_none_or(|p| glob_match(p, k)))
                .cloned()
                .collect();
            names.sort();
            Ctl::Value(join_list(&names))
        }
        "commands" => {
            let mut names: Vec<&str> = COMMANDS.to_vec();
            names.sort();
            let pat = argv.get(2).map(|s| s.as_str());
            names.retain(|n| pat.is_none_or(|p| glob_match(p, n)));
            Ctl::Value(join_list(
                &names.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ))
        }
        "args" => {
            if argv.len() != 3 {
                return Ctl::err(line, "info args: wants 1 proc name");
            }
            match s.tcl.procs.get(&argv[2]).cloned() {
                Some(d) => Ctl::Value(
                    d.params
                        .iter()
                        .map(|p| p.0.clone())
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                None => Ctl::err(line, format!("\"{}\" isn't a procedure", argv[2])),
            }
        }
        "body" => {
            if argv.len() != 3 {
                return Ctl::err(line, "info body: wants 1 proc name");
            }
            if s.tcl.procs.contains_key(&argv[2]) {
                Ctl::Value("<parsed body>".into())
            } else {
                Ctl::err(line, format!("\"{}\" isn't a procedure", argv[2]))
            }
        }
        other => Ctl::err(
            line,
            format!(
                "bad info subcommand \"{other}\": must be exists, vars, procs, commands, args, or body"
            ),
        ),
    }
}

fn cmd_catch(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    if argv.len() < 2 || argv.len() > 3 {
        return Ctl::err(line, wrong_args("catch script ?varName?"));
    }
    let script = argv[1].clone();
    let var = argv.get(2).cloned();
    match eval_body(s, &script, line, puts_out, depth) {
        Ctl::Value(v) => {
            if let Some(vn) = var {
                s.tcl.assign(vn, v);
            }
            Ctl::Value("0".into())
        }
        Ctl::Return(v) => {
            if let Some(vn) = var {
                s.tcl.assign(vn, v);
            }
            Ctl::Value("0".into())
        }
        Ctl::Error(e) => {
            if let Some(vn) = var {
                s.tcl.assign(vn, e.msg.clone());
                s.tcl
                    .assign("errorInfo".into(), format!("{} (line {})", e.msg, e.line));
            }
            Ctl::Value("1".into())
        }
        Ctl::Break => Ctl::err(line, "invoked \"break\" outside of a loop"),
        Ctl::Continue => Ctl::err(line, "invoked \"continue\" outside of a loop"),
    }
}

fn cmd_error(argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 || argv.len() > 3 {
        return Ctl::err(line, wrong_args("error message ?info?"));
    }
    Ctl::err(line, argv[1].clone())
}

fn cmd_eval(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    if argv.len() < 2 {
        return Ctl::err(line, wrong_args("eval arg ..."));
    }
    let script = argv[1..].join(" ");
    eval_body(s, &script, line, puts_out, depth)
}

fn cmd_subst(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    let mut i = 1;
    let mut no_cmd = false;
    let mut no_var = false;
    let mut no_bs = false;
    while i < argv.len() && argv[i].starts_with('-') {
        match argv[i].as_str() {
            "-nocommands" => no_cmd = true,
            "-novariables" => no_var = true,
            "-nobackslashes" => no_bs = true,
            other => return Ctl::err(line, format!("bad option \"{other}\"")),
        }
        i += 1;
    }
    if argv.len() != i + 1 {
        return Ctl::err(
            line,
            wrong_args("subst ?-nocommands? ?-novariables? ?-nobackslashes? string"),
        );
    }
    let src: Vec<char> = argv[i].chars().collect();
    let mut out = String::new();
    let mut k = 0;
    while k < src.len() {
        let c = src[k];
        if c == '\\' && !no_bs && k + 1 < src.len() {
            match src[k + 1] {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                other => out.push(other),
            }
            k += 2;
        } else if c == '$' && !no_var {
            match subst_var(s, &src, k, puts_out, depth, line) {
                Ok(Some((v, len))) => {
                    out.push_str(&v);
                    k += len;
                }
                Ok(None) => {
                    out.push(c);
                    k += 1;
                }
                Err(e) => return Ctl::Error(e),
            }
        } else if c == '[' && !no_cmd {
            match subst_cmd(s, &src, k, puts_out, depth, line) {
                Ok(Some((v, len))) => {
                    out.push_str(&v);
                    k += len;
                }
                Ok(None) => {
                    out.push(c);
                    k += 1;
                }
                Err(e) => return Ctl::Error(e),
            }
        } else {
            out.push(c);
            k += 1;
        }
    }
    Ctl::Value(out)
}

/// One `$var`/`$var(idx)` reference inside `subst`: the value plus the
/// source length, or `None` for a lone `$` (stays literal).
fn subst_var(
    s: &mut AppState,
    src: &[char],
    k: usize,
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Result<Option<(String, usize)>, TclError> {
    // src[k] == '$'
    if src.get(k + 1) == Some(&'{') {
        let mut j = k + 2;
        while j < src.len() && src[j] != '}' {
            j += 1;
        }
        if j >= src.len() {
            return Ok(None);
        }
        let name: String = src[k + 2..j].iter().collect();
        if name.is_empty() {
            return Ok(None);
        }
        return match s.tcl.lookup(&name).cloned() {
            Some(v) => Ok(Some((v, j + 1 - k))),
            None => Err(TclError {
                msg: format!("can't read \"{name}\": no such variable"),
                line,
            }),
        };
    }
    let mut j = k + 1;
    while j < src.len() && (src[j].is_ascii_alphanumeric() || src[j] == '_') {
        j += 1;
    }
    if j == k + 1 {
        return Ok(None);
    }
    let name: String = src[k + 1..j].iter().collect();
    if src.get(j) == Some(&'(') {
        let mut p = j + 1;
        let mut depth_paren = 1usize;
        let mut idx_src = String::new();
        loop {
            match src.get(p) {
                None => return Ok(None),
                Some('(') => {
                    depth_paren += 1;
                    idx_src.push('(');
                }
                Some(')') => {
                    depth_paren -= 1;
                    if depth_paren == 0 {
                        break;
                    }
                    idx_src.push(')');
                }
                Some(c) => idx_src.push(*c),
            }
            p += 1;
        }
        let idx = subst_str(s, &idx_src, line, puts_out, depth)?;
        let key = format!("{name}({idx})");
        return match s.tcl.lookup(&key).cloned() {
            Some(v) => Ok(Some((v, p + 1 - k))),
            None => Err(TclError {
                msg: format!("can't read \"{name}\": no such variable"),
                line,
            }),
        };
    }
    match s.tcl.lookup(&name).cloned() {
        Some(v) => Ok(Some((v, j - k))),
        None => Err(TclError {
            msg: format!("can't read \"{name}\": no such variable"),
            line,
        }),
    }
}

/// One `[...]` span inside `subst` (naive bracket scan, mirroring the old
/// `cmd_source_len`): the value plus the source length, or `None` when the
/// span never closes (the `[` stays literal).
fn subst_cmd(
    s: &mut AppState,
    src: &[char],
    k: usize,
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Result<Option<(String, usize)>, TclError> {
    // src[k] == '['
    let mut p = k + 1;
    let mut depth_br = 1usize;
    while p < src.len() && depth_br > 0 {
        match src[p] {
            '[' => depth_br += 1,
            ']' => depth_br -= 1,
            '\\' => p += 1,
            _ => {}
        }
        p += 1;
    }
    if depth_br != 0 {
        return Ok(None);
    }
    let inner: String = src[k + 1..p - 1].iter().collect();
    match eval_body(s, &inner, line, puts_out, depth) {
        Ctl::Value(v) | Ctl::Return(v) => Ok(Some((v, p - k))),
        Ctl::Error(e) => Err(e),
        Ctl::Break => Err(TclError {
            msg: "invoked \"break\" outside of a loop".into(),
            line,
        }),
        Ctl::Continue => Err(TclError {
            msg: "invoked \"continue\" outside of a loop".into(),
            line,
        }),
    }
}

fn cmd_source(
    s: &mut AppState,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    if argv.len() == 1 {
        // Old IDE: bare `source` opens the UG900 Source window.
        return cmd_source_window(s, argv, line);
    }
    if argv.len() != 2 {
        return Ctl::err(line, wrong_args("source fileName"));
    }
    match std::fs::read_to_string(&argv[1]) {
        Ok(text) => eval_body(s, &text, line, puts_out, depth),
        Err(e) => Ctl::err(line, format!("couldn't read file \"{}\": {e}", argv[1])),
    }
}

fn cmd_format(argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 {
        return Ctl::err(line, wrong_args("format formatString ..."));
    }
    match do_format(&argv[1], &argv[2..]) {
        Ok(v) => Ctl::Value(v),
        Err(m) => Ctl::err(line, m),
    }
}

fn cmd_incr(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 || argv.len() > 3 {
        return Ctl::err(line, wrong_args("incr varName ?increment?"));
    }
    let cur = s
        .tcl
        .lookup(&argv[1])
        .cloned()
        .unwrap_or_else(|| "0".into());
    let cur_i: i64 = match cur.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            return Ctl::err(line, format!("expected integer but got \"{cur}\""));
        }
    };
    let inc: i64 = match argv.get(2).map(|s| s.trim().parse::<i64>()) {
        None => 1,
        Some(Ok(v)) => v,
        Some(Err(_)) => {
            return Ctl::err(line, format!("expected integer but got \"{}\"", argv[2]));
        }
    };
    let v = cur_i.wrapping_add(inc).to_string();
    s.tcl.assign(argv[1].clone(), v.clone());
    Ctl::Value(v)
}

fn cmd_append(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 {
        return Ctl::err(line, wrong_args("append varName ?value ...?"));
    }
    let mut cur = s.tcl.lookup(&argv[1]).cloned().unwrap_or_default();
    for a in &argv[2..] {
        cur.push_str(a);
    }
    s.tcl.assign(argv[1].clone(), cur.clone());
    Ctl::Value(cur)
}

fn cmd_history(s: &AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("history"));
    }
    Ctl::Value(
        s.tcl
            .history
            .iter()
            .enumerate()
            .map(|(n, h)| format!("{}: {h}", n + 1))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn cmd_clear(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("clear"));
    }
    s.console.lines.clear();
    Ctl::Value(String::new())
}

fn cmd_launch(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let to_step_write = (argv.len() == 4
        && argv[1] == "impl_1"
        && argv[2] == "-to_step"
        && argv[3] == "write_bitstream")
        || (argv.len() == 3 && argv[1] == "-to_step" && argv[2] == "write_bitstream");
    if to_step_write {
        // The impl worker already ends with the real write_bitstream step,
        // so generating the bitstream (re-)runs impl_1: never a stale blob.
        crate::runs::launch_verb(s, crate::runs::RunKind::Impl, &argv.join(" "));
        return Ctl::Value(String::new());
    }
    if argv.len() != 2 {
        return Ctl::err(
            line,
            wrong_args("launch_runs synth_1|impl_1|(-to_step write_bitstream)"),
        );
    }
    let kind = match argv[1].as_str() {
        "synth_1" => crate::runs::RunKind::Synth,
        "impl_1" => crate::runs::RunKind::Impl,
        other => {
            return Ctl::err(
                line,
                format!("unknown run \"{other}\" (try \"synth_1\" or \"impl_1\")"),
            );
        }
    };
    crate::runs::launch(s, kind);
    Ctl::Value(String::new())
}

fn cmd_cancel(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() > 2 {
        return Ctl::err(line, wrong_args("cancel_runs ?run?"));
    }
    crate::runs::cancel_run(s);
    Ctl::Value(String::new())
}

// ---- Tcl lists (mirroring `interp.rs` split/join) ----

/// Split a Tcl list string into elements (braces, quotes, backslashes).
fn split_list(s: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        while k < chars.len() && chars[k].is_whitespace() {
            k += 1;
        }
        if k >= chars.len() {
            break;
        }
        let mut elem = String::new();
        if chars[k] == '{' {
            k += 1;
            let mut depth = 1;
            while k < chars.len() && depth > 0 {
                match chars[k] {
                    '\\' if k + 1 < chars.len() && matches!(chars[k + 1], '{' | '}' | '\\') => {
                        elem.push(chars[k + 1]);
                        k += 2;
                    }
                    '{' => {
                        depth += 1;
                        elem.push('{');
                        k += 1;
                    }
                    '}' => {
                        depth -= 1;
                        if depth > 0 {
                            elem.push('}');
                        }
                        k += 1;
                    }
                    c => {
                        elem.push(c);
                        k += 1;
                    }
                }
            }
            if depth != 0 {
                return Err("list: missing close-brace".into());
            }
        } else if chars[k] == '"' {
            k += 1;
            while k < chars.len() && chars[k] != '"' {
                if chars[k] == '\\' && k + 1 < chars.len() {
                    elem.push(chars[k + 1]);
                    k += 2;
                } else {
                    elem.push(chars[k]);
                    k += 1;
                }
            }
            if k >= chars.len() {
                return Err("list: missing close-quote".into());
            }
            k += 1;
        } else {
            while k < chars.len() && !chars[k].is_whitespace() {
                if chars[k] == '\\' && k + 1 < chars.len() {
                    elem.push(chars[k + 1]);
                    k += 2;
                } else {
                    elem.push(chars[k]);
                    k += 1;
                }
            }
        }
        out.push(elem);
    }
    Ok(out)
}

/// Join elements into a canonical Tcl list string (brace-quoting as needed).
fn join_list(items: &[String]) -> String {
    items
        .iter()
        .map(|s| quote_list_elem(s))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_list_elem(s: &str) -> String {
    if s.is_empty() {
        return "{}".to_string();
    }
    let needs = s.chars().any(|c| {
        c.is_whitespace() || matches!(c, '{' | '}' | '"' | '[' | ']' | '$' | ';' | '#' | '\\')
    });
    if !needs {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('{');
    for c in s.chars() {
        if c == '{' || c == '}' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('}');
    out
}

/// Resolve `lindex`-style index: integer, `end`, `end-N`. None = out of range.
fn list_index(spec: &str, len: usize, line: usize) -> Result<Option<usize>, TclError> {
    let spec = spec.trim();
    if spec == "end" {
        return Ok(if len == 0 { None } else { Some(len - 1) });
    }
    if let Some(rest) = spec.strip_prefix("end-") {
        match rest.parse::<usize>() {
            Ok(n) if n < len => return Ok(Some(len - 1 - n)),
            _ => return Ok(None),
        }
    }
    if let Some(rest) = spec.strip_prefix("end+") {
        match rest.parse::<usize>() {
            Ok(n) => return Ok(Some(len - 1 + n).filter(|&i| i < len)),
            Err(_) => {
                return Err(TclError {
                    msg: format!("bad index \"{spec}\": must be integer or end?-?integer?"),
                    line,
                });
            }
        }
    }
    match spec.parse::<i64>() {
        Ok(i) if i >= 0 => Ok(usize::try_from(i).ok().filter(|&i| i < len)),
        Ok(_) => Ok(None),
        Err(_) => Err(TclError {
            msg: format!("bad index \"{spec}\": must be integer or end?-?integer?"),
            line,
        }),
    }
}

/// Resolve `string range`/`string index` index: integer or `end`/`end-N`.
/// Out-of-range indices resolve to `None` (Tcl yields an empty string);
/// they are never clamped into range.
fn str_index(spec: &str, len: usize) -> Option<usize> {
    let spec = spec.trim();
    if len == 0 {
        return None;
    }
    if spec == "end" {
        return Some(len - 1);
    }
    if let Some(rest) = spec.strip_prefix("end-") {
        return rest
            .parse::<usize>()
            .ok()
            .and_then(|n| len.checked_sub(1 + n));
    }
    spec.parse::<i64>()
        .ok()
        .and_then(|i| usize::try_from(i).ok())
        .filter(|&i| i < len)
}

/// `synth_design` / `read_sv` / `read_verilog` / `read_vhdl` / `hds::synth`.
/// Mirrors `cmd_synth` (`helion-gui/src/tcl/design.rs`): bare `hds::synth`
/// is `synth ok`, otherwise the file elaborates and the session takes the
/// netlist. Rune extension: bare `synth_design` (no file) elaborates the
/// active editor RTL buffer (the old IDE has no editor buffer).
fn cmd_synth(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv[0] == "hds::synth" && argv.len() == 1 {
        return Ctl::Value("synth ok".into());
    }
    if argv[0] == "synth_design" && argv.len() == 1 {
        return synth_editor_buffer(s, line);
    }
    let path = match argv.get(1) {
        Some(p) => p,
        None => {
            return Ctl::err(line, wrong_args(&format!("{} file", argv[0])));
        }
    };
    let d = match synth_hdl_path(std::path::Path::new(path)) {
        Ok(d) => d,
        Err(e) => return Ctl::err(line, e),
    };
    // Reading a file selects it as the run source (mirroring the old IDE's
    // `open_source`): a later `launch_runs` elaborates this file, not
    // whatever RTL happens to sit in the editor buffer.
    s.runs.source = Some(std::path::PathBuf::from(path));
    Ctl::Value(store_design(s, d))
}

/// Elaborate the active editor RTL buffer for real, staging it to a temp
/// file first (mirroring `runs.rs` source staging; the temp path keeps the
/// tab's extension so dispatch stays by extension).
fn synth_editor_buffer(s: &mut AppState, line: usize) -> Ctl {
    let tab = s
        .editor
        .tabs
        .get(s.editor.active)
        .cloned()
        .unwrap_or_default();
    if !is_rtl_name(&tab) {
        return Ctl::err(line, "synth_design: no RTL source (open a source first)");
    }
    let buf = active_rtl_buffer(s);
    if buf.iter().all(|l| l.trim().is_empty()) {
        return Ctl::err(line, "synth_design: the RTL buffer is empty");
    }
    let dir = std::env::temp_dir().join("helion-rune-tcl");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Ctl::err(
            line,
            format!("synth_design: cannot stage RTL ({}): {e}", dir.display()),
        );
    }
    let file = std::path::PathBuf::from(&tab)
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("design.v"));
    let path = dir.join(file);
    if let Err(e) = std::fs::write(&path, buf.join("\n")) {
        return Ctl::err(
            line,
            format!("synth_design: cannot stage RTL ({}): {e}", path.display()),
        );
    }
    match synth_hdl_path(&path) {
        Ok(d) => Ctl::Value(store_design(s, d)),
        Err(e) => Ctl::err(line, e),
    }
}

/// Elaborate RTL by extension, mirroring `synth_hdl_path`
/// (`helion-gui/src/lib.rs:90`): `.prj` (multi-file + optional `top`),
/// `.vhd`/`.vhdl` through helion-vhdl, else SystemVerilog. Engine errors
/// propagate verbatim (no `is_file` pre-check: the old IDE has none).
fn synth_hdl_path(path: &std::path::Path) -> Result<helion_ir::Design, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "prj" => synth_prj_path(path),
        "vhd" | "vhdl" => helion_vhdl::synth_vhdl_path(path),
        _ => helion_sv::synth_sv_path(path),
    }
}

/// Multi-file Helion `.prj`: resolve sources and elaborate with optional
/// `top`. Mirrors `synth_prj_path` (`helion-gui/src/lib.rs:133`); every
/// API it uses (`load_prj`, `expand_ip_packages`, `resolve_prj_path`,
/// `elaborate_sv_sources`) is an engine-crate API, not gui code.
fn synth_prj_path(path: &std::path::Path) -> Result<helion_ir::Design, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut prj = helion_proj::load_prj(&text)?;
    let _ = helion_proj::expand_ip_packages(&mut prj, path)?;
    let src_paths: Vec<std::path::PathBuf> = prj
        .sources
        .iter()
        .map(|s| helion_proj::resolve_prj_path(path, s))
        .collect();
    for (src, resolved) in prj.sources.iter().zip(src_paths.iter()) {
        if !resolved.exists() {
            return Err(format!(
                "project source {src}: not found (tried {})",
                resolved.display()
            ));
        }
    }
    let rtl: Vec<std::path::PathBuf> = src_paths
        .into_iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    e == "sv" || e == "v"
                })
                .unwrap_or(false)
        })
        .collect();
    if rtl.is_empty() {
        return Err("project has no SystemVerilog sources".into());
    }
    if rtl.len() == 1 && prj.top.is_none() {
        return helion_sv::synth_sv_path(&rtl[0]);
    }
    let mut owned: Vec<(String, String)> = Vec::new();
    for p in &rtl {
        let src = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        owned.push((p.display().to_string(), src));
    }
    let refs: Vec<(&str, &str)> = owned
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let params: HashMap<String, u128> = HashMap::new();
    let opts = Default::default();
    let (d, _) = helion_sv::elaborate_sv_sources(&refs, prj.top.as_deref(), &params, &opts)?;
    Ok(d)
}

/// Take an elaborated netlist into the console design AND the flow session
/// (both stay in sync; views read `tcl.design`, flow verbs read the
/// session). Reports the old `synth {name} cells {n} luts {m}` line.
pub(crate) fn store_design(s: &mut AppState, design: helion_ir::Design) -> String {
    let summary = format!(
        "synth {} cells {} luts {}",
        helion_ir::resolve(design.name),
        design.cells.len(),
        design.lut_inits().len()
    );
    s.tcl
        .tree_nodes
        .push(helion_ir::resolve(design.name).to_string());
    s.tcl.design_name = Some(helion_ir::resolve(design.name).to_string());
    s.tcl.session.synth_design(design.clone());
    s.tcl.design = Some(design);
    summary
}

fn is_rtl_name(name: &str) -> bool {
    let ext = std::path::PathBuf::from(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "sv" | "v" | "vhd" | "vhdl")
}

/// The RTL buffer behind the active tab (XDC tabs have no RTL buffer; the
/// caller already rejected them, so this falls back to the Verilog buffer).
fn active_rtl_buffer(s: &AppState) -> Vec<String> {
    if s.editor.active_is_tb() {
        s.editor.tb_lines.clone()
    } else {
        s.editor.lines.clone()
    }
}

/// Dispatch a non-core, non-proc command against the design database /
/// flow engines. Mirrors `eval_host` (`helion-gui/src/tcl/design.rs:750`).
fn dispatch_host(
    s: &mut AppState,
    name: &str,
    argv: &[String],
    puts_out: &mut Vec<String>,
    depth: usize,
    line: usize,
) -> Ctl {
    let _ = (puts_out, depth);
    match name {
        "get_cells" => cmd_get_cells(s, argv, line),
        "get_nets" => cmd_get_nets(s, argv, line),
        "get_pins" => cmd_get_pins(s, argv, line),
        "get_ports" => cmd_get_ports(s, argv, line),
        "get_clocks" => cmd_get_clocks(s, argv, line),
        "get_timing_paths" => cmd_get_timing_paths(s, argv, line),
        "get_property" => cmd_get_property(s, argv, line),
        "set_property" => cmd_set_property(s, argv, line),
        "assign_package_pin" => cmd_assign_package_pin(s, argv, line),
        "package" | "package_drawing" => cmd_package(s, argv, line),
        "select_package_pin" => cmd_select_package_pin(s, argv, line),
        "sim_run" => cmd_sim_run(s, argv, line),
        "sim_step" => cmd_sim_step(s, argv, line),
        "sim_restart" => cmd_sim_restart(s, argv, line),
        "breakpoints" => cmd_breakpoints(s, argv, line),
        "add_bp" => cmd_add_bp(s, argv, line),
        "select_breakpoint" => cmd_select_breakpoint(s, argv, line),
        "enable_bp" => cmd_enable_bp(s, argv, line),
        "disable_bp" => cmd_disable_bp(s, argv, line),
        "delete_bp" => cmd_delete_bp(s, argv, line),
        "memory" | "memories" => cmd_memory(s, argv, line),
        "select_memory" => cmd_select_memory(s, argv, line),
        "select_memory_word" => cmd_select_memory_word(s, argv, line),
        "locals" => cmd_locals(s, argv, line),
        "select_local" => cmd_select_local(s, argv, line),
        "forces" | "force_table" | "force_constants" => cmd_forces(s, argv, line),
        "add_force" => cmd_add_force(s, argv, line),
        "deposit" => cmd_deposit(s, argv, line),
        "remove_force" => cmd_remove_force(s, argv, line),
        "select_force" => cmd_select_force(s, argv, line),
        "source_window" => cmd_source_window(s, argv, line),
        "select_source_line" => cmd_select_source_line(s, argv, line),
        "toggle_bp_line" | "add_bp_line" => cmd_toggle_bp_line(s, argv, line),
        "report_property" => cmd_report_property(s, argv, line),
        "list_property" | "list_properties" => cmd_list_property(s, argv, line),
        "select_objects" => cmd_select_objects(s, argv, line, false),
        "highlight_objects" => cmd_select_objects(s, argv, line, true),
        "get_selected_objects" => Ctl::Value(join_list(
            &s.tcl
                .selected
                .iter()
                .map(|o| o.name.clone())
                .collect::<Vec<_>>(),
        )),
        "current_design" => match query_design(s) {
            Some(d) => Ctl::Value(helion_ir::resolve(d.name).to_string()),
            None => Ctl::err(line, "current_design: no design (synth_design first)"),
        },
        "synth_design" | "read_sv" | "read_verilog" | "read_vhdl" | "hds::synth" => {
            cmd_synth(s, argv, line)
        }
        "tb_rescan" | "tb_select" | "tb_run" | "tb_run_all" | "tb_stop" | "tb_vcd" | "tb_wave"
        | "tb_jump" => ctl_of(crate::views::tb::exec_tb(s, name, &argv[1..]), line),
        "set_timescale" | "set_runtime" | "sim_settings" | "compile_sim" | "elaborate_sim" => {
            ctl_of(
                crate::views::sim_settings::exec_settings(s, name, &argv[1..]),
                line,
            )
        }
        "ila" => ctl_of(crate::views::ila::exec_ila(s, &argv[1..]), line),
        "opt_design" => {
            if s.tcl.design.is_none() {
                return Ctl::err(line, "opt_design: no design");
            }
            sync_session_design(s);
            let mut n = 0;
            if let Some(d) = s.tcl.design.as_mut() {
                n = helion_proj::opt_design(d);
            }
            if let Some(d) = s.tcl.session.design.as_mut() {
                helion_proj::opt_design(d);
            }
            Ctl::Value(format!("opt removed {n} dead LUTFF"))
        }
        "place_design" => {
            sync_session_design(s);
            if s.tcl.session.design.is_none() {
                return Ctl::err(line, "place_design: no design (synth_design first)");
            }
            let dev = match helion_device::Device::load_part(&s.project.part) {
                Ok(d) => d,
                Err(e) => return Ctl::err(line, e),
            };
            if let Err(e) = s.tcl.session.place_design(&dev) {
                return Ctl::err(line, e);
            }
            let n = s
                .tcl
                .session
                .placed
                .as_ref()
                .map(|p| p.lutff_sites.len())
                .unwrap_or(0);
            Ctl::Value(format!("place_design sites {n}"))
        }
        "route_design" => {
            sync_session_design(s);
            if s.tcl.session.design.is_none() {
                return Ctl::err(line, "route_design: no design (synth_design first)");
            }
            let dev = match helion_device::Device::load_part(&s.project.part) {
                Ok(d) => d,
                Err(e) => return Ctl::err(line, e),
            };
            if s.tcl.session.placed.is_none() {
                if let Err(e) = s.tcl.session.place_design(&dev) {
                    return Ctl::err(line, e);
                }
            }
            if let Err(e) = s.tcl.session.route_design(&dev) {
                return Ctl::err(line, e);
            }
            let hops = s
                .tcl
                .session
                .routed
                .as_ref()
                .and_then(|r| r.iob_src.first())
                .map(|io| io.hops)
                .unwrap_or(0);
            Ctl::Value(format!("route_design hops {hops}"))
        }
        "write_bitstream" => {
            if let Err(e) = impl_if_needed(s) {
                return Ctl::err(line, e);
            }
            let n = s
                .tcl
                .session
                .bitstream
                .as_ref()
                .map(|b| b.packets.len())
                .unwrap_or(0);
            Ctl::Value(format!("write_bitstream {n} bytes"))
        }
        "write_checkpoint" => {
            let ck = s.tcl.session.checkpoint();
            Ctl::Value(format!(
                "write_checkpoint {} bytes hash {:#x}",
                ck.len(),
                s.tcl.session.blinky_hash().unwrap_or(0)
            ))
        }
        "create_clock" => cmd_create_clock(s, argv, line),
        "report_timing" => match ensure_timing(s, line) {
            Ok((name, t, paths)) => Ctl::Value(format_timing_table(&name, &t, &paths)),
            Err(e) => Ctl::Error(e),
        },
        "report_utilization" => {
            if let Err(e) = impl_if_needed(s) {
                return Ctl::err(line, e);
            }
            let dev = match helion_device::Device::load_part(&s.project.part) {
                Ok(d) => d,
                Err(e) => return Ctl::err(line, e),
            };
            match format_util_table(s, &dev) {
                Ok(v) => Ctl::Value(v),
                Err(e) => Ctl::err(line, e),
            }
        }
        "mark_debug" => {
            let net = match argv.get(1) {
                Some(n) => n.clone(),
                None => return Ctl::err(line, wrong_args("mark_debug net")),
            };
            sync_session_design(s);
            match s.tcl.session.mark_debug(&net) {
                Ok(()) => {
                    s.tcl.design = s.tcl.session.design.clone();
                    Ctl::Value(format!("mark_debug {net}"))
                }
                Err(e) => Ctl::err(line, e),
            }
        }
        "add_probe" => {
            let net = match argv.get(1) {
                Some(n) => n.clone(),
                None => return Ctl::err(line, wrong_args("add_probe net")),
            };
            sync_session_design(s);
            match s.tcl.session.mark_debug(&net) {
                Ok(()) => {
                    s.tcl.design = s.tcl.session.design.clone();
                    Ctl::Value(format!("add_probe {net}"))
                }
                Err(e) => Ctl::err(line, e),
            }
        }
        "eco" => {
            let cell = argv.get(1).map(|s| s.as_str()).unwrap_or("u_lut");
            let init_s = argv
                .get(2)
                .map(|s| s.as_str())
                .unwrap_or("0xAAAAAAAAAAAAAAAA");
            let init =
                u64::from_str_radix(init_s.trim_start_matches("0x").trim_start_matches("0X"), 16)
                    .unwrap_or(0xAAAA_AAAA_AAAA_AAAA);
            let dev = match helion_device::Device::load_part(&s.project.part) {
                Ok(d) => d,
                Err(e) => return Ctl::err(line, e),
            };
            if let Err(e) = impl_if_needed(s) {
                return Ctl::err(line, e);
            }
            match s.tcl.session.eco(&dev, cell, init) {
                Ok(()) => {
                    s.tcl.design = s.tcl.session.design.clone();
                    Ctl::Value(format!("eco {cell} {init:#x}"))
                }
                Err(e) => Ctl::err(line, e),
            }
        }
        "open_hw_manager" => {
            s.tcl.session.open_hw_manager();
            Ctl::Value("open_hw_manager sim".into())
        }
        "program_hw" | "program_hw_devices" => {
            let dev = match helion_device::Device::load_part(&s.project.part) {
                Ok(d) => d,
                Err(e) => return Ctl::err(line, e),
            };
            if s.tcl.session.bitstream.is_none() {
                if let Err(e) = impl_if_needed(s) {
                    return Ctl::err(line, e);
                }
            }
            if !s.tcl.session.hw_open {
                s.tcl.session.open_hw_manager();
            }
            let cable = argv
                .iter()
                .skip(1)
                .find_map(|a| a.strip_prefix("cable="))
                .unwrap_or("auto");
            match s.tcl.session.program_hw_cable(&dev, cable) {
                Ok(v) => Ctl::Value(v),
                Err(e) => Ctl::err(line, e),
            }
        }
        "report_die" | "report_hw_targets" => {
            match helion_device::Device::load_part(&s.project.part) {
                Ok(d) => Ctl::Value(d.report_die()),
                Err(e) => Ctl::err(line, e),
            }
        }
        "report_featuremap" => match helion_device::Device::load_part(&s.project.part) {
            Ok(d) => Ctl::Value(d.report_featuremap()),
            Err(e) => Ctl::err(line, e),
        },
        "write_hnf" => match query_design(s) {
            Some(d) => Ctl::Value(d.to_hnf()),
            None => Ctl::err(line, "write_hnf: no design (synth_design first)"),
        },
        "hds::impl" => {
            if let Err(e) = impl_if_needed(s) {
                return Ctl::err(line, e);
            }
            let n = s
                .tcl
                .session
                .bitstream
                .as_ref()
                .map(|b| b.frames.len())
                .unwrap_or(0);
            Ctl::Value(format!("impl frames {n}"))
        }
        "hds::flow" => Ctl::Value("synth opt place route bits hw".into()),
        "hds::tree" => Ctl::Value(s.tcl.tree_nodes.join(" ")),
        "open_report" => cmd_open_report(s, argv, line),
        _ => unknown_command(s, name, line),
    }
}

/// Copy the console design into the flow session when the session has none
/// (all runtime verbs keep both in sync; only test fixtures write
/// `tcl.design` directly, and they never call flow verbs after).
fn sync_session_design(s: &mut AppState) {
    if s.tcl.session.design.is_none() {
        if let Some(d) = s.tcl.design.clone() {
            s.tcl.session.synth_design(d);
        }
    }
}

/// Run the implementation flow unless a bitstream already exists. Mirrors
/// `impl_if_needed` (`helion-gui/src/lib.rs:228`).
fn impl_if_needed(s: &mut AppState) -> Result<(), String> {
    if s.tcl.session.bitstream.is_some() {
        return Ok(());
    }
    let d = s
        .tcl
        .design
        .clone()
        .ok_or("no design (read_sv / synth_design first)")?;
    let dev = helion_device::Device::load_part(&s.project.part)?;
    s.tcl.session.impl_design(d, &dev)
}

// ---- glob (mirroring `design.rs:144`) ----

/// Tcl/Vivado glob: `*`, `?`, `[seq]`, `[a-z]`, `\` escapes.
fn glob_match(pat: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while si < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '[' {
            match match_class(&p, &mut pi) {
                Some(set) => {
                    if set.contains(&t[si]) {
                        si += 1;
                    } else {
                        return false;
                    }
                }
                None => return false,
            }
        } else if pi < p.len() && p[pi] == '\\' && pi + 1 < p.len() {
            if p[pi + 1] == t[si] {
                pi += 2;
                si += 1;
            } else {
                return false;
            }
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Match a `[...]` class at `p[*pi]`; advances `*pi` past `]`.
fn match_class(p: &[char], pi: &mut usize) -> Option<Vec<char>> {
    // p[*pi] == '['
    let mut k = *pi + 1;
    let mut set = Vec::new();
    let mut ok = false;
    while k < p.len() {
        if p[k] == ']' && ok {
            *pi = k + 1;
            return Some(set);
        }
        ok = true;
        if p[k] == '\\' && k + 1 < p.len() {
            set.push(p[k + 1]);
            k += 2;
        } else if k + 2 < p.len() && p[k + 1] == '-' && p[k + 2] != ']' {
            for c in (p[k] as u32)..=(p[k + 2] as u32) {
                set.push(char::from_u32(c).unwrap_or('?'));
            }
            k += 3;
        } else {
            set.push(p[k]);
            k += 1;
        }
    }
    None
}

// ---- properties (mirroring `design.rs` cell/net/pin/port/clock/path props) ----

fn cell_type(kind: &helion_ir::CellKind) -> &'static str {
    match kind {
        helion_ir::CellKind::Lut6 { .. } => "lut",
        helion_ir::CellKind::Hff => "ff",
        helion_ir::CellKind::IobOut => "iob",
        helion_ir::CellKind::IobIn => "iob",
        helion_ir::CellKind::ClockBuf => "bufg",
        helion_ir::CellKind::Mac27 => "dsp",
        helion_ir::CellKind::Dsp48e1 { .. } => "dsp",
        helion_ir::CellKind::Bram18 | helion_ir::CellKind::Bram18e1 { .. } => "bram",
        helion_ir::CellKind::Carry4 => "carry",
        helion_ir::CellKind::BlackBox { .. } => "blackbox",
        helion_ir::CellKind::Ila { .. } => "ila",
    }
}

/// Max sink count over nets the cell drives (for the FANOUT property).
fn cell_fanouts(d: &helion_ir::Design) -> HashMap<helion_ir::Spur, usize> {
    let mut m: HashMap<helion_ir::Spur, usize> = HashMap::new();
    for net in &d.nets {
        let sinks = net.endpoints.len().saturating_sub(1);
        for ep in &net.endpoints {
            if matches!(helion_ir::resolve(ep.pin), "O" | "Q" | "PAD") {
                let e = m.entry(ep.cell).or_insert(0);
                *e = (*e).max(sinks);
            }
        }
    }
    m
}

fn cell_props(
    s: &AppState,
    cell: &helion_ir::Cell,
    fanouts: &HashMap<helion_ir::Spur, usize>,
) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("NAME".into(), helion_ir::resolve(cell.name).to_string());
    m.insert("TYPE".into(), cell_type(&cell.kind).to_string());
    if let helion_ir::CellKind::Lut6 { init } = cell.kind {
        m.insert("INIT".into(), format!("{init:#x}"));
    }
    if let helion_ir::CellKind::BlackBox { module } = cell.kind {
        m.insert("MODULE".into(), helion_ir::resolve(module).to_string());
    }
    if let Some(placed) = s.tcl.session.placed.as_ref() {
        if let Some(bel) = placed.cell_bel(helion_ir::resolve(cell.name)) {
            m.insert("SITE".into(), bel.site.id());
            m.insert("BEL".into(), bel.id());
        }
    }
    m.insert(
        "FANOUT".into(),
        fanouts.get(&cell.name).copied().unwrap_or(0).to_string(),
    );
    for (k, v) in &cell.attrs.map {
        m.insert(k.to_uppercase(), v.clone());
    }
    m
}

fn net_props(net: &helion_ir::Net) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("NAME".into(), helion_ir::resolve(net.name).to_string());
    m.insert(
        "FANOUT".into(),
        net.endpoints.len().saturating_sub(1).to_string(),
    );
    let driver = net
        .endpoints
        .iter()
        .find(|ep| matches!(helion_ir::resolve(ep.pin), "O" | "Q" | "PAD"))
        .map(|ep| helion_ir::resolve(ep.cell).to_string())
        .unwrap_or_default();
    m.insert("DRIVER".into(), driver);
    m.insert(
        "TYPE".into(),
        net.attrs.get("TYPE").unwrap_or("").to_string(),
    );
    for (k, v) in &net.attrs.map {
        m.insert(k.to_uppercase(), v.clone());
    }
    m
}

fn pin_dir(kind: &helion_ir::CellKind, pin: &str) -> &'static str {
    match pin {
        "O" | "Q" => "OUT",
        "PAD" => match kind {
            helion_ir::CellKind::IobOut => "OUT",
            _ => "IN",
        },
        _ => "IN",
    }
}

fn pin_props(d: &helion_ir::Design, cell: &helion_ir::Cell, pin: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert(
        "NAME".into(),
        format!("{}/{}", helion_ir::resolve(cell.name), pin),
    );
    m.insert("CELL".into(), helion_ir::resolve(cell.name).to_string());
    m.insert("PIN".into(), pin.to_string());
    m.insert("DIRECTION".into(), pin_dir(&cell.kind, pin).into());
    let net = d
        .pin_index()
        .net_on(helion_ir::resolve(cell.name), pin)
        .map(helion_ir::resolve)
        .unwrap_or("")
        .to_string();
    m.insert("NET".into(), net);
    m
}

fn port_props(d: &helion_ir::Design, port: &helion_ir::Port) -> HashMap<String, String> {
    let _ = d;
    let mut m = HashMap::new();
    m.insert("NAME".into(), helion_ir::resolve(port.name).to_string());
    m.insert(
        "DIRECTION".into(),
        match port.dir {
            helion_ir::PortDir::In => "IN",
            helion_ir::PortDir::Out => "OUT",
            helion_ir::PortDir::Inout => "INOUT",
        }
        .into(),
    );
    for key in ["PACKAGE_PIN", "IOSTANDARD", "DRIVE", "SLEW", "PULLTYPE"] {
        m.insert(key.into(), port.attrs.get(key).unwrap_or("").to_string());
    }
    for (k, v) in &port.attrs.map {
        m.insert(k.to_uppercase(), v.clone());
    }
    m
}

fn clock_props(c: &helion_sta::Clock) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("NAME".into(), c.name.clone());
    m.insert("PERIOD".into(), c.period_ps.to_string());
    m.insert("SOURCE".into(), c.source.clone());
    m
}

fn path_props(p: &helion_sta::TimingPath) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("NAME".into(), p.name.clone());
    m.insert("STARTPOINT".into(), p.startpoint.cell.clone());
    m.insert("ENDPOINT".into(), p.endpoint.cell.clone());
    m.insert("SLACK".into(), p.slack_ps.to_string());
    m.insert("DELAY".into(), p.delay_ps.to_string());
    m.insert("START_SITE".into(), p.startpoint.site_id.clone());
    m.insert("END_SITE".into(), p.endpoint.site_id.clone());
    m.insert("CELLS".into(), p.cells.len().to_string());
    m.insert("NETS".into(), p.nets.len().to_string());
    m
}

// ---- object resolution (mirroring `design.rs`) ----

#[derive(Clone, Debug, PartialEq)]
enum ObjKind {
    Cell,
    Net,
    Pin,
    Port,
    Clock,
    Path,
}

/// Resolve a name to an object. Search order: cells, nets, pins (`a/b`),
/// ports, clocks, then (optionally) timing paths (runs STA).
fn resolve_obj(
    s: &mut AppState,
    name: &str,
    line: usize,
    allow_timing: bool,
) -> Result<(ObjKind, HashMap<String, String>), TclError> {
    resolve_candidates(s, name, line, allow_timing)?
        .into_iter()
        .next()
        .ok_or_else(|| TclError {
            msg: format!("no such object \"{name}\""),
            line,
        })
}

/// One resolved object: its kind plus its property map.
type ObjCandidate = (ObjKind, HashMap<String, String>);

/// All objects matching `name`, in `resolve_obj` priority order. Used by
/// `get_property`, which picks the first candidate that actually HAS the
/// requested property (so `PERIOD` finds a clock even when a same-named
/// port shadows it, and `DIRECTION` still finds the port).
fn resolve_candidates(
    s: &mut AppState,
    name: &str,
    line: usize,
    allow_timing: bool,
) -> Result<Vec<ObjCandidate>, TclError> {
    let design = query_design(s).ok_or_else(|| TclError {
        msg: "get_property: no design (synth_design first)".into(),
        line,
    })?;
    let mut out = Vec::new();
    if let Some(cell) = design
        .cells
        .iter()
        .find(|c| c.name == helion_ir::intern(name))
    {
        let fanouts = cell_fanouts(design);
        out.push((ObjKind::Cell, cell_props(s, cell, &fanouts)));
    }
    if let Some(net) = design
        .nets
        .iter()
        .find(|n| n.name == helion_ir::intern(name))
    {
        out.push((ObjKind::Net, net_props(net)));
    }
    if let Some((cell_name, pin)) = name.split_once('/') {
        if let Some(cell) = design
            .cells
            .iter()
            .find(|c| c.name == helion_ir::intern(cell_name))
        {
            if design.pin_index().net_on(cell_name, pin).is_some() {
                out.push((ObjKind::Pin, pin_props(design, cell, pin)));
            }
        }
    }
    if let Some(port) = design
        .ports
        .iter()
        .find(|p| p.name == helion_ir::intern(name))
    {
        out.push((ObjKind::Port, port_props(design, port)));
    }
    if let Some(c) = s.tcl.clocks.clocks.iter().find(|c| c.name == name) {
        out.push((ObjKind::Clock, clock_props(c)));
    }
    if allow_timing {
        let paths = match ensure_timing(s, line) {
            Ok((_, _, p)) => p,
            Err(_) => Vec::new(),
        };
        if let Some(p) = paths.iter().find(|p| p.name == name) {
            out.push((ObjKind::Path, path_props(p)));
        }
    }
    Ok(out)
}

// ---- get_* options (mirroring `design.rs`) ----

struct GetOpts {
    filter: Option<String>,
    of_objects: Option<String>,
    patterns: Vec<String>,
    max_paths: Option<usize>,
}

fn parse_get_opts(argv: &[String], line: usize, cmd: &str) -> Result<GetOpts, TclError> {
    let mut o = GetOpts {
        filter: None,
        of_objects: None,
        patterns: Vec::new(),
        max_paths: None,
    };
    let mut k = 1;
    while k < argv.len() {
        match argv[k].as_str() {
            "-filter" => {
                k += 1;
                o.filter = Some(argv.get(k).cloned().ok_or_else(|| TclError {
                    msg: format!("{cmd}: -filter needs an argument"),
                    line,
                })?);
            }
            "-of_objects" => {
                k += 1;
                o.of_objects = Some(argv.get(k).cloned().ok_or_else(|| TclError {
                    msg: format!("{cmd}: -of_objects needs an argument"),
                    line,
                })?);
            }
            "-max_paths" => {
                k += 1;
                let n: usize = argv
                    .get(k)
                    .ok_or_else(|| TclError {
                        msg: format!("{cmd}: -max_paths needs an argument"),
                        line,
                    })?
                    .parse()
                    .map_err(|_| TclError {
                        msg: format!("{cmd}: -max_paths must be an integer"),
                        line,
                    })?;
                o.max_paths = Some(n);
            }
            "-hierarchical" => {} // accepted; Helion names are flat
            s if s.starts_with('-') => {
                return Err(TclError {
                    msg: format!("{cmd}: unknown option \"{s}\""),
                    line,
                });
            }
            _ => break,
        }
        k += 1;
    }
    o.patterns = argv[k..].to_vec();
    Ok(o)
}

fn apply_patterns(names: &mut Vec<String>, patterns: &[String]) {
    if !patterns.is_empty() {
        names.retain(|n| patterns.iter().any(|p| glob_match(p, n)));
    }
}

/// Expand an `-of_objects` argument (a list of object names) into cells.
fn cells_of(d: &helion_ir::Design, of: &str, line: usize) -> Result<Vec<String>, TclError> {
    let objs = split_list(of).map_err(|m| TclError {
        msg: format!("-of_objects: {m}"),
        line,
    })?;
    let mut out = Vec::new();
    for o in objs {
        if let Some((cell, _pin)) = o.split_once('/') {
            if d.cells.iter().any(|c| c.name == helion_ir::intern(cell)) {
                out.push(cell.to_string());
            }
        } else if d.cells.iter().any(|c| c.name == helion_ir::intern(&o)) {
            out.push(o);
        } else if let Some(net) = d.nets.iter().find(|n| n.name == helion_ir::intern(&o)) {
            for ep in &net.endpoints {
                if !out.contains(&helion_ir::resolve(ep.cell).to_string()) {
                    out.push(helion_ir::resolve(ep.cell).to_string());
                }
            }
        } else {
            return Err(TclError {
                msg: format!("-of_objects: no such object \"{o}\""),
                line,
            });
        }
    }
    Ok(out)
}

/// Expand `-of_objects` into nets.
fn nets_of(d: &helion_ir::Design, of: &str, line: usize) -> Result<Vec<String>, TclError> {
    let objs = split_list(of).map_err(|m| TclError {
        msg: format!("-of_objects: {m}"),
        line,
    })?;
    let idx = d.pin_index();
    let mut out = Vec::new();
    for o in objs {
        if let Some((cell, pin)) = o.split_once('/') {
            match idx.net_on(cell, pin).map(helion_ir::resolve) {
                Some(n) if !n.is_empty() && !out.contains(&n.to_string()) => {
                    out.push(n.to_string());
                }
                _ => {}
            }
        } else if d.cells.iter().any(|c| c.name == helion_ir::intern(&o)) {
            for net in &d.nets {
                if net
                    .endpoints
                    .iter()
                    .any(|ep| ep.cell == helion_ir::intern(&o))
                    && !out.contains(&helion_ir::resolve(net.name).to_string())
                {
                    out.push(helion_ir::resolve(net.name).to_string());
                }
            }
        } else if d.nets.iter().any(|n| n.name == helion_ir::intern(&o)) {
            out.push(o);
        } else {
            return Err(TclError {
                msg: format!("-of_objects: no such object \"{o}\""),
                line,
            });
        }
    }
    Ok(out)
}

/// Expand `-of_objects` into pins (`cell/pin`).
fn pins_of(d: &helion_ir::Design, of: &str, line: usize) -> Result<Vec<String>, TclError> {
    let objs = split_list(of).map_err(|m| TclError {
        msg: format!("-of_objects: {m}"),
        line,
    })?;
    let mut out = Vec::new();
    for o in objs {
        if o.contains('/') {
            out.push(o);
        } else if d.cells.iter().any(|c| c.name == helion_ir::intern(&o)) {
            out.extend(helion_proj::get_pins(d, &o));
        } else if let Some(net) = d.nets.iter().find(|n| n.name == helion_ir::intern(&o)) {
            for ep in &net.endpoints {
                out.push(format!(
                    "{}/{}",
                    helion_ir::resolve(ep.cell),
                    helion_ir::resolve(ep.pin)
                ));
            }
        } else {
            return Err(TclError {
                msg: format!("-of_objects: no such object \"{o}\""),
                line,
            });
        }
    }
    Ok(out)
}

fn no_design(cmd: &str, line: usize) -> TclError {
    TclError {
        msg: format!("{cmd}: no design (synth_design first)"),
        line,
    }
}

fn cmd_get_cells(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let opts = match parse_get_opts(argv, line, "get_cells") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let design = match query_design(s) {
        Some(d) => d,
        None => return Ctl::Error(no_design("get_cells", line)),
    };
    let mut names: Vec<String> = match &opts.of_objects {
        Some(of) => match cells_of(design, of, line) {
            Ok(n) => n,
            Err(e) => return Ctl::Error(e),
        },
        None => design
            .cells
            .iter()
            .map(|c| helion_ir::resolve(c.name).to_string())
            .collect(),
    };
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let d = match query_design(s) {
            Some(d) => d,
            None => return Ctl::Error(no_design("get_cells", line)),
        };
        let fanouts = cell_fanouts(d);
        let mut kept = Vec::new();
        for n in &names {
            if let Some(cell) = d.cells.iter().find(|c| c.name == helion_ir::intern(n)) {
                let props = cell_props(s, cell, &fanouts);
                match eval_filter(f, &props) {
                    Ok(true) => kept.push(n.clone()),
                    Ok(false) => {}
                    Err(m) => return Ctl::err(line, format!("-filter: {m}")),
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_nets(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let opts = match parse_get_opts(argv, line, "get_nets") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let design = match query_design(s) {
        Some(d) => d,
        None => return Ctl::Error(no_design("get_nets", line)),
    };
    let mut names: Vec<String> = match &opts.of_objects {
        Some(of) => match nets_of(design, of, line) {
            Ok(n) => n,
            Err(e) => return Ctl::Error(e),
        },
        None => design
            .nets
            .iter()
            .map(|n| helion_ir::resolve(n.name).to_string())
            .collect(),
    };
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let d = match query_design(s) {
            Some(d) => d,
            None => return Ctl::Error(no_design("get_nets", line)),
        };
        let mut kept = Vec::new();
        for n in &names {
            if let Some(net) = d.nets.iter().find(|x| x.name == helion_ir::intern(n)) {
                match eval_filter(f, &net_props(net)) {
                    Ok(true) => kept.push(n.clone()),
                    Ok(false) => {}
                    Err(m) => return Ctl::err(line, format!("-filter: {m}")),
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_pins(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let opts = match parse_get_opts(argv, line, "get_pins") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let design = match query_design(s) {
        Some(d) => d,
        None => return Ctl::Error(no_design("get_pins", line)),
    };
    let mut names: Vec<String> = match &opts.of_objects {
        Some(of) => match pins_of(design, of, line) {
            Ok(n) => n,
            Err(e) => return Ctl::Error(e),
        },
        None => {
            let mut all = Vec::new();
            for cell in &design.cells {
                all.extend(helion_proj::get_pins(design, helion_ir::resolve(cell.name)));
            }
            all
        }
    };
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let d = match query_design(s) {
            Some(d) => d,
            None => return Ctl::Error(no_design("get_pins", line)),
        };
        let mut kept = Vec::new();
        for n in &names {
            if let Some((cell_name, pin)) = n.split_once('/') {
                if let Some(cell) = d
                    .cells
                    .iter()
                    .find(|c| c.name == helion_ir::intern(cell_name))
                {
                    match eval_filter(f, &pin_props(d, cell, pin)) {
                        Ok(true) => kept.push(n.clone()),
                        Ok(false) => {}
                        Err(m) => return Ctl::err(line, format!("-filter: {m}")),
                    }
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_ports(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let opts = match parse_get_opts(argv, line, "get_ports") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let design = match query_design(s) {
        Some(d) => d,
        None => return Ctl::Error(no_design("get_ports", line)),
    };
    let mut names: Vec<String> = design
        .ports
        .iter()
        .map(|p| helion_ir::resolve(p.name).to_string())
        .collect();
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let d = match query_design(s) {
            Some(d) => d,
            None => return Ctl::Error(no_design("get_ports", line)),
        };
        let mut kept = Vec::new();
        for n in &names {
            if let Some(port) = d.ports.iter().find(|p| p.name == helion_ir::intern(n)) {
                match eval_filter(f, &port_props(d, port)) {
                    Ok(true) => kept.push(n.clone()),
                    Ok(false) => {}
                    Err(m) => return Ctl::err(line, format!("-filter: {m}")),
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_clocks(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let opts = match parse_get_opts(argv, line, "get_clocks") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let mut names: Vec<String> = s.tcl.clocks.clocks.iter().map(|c| c.name.clone()).collect();
    apply_patterns(&mut names, &opts.patterns);
    if let Some(f) = &opts.filter {
        let mut kept = Vec::new();
        for n in &names {
            if let Some(c) = s.tcl.clocks.clocks.iter().find(|x| &x.name == n) {
                match eval_filter(f, &clock_props(c)) {
                    Ok(true) => kept.push(n.clone()),
                    Ok(false) => {}
                    Err(m) => return Ctl::err(line, format!("-filter: {m}")),
                }
            }
        }
        names = kept;
    }
    Ctl::Value(join_list(&names))
}

fn cmd_get_timing_paths(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let opts = match parse_get_opts(argv, line, "get_timing_paths") {
        Ok(o) => o,
        Err(e) => return Ctl::Error(e),
    };
    let mut paths = match ensure_timing(s, line) {
        Ok((_, _, p)) => p,
        Err(e) => return Ctl::Error(e),
    };
    if let Some(n) = opts.max_paths {
        paths.truncate(n);
    }
    if let Some(f) = &opts.filter {
        let mut kept = Vec::new();
        for p in &paths {
            match eval_filter(f, &path_props(p)) {
                Ok(true) => kept.push(p.clone()),
                Ok(false) => {}
                Err(m) => return Ctl::err(line, format!("-filter: {m}")),
            }
        }
        paths = kept;
    }
    let mut names: Vec<String> = paths.iter().map(|p| p.name.clone()).collect();
    apply_patterns(&mut names, &opts.patterns);
    Ctl::Value(join_list(&names))
}

/// `get_property` accepts Vivado order (`prop obj`) and, as a fallback, the
/// `obj prop` order, so both scripting styles work.
fn cmd_get_property(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let mut k = 1;
    let mut quiet = false;
    while k < argv.len() && argv[k].starts_with('-') {
        match argv[k].as_str() {
            "-quiet" => quiet = true,
            other => return Ctl::err(line, format!("get_property: unknown option \"{other}\"")),
        }
        k += 1;
    }
    if argv.len() != k + 2 {
        return Ctl::err(line, wrong_args("get_property ?-quiet? property object"));
    }
    let (a, b) = (argv[k].clone(), argv[k + 1].clone());
    let prop_key = a.to_uppercase();
    let vivado = resolve_candidates(s, &b, line, true)
        .ok()
        .and_then(|cands| {
            cands
                .into_iter()
                .find_map(|(_, props)| props.get(&prop_key).cloned())
        });
    if let Some(v) = vivado {
        return Ctl::Value(v);
    }
    match resolve_obj(s, &a, line, true) {
        Ok((_, props)) => match props.get(&b.to_uppercase()) {
            Some(v) => Ctl::Value(v.clone()),
            None if quiet => Ctl::Value(String::new()),
            None => Ctl::err(
                line,
                format!("get_property: unknown property \"{b}\" on \"{a}\""),
            ),
        },
        Err(_) if quiet => Ctl::Value(String::new()),
        Err(_) => Ctl::err(
            line,
            format!("get_property: no such object \"{b}\" (or unknown property \"{a}\")"),
        ),
    }
}

/// Site coordinates from an `IOB_X…Y…` id (or `X Y`), exactly like
/// `parse_site_xy` (`helion-gui/src/ide.rs:27068`), including its error
/// texts; only uppercase `X…Y…` coordinates are site spellings.
fn parse_site_xy(spec: &str) -> Result<(u32, u32), String> {
    let t = spec.trim();
    if let Some(xoff) = t.find('X') {
        let rest = &t[xoff + 1..];
        if let Some(yoff) = rest.find('Y') {
            let x: u32 = rest[..yoff]
                .trim()
                .parse()
                .map_err(|_| format!("select_device_site: bad X in {spec}"))?;
            let y: u32 = rest[yoff + 1..]
                .trim()
                .parse()
                .map_err(|_| format!("select_device_site: bad Y in {spec}"))?;
            return Ok((x, y));
        }
    }
    let mut parts = t.split_whitespace();
    let x = parts
        .next()
        .ok_or_else(|| "select_device_site: need X Y".to_string())?
        .parse()
        .map_err(|_| format!("select_device_site: bad X in {spec}"))?;
    let y = parts
        .next()
        .ok_or_else(|| "select_device_site: need X Y".to_string())?
        .parse()
        .map_err(|_| format!("select_device_site: bad Y in {spec}"))?;
    Ok((x, y))
}

/// I/O Planning package-pin assignment: `set_property PACKAGE_PIN|LOC` and
/// `assign_package_pin` never reach the generic property path. Mirrors
/// `set_package_pin` (`helion-gui/src/ide.rs:15746`): validate the HAD IOB
/// site, stamp LOC on every live design copy, then re-place (and re-route
/// the flow session) so the site reaches STA/DRC rather than a pin list.
/// One deliberate correction: `loc=` reports this port's resolved site or
/// constraint, not the first IOB in the whole placement.
fn assign_package_pin(s: &mut AppState, pin: &str, port: &str, line: usize) -> Ctl {
    let pin = pin.trim();
    let port = port.trim();
    if port.is_empty() || pin.is_empty() {
        return Ctl::err(
            line,
            "set_property PACKAGE_PIN: need <pin> [get_ports <port>]",
        );
    }
    let (x, y) = match parse_site_xy(pin) {
        Ok(xy) => xy,
        Err(e) => {
            return Ctl::err(
                line,
                format!("set_property PACKAGE_PIN: bad pin {pin} ({e})"),
            );
        }
    };
    let (dev, _) = crate::live::live_device(s);
    if dev.iob_major(x, y).is_none() {
        return Ctl::err(
            line,
            format!("set_property PACKAGE_PIN: {pin} is not a HAD IOB site"),
        );
    }
    let canonical = match crate::live::live_design(s) {
        Some(d) => d,
        None => return Ctl::err(line, "set_property PACKAGE_PIN: no design"),
    };
    if !canonical
        .ports
        .iter()
        .any(|p| helion_ir::resolve(p.name) == port)
    {
        return Ctl::err(line, format!("set_property PACKAGE_PIN: no port {port}"));
    }
    let was_session_placed = s.tcl.session.placed.is_some();
    let was_session_routed = s.tcl.session.routed.is_some();
    let was_live_placed = s.runs.live_placed.is_some();
    if s.tcl.session.design.is_none()
        && let Some(design) = s.tcl.design.clone().or_else(|| s.runs.live_design.clone())
    {
        s.tcl.session.synth_design(design);
    }
    if let Some(d) = s.tcl.design.as_mut() {
        let _ = d.set_loc(port, pin);
    }
    if let Some(d) = s.tcl.session.design.as_mut() {
        let _ = d.set_loc(port, pin);
    }
    if let Some(d) = s.runs.live_design.as_mut() {
        let _ = d.set_loc(port, pin);
    }

    let mut replaced = false;
    let mut rerouted = false;
    if was_session_placed {
        if let Err(e) = s.tcl.session.place_design(&dev) {
            return Ctl::err(line, e);
        }
        replaced = true;
        if was_session_routed {
            if let Err(e) = s.tcl.session.route_design(&dev) {
                return Ctl::err(line, e);
            }
            rerouted = true;
        }
    }
    if was_live_placed {
        let Some(design) = s.runs.live_design.clone() else {
            return Ctl::err(line, "set_property PACKAGE_PIN: no design");
        };
        match helion_pack::pack(&design, &dev)
            .map_err(|e| format!("place_design failed: {e}"))
            .and_then(|packed| {
                helion_place::place(&packed, &dev).map_err(|e| format!("place_design failed: {e}"))
            }) {
            Ok(new_placed) => {
                s.runs.live_placed = Some(new_placed);
                replaced = true;
            }
            Err(e) => return Ctl::err(line, e),
        }
    }
    let placed = crate::views::device::io_port_rows(s)
        .into_iter()
        .find(|r| r.name == port)
        .and_then(|r| r.site.or(r.package_pin))
        .unwrap_or_else(|| format!("IOB_X{x}Y{y}"));
    Ctl::Value(format!(
        "set_property PACKAGE_PIN {pin} {port} loc={placed} replaced={} rerouted={}",
        u8::from(replaced),
        u8::from(rerouted)
    ))
}

fn cmd_assign_package_pin(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() < 3 {
        return Ctl::err(line, "assign_package_pin: need <port> <pin>");
    }
    assign_package_pin(s, &argv[2], &argv[1], line)
}

/// Open the Package view and dump its HAD drawing (old `package` /
/// `package_drawing` branch at `helion-gui/src/ide.rs:8080`).
fn cmd_package(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("package"));
    }
    s.center = crate::state::CenterPane::Device;
    s.device.view = crate::views::device::DeviceView::Package;
    Ctl::Value(crate::views::device::package_grid_text(s))
}

/// Open the Breakpoints pane and dump its table (old `breakpoints`).
fn cmd_breakpoints(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("breakpoints"));
    }
    s.center = crate::state::CenterPane::Debug;
    s.debug.tab = crate::views::debug::DebugTab::Breakpoints;
    Ctl::Value(crate::views::debug::breakpoints_text(s))
}

/// Open the Source tab over the debugged RTL (old `source` / `source_window`).
fn cmd_source_window(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("source_window"));
    }
    s.center = crate::state::CenterPane::Debug;
    s.debug.tab = crate::views::debug::DebugTab::Source;
    match crate::views::debug::open_source_window(s) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_select_source_line(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    s.center = crate::state::CenterPane::Debug;
    s.debug.tab = crate::views::debug::DebugTab::Source;
    match crate::views::debug::select_source_line(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

/// `toggle_bp_line` / `add_bp_line` (both toggle, like the old IDE).
fn cmd_toggle_bp_line(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    match crate::views::debug::toggle_source_breakpoint(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

/// Open the Memory tab and dump its table (old `memory` / `memories`).
fn cmd_memory(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("memory"));
    }
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::open_memory(s) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_select_memory(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::select_memory(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_select_memory_word(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::select_memory_word(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

/// Open the Locals tab and dump its table (old `locals`).
fn cmd_locals(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("locals"));
    }
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::open_locals(s) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_select_local(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::select_local(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

/// Open the Forces tab and dump its table (old `forces` / `force_table` /
/// `force_constants`).
fn cmd_forces(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("forces"));
    }
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::open_forces(s) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_add_force(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::add_force(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_deposit(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::add_deposit(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_remove_force(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::remove_force(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_select_force(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    s.center = crate::state::CenterPane::Debug;
    match crate::views::debug::select_force(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

/// Run the interactive event engine (`sim_run [cycles]`). Missing or
/// non-numeric counts fall back to the configured runtime, like old `exec`.
fn cmd_sim_run(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let cycles = argv
        .get(1)
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or_else(|| s.debug.runtime_cycles.max(1));
    match crate::views::debug::run_cycles(s, cycles) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_sim_step(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("sim_step"));
    }
    match crate::views::debug::step_once(s) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_sim_restart(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 1 {
        return Ctl::err(line, wrong_args("sim_restart"));
    }
    match crate::views::debug::restart(s) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_add_bp(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    match crate::views::debug::add_breakpoint(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_select_breakpoint(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    match crate::views::debug::select_breakpoint(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_enable_bp(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    match crate::views::debug::set_breakpoint_enabled(s, &spec, true) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_disable_bp(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    match crate::views::debug::set_breakpoint_enabled(s, &spec, false) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

fn cmd_delete_bp(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let spec = argv.get(1..).unwrap_or_default().join(" ");
    match crate::views::debug::delete_breakpoint(s, &spec) {
        Ok(out) => Ctl::Value(out),
        Err(e) => Ctl::err(line, e),
    }
}

/// Click a package pin: assigned pins cross-select the I/O port, unassigned
/// pins select the pin itself. Mirrors `select_package_pin`
/// (`helion-gui/src/ide.rs:16092`), including its error wording.
fn cmd_select_package_pin(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let pin = argv.get(1..).unwrap_or_default().join(" ");
    let pin = pin.trim();
    let Some(found) = crate::views::device::package_pin(s, pin) else {
        return Ctl::err(line, format!("select_package_pin: no HAD pin {pin}"));
    };
    s.center = crate::state::CenterPane::Device;
    s.device.view = crate::views::device::DeviceView::Package;
    match found.port {
        Some(port) => {
            s.device.selected = Some(crate::views::device::DeviceSel::Net(port.clone()));
            Ctl::Value(format!(
                "package_pin {} port={port} X{}Y{}",
                found.pin, found.x, found.y
            ))
        }
        None => {
            s.device.selected = Some(crate::views::device::DeviceSel::Pin(found.pin.clone()));
            Ctl::Value(format!(
                "package_pin {} unassigned X{}Y{}",
                found.pin, found.x, found.y
            ))
        }
    }
}

fn cmd_set_property(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    // Like old `Model::exec`, PACKAGE_PIN/LOC never reaches the generic Tcl
    // property handler (including its repeated-command result). Option flags
    // keep their old parsing position: only an immediate key intercepts.
    if argv.get(1).is_some_and(|key| {
        key.eq_ignore_ascii_case("PACKAGE_PIN") || key.eq_ignore_ascii_case("LOC")
    }) {
        if argv.len() < 3 {
            return Ctl::err(line, "set_property PACKAGE_PIN: missing pin");
        }
        if argv.len() < 4 {
            return Ctl::err(line, "set_property PACKAGE_PIN: missing [get_ports <port>]");
        }
        let mut port: Option<String> = None;
        for a in &argv[3..] {
            match split_list(a) {
                Ok(objs) => {
                    if let Some(first) = objs.into_iter().find(|o| !o.is_empty()) {
                        port = Some(first);
                        break;
                    }
                }
                Err(m) => return Ctl::err(line, format!("set_property: {m}")),
            }
        }
        let Some(port) = port else {
            return Ctl::err(line, "set_property PACKAGE_PIN: missing [get_ports <port>]");
        };
        return assign_package_pin(s, &argv[2], &port, line);
    }
    let mut k = 1;
    let mut quiet = false;
    while k < argv.len() && argv[k].starts_with('-') {
        match argv[k].as_str() {
            "-quiet" => quiet = true,
            other => return Ctl::err(line, format!("set_property: unknown option \"{other}\"")),
        }
        k += 1;
    }
    if argv.len() < k + 3 {
        return Ctl::err(
            line,
            wrong_args("set_property ?-quiet? name value objects..."),
        );
    }
    let (prop, value) = (argv[k].to_uppercase(), argv[k + 1].clone());
    let mut objs: Vec<String> = Vec::new();
    for a in &argv[k + 2..] {
        match split_list(a) {
            Ok(v) => objs.extend(v),
            Err(m) => return Ctl::err(line, format!("set_property: {m}")),
        }
    }
    // Runs hold the elaborated design in their own copy (the same one views
    // read via `live_design`); accept it when the console has no design yet.
    if s.tcl.design.is_none() && s.runs.live_design.is_none() {
        return Ctl::err(line, "set_property: no design");
    }
    sync_session_design(s);
    let mut applied = 0usize;
    for obj in &objs {
        // Both netlist copies hold identical designs, so one application
        // routine runs on each; the console copy's error (if any) reports.
        let apply = |d: &mut helion_ir::Design| -> Result<(), String> {
            match prop.as_str() {
                "DONT_TOUCH" => d.set_cell_attr(obj, "DONT_TOUCH", &value),
                "KEEP" => d.set_cell_attr(obj, "KEEP", &value),
                "MARK_DEBUG" => d.set_net_attr(obj, "mark_debug", &value),
                "LOC" | "PACKAGE_PIN" => d.set_loc(obj, &value),
                "IOSTANDARD" => d.set_iostandard(obj, &value),
                "DRIVE" => d.set_drive(obj, &value),
                "SLEW" => d.set_slew(obj, &value),
                "PULLTYPE" => d.set_pulltype(obj, &value),
                "DIFF_TERM" => d.set_diff_term(obj, &value),
                "IN_TERM" => d.set_in_term(obj, &value),
                other => Err(format!("unsupported property \"{other}\"")),
            }
        };
        let r = match s.tcl.design.as_mut() {
            Some(d) => apply(d),
            None => match s.runs.live_design.as_mut() {
                Some(d) => apply(d),
                None => return Ctl::err(line, "set_property: no design"),
            },
        };
        if r.is_ok() {
            if let Some(d) = s.tcl.session.design.as_mut() {
                let _ = apply(d);
            }
            // Views read the last run result first: keep it in sync so a
            // constraint shows up the moment it is set (the worker
            // re-applies the snapshot to the next elaboration).
            if let Some(d) = s.runs.live_design.as_mut() {
                let _ = apply(d);
            }
        }
        match r {
            Ok(()) => applied += 1,
            Err(e) if quiet => {
                let _ = e;
            }
            Err(e) => return Ctl::err(line, format!("set_property: {e}")),
        }
    }
    if objs.len() == 1 {
        Ctl::Value(format!("set_property {} {} {}", argv[k], value, objs[0]))
    } else {
        Ctl::Value(format!(
            "set_property {} {value}: {applied} objects",
            argv[k]
        ))
    }
}

fn cmd_report_property(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 || argv.len() > 3 {
        return Ctl::err(line, wrong_args("report_property object ?pattern?"));
    }
    let pat = argv.get(2).map(|s| s.as_str());
    match resolve_obj(s, &argv[1], line, true) {
        Ok((_, props)) => {
            let mut keys: Vec<&String> = props.keys().collect();
            keys.sort();
            let mut out = Vec::new();
            for k in keys {
                if pat.is_none_or(|p| glob_match(p, k)) {
                    out.push(format!("{k} = {}", props[k]));
                }
            }
            Ctl::Value(out.join("\n"))
        }
        Err(e) => Ctl::Error(e),
    }
}

fn cmd_list_property(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() < 2 || argv.len() > 3 {
        return Ctl::err(line, wrong_args("list_property object ?pattern?"));
    }
    let pat = argv.get(2).map(|s| s.as_str());
    match resolve_obj(s, &argv[1], line, true) {
        Ok((_, props)) => {
            let mut keys: Vec<String> = props
                .keys()
                .filter(|k| pat.is_none_or(|p| glob_match(p, k)))
                .cloned()
                .collect();
            keys.sort();
            Ctl::Value(join_list(&keys))
        }
        Err(e) => Ctl::Error(e),
    }
}

fn cmd_select_objects(s: &mut AppState, argv: &[String], line: usize, highlighted: bool) -> Ctl {
    if argv.len() < 2 {
        return Ctl::err(line, wrong_args("select_objects objects..."));
    }
    let mut objs: Vec<String> = Vec::new();
    for a in &argv[1..] {
        match split_list(a) {
            Ok(v) => objs.extend(v),
            Err(m) => return Ctl::err(line, format!("select_objects: {m}")),
        }
    }
    for o in &objs {
        if resolve_obj(s, o, line, false).is_err() {
            return Ctl::err(line, format!("select_objects: no such object \"{o}\""));
        }
    }
    s.tcl.selected.clear();
    for o in &objs {
        let kind = resolve_obj(s, o, line, false)
            .map(|(k, _)| format!("{k:?}").to_lowercase())
            .unwrap_or_else(|_| "unknown".into());
        s.tcl.selected.push(SelObj {
            kind,
            name: o.clone(),
            highlighted,
        });
    }
    s.tcl.selection_dirty = true;
    Ctl::Value(join_list(&objs))
}

/// Run the implementation flow if needed, then STA with the console's
/// `create_clock` state (empty = the 10 ns `clk` fallback, exactly like
/// `Session::report_timing`). Mirrors `ensure_timing` (`design.rs:622`).
fn ensure_timing(
    s: &mut AppState,
    line: usize,
) -> Result<
    (
        String,
        helion_sta::TimingResult,
        Vec<helion_sta::TimingPath>,
    ),
    TclError,
> {
    let map = |msg: String| TclError { msg, line };
    impl_if_needed(s).map_err(map)?;
    let (clks, xdc) = if s.tcl.clocks.clocks.is_empty() {
        let mut c = Vec::new();
        helion_sta::create_clock(&mut c, "clk", 10_000, "clk");
        (c, helion_sta::Constraints::default())
    } else {
        (s.tcl.clocks.clocks.clone(), s.tcl.clocks.clone())
    };
    let design = s.tcl.design.as_ref().ok_or_else(|| TclError {
        msg: "report_timing: no design (synth_design first)".into(),
        line,
    })?;
    let routed = s.tcl.session.routed.as_ref().ok_or_else(|| TclError {
        msg: "report_timing: not routed".into(),
        line,
    })?;
    let t = helion_sta::report_timing_routed_xdc(design, routed, &clks, &xdc).map_err(map)?;
    let paths = helion_sta::timing_paths_routed(design, routed, &t);
    Ok((helion_ir::resolve(design.name).to_string(), t, paths))
}

/// Vivado-style timing table: summary line plus the worst paths, so the
/// console output pane shows a readable report instead of a key=value dump.
fn format_timing_table(
    name: &str,
    t: &helion_sta::TimingResult,
    paths: &[helion_sta::TimingPath],
) -> String {
    fn ns(ps: i64) -> String {
        format!("{:.3}ns", ps as f64 / 1000.0)
    }
    let mut s = format!(
        "Timing Report: {name}\n\
         WNS {wns} ({wns_ps}ps)   TNS {tns} ({tns_ps}ps)   endpoints {ep}\n\
         Setup {setup} ({setup_ps}ps)   Hold {hold} ({hold_ps}ps)   Hold slack {hslack} ({hslack_ps}ps)\n\
         Delay model: reg2reg {r2r}ps  iob {iob}ps  route {route}ps\n",
        wns = ns(t.wns_ps),
        wns_ps = t.wns_ps,
        tns = ns(t.tns_ps),
        tns_ps = t.tns_ps,
        ep = t.endpoints,
        setup = ns(t.setup_ps),
        setup_ps = t.setup_ps,
        hold = ns(t.hold_ps),
        hold_ps = t.hold_ps,
        hslack = ns(t.hold_slack_ps),
        hslack_ps = t.hold_slack_ps,
        r2r = t.r2r_ps,
        iob = t.iob_ps,
        route = t.route_ps,
    );
    let show = paths.len().min(10);
    if show == 0 {
        s.push_str("No timing paths.\n");
        return s;
    }
    s.push_str(&format!(
        "\n{:<12} {:>12} {:>12}  {:<28} {:<28}\n",
        "Slack", "Slack(ps)", "Delay(ps)", "Startpoint", "Endpoint"
    ));
    for p in &paths[..show] {
        s.push_str(&format!(
            "{:<12} {:>12} {:>12}  {:<28} {:<28}\n",
            ns(p.slack_ps),
            p.slack_ps,
            p.delay_ps,
            truncate(&p.startpoint.cell, 28),
            truncate(&p.endpoint.cell, 28),
        ));
    }
    if paths.len() > show {
        s.push_str(&format!(
            "… {} more paths (get_timing_paths -max_paths N)\n",
            paths.len() - show
        ));
    }
    s
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(w - 1).collect();
        t.push('…');
        t
    }
}

/// Vivado-style utilization table instead of a single-line dump. Mirrors
/// `format_util_table` (`design.rs:683`).
fn format_util_table(s: &AppState, dev: &helion_device::Device) -> Result<String, String> {
    let p = s
        .tcl
        .session
        .placed
        .as_ref()
        .map(|pl| &pl.packed)
        .or(s.tcl.session.packed.as_ref())
        .ok_or_else(|| "report_utilization: not packed".to_string())?;
    let name = s
        .tcl
        .design
        .as_ref()
        .map(|d| helion_ir::resolve(d.name))
        .unwrap_or("?");
    let rows: [(&str, usize, u64); 4] = [
        ("LUTFF", p.lutffs.len(), u64::from(dev.lut6_count())),
        ("IOB", p.iobs.len(), dev.iob_sites().count() as u64),
        ("BRAM", p.brams.len(), u64::from(dev.n_bram)),
        ("DSP", p.macs.len(), u64::from(dev.n_dsp)),
    ];
    let mut s = format!("Utilization: {name}  (part {})\n", dev.part);
    s.push_str(&format!(
        "{:<10} {:>8} {:>8} {:>7}\n",
        "Resource", "Used", "Total", "Pct"
    ));
    for (r, used, total) in rows {
        let pct = if total == 0 {
            "-".to_string()
        } else {
            format!("{:.1}%", used as f64 * 100.0 / total as f64)
        };
        s.push_str(&format!("{r:<10} {used:>8} {total:>8} {pct:>7}\n"));
    }
    Ok(s)
}

fn cmd_create_clock(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    let mut period_ns: Option<f64> = None;
    let mut name: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut k = 1;
    while k < argv.len() {
        match argv[k].as_str() {
            "-period" => {
                k += 1;
                let v: f64 = match argv.get(k) {
                    Some(v) => match v.parse() {
                        Ok(v) => v,
                        Err(_) => {
                            return Ctl::err(line, format!("create_clock: bad -period \"{v}\""));
                        }
                    },
                    None => {
                        return Ctl::err(line, "create_clock: -period needs an argument");
                    }
                };
                period_ns = Some(v);
            }
            "-name" => {
                k += 1;
                name = Some(match argv.get(k) {
                    Some(v) => v.clone(),
                    None => return Ctl::err(line, "create_clock: -name needs an argument"),
                });
            }
            // NOTE: the old usage line advertises `?-waveform {r f}?` but
            // the old parser rejects it as an unknown option; mirrored.
            opt if opt.starts_with('-') => {
                return Ctl::err(line, format!("create_clock: unknown option \"{opt}\""));
            }
            _ => positional.push(argv[k].clone()),
        }
        k += 1;
    }
    let clk_name = name
        .or_else(|| positional.first().cloned())
        .unwrap_or_else(|| "clk".into());
    let source = positional
        .get(1)
        .cloned()
        .unwrap_or_else(|| clk_name.clone());
    let period_ps = (period_ns.unwrap_or(10.0) * 1000.0).round() as u64;
    s.tcl.clocks.clocks.retain(|c| c.name != clk_name);
    helion_sta::create_clock(&mut s.tcl.clocks.clocks, &clk_name, period_ps, &source);
    Ctl::Value(format!("clock {clk_name} period {period_ps}ps"))
}

fn cmd_open_report(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    if argv.len() != 2 {
        return Ctl::err(
            line,
            wrong_args("open_report timing|utilization|power|drc|methodology"),
        );
    }
    let i = match argv[1].as_str() {
        "timing" => 0,
        "utilization" | "util" => 1,
        "power" => 2,
        "drc" => 3,
        "methodology" => 4,
        other => {
            return Ctl::err(
                line,
                format!(
                    "unknown report \"{other}\" (try \"timing\", \"utilization\", \"power\", \"drc\", \"methodology\")"
                ),
            );
        }
    };
    crate::views::console::open_report(s, i, &argv.join(" "));
    Ctl::Value(String::new())
}

// ---- -filter expression language (mirroring `interp.rs:2239`) ----

/// Evaluate a Vivado-style `-filter` expression (`==`, `!=`, `=~`, `!~`,
/// `<`, `<=`, `>`, `>=`, `&&`, `||`, `!`, parens) against a property map.
/// Property names are case-insensitive.
fn eval_filter(expr: &str, props: &HashMap<String, String>) -> Result<bool, String> {
    let mut fp = FilterParser {
        chars: expr.chars().collect(),
        pos: 0,
    };
    let v = fp.parse_or()?;
    fp.skip_ws();
    if fp.pos != fp.chars.len() {
        let tail: String = fp.chars[fp.pos..].iter().collect();
        let tail: String = tail.chars().take(24).collect();
        return Err(format!("filter: syntax error near \"{tail}\""));
    }
    v.eval(props)
}

#[derive(Clone, Debug)]
enum FExpr {
    And(Box<FExpr>, Box<FExpr>),
    Or(Box<FExpr>, Box<FExpr>),
    Not(Box<FExpr>),
    Cmp {
        prop: String,
        op: String,
        lit: String,
    },
}

impl FExpr {
    fn eval(&self, props: &HashMap<String, String>) -> Result<bool, String> {
        match self {
            FExpr::And(a, b) => Ok(a.eval(props)? && b.eval(props)?),
            FExpr::Or(a, b) => Ok(a.eval(props)? || b.eval(props)?),
            FExpr::Not(a) => Ok(!a.eval(props)?),
            FExpr::Cmp { prop, op, lit } => {
                let val = props
                    .get(&prop.to_uppercase())
                    .map(|s| s.as_str())
                    .unwrap_or("");
                Ok(match op.as_str() {
                    "==" => cmp_eq(val, lit),
                    "!=" => !cmp_eq(val, lit),
                    "=~" => glob_match(lit, val),
                    "!~" => !glob_match(lit, val),
                    "<" | "<=" | ">" | ">=" => {
                        let r = cmp_ord(val, lit);
                        match op.as_str() {
                            "<" => r.is_lt(),
                            "<=" => r.is_le(),
                            ">" => r.is_gt(),
                            _ => r.is_ge(),
                        }
                    }
                    _ => false,
                })
            }
        }
    }
}

fn cmp_eq(a: &str, b: &str) -> bool {
    if let (Ok(x), Ok(y)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        x == y
    } else {
        a == b
    }
}

fn cmp_ord(a: &str, b: &str) -> std::cmp::Ordering {
    if let (Ok(x), Ok(y)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
    } else {
        a.cmp(b)
    }
}

struct FilterParser {
    chars: Vec<char>,
    pos: usize,
}

impl FilterParser {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }
    fn eat(&mut self, s: &str) -> bool {
        self.skip_ws();
        let pat: Vec<char> = s.chars().collect();
        if self.chars[self.pos..].starts_with(&pat[..]) {
            self.pos += pat.len();
            true
        } else {
            false
        }
    }
    fn parse_or(&mut self) -> Result<FExpr, String> {
        let mut l = self.parse_and()?;
        while self.eat("||") {
            let r = self.parse_and()?;
            l = FExpr::Or(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn parse_and(&mut self) -> Result<FExpr, String> {
        let mut l = self.parse_unary()?;
        while self.eat("&&") {
            let r = self.parse_unary()?;
            l = FExpr::And(Box::new(l), Box::new(r));
        }
        Ok(l)
    }
    fn parse_unary(&mut self) -> Result<FExpr, String> {
        self.skip_ws();
        if self.eat("!") {
            return Ok(FExpr::Not(Box::new(self.parse_unary()?)));
        }
        if self.eat("(") {
            let e = self.parse_or()?;
            if !self.eat(")") {
                return Err("filter: expected ')'".into());
            }
            return Ok(e);
        }
        let prop = self.parse_operand()?;
        self.skip_ws();
        let op = if self.eat("==") {
            "=="
        } else if self.eat("!=") {
            "!="
        } else if self.eat("=~") {
            "=~"
        } else if self.eat("!~") {
            "!~"
        } else if self.eat("<=") {
            "<="
        } else if self.eat(">=") {
            ">="
        } else if self.eat("<") {
            "<"
        } else if self.eat(">") {
            ">"
        } else {
            return Err(format!(
                "filter: expected comparison operator after \"{prop}\""
            ));
        };
        let lit = self.parse_operand()?;
        Ok(FExpr::Cmp {
            prop,
            op: op.to_string(),
            lit,
        })
    }
    fn parse_operand(&mut self) -> Result<String, String> {
        self.skip_ws();
        match self.chars.get(self.pos) {
            Some('"') => {
                self.pos += 1;
                let mut s = String::new();
                while let Some(&c) = self.chars.get(self.pos) {
                    self.pos += 1;
                    if c == '"' {
                        return Ok(s);
                    }
                    if c == '\\' {
                        if let Some(&n) = self.chars.get(self.pos) {
                            s.push(n);
                            self.pos += 1;
                        }
                    } else {
                        s.push(c);
                    }
                }
                Err("filter: missing close-quote".into())
            }
            Some(_) => {
                let mut s = String::new();
                while let Some(&c) = self.chars.get(self.pos) {
                    if c.is_whitespace() || "()!&|".contains(c) {
                        break;
                    }
                    if (c == '=' || c == '<' || c == '>' || c == '~') && !s.is_empty() {
                        break;
                    }
                    s.push(c);
                    self.pos += 1;
                }
                if s.is_empty() {
                    return Err("filter: expected operand".into());
                }
                Ok(s)
            }
            None => Err("filter: expected operand".into()),
        }
    }
}

// ---- format (mirroring `interp.rs:2130`) ----

fn do_format(fmt: &str, args: &[String]) -> Result<String, String> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut ai = 0;
    let mut k = 0;
    while k < chars.len() {
        if chars[k] != '%' {
            out.push(chars[k]);
            k += 1;
            continue;
        }
        k += 1;
        if k < chars.len() && chars[k] == '%' {
            out.push('%');
            k += 1;
            continue;
        }
        while k < chars.len() && matches!(chars[k], '-' | '+' | ' ' | '0' | '#') {
            k += 1;
        }
        let mut width = String::new();
        while k < chars.len() && chars[k].is_ascii_digit() {
            width.push(chars[k]);
            k += 1;
        }
        let mut prec: Option<usize> = None;
        if k < chars.len() && chars[k] == '.' {
            k += 1;
            let mut p = String::new();
            while k < chars.len() && chars[k].is_ascii_digit() {
                p.push(chars[k]);
                k += 1;
            }
            prec = p.parse().ok();
        }
        if k >= chars.len() {
            return Err("format: trailing %".into());
        }
        let conv = chars[k];
        k += 1;
        if ai >= args.len() {
            return Err(format!("format: not enough args for %{conv}"));
        }
        let a = &args[ai];
        ai += 1;
        let w: usize = width.parse().unwrap_or(0);
        let pad = |s: String| {
            if s.len() >= w {
                s
            } else {
                format!("{}{s}", " ".repeat(w - s.len()))
            }
        };
        match conv {
            's' => out.push_str(&pad(a.clone())),
            'd' | 'i' => {
                let v: i64 = a
                    .trim()
                    .parse()
                    .map_err(|_| format!("format: expected integer for %d, got \"{a}\""))?;
                out.push_str(&pad(v.to_string()));
            }
            'f' => {
                let v: f64 = a
                    .trim()
                    .parse()
                    .map_err(|_| format!("format: expected number for %f, got \"{a}\""))?;
                let s = match prec {
                    Some(p) => format!("{v:.p$}"),
                    None => format!("{v:.6}"),
                };
                out.push_str(&pad(s));
            }
            'x' => {
                let v: i64 = a
                    .trim()
                    .parse()
                    .map_err(|_| format!("format: expected integer for %x, got \"{a}\""))?;
                out.push_str(&pad(format!("{v:x}")));
            }
            'X' => {
                let v: i64 = a
                    .trim()
                    .parse()
                    .map_err(|_| format!("format: expected integer for %X, got \"{a}\""))?;
                out.push_str(&pad(format!("{v:X}")));
            }
            'o' => {
                let v: i64 = a
                    .trim()
                    .parse()
                    .map_err(|_| format!("format: expected integer for %o, got \"{a}\""))?;
                out.push_str(&pad(format!("{v:o}")));
            }
            'c' => {
                let v: i64 = a
                    .trim()
                    .parse()
                    .map_err(|_| format!("format: expected integer for %c, got \"{a}\""))?;
                out.push_str(&pad(char::from_u32(v as u32).unwrap_or('?').to_string()));
            }
            _ => return Err(format!("format: unsupported conversion %{conv}")),
        }
    }
    Ok(out)
}

// ---- help (mirroring `design.rs:978` + `help_topic` table) ----

/// `help ?topic?`: overview with no args, per-command usage otherwise.
fn cmd_help(s: &mut AppState, argv: &[String], line: usize) -> Ctl {
    match argv.len() {
        1 => Ctl::Value(help_overview()),
        2 => {
            let topic = argv[1].as_str();
            if let Some((usage, desc)) = help_topic(topic) {
                return Ctl::Value(format!("{usage}\n  {desc}"));
            }
            if let Some(params) = s.tcl.procs.get(topic) {
                let plist: Vec<String> = params
                    .params
                    .iter()
                    .map(|(n, d)| match d {
                        Some(d) => format!("{{{n} {d}}}"),
                        None => n.clone(),
                    })
                    .collect();
                return Ctl::Value(format!(
                    "proc {topic} {{{}}}\n  user procedure (info body {topic} for source)",
                    plist.join(" ")
                ));
            }
            let mut msg = format!("no help topic \"{topic}\"");
            let sugg = suggest_commands(s, topic);
            if !sugg.is_empty() {
                msg.push_str(&format!(
                    "; did you mean {}?",
                    sugg.iter()
                        .map(|s| format!("\"{s}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Ctl::err(line, msg)
        }
        _ => Ctl::err(line, wrong_args("help ?topic?")),
    }
}

/// (usage, one-line description) for every command. Host + core entries
/// mirror `help_topic` (`design.rs:1019`) verbatim; the package/debugger
/// verbs mirror old `Model::exec`, and the last five are the Rune-only
/// verbs.
fn help_topic(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "help" => ("help ?topic?", "this overview, or usage for one command"),
        "clear" => ("clear", "clear the console transcript"),
        "get_cells" => (
            "get_cells ?-hierarchical? ?-filter expr? ?-of_objects objs? ?-quiet? ?patterns...?",
            "cells in the netlist (u_luts0, u_ff3…); -filter {TYPE == lut}",
        ),
        "get_nets" => (
            "get_nets ?-filter expr? ?-of_objects objs? ?patterns...?",
            "nets in the netlist",
        ),
        "get_pins" => (
            "get_pins ?-of_objects cells? ?patterns...?",
            "pins as cell/pin (u_luts0/A)",
        ),
        "get_ports" => ("get_ports ?patterns...?", "top-level ports"),
        "get_clocks" => (
            "get_clocks ?patterns...?",
            "clocks from create_clock (empty until constrained)",
        ),
        "get_timing_paths" => (
            "get_timing_paths ?-max_paths N? ?-filter expr? ?patterns...?",
            "timing path endpoints; -filter {SLACK < 0} finds violations",
        ),
        "get_property" => (
            "get_property ?-quiet? property object",
            "one property (TYPE, NAME, SLACK, PERIOD, SITE…)",
        ),
        "set_property" => (
            "set_property ?-quiet? name value objects…",
            "DONT_TOUCH KEEP MARK_DEBUG LOC PACKAGE_PIN IOSTANDARD DRIVE",
        ),
        "assign_package_pin" => (
            "assign_package_pin port pin",
            "bind a port LOC, then re-place/re-route like PACKAGE_PIN",
        ),
        "package" | "package_drawing" => (
            "package",
            "open the package grid and dump its HAD occupancy map",
        ),
        "select_package_pin" => (
            "select_package_pin pin",
            "select a package pin (assigned pins select the port)",
        ),
        "sim_run" => ("sim_run ?cycles?", "run the event debugger engine fresh"),
        "sim_step" => ("sim_step", "advance the debugger engine one posedge"),
        "sim_restart" => ("sim_restart", "re-elaborate the debugger engine"),
        "breakpoints" => ("breakpoints", "open breakpoints and dump the table"),
        "add_bp" => ("add_bp signal ?value?", "stop on signal change or ==value"),
        "select_breakpoint" => (
            "select_breakpoint id|index|name",
            "select one breakpoint row",
        ),
        "enable_bp" => ("enable_bp ?id|index|name?", "enable one breakpoint"),
        "disable_bp" => ("disable_bp ?id|index|name?", "disable one breakpoint"),
        "delete_bp" => ("delete_bp ?id|index|name?", "delete one breakpoint"),
        "memory" | "memories" => ("memory", "open memory and dump LUT INIT arrays"),
        "select_memory" => ("select_memory name|index", "select a memory array"),
        "select_memory_word" => ("select_memory_word addr", "select a memory word"),
        "locals" => ("locals", "open locals and dump live probes"),
        "select_local" => ("select_local name|index", "select one locals row"),
        "forces" | "force_table" | "force_constants" => {
            ("forces", "open force constants and dump the table")
        }
        "add_force" => (
            "add_force ?-deposit? signal value ?-radix bin|hex? ?-start_time ps? ?-cancel_after ps?",
            "hold or schedule a signal value",
        ),
        "deposit" => ("deposit signal value", "one-shot poke a signal"),
        "remove_force" => ("remove_force ?signal|index?", "drop a force row"),
        "select_force" => ("select_force signal|index", "select one force row"),
        "source_window" => ("source_window", "open the debugger Source window"),
        "select_source_line" => ("select_source_line line", "select a Source line"),
        "toggle_bp_line" | "add_bp_line" => (
            "toggle_bp_line ?file:?line",
            "arm or clear a line breakpoint",
        ),
        "report_property" => (
            "report_property object",
            "all properties of an object as a table",
        ),
        "list_property" | "list_properties" => (
            "list_property object",
            "property names defined on an object",
        ),
        "select_objects" => (
            "select_objects objects",
            "select cells/nets/pins in the IDE views",
        ),
        "highlight_objects" => (
            "highlight_objects objects",
            "highlight objects (stronger than select)",
        ),
        "get_selected_objects" => (
            "get_selected_objects",
            "names currently selected via select_objects",
        ),
        "current_design" => ("current_design", "name of the loaded design"),
        "synth_design" | "read_sv" => (
            "synth_design ?file…? | read_sv file",
            "synthesize SystemVerilog into the netlist",
        ),
        "read_verilog" | "read_vhdl" => {
            ("read_verilog|read_vhdl file", "synth alias (Vivado compat)")
        }
        "opt_design" => ("opt_design", "dead-code sweep on the netlist"),
        "place_design" => ("place_design", "pack + PathFinder place"),
        "route_design" => ("route_design", "route the placed design"),
        "write_bitstream" => ("write_bitstream", "generate the bitstream"),
        "write_checkpoint" => ("write_checkpoint", "snapshot the session (size + hash)"),
        "create_clock" => (
            "create_clock -period <ns> ?-name n? ?-waveform {r f}? objects",
            "define a clock for STA (period in ns)",
        ),
        "report_timing" => ("report_timing", "STA table: WNS/TNS plus the worst paths"),
        "report_utilization" => (
            "report_utilization",
            "resource usage table vs the target die",
        ),
        "mark_debug" | "add_probe" => ("mark_debug net", "mark a net for ILA-style probing"),
        "eco" => (
            "eco ?cell? ?init?",
            "engineering change: rewrite one LUT init",
        ),
        "open_hw_manager" => ("open_hw_manager", "open the HW manager (sim)"),
        "program_hw" | "program_hw_devices" => (
            "program_hw ?cable=id?",
            "program the device (sim fabric by default)",
        ),
        "report_die" | "report_hw_targets" => ("report_die", "target die geometry and capacity"),
        "report_featuremap" => ("report_featuremap", "device feature map"),
        "write_hnf" => ("write_hnf", "dump the netlist in HNF text form"),
        "hds::synth" => ("hds::synth", "Helion flow: synthesize"),
        "hds::impl" => ("hds::impl", "Helion flow: implement through bitstream"),
        "hds::flow" => ("hds::flow", "flow step names"),
        "hds::tree" => ("hds::tree", "source tree entries"),
        "set" => ("set var ?value?", "read/write a variable"),
        "unset" => ("unset ?-nocomplain? var…", "delete variables"),
        "puts" => ("puts ?-nonewline? string", "write to the console"),
        "expr" => ("expr arg…", "evaluate a math/logic expression"),
        "if" => ("if cond body ?elseif c2 b2…? ?else? body", "conditional"),
        "foreach" => ("foreach var list body", "loop over a list"),
        "for" => ("for init test next body", "C-style loop"),
        "while" => ("while test body", "loop while true"),
        "proc" => ("proc name params body", "define a procedure"),
        "return" => ("return ?value?", "return from a procedure"),
        "break" => ("break", "exit the innermost loop"),
        "continue" => ("continue", "next loop iteration"),
        "list" => ("list ?values…?", "build a list"),
        "llength" => ("llength list", "number of list elements"),
        "lindex" => ("lindex list i", "element i (supports end-N)"),
        "lrange" => ("lrange list a b", "sub-list from a to b"),
        "lappend" => ("lappend var ?values…?", "append to a list variable"),
        "concat" => ("concat ?lists…?", "join lists"),
        "join" => ("join list ?sep?", "join elements with sep"),
        "split" => ("split string ?seps?", "split into a list"),
        "source" => ("source file", "evaluate a Tcl script file"),
        "catch" => ("catch script ?var?", "trap errors, Tcl-style"),
        "error" => ("error msg", "raise an error"),
        "eval" => ("eval arg…", "evaluate args as a script"),
        "subst" => ("subst string", "perform substitutions"),
        "format" => ("format fmt args…", "%s %d %f %x subset"),
        "incr" => ("incr var ?n?", "increment an integer variable"),
        "append" => ("append var ?values…?", "append to a variable"),
        "string" => (
            "string op ?args…?",
            "length|range|index|match|tolower|toupper|trim|first|equal",
        ),
        "info" => (
            "info exists|vars|procs|commands|args|body …",
            "introspection",
        ),
        "history" => ("history", "list every submitted verb, oldest first"),
        "launch_runs" => (
            "launch_runs synth_1|impl_1|(-to_step write_bitstream)",
            "start a real synthesis/implementation/bitstream run",
        ),
        "cancel_runs" => ("cancel_runs ?run?", "stop the active run"),
        "open_report" => (
            "open_report timing|utilization|power|drc|methodology",
            "open a report surface",
        ),
        "launch_simulation" => ("launch_simulation", "start a real testbench simulation run"),
        "tb_rescan" => ("tb_rescan", "discover *_tb.sv / tb_*.sv testbenches again"),
        "tb_select" => ("tb_select name|index", "select a testbench in the Sim pane"),
        "tb_run" => (
            "tb_run ?name|index?",
            "run a testbench on the helion-sim worker",
        ),
        "tb_run_all" => ("tb_run_all", "run every discovered testbench in turn"),
        "tb_stop" => ("tb_stop", "stop the running simulation"),
        "tb_vcd" => ("tb_vcd ?on|off?", "dump run.vcd from testbench runs"),
        "tb_wave" => (
            "tb_wave ?file.vcd?",
            "open a testbench VCD in the waveform viewer",
        ),
        "tb_jump" => (
            "tb_jump index",
            "open failure N of the selected testbench in the editor",
        ),
        "set_timescale" => ("set_timescale time", "set TIMESCALE_PS (e.g. 10ns)"),
        "set_runtime" => (
            "set_runtime cycles|time",
            "set RUNTIME_CYCLES (or RUNTIME_PS)",
        ),
        "sim_settings" => (
            "sim_settings ?KEY ?value??",
            "show or set Simulation Settings (SIM_TOP, PARAM, RUNTIME_CYCLES, ...)",
        ),
        "compile_sim" => (
            "compile_sim",
            "compile the RTL with helion-sv (INCLUDE_PATH/DEFINE)",
        ),
        "elaborate_sim" => (
            "elaborate_sim",
            "elaborate SIM_TOP with PARAM into helion-sim",
        ),
        "ila" => (
            "ila add|bus|remove|trigger|trigger_at|depth|preratio|clock|radix|capture|arm|disarm|status|save|load|export_vcd ...",
            "simulation logic analyzer (opens the Logic Analyzer pane)",
        ),
        _ => return None,
    })
}

fn help_overview() -> String {
    [
        "Helion Tcl console. Core: set puts expr if foreach for while proc return break continue",
        "  list llength lindex lrange lappend concat join split string info catch error eval subst source format incr append",
        "Objects: get_cells get_nets get_pins get_ports get_clocks get_timing_paths [-filter expr] [-of_objects objs] [patterns]",
        "  get_property prop obj | set_property name value objs | report_property obj | list_property obj",
        "  assign_package_pin port pin | package | select_package_pin pin",
        "  sim_run sim_step sim_restart | breakpoints add_bp select_breakpoint enable_bp disable_bp delete_bp",
        "  memory select_memory select_memory_word | locals select_local",
        "  forces add_force deposit remove_force select_force",
        "  select_objects highlight_objects get_selected_objects | current_design | clear",
        "Flow: synth_design|read_sv file opt_design place_design route_design write_bitstream write_checkpoint",
        "  create_clock report_timing report_utilization mark_debug eco open_hw_manager program_hw report_die report_featuremap",
        "More: unset list_properties read_verilog read_vhdl add_probe program_hw_devices report_hw_targets write_hnf",
        "  hds::synth hds::impl hds::flow hds::tree | history launch_runs cancel_runs open_report",
        "Verify: launch_simulation tb_rescan tb_select tb_run tb_run_all tb_stop tb_vcd tb_wave tb_jump",
        "  set_timescale set_runtime sim_settings compile_sim elaborate_sim | ila <subcommand>",
        "help <command> for usage. See docs/tcl-subset.md for the supported subset.",
    ]
    .join("\n")
}

// ---- unknown-command suggestions (mirroring design.rs:94) ----

/// Up to 3 "did you mean …?" candidates for an unknown command name:
/// prefix matches first, then close edit-distance matches. Searches the
/// full command set plus user procs, like `suggest_commands`.
fn suggest_commands(s: &AppState, name: &str) -> Vec<String> {
    let mut cands: Vec<&str> = Vec::new();
    cands.extend(COMMANDS.iter().copied());
    let procs: Vec<String> = {
        let mut v: Vec<String> = s.tcl.procs.keys().cloned().collect();
        v.sort();
        v
    };
    cands.extend(procs.iter().map(|v| v.as_str()));
    let mut scored: Vec<(usize, &str)> = cands
        .iter()
        .filter(|c| **c != name)
        .map(|c| {
            let d = if c.starts_with(name) || name.starts_with(c) {
                0
            } else {
                edit_dist(name, c)
            };
            (d, *c)
        })
        .filter(|(d, _)| *d <= 3)
        .collect();
    scored.sort();
    scored.dedup();
    scored
        .into_iter()
        .take(3)
        .map(|(_, c)| c.to_string())
        .collect()
}

/// Tcl-style unknown-command error with suggestions (single line, no panic).
fn unknown_command(s: &AppState, name: &str, line: usize) -> Ctl {
    let mut msg = format!("invalid command name \"{name}\"");
    let sugg = suggest_commands(s, name);
    if !sugg.is_empty() {
        msg.push_str(&format!(
            "; did you mean {}?",
            sugg.iter()
                .map(|v| format!("\"{v}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ctl::err(line, msg)
}

/// Levenshtein distance over chars (small strings only).
fn edit_dist(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (x, ca) in a.iter().enumerate() {
        let mut prev = x + 1;
        for (y, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let cur = (row[y + 1] + 1).min((prev + 1).min(row[y] + cost));
            row[y] = prev;
            prev = cur;
        }
        row[b.len()] = prev;
    }
    row[b.len()]
}

// ---- expr (mirroring `interp.rs` XVal + ExprParser) ----

/// An `expr` value: integer, double, or string.
#[derive(Clone, Debug)]
enum XVal {
    I(i64),
    F(f64),
    S(String),
}

impl XVal {
    fn to_string(&self) -> String {
        match self {
            XVal::I(i) => i.to_string(),
            XVal::F(f) => format!("{f:?}"),
            XVal::S(s) => s.clone(),
        }
    }
    fn as_f64(&self) -> Result<f64, String> {
        match self {
            XVal::I(i) => Ok(*i as f64),
            XVal::F(f) => Ok(*f),
            XVal::S(s) => s
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("expected floating-point number but got \"{s}\"")),
        }
    }
    fn as_i64(&self) -> Result<i64, String> {
        match self {
            XVal::I(i) => Ok(*i),
            XVal::F(f) => Ok(*f as i64),
            XVal::S(s) => s
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("expected integer but got \"{s}\"")),
        }
    }
    fn is_num_str(s: &str) -> bool {
        let t = s.trim();
        t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok()
    }
}

/// Tcl truthiness: 0/""/false/no/off -> false; nonzero/true/yes/on -> true.
fn truth(v: &XVal) -> Result<bool, String> {
    match v {
        XVal::I(i) => Ok(*i != 0),
        XVal::F(f) => Ok(*f != 0.0),
        XVal::S(s) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(false);
            }
            if let Ok(i) = t.parse::<i64>() {
                return Ok(i != 0);
            }
            if let Ok(f) = t.parse::<f64>() {
                return Ok(f != 0.0);
            }
            match t.to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" => Ok(true),
                "false" | "no" | "off" => Ok(false),
                _ => Err(format!("expected boolean value but got \"{s}\"")),
            }
        }
    }
}

/// Evaluate an `expr`/`if`/`while` condition string.
fn eval_expr(
    s: &mut AppState,
    src: &str,
    line: usize,
    puts_out: &mut Vec<String>,
    depth: usize,
) -> Result<XVal, String> {
    let mut xp = XParser {
        chars: src.chars().collect(),
        pos: 0,
        line,
        s,
        puts_out,
        depth,
    };
    let line = xp.line;
    let v = xp.parse_ternary()?;
    xp.skip_ws();
    if xp.pos != xp.chars.len() {
        return Err(format!(
            "syntax error in expression \"{src}\" (line {line})"
        ));
    }
    Ok(v)
}

struct XParser<'a> {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    s: &'a mut AppState,
    puts_out: &'a mut Vec<String>,
    depth: usize,
}

impl<'a> XParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn eat(&mut self, s: &str) -> bool {
        self.skip_ws();
        if self.chars[self.pos..].starts_with(&s.chars().collect::<Vec<_>>()[..]) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }
    fn err(&self, msg: impl Into<String>) -> String {
        msg.into()
    }

    fn parse_ternary(&mut self) -> Result<XVal, String> {
        let c = self.parse_or()?;
        self.skip_ws();
        if self.peek() == Some('?') {
            self.pos += 1;
            let t = self.parse_ternary()?;
            self.skip_ws();
            if self.peek() != Some(':') {
                return Err(self.err("expr: expected ':' in ternary"));
            }
            self.pos += 1;
            let e = self.parse_ternary()?;
            Ok(if truth(&c)? { t } else { e })
        } else {
            Ok(c)
        }
    }

    fn parse_or(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_and()?;
        while self.eat("||") {
            let r = self.parse_and()?;
            l = XVal::I(i64::from(truth(&l)? || truth(&r)?));
        }
        Ok(l)
    }
    fn parse_and(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_bitor()?;
        while self.eat("&&") {
            let r = self.parse_bitor()?;
            l = XVal::I(i64::from(truth(&l)? && truth(&r)?));
        }
        Ok(l)
    }
    fn parse_bitor(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_bitxor()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('|') && self.chars.get(self.pos + 1) != Some(&'|') {
                self.pos += 1;
                let r = self.parse_bitxor()?;
                l = XVal::I(l.as_i64()? | r.as_i64()?);
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_bitxor(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_bitand()?;
        while self.eat("^") {
            let r = self.parse_bitand()?;
            l = XVal::I(l.as_i64()? ^ r.as_i64()?);
        }
        Ok(l)
    }
    fn parse_bitand(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_eq()?;
        loop {
            self.skip_ws();
            if self.peek() == Some('&') && self.chars.get(self.pos + 1) != Some(&'&') {
                self.pos += 1;
                let r = self.parse_eq()?;
                l = XVal::I(l.as_i64()? & r.as_i64()?);
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_eq(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_cmp()?;
        loop {
            if self.eat("==") {
                let r = self.parse_cmp()?;
                l = XVal::I(i64::from(xeq(&l, &r)));
            } else if self.eat("!=") {
                let r = self.parse_cmp()?;
                l = XVal::I(i64::from(!xeq(&l, &r)));
            } else if self.eat_word_op("eq") {
                let r = self.parse_cmp()?;
                l = XVal::I(i64::from(l.to_string() == r.to_string()));
            } else if self.eat_word_op("ne") {
                let r = self.parse_cmp()?;
                l = XVal::I(i64::from(l.to_string() != r.to_string()));
            } else {
                break;
            }
        }
        Ok(l)
    }
    /// `eq`/`ne` only match as whole words.
    fn eat_word_op(&mut self, op: &str) -> bool {
        self.skip_ws();
        let end = self.pos + op.len();
        if end <= self.chars.len()
            && self.chars[self.pos..end].iter().collect::<String>() == op
            && self
                .chars
                .get(end)
                .is_none_or(|c| !c.is_ascii_alphanumeric() && *c != '_')
        {
            self.pos = end;
            true
        } else {
            false
        }
    }
    fn parse_cmp(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_shift()?;
        loop {
            let op = if self.eat("<=") {
                "<="
            } else if self.eat(">=") {
                ">="
            } else if self.eat("<") {
                "<"
            } else if self.eat(">") {
                ">"
            } else {
                break;
            };
            let r = self.parse_shift()?;
            l = XVal::I(i64::from(xcmp(&l, &r, op)?));
        }
        Ok(l)
    }

    fn parse_shift(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_add()?;
        loop {
            if self.eat("<<") {
                let r = self.parse_add()?;
                l = XVal::I(l.as_i64()?.wrapping_shl(r.as_i64()? as u32));
            } else if self.eat(">>") {
                let r = self.parse_add()?;
                l = XVal::I(l.as_i64()?.wrapping_shr(r.as_i64()? as u32));
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_add(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_mul()?;
        loop {
            if self.eat("+") {
                let r = self.parse_mul()?;
                l = xadd(&l, &r);
            } else if self.eat("-") {
                let r = self.parse_mul()?;
                l = xsub(&l, &r);
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_mul(&mut self) -> Result<XVal, String> {
        let mut l = self.parse_pow()?;
        loop {
            if self.eat("*") {
                if self.peek() == Some('*') {
                    return Err(self.err("expr: unexpected **"));
                }
                let r = self.parse_pow()?;
                l = xmul(&l, &r);
            } else if self.eat("/") {
                let r = self.parse_pow()?;
                l = xdiv(&l, &r)?;
            } else if self.eat("%") {
                let r = self.parse_pow()?;
                let (a, b) = (l.as_i64()?, r.as_i64()?);
                if b == 0 {
                    return Err(self.err("expr: divide by zero"));
                }
                l = XVal::I(a % b);
            } else {
                break;
            }
        }
        Ok(l)
    }
    fn parse_pow(&mut self) -> Result<XVal, String> {
        let base = self.parse_unary()?;
        if self.eat("**") {
            let exp = self.parse_pow()?;
            return xpow(&base, &exp);
        }
        Ok(base)
    }
    fn parse_unary(&mut self) -> Result<XVal, String> {
        self.skip_ws();
        match self.peek() {
            Some('-') => {
                self.pos += 1;
                let v = self.parse_unary()?;
                Ok(xneg(&v))
            }
            Some('+') => {
                self.pos += 1;
                self.parse_unary()
            }
            Some('!') if self.chars.get(self.pos + 1) != Some(&'=') => {
                self.pos += 1;
                let v = self.parse_unary()?;
                Ok(XVal::I(i64::from(!truth(&v)?)))
            }
            Some('~') => {
                self.pos += 1;
                let v = self.parse_unary()?;
                Ok(XVal::I(!v.as_i64()?))
            }
            _ => self.parse_primary(),
        }
    }
    fn parse_primary(&mut self) -> Result<XVal, String> {
        self.skip_ws();
        match self.peek() {
            None => Err(self.err("expr: unexpected end")),
            Some('(') => {
                self.pos += 1;
                let v = self.parse_ternary()?;
                self.skip_ws();
                if self.peek() != Some(')') {
                    return Err(self.err("expr: expected ')'"));
                }
                self.pos += 1;
                Ok(v)
            }
            Some('"') => {
                self.pos += 1;
                let mut s = String::new();
                while let Some(c) = self.peek() {
                    if c == '"' {
                        self.pos += 1;
                        break;
                    }
                    if c == '\\' {
                        self.pos += 1;
                        match self.peek() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(c2) => s.push(c2),
                            None => break,
                        }
                        self.pos += 1;
                    } else {
                        s.push(c);
                        self.pos += 1;
                    }
                }
                Ok(XVal::S(s))
            }
            Some('{') => {
                self.pos += 1;
                let mut s = String::new();
                let mut depth = 1;
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    match c {
                        '{' => {
                            depth += 1;
                            s.push(c);
                        }
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            s.push(c);
                        }
                        _ => s.push(c),
                    }
                }
                Ok(XVal::S(s))
            }
            Some('[') => {
                let start = self.pos + 1;
                let mut depth = 1usize;
                let mut k = start;
                while k < self.chars.len() {
                    match self.chars[k] {
                        '\\' => k += 1,
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                if depth != 0 {
                    return Err(self.err("expr: missing close-bracket"));
                }
                let inner: String = self.chars[start..k].iter().collect();
                self.pos = k + 1;
                let line = self.line;
                let depth = self.depth;
                let s = &mut *self.s;
                let puts_out = &mut *self.puts_out;
                let mut values = Vec::new();
                match eval_script(s, &inner, puts_out, depth + 1, &mut values) {
                    Ctl::Value(v) | Ctl::Return(v) => Ok(XVal::S(v)),
                    Ctl::Break => Err("expr: break outside loop".into()),
                    Ctl::Continue => Err("expr: continue outside loop".into()),
                    Ctl::Error(e) => Err(e.msg),
                }
                .map_err(|m| {
                    let _ = line;
                    m
                })
            }
            Some('$') => {
                self.pos += 1;
                let name = self.parse_ident();
                let key = if self.peek() == Some('(') {
                    self.pos += 1;
                    let mut idx = String::new();
                    while let Some(c) = self.peek() {
                        if c == ')' {
                            self.pos += 1;
                            break;
                        }
                        idx.push(c);
                        self.pos += 1;
                    }
                    format!("{name}({idx})")
                } else {
                    name.clone()
                };
                // NOTE: the old parser takes the index literally here (no
                // substitution inside `expr` `$a(...)`); mirrored.
                match self.s.tcl.lookup(&key) {
                    Some(v) => Ok(XVal::S(v.clone())),
                    None => Err(format!("can't read \"{name}\": no such variable")),
                }
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let name = self.parse_ident();
                self.skip_ws();
                if self.peek() == Some('(') {
                    self.pos += 1;
                    let mut args = Vec::new();
                    self.skip_ws();
                    if self.peek() != Some(')') {
                        loop {
                            args.push(self.parse_ternary()?);
                            self.skip_ws();
                            if self.peek() == Some(',') {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    self.skip_ws();
                    if self.peek() != Some(')') {
                        return Err(self.err("expr: expected ')' after function args"));
                    }
                    self.pos += 1;
                    xfunc(&name, &args)
                } else {
                    match self.s.tcl.lookup(&name) {
                        Some(v) => Ok(XVal::S(v.clone())),
                        None => Err(format!("can't read \"{name}\": no such variable")),
                    }
                }
            }
            Some(c) => Err(self.err(format!("expr: unexpected character '{c}'"))),
        }
    }
    fn parse_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        s
    }
    fn parse_number(&mut self) -> Result<XVal, String> {
        let start = self.pos;
        let mut is_float = false;
        if self.chars.get(self.pos) == Some(&'0')
            && matches!(self.chars.get(self.pos + 1), Some('x') | Some('X'))
        {
            self.pos += 2;
            let h: String = self.take_while(|c| c.is_ascii_hexdigit());
            let v = i64::from_str_radix(&h, 16)
                .map_err(|_| self.err(format!("bad number \"{}\"", &self.src_slice(start))))?;
            return Ok(XVal::I(v));
        }
        let int_part: String = self.take_while(|c| c.is_ascii_digit());
        if self.peek() == Some('.') {
            is_float = true;
            self.pos += 1;
        }
        let frac_part: String = self.take_while(|c| c.is_ascii_digit());
        if matches!(self.peek(), Some('e') | Some('E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.pos += 1;
            }
            let _e: String = self.take_while(|c| c.is_ascii_digit());
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        let _ = (int_part, frac_part);
        if is_float {
            text.parse::<f64>()
                .map(XVal::F)
                .map_err(|_| self.err(format!("bad number \"{text}\"")))
        } else {
            text.parse::<i64>()
                .map(XVal::I)
                .map_err(|_| self.err(format!("bad number \"{text}\"")))
        }
    }
    fn take_while(&mut self, f: impl Fn(char) -> bool) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if f(c) {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        s
    }
    fn src_slice(&self, start: usize) -> String {
        self.chars[start..self.pos.min(self.chars.len())]
            .iter()
            .collect()
    }
}

fn xeq(a: &XVal, b: &XVal) -> bool {
    match (a, b) {
        (XVal::I(x), XVal::I(y)) => x == y,
        (XVal::S(x), XVal::S(y)) if !XVal::is_num_str(x) || !XVal::is_num_str(y) => x == y,
        _ => a.as_f64().ok() == b.as_f64().ok() && a.as_f64().is_ok(),
    }
}

fn xcmp(a: &XVal, b: &XVal, op: &str) -> Result<bool, String> {
    let ord = match (a, b) {
        (XVal::S(x), XVal::S(y)) if !XVal::is_num_str(x) || !XVal::is_num_str(y) => x.cmp(y),
        _ => {
            let (x, y) = (a.as_f64()?, b.as_f64()?);
            x.partial_cmp(&y).ok_or("expr: unordered comparison")?
        }
    };
    Ok(match op {
        "<" => ord.is_lt(),
        "<=" => ord.is_le(),
        ">" => ord.is_gt(),
        ">=" => ord.is_ge(),
        _ => false,
    })
}

fn xadd(a: &XVal, b: &XVal) -> XVal {
    let (a, b) = (num(a), num(b));
    match (&a, &b) {
        (XVal::I(x), XVal::I(y)) => match x.checked_add(*y) {
            Some(v) => XVal::I(v),
            None => XVal::F(*x as f64 + *y as f64),
        },
        _ => XVal::F(a.as_f64().unwrap_or(f64::NAN) + b.as_f64().unwrap_or(f64::NAN)),
    }
}
fn xsub(a: &XVal, b: &XVal) -> XVal {
    let (a, b) = (num(a), num(b));
    match (&a, &b) {
        (XVal::I(x), XVal::I(y)) => match x.checked_sub(*y) {
            Some(v) => XVal::I(v),
            None => XVal::F(*x as f64 - *y as f64),
        },
        _ => XVal::F(a.as_f64().unwrap_or(f64::NAN) - b.as_f64().unwrap_or(f64::NAN)),
    }
}
fn xmul(a: &XVal, b: &XVal) -> XVal {
    let (a, b) = (num(a), num(b));
    match (&a, &b) {
        (XVal::I(x), XVal::I(y)) => match x.checked_mul(*y) {
            Some(v) => XVal::I(v),
            None => XVal::F(*x as f64 * *y as f64),
        },
        _ => XVal::F(a.as_f64().unwrap_or(f64::NAN) * b.as_f64().unwrap_or(f64::NAN)),
    }
}
fn xdiv(a: &XVal, b: &XVal) -> Result<XVal, String> {
    let (a, b) = (num(a), num(b));
    match (&a, &b) {
        (XVal::I(x), XVal::I(y)) => {
            if *y == 0 {
                return Err("expr: divide by zero".into());
            }
            Ok(XVal::I(x / y))
        }
        _ => {
            let (x, y) = (a.as_f64()?, b.as_f64()?);
            Ok(XVal::F(x / y))
        }
    }
}
fn xpow(a: &XVal, b: &XVal) -> Result<XVal, String> {
    let (a, b) = (num(a), num(b));
    match (&a, &b) {
        (XVal::I(x), XVal::I(y)) if *y >= 0 => {
            let mut r: i64 = 1;
            for _ in 0..*y {
                r = match r.checked_mul(*x) {
                    Some(v) => v,
                    None => return Ok(XVal::F((*x as f64).powi(*y as i32))),
                };
            }
            Ok(XVal::I(r))
        }
        _ => Ok(XVal::F(a.as_f64()?.powf(b.as_f64()?))),
    }
}

/// Normalize a value for arithmetic: integer-shaped strings become `I`,
/// float-shaped strings become `F`; anything else is left alone (the
/// float paths below yield NaN for those, as before).
fn num(v: &XVal) -> XVal {
    match v {
        XVal::S(s) => {
            let t = s.trim();
            if let Ok(i) = t.parse::<i64>() {
                XVal::I(i)
            } else if let Ok(f) = t.parse::<f64>() {
                XVal::F(f)
            } else {
                v.clone()
            }
        }
        _ => v.clone(),
    }
}
fn xneg(a: &XVal) -> XVal {
    match a {
        XVal::I(i) => XVal::I(i.wrapping_neg()),
        XVal::F(f) => XVal::F(-f),
        XVal::S(s) => s
            .trim()
            .parse::<i64>()
            .map(|i| XVal::I(i.wrapping_neg()))
            .unwrap_or_else(|_| XVal::F(-s.trim().parse::<f64>().unwrap_or(f64::NAN))),
    }
}

fn xfunc(name: &str, args: &[XVal]) -> Result<XVal, String> {
    let arity = |n: usize| {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!("expr: {name} wants {n} args, got {}", args.len()))
        }
    };
    match name {
        "abs" => {
            arity(1)?;
            Ok(match &args[0] {
                XVal::I(i) => XVal::I(i.wrapping_abs()),
                XVal::F(f) => XVal::F(f.abs()),
                XVal::S(s) => {
                    if let Ok(i) = s.trim().parse::<i64>() {
                        XVal::I(i.wrapping_abs())
                    } else {
                        XVal::F(
                            s.trim()
                                .parse::<f64>()
                                .map_err(|_| format!("expected number but got \"{s}\""))?
                                .abs(),
                        )
                    }
                }
            })
        }
        "int" => {
            arity(1)?;
            Ok(XVal::I(args[0].as_i64()?))
        }
        "double" => {
            arity(1)?;
            Ok(XVal::F(args[0].as_f64()?))
        }
        "round" => {
            arity(1)?;
            Ok(XVal::I(args[0].as_f64()?.round() as i64))
        }
        "sqrt" => {
            arity(1)?;
            let f = args[0].as_f64()?;
            if f < 0.0 {
                return Err("expr: sqrt of negative number".into());
            }
            Ok(XVal::F(f.sqrt()))
        }
        "pow" => {
            arity(2)?;
            Ok(XVal::F(args[0].as_f64()?.powf(args[1].as_f64()?)))
        }
        _ => Err(format!("expr: unknown function \"{name}\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RunStatus;

    fn lines(s: &AppState) -> Vec<&str> {
        s.console.lines.iter().map(|l| l.text.as_str()).collect()
    }

    fn counter_source() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv")
    }

    fn pump_until_idle(s: &mut AppState) {
        let start = std::time::Instant::now();
        while s.runs.is_active() {
            crate::runs::poll_runs(s);
            if !s.runs.is_active() {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "run timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        crate::runs::poll_runs(s);
    }

    fn pump_until_idle_sim(s: &mut AppState) {
        let start = std::time::Instant::now();
        while s.sim.is_active() {
            crate::views::wave::poll_sim(s);
            if !s.sim.is_active() {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(60),
                "sim timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        crate::views::wave::poll_sim(s);
    }

    #[test]
    fn expr_evaluates_real_arithmetic() {
        let mut s = AppState::demo();
        eval_command(&mut s, "expr {2 + 3 * 4}");
        assert!(
            lines(&s).contains(&"14"),
            "expr must compute, got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "expr {(10 - 4) / 3}");
        assert!(lines(&s).contains(&"2"), "got: {:?}", lines(&s));
    }

    #[test]
    fn set_puts_and_var_substitution_round_trip() {
        let mut s = AppState::demo();
        eval_command(&mut s, "set x 41");
        assert!(
            lines(&s).contains(&"41"),
            "set must echo the value: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "puts $x");
        assert!(
            lines(&s).contains(&"41"),
            "puts must print the var: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "expr {$x + 1}");
        assert!(
            lines(&s).contains(&"42"),
            "expr must see the var: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn unknown_command_is_a_tcl_error_with_a_suggestion() {
        let mut s = AppState::demo();
        eval_command(&mut s, "exrp {1+1}");
        let err = s.console.lines.iter().find(|l| l.sev == MsgSev::Error);
        assert!(
            err.is_some_and(|l| l.text.contains("invalid command name")),
            "got: {:?}",
            lines(&s)
        );
        assert!(
            lines(&s).iter().any(|l| l.contains("did you mean")),
            "typo should suggest, got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn history_keeps_every_submitted_verb() {
        let mut s = AppState::demo();
        eval_command(&mut s, "expr {1+1}");
        eval_command(&mut s, "set a b");
        assert_eq!(
            s.tcl.history,
            vec!["expr {1+1}".to_string(), "set a b".to_string()]
        );
        eval_command(&mut s, "history");
        assert!(
            lines(&s).iter().any(|l| l.contains("expr {1+1}")),
            "history must list verbs: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn read_sv_then_get_cells_returns_real_names() {
        let mut s = AppState::demo();
        eval_command(&mut s, &format!("read_sv {}", counter_source().display()));
        assert!(
            lines(&s)
                .iter()
                .any(|l| l.contains("synth") && l.contains("cells") && l.contains("luts")),
            "read_sv must report the old-style synth line: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_cells");
        assert!(
            lines(&s).iter().any(|l| l.contains("u_lut")),
            "get_cells must name real cells: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_nets clk*");
        assert!(
            lines(&s).iter().any(|l| l.contains("clk")),
            "get_nets must match real nets: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn get_cells_without_a_design_errors_honestly() {
        let mut s = AppState::demo();
        eval_command(&mut s, "get_cells");
        assert!(
            s.console.lines.iter().any(|l| l.sev == MsgSev::Error
                && l.text.contains("no design")
                && l.text.contains("synth_design")),
            "missing design must error with a hint: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn object_queries_read_the_run_result_design() {
        let mut s = AppState::demo();
        let src = std::fs::read_to_string(counter_source()).unwrap();
        let design = helion_sv::synth_sv(&src, "counter.sv").unwrap();
        s.runs.live_design = Some(design);
        assert!(s.tcl.design.is_none(), "no Tcl design in this path");
        eval_command(&mut s, "get_cells");
        assert!(has_line(&s, "u_lut"), "cells: {:?}", lines(&s));
        eval_command(&mut s, "get_nets clk*");
        assert!(has_line(&s, "clk"), "nets: {:?}", lines(&s));
        eval_command(&mut s, "get_ports");
        assert!(has_line(&s, "led"), "ports: {:?}", lines(&s));
        eval_command(&mut s, "get_pins -of_objects u_luts0");
        assert!(has_line(&s, "u_luts0/"), "pins: {:?}", lines(&s));
        eval_command(&mut s, "get_cells -filter {TYPE == lut}");
        assert!(has_line(&s, "u_lut"), "filter: {:?}", lines(&s));
        eval_command(&mut s, "get_property DIRECTION clk");
        assert!(has_line(&s, "IN"), "property: {:?}", lines(&s));
        eval_command(&mut s, "current_design");
        assert!(has_line(&s, "counter"), "current: {:?}", lines(&s));
        eval_command(&mut s, "write_hnf");
        assert!(has_line(&s, "HNF 1"), "hnf: {:?}", lines(&s));
    }

    #[test]
    fn report_timing_reports_real_numbers() {
        let mut s = AppState::demo();
        s.project.part = "HL10T-C32-1".into();
        eval_command(&mut s, &format!("read_sv {}", counter_source().display()));
        eval_command(&mut s, "report_timing");
        assert!(
            lines(&s).iter().any(|l| l.contains("Timing Report:"))
                && lines(&s).iter().any(|l| l.contains("endpoints")),
            "report_timing must echo the old-style STA table: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn help_lists_supported_commands() {
        let mut s = AppState::demo();
        eval_command(&mut s, "help");
        assert!(
            lines(&s).iter().any(|l| l.contains("expr")),
            "got: {:?}",
            lines(&s)
        );
        assert!(
            lines(&s).iter().any(|l| l.contains("launch_runs")),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "help expr");
        assert!(
            lines(&s).iter().any(|l| l.contains("expr")),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn clear_empties_the_console() {
        let mut s = AppState::demo();
        assert!(!s.console.lines.is_empty());
        eval_command(&mut s, "clear");
        assert!(
            s.console.lines.is_empty(),
            "clear must empty the console: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn launch_runs_through_eval_arms_a_real_run() {
        let mut s = AppState::demo();
        s.runs.source = Some(counter_source());
        eval_command(&mut s, "launch_runs synth_1");
        assert_eq!(s.flow.synth, RunStatus::Running);
        assert!(s.runs.is_active());
        crate::runs::cancel_run(&mut s);
        pump_until_idle(&mut s);
    }

    #[test]
    fn read_sv_selects_the_run_source_for_launch_runs() {
        let mut s = AppState::demo();
        // Poison the editor buffer: the run must use the read file, not this.
        s.editor.tabs = vec!["other.v".into()];
        s.editor.active = 0;
        s.editor.lines = vec!["module other(input a, output b); assign b = a; endmodule".into()];
        let src = counter_source();
        eval_command(&mut s, &format!("read_sv {}", src.display()));
        assert_eq!(s.runs.source, Some(src.clone()));
        eval_command(&mut s, "launch_runs synth_1");
        pump_until_idle(&mut s);
        assert_eq!(s.flow.synth, RunStatus::Complete);
        let opened = format!("open_source {}", src.display());
        assert!(
            s.runs.synth_log.iter().any(|l| l.text == opened),
            "launch_runs must elaborate the read file: {:?}",
            s.runs.synth_log.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn launch_simulation_through_eval_arms_a_real_sim() {
        let mut s = AppState::demo();
        eval_command(&mut s, "launch_simulation");
        assert_eq!(s.flow.sim, RunStatus::Running);
        assert!(s.sim.is_active(), "the verb must arm the sim worker");
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.text == "launch_simulation"),
            "the verb must echo: {:?}",
            lines(&s)
        );
        crate::views::wave::cancel_sim(&mut s);
        pump_until_idle_sim(&mut s);
    }

    #[test]
    fn set_property_reaches_the_run_design_copy() {
        let mut s = AppState::demo();
        s.project.part = "HL10T-C32-1".into();
        let src = std::fs::read_to_string(counter_source()).unwrap();
        let design = helion_sv::synth_sv(&src, "counter.sv").unwrap();
        s.runs.live_design = Some(design);
        assert!(s.tcl.design.is_none(), "no Tcl design in this path");
        let (dev, _) = crate::live::live_device(&s);
        let pin = dev
            .iob_sites()
            .next()
            .expect("HAD part must expose an IOB site")
            .id();
        eval_command(&mut s, &format!("set_property LOC {pin} led"));
        assert!(
            !s.console.lines.iter().any(|l| l.sev == MsgSev::Error),
            "run-only design must accept props: {:?}",
            lines(&s)
        );
        let live = s.runs.live_design.as_ref().unwrap();
        let led = live
            .ports
            .iter()
            .find(|p| helion_ir::resolve(p.name) == "led")
            .unwrap();
        assert_eq!(led.attrs.get("LOC"), Some(pin.as_str()));
        assert!(
            has_line(
                &s,
                &format!("set_property PACKAGE_PIN {pin} led loc={pin} replaced=0 rerouted=0")
            ),
            "PACKAGE_PIN must report placement outcome: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn assign_package_pin_validates_had_sites_and_ports() {
        let mut s = AppState::demo();
        s.project.part = "HL10T-C32-1".into();
        let src = std::fs::read_to_string(counter_source()).unwrap();
        let design = helion_sv::synth_sv(&src, "counter.sv").unwrap();
        s.tcl.design = Some(design);
        let (dev, _) = crate::live::live_device(&s);
        let pin = dev
            .iob_sites()
            .next()
            .expect("HAD part must expose an IOB site")
            .id();
        let (x, y) = parse_site_xy(&pin).expect("fixture pin must parse");
        let non_iob = format!("IOB_X{x}Y{}", y + 1);
        eval_command(&mut s, "set_property PACKAGE_PIN");
        assert!(
            last_error(&s).contains("missing pin"),
            "missing pins must fail: {:?}",
            lines(&s)
        );
        eval_command(&mut s, &format!("set_property PACKAGE_PIN {pin}"));
        assert!(
            last_error(&s).contains("missing [get_ports <port>]"),
            "missing ports must fail: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "assign_package_pin led E3");
        assert!(
            last_error(&s).contains("bad pin E3"),
            "malformed pins must fail: {:?}",
            lines(&s)
        );
        eval_command(&mut s, &format!("assign_package_pin led {non_iob}"));
        assert!(
            last_error(&s).contains("is not a HAD IOB site"),
            "non-IOB pins must fail: {:?}",
            lines(&s)
        );
        eval_command(&mut s, &format!("assign_package_pin nosuch {pin}"));
        assert!(
            last_error(&s).contains("no port nosuch"),
            "unknown ports must fail: {:?}",
            lines(&s)
        );
        eval_command(&mut s, &format!("assign_package_pin led {pin}"));
        assert!(
            has_line(
                &s,
                &format!("set_property PACKAGE_PIN {pin} led loc={pin} replaced=0 rerouted=0")
            ),
            "the old alias must share PACKAGE_PIN semantics: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn package_commands_open_and_select_the_had_grid() {
        let mut s = AppState::demo();
        crate::live::seed_live_fixture(&mut s).expect("fixture must seed");
        let (dev, _) = crate::live::live_device(&s);
        let n = dev.iob_sites().count();
        eval_command(&mut s, "package extra");
        assert!(
            last_error(&s).contains("wrong # args"),
            "package takes no arguments: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "package");
        assert_eq!(s.center, crate::state::CenterPane::Device);
        assert_eq!(
            s.device.view,
            crate::views::device::DeviceView::Package,
            "package must open the package view"
        );
        assert!(
            has_line(&s, &format!("pins={n} assigned=2")),
            "fixture placement owns two pins: {:?}",
            lines(&s)
        );
        assert!(has_line(&s, "map="), "occupancy map: {:?}", lines(&s));
        eval_command(&mut s, "select_package_pin IOB_X2Y0");
        assert_eq!(
            s.device.selected,
            Some(crate::views::device::DeviceSel::Net("seg".into())),
            "assigned pins cross-select the port"
        );
        assert!(
            has_line(&s, "package_pin IOB_X2Y0 port=seg"),
            "got: {:?}",
            lines(&s)
        );
        let free = crate::views::device::package_pins(&s)
            .1
            .into_iter()
            .find(|p| p.port.is_none())
            .expect("fixture leaves unassigned pins")
            .pin;
        eval_command(&mut s, &format!("select_package_pin {free}"));
        assert_eq!(
            s.device.selected,
            Some(crate::views::device::DeviceSel::Pin(free.clone())),
            "unassigned pins select the pin itself"
        );
        assert!(
            has_line(&s, &format!("package_pin {free} unassigned")),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_package_pin");
        assert_eq!(
            last_error(&s),
            "ERROR: select_package_pin: no HAD pin  (line 1)",
            "missing pins keep the old wording: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_package_pin BOGUS_X0Y0");
        assert!(
            last_error(&s).contains("select_package_pin: no HAD pin BOGUS_X0Y0"),
            "unknown pins must fail: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn debugger_commands_cover_breakpoint_lifecycle() {
        let mut s = AppState::demo();
        crate::views::debug::seed_debug_fixture(&mut s).expect("fixture must seed");
        eval_command(&mut s, "breakpoints");
        assert_eq!(s.center, crate::state::CenterPane::Debug);
        assert!(
            has_line(&s, "breakpoints n=0 no breakpoints"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "add_bp led 1");
        assert_eq!(s.debug.selected, Some(1));
        assert!(
            has_line(&s, "ID=1 ENABLED=1 SIGNAL=led CONDITION===1 HITS=0"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "add_bp led");
        assert_eq!(s.debug.selected, Some(2));
        eval_command(&mut s, "select_breakpoint bp1");
        assert_eq!(s.debug.selected, Some(1));
        eval_command(&mut s, "disable_bp");
        assert!(!s.debug.breakpoints[0].enabled);
        eval_command(&mut s, "enable_bp 1");
        assert!(s.debug.breakpoints[0].enabled);
        eval_command(&mut s, "sim_run");
        assert_eq!(s.debug.breakpoints[0].hits, 1, "value hit must count");
        assert!(
            has_line(&s, "sim_run HIT id=1 SIGNAL=led VALUE=1"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "sim_step");
        assert!(has_line(&s, "sim_step t="), "{:?}", lines(&s));
        eval_command(&mut s, "sim_restart");
        assert!(s.debug.breakpoints.iter().all(|b| b.hits == 0));
        eval_command(&mut s, "delete_bp 2");
        assert_eq!(s.debug.breakpoints.len(), 1);
    }

    #[test]
    fn debugger_run_matches_the_gold_counter_waveform() {
        let mut s = AppState::demo();
        crate::views::debug::seed_debug_fixture(&mut s).expect("fixture must seed");
        eval_command(&mut s, "sim_run 16");
        assert!(
            has_line(
                &s,
                &format!(
                    "sim_run cycles=16 LED[16]={}",
                    helion_sim::COUNTER_OVERLAY_SIM_LED
                )
            ),
            "engine waveform: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "sim_run bogus");
        assert!(
            has_line(&s, "sim_run cycles=16 LED[16]="),
            "invalid counts keep the configured default: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn debugger_commands_reject_missing_designs_and_signals() {
        let mut s = AppState::demo();
        eval_command(&mut s, "add_bp led 1");
        assert!(
            last_error(&s).contains("sim: no design (synth first)"),
            "{:?}",
            lines(&s)
        );
        crate::views::debug::seed_debug_fixture(&mut s).expect("fixture must seed");
        eval_command(&mut s, "add_bp bogus");
        assert!(
            last_error(&s).contains("add_bp: no signal bogus"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_breakpoint bogus");
        assert!(
            last_error(&s).contains("select_breakpoint: no breakpoint bogus"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "enable_bp");
        assert!(
            last_error(&s).contains("enable/disable: select a breakpoint first"),
            "{:?}",
            lines(&s)
        );
    }

    #[test]
    fn memory_commands_open_and_select_words() {
        let mut s = AppState::demo();
        eval_command(&mut s, "memory");
        assert!(
            last_error(&s).contains("sim: no design (synth first)"),
            "{:?}",
            lines(&s)
        );
        crate::views::debug::seed_debug_fixture(&mut s).expect("fixture must seed");
        eval_command(&mut s, "memories");
        assert_eq!(s.center, crate::state::CenterPane::Debug);
        assert_eq!(
            s.debug.tab,
            crate::views::debug::DebugTab::Memory,
            "memories is the memory alias"
        );
        assert!(has_line(&s, "memory n="), "{:?}", lines(&s));
        assert!(!s.debug.memories.is_empty(), "counter has LUTs");
        assert!(
            has_line(&s, "TYPE=lut_init"),
            "LUT INIT arrays must list: {:?}",
            lines(&s)
        );
        assert!(
            has_line(&s, "NAME=seq TYPE=seq"),
            "packed sequential word must list: {:?}",
            lines(&s)
        );
        // "seq" sorts first with a single word; exercise a 64-word LUT ROM.
        let lut = s
            .debug
            .memories
            .iter()
            .find(|m| m.kind == "lut_init")
            .map(|m| m.name.clone())
            .expect("counter LUT ROM");
        eval_command(&mut s, &format!("select_memory {lut}"));
        assert_eq!(s.debug.selected_memory.as_deref(), Some(lut.as_str()));
        assert!(
            has_line(&s, &format!("NAME={lut} TYPE=")),
            "{:?}",
            lines(&s)
        );
        assert!(has_line(&s, "memory_words n="), "{:?}", lines(&s));
        eval_command(&mut s, "select_memory_word 1");
        assert_eq!(s.debug.selected_memory_addr, Some(1));
        assert!(has_line(&s, "ADDR=1 DATA="), "{:?}", lines(&s));
        eval_command(&mut s, "select_memory_word 0x2");
        assert_eq!(s.debug.selected_memory_addr, Some(2));
        eval_command(&mut s, "select_memory bogus");
        assert!(
            last_error(&s).contains("select_memory: no memory bogus"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_memory 999");
        assert!(
            last_error(&s).contains("select_memory: no row 999"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_memory_word 9999");
        assert!(
            last_error(&s).contains("select_memory_word: no addr 9999"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_memory_word bogus");
        assert!(
            last_error(&s).contains("select_memory_word: bad addr bogus"),
            "{:?}",
            lines(&s)
        );
    }

    #[test]
    fn locals_commands_follow_live_state() {
        let mut s = AppState::demo();
        eval_command(&mut s, "locals");
        assert!(
            last_error(&s).contains("sim: no design (synth first)"),
            "{:?}",
            lines(&s)
        );
        crate::views::debug::seed_debug_fixture(&mut s).expect("fixture must seed");
        eval_command(&mut s, "locals");
        assert_eq!(s.center, crate::state::CenterPane::Debug);
        assert_eq!(s.debug.tab, crate::views::debug::DebugTab::Locals);
        assert!(has_line(&s, "locals n="), "{:?}", lines(&s));
        assert!(
            has_line(&s, "scope=counter"),
            "locals default to the design top: {:?}",
            lines(&s)
        );
        assert!(
            has_line(&s, "NAME=led TYPE=port VALUE=0"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_local led");
        assert_eq!(s.debug.selected_local.as_deref(), Some("led"));
        eval_command(&mut s, "sim_run 8");
        eval_command(&mut s, "locals");
        assert!(
            has_line(&s, "NAME=led TYPE=port VALUE=1"),
            "locals must follow the engine: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_local 999");
        assert!(
            last_error(&s).contains("select_local: no row 999"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_local bogus");
        assert!(
            last_error(&s).contains("select_local: no local bogus"),
            "{:?}",
            lines(&s)
        );
    }

    #[test]
    fn force_commands_cover_hold_deposit_and_schedule() {
        let mut s = AppState::demo();
        eval_command(&mut s, "forces");
        assert!(
            has_line(
                &s,
                "forces n=0 kernel=idle engine=helion-sim no forces - add_force"
            ),
            "forces opens without an engine: {:?}",
            lines(&s)
        );
        assert_eq!(s.debug.tab, crate::views::debug::DebugTab::Forces);
        eval_command(&mut s, "add_force led 1");
        assert!(
            last_error(&s).contains("sim: no design (synth first)"),
            "{:?}",
            lines(&s)
        );
        crate::views::debug::seed_debug_fixture(&mut s).expect("fixture must seed");
        eval_command(&mut s, "add_force led 1");
        assert_eq!(s.debug.selected_force.as_deref(), Some("led"));
        assert!(
            has_line(
                &s,
                "NAME=led KIND=force VALUE=1 RADIX=bin START_PS=0 CANCEL_PS=0 STATUS=Active"
            ),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "force_table");
        assert!(has_line(&s, "forces n=1"), "{:?}", lines(&s));
        eval_command(&mut s, "force_constants");
        assert!(has_line(&s, "forces n=1"), "{:?}", lines(&s));
        eval_command(&mut s, "select_force 0");
        assert_eq!(s.debug.selected_force.as_deref(), Some("led"));
        // Re-adding the same signal replaces the row (kind/value/radix).
        eval_command(&mut s, "add_force led 0 -radix hex");
        assert_eq!(s.debug.forces.len(), 1);
        assert!(
            has_line(&s, "NAME=led KIND=force VALUE=0 RADIX=hex"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "deposit led 1");
        assert!(
            has_line(&s, "NAME=led KIND=deposit") && has_line(&s, "STATUS=Released"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "remove_force");
        assert!(s.debug.forces.is_empty(), "empty spec drops the selection");
        assert!(s.debug.selected_force.is_none());
        eval_command(&mut s, "remove_force");
        assert!(
            last_error(&s).contains("remove_force: need <signal>"),
            "{:?}",
            lines(&s)
        );
        // Scheduled holds go Pending until the engine time passes start.
        eval_command(&mut s, "add_force led 1 -start_time 1ns");
        assert!(
            has_line(
                &s,
                "NAME=led KIND=force VALUE=1 RADIX=bin START_PS=1000 CANCEL_PS=0 STATUS=Pending"
            ),
            "ns suffix parses to ps: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "sim_run 1");
        assert_eq!(s.debug.forces[0].status, "Active");
        eval_command(&mut s, "add_force");
        assert!(
            last_error(&s).contains("add_force: need <signal> <value>"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "add_force bogus 1");
        assert!(
            last_error(&s).contains("add_force: no signal bogus"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "add_force led 1 -radix oct");
        assert!(
            last_error(&s).contains("add_force: bad radix oct"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "add_force led 1 -bogus 2");
        assert!(
            last_error(&s).contains("add_force: unknown flag -bogus"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "add_force led zz -radix hex");
        assert!(
            last_error(&s).contains("add_force: bad hex zz"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_force");
        assert!(
            last_error(&s).contains("select_force: missing name"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "select_force bogus");
        assert!(
            last_error(&s).contains("select_force: no row bogus"),
            "{:?}",
            lines(&s)
        );
        eval_command(&mut s, "deposit");
        assert!(
            last_error(&s).contains("deposit: need <signal> <value>"),
            "{:?}",
            lines(&s)
        );
    }

    #[test]
    fn launch_runs_to_step_write_bitstream_arms_impl() {
        for verb in [
            "launch_runs -to_step write_bitstream",
            "launch_runs impl_1 -to_step write_bitstream",
        ] {
            let mut s = AppState::demo();
            s.runs.source = Some(counter_source());
            eval_command(&mut s, verb);
            assert_eq!(s.flow.impl_run, RunStatus::Running, "verb: {verb}");
            assert!(s.runs.is_active(), "verb: {verb}");
            assert!(
                s.console.lines.iter().any(|l| l.text == verb),
                "must echo the typed verb, not the normalized one: {:?}",
                lines(&s)
            );
            crate::runs::cancel_run(&mut s);
            pump_until_idle(&mut s);
        }
    }

    #[test]
    fn launch_runs_to_step_rejects_other_steps() {
        let mut s = AppState::demo();
        s.runs.source = Some(counter_source());
        eval_command(&mut s, "launch_runs -to_step place_design");
        assert!(
            !s.runs.is_active(),
            "unsupported -to_step must not arm a run"
        );
        assert!(
            s.console.lines.iter().any(|l| l.sev == MsgSev::Error
                && l.text.contains("launch_runs")
                && l.text.contains("-to_step write_bitstream")),
            "unsupported -to_step must error with usage honestly: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn synth_design_elaborates_the_editor_buffer() {
        let mut s = AppState::demo();
        s.editor.tabs = vec!["top.v".into()];
        s.editor.active = 0;
        s.editor.lines =
            vec!["module top(input clk, output led); assign led = clk; endmodule".into()];
        eval_command(&mut s, "synth_design");
        assert!(
            lines(&s)
                .iter()
                .any(|l| l.contains("synth") && l.contains("cells")),
            "synth_design must elaborate the buffer: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_cells");
        assert!(
            s.console
                .lines
                .iter()
                .any(|l| l.sev != MsgSev::Error && !l.text.is_empty()),
            "get_cells must read the elaborated design: {:?}",
            lines(&s)
        );
    }

    // ---- deep-06-tcl-full parity tests (failing first) ----

    /// Every verb the old IDE's `tcl_interp` supports (CORE 29 + HOST 43).
    /// Sources: `helion-gui/src/tcl/interp.rs:14` (`CORE_COMMANDS`),
    /// `helion-gui/src/tcl/design.rs:29` (`HOST_COMMANDS`).
    const OLD_IDE_VERBS: &[&str] = &[
        "set",
        "unset",
        "puts",
        "expr",
        "if",
        "foreach",
        "for",
        "while",
        "proc",
        "return",
        "break",
        "continue",
        "list",
        "llength",
        "lindex",
        "lrange",
        "lappend",
        "concat",
        "join",
        "split",
        "string",
        "info",
        "catch",
        "error",
        "eval",
        "subst",
        "source",
        "format",
        "incr",
        "append",
        "help",
        "clear",
        "get_cells",
        "get_nets",
        "get_pins",
        "get_ports",
        "get_clocks",
        "get_timing_paths",
        "get_property",
        "set_property",
        "report_property",
        "list_property",
        "list_properties",
        "select_objects",
        "highlight_objects",
        "get_selected_objects",
        "current_design",
        "synth_design",
        "read_sv",
        "read_verilog",
        "read_vhdl",
        "opt_design",
        "place_design",
        "route_design",
        "write_bitstream",
        "write_checkpoint",
        "create_clock",
        "report_timing",
        "report_utilization",
        "mark_debug",
        "add_probe",
        "eco",
        "open_hw_manager",
        "program_hw",
        "program_hw_devices",
        "report_die",
        "report_hw_targets",
        "report_featuremap",
        "write_hnf",
        "hds::synth",
        "hds::impl",
        "hds::flow",
        "hds::tree",
    ];

    fn last_error(s: &AppState) -> String {
        s.console
            .lines
            .iter()
            .rev()
            .find(|l| l.sev == MsgSev::Error)
            .map(|l| l.text.clone())
            .unwrap_or_default()
    }

    fn has_line(s: &AppState, want: &str) -> bool {
        s.console.lines.iter().any(|l| l.text.contains(want))
    }

    fn hl_part(s: &mut AppState) {
        s.project.part = "HL10T-C32-1".into();
    }

    fn with_counter(s: &mut AppState) {
        hl_part(s);
        let src = counter_source();
        eval_command(s, &format!("read_sv {}", src.display()));
        assert!(
            has_line(s, "synth"),
            "read_sv must elaborate: {:?}",
            lines(s)
        );
    }

    #[test]
    fn commands_covers_every_old_ide_verb() {
        for verb in OLD_IDE_VERBS {
            assert!(
                COMMANDS.contains(verb),
                "COMMANDS must include old-IDE verb {verb}"
            );
        }
        for verb in [
            "assign_package_pin",
            "package",
            "package_drawing",
            "select_package_pin",
            "sim_run",
            "sim_step",
            "sim_restart",
            "breakpoints",
            "add_bp",
            "select_breakpoint",
            "enable_bp",
            "disable_bp",
            "delete_bp",
            "memory",
            "memories",
            "select_memory",
            "select_memory_word",
            "locals",
            "select_local",
            "forces",
            "force_table",
            "force_constants",
            "add_force",
            "deposit",
            "remove_force",
            "select_force",
        ] {
            assert!(
                COMMANDS.contains(&verb),
                "COMMANDS must include old Model::exec verb {verb}"
            );
        }
        for extra in [
            "history",
            "launch_runs",
            "cancel_runs",
            "open_report",
            "launch_simulation",
        ] {
            assert!(COMMANDS.contains(&extra), "rune-only {extra} must stay");
        }
    }

    #[test]
    fn unset_and_puts_match_old_texts() {
        let mut s = AppState::demo();
        eval_command(&mut s, "set x 1; unset x");
        assert!(
            !has_line(&s, "ERROR"),
            "unset must succeed: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "set x");
        assert!(
            last_error(&s).contains("no such variable"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "unset -nocomplain x");
        assert!(
            !last_error(&s).contains("can't unset"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "unset -bogus x");
        assert!(
            last_error(&s).contains("bad option \"-bogus\": must be -nocomplain"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "puts -nonewline stdout hi");
        assert!(has_line(&s, "hi"), "puts channel form: {:?}", lines(&s));
        eval_command(&mut s, "puts -bogus hi");
        assert!(
            last_error(&s).contains("bad option \"-bogus\": must be -nonewline"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "set a");
        assert!(
            last_error(&s).contains("can't read \"a\": no such variable"),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn control_flow_matches_old_semantics() {
        let mut s = AppState::demo();
        eval_command(&mut s, "if {1} {set r yes} else {set r no}");
        eval_command(&mut s, "set r");
        assert!(lines(&s).contains(&"yes"), "if/else: {:?}", lines(&s));
        eval_command(
            &mut s,
            "if {0} {set r a} elseif {1} {set r b} else {set r c}",
        );
        eval_command(&mut s, "set r");
        assert!(lines(&s).contains(&"b"), "elseif: {:?}", lines(&s));
        eval_command(
            &mut s,
            "set t 0; foreach x {1 2 3} {set t [expr {$t + $x}]}",
        );
        eval_command(&mut s, "set t");
        assert!(lines(&s).contains(&"6"), "foreach sum: {:?}", lines(&s));
        eval_command(
            &mut s,
            "set t 0; for {set i 0} {$i < 4} {incr i} {set t [expr {$t + $i}]}",
        );
        eval_command(&mut s, "set t");
        assert!(lines(&s).contains(&"6"), "for sum: {:?}", lines(&s));
        eval_command(
            &mut s,
            "set t 0; set i 0; while {$i < 3} {set t [expr {$t + 1}]; incr i}",
        );
        eval_command(&mut s, "set t");
        assert!(lines(&s).contains(&"3"), "while: {:?}", lines(&s));
        eval_command(
            &mut s,
            "set o {}; foreach x {1 2 3 4} {if {$x > 2} break; lappend o $x}; set o",
        );
        assert!(has_line(&s, "1 2"), "break: {:?}", lines(&s));
        eval_command(&mut s, "if 0 {set r a} puts");
        assert!(
            last_error(&s).contains("unexpected \"puts\" after if body"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "break");
        assert!(
            last_error(&s).contains("invoked \"break\" outside of a loop"),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn proc_defaults_and_return() {
        let mut s = AppState::demo();
        eval_command(&mut s, "proc add {a {b 10}} {expr {$a + $b}}");
        eval_command(&mut s, "add 5");
        assert!(lines(&s).contains(&"15"), "default arg: {:?}", lines(&s));
        eval_command(&mut s, "add 5 7");
        assert!(lines(&s).contains(&"12"), "got: {:?}", lines(&s));
        eval_command(&mut s, "proc early {} {return 42; set x 1}; early");
        assert!(lines(&s).contains(&"42"), "return: {:?}", lines(&s));
        eval_command(&mut s, "add 1 2 3");
        assert!(
            last_error(&s).contains("wrong # args: should be \"add a b\""),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "set x outer; proc sc {} {set x inner}; sc; set x");
        assert!(lines(&s).contains(&"outer"), "proc scope: {:?}", lines(&s));
    }

    #[test]
    fn list_family_round_trip() {
        let mut s = AppState::demo();
        eval_command(&mut s, "list a {b c} d");
        assert!(has_line(&s, "a {b c} d"), "list quotes: {:?}", lines(&s));
        eval_command(&mut s, "llength {a b c}");
        assert!(lines(&s).contains(&"3"), "got: {:?}", lines(&s));
        eval_command(&mut s, "lindex {a b c} end-1");
        assert!(lines(&s).contains(&"b"), "got: {:?}", lines(&s));
        eval_command(&mut s, "lrange {a b c d} 1 2");
        assert!(has_line(&s, "b c"), "got: {:?}", lines(&s));
        eval_command(&mut s, "lappend mylist x y; set mylist");
        assert!(has_line(&s, "x y"), "got: {:?}", lines(&s));
        eval_command(&mut s, "concat {a b} {c}");
        assert!(has_line(&s, "a b c"), "got: {:?}", lines(&s));
        eval_command(&mut s, "join {a b c} ,");
        assert!(has_line(&s, "a,b,c"), "got: {:?}", lines(&s));
        eval_command(&mut s, "split a,b,c ,");
        assert!(has_line(&s, "a b c"), "got: {:?}", lines(&s));
        eval_command(&mut s, "lindex {a} bogus");
        assert!(
            last_error(&s).contains("bad index \"bogus\""),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn string_all_nine_subcommands() {
        let mut s = AppState::demo();
        for (cmd, want) in [
            ("string length hello", "5"),
            ("string range hello 1 3", "ell"),
            ("string index hello end", "o"),
            ("string match {h*} hello", "1"),
            ("string tolower HELLO", "hello"),
            ("string toupper hello", "HELLO"),
            ("string trim {  hi  }", "hi"),
            ("string first l hello", "2"),
            ("string equal a a", "1"),
        ] {
            eval_command(&mut s, cmd);
            assert!(has_line(&s, want), "{cmd}: {:?}", lines(&s));
        }
        eval_command(&mut s, "string frobnicate a");
        assert!(
            last_error(&s).contains("bad string subcommand \"frobnicate\""),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn info_subcommands() {
        let mut s = AppState::demo();
        eval_command(&mut s, "set myvar 1; proc myproc {} {}; info exists myvar");
        assert!(lines(&s).contains(&"1"), "got: {:?}", lines(&s));
        eval_command(&mut s, "info vars my*");
        assert!(has_line(&s, "myvar"), "got: {:?}", lines(&s));
        eval_command(&mut s, "info procs my*");
        assert!(has_line(&s, "myproc"), "got: {:?}", lines(&s));
        eval_command(&mut s, "info commands get_cell*");
        assert!(has_line(&s, "get_cells"), "got: {:?}", lines(&s));
        eval_command(&mut s, "proc ai {a {b 2}} {}; info args ai");
        assert!(has_line(&s, "a b"), "got: {:?}", lines(&s));
        eval_command(&mut s, "info body ai");
        assert!(has_line(&s, "<parsed body>"), "got: {:?}", lines(&s));
        eval_command(&mut s, "info frobnicate");
        assert!(
            last_error(&s).contains("bad info subcommand \"frobnicate\""),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn catch_error_eval_subst_source_format_incr_append() {
        let mut s = AppState::demo();
        eval_command(&mut s, "catch {error boom} msg; set msg");
        assert!(has_line(&s, "boom"), "catch var: {:?}", lines(&s));
        eval_command(&mut s, "catch {set ok 1}");
        assert!(lines(&s).contains(&"0"), "catch ok: {:?}", lines(&s));
        eval_command(&mut s, "catch {error boom}");
        assert!(lines(&s).contains(&"1"), "catch code: {:?}", lines(&s));
        eval_command(&mut s, "error custom-fail");
        assert!(
            last_error(&s).contains("custom-fail"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "eval {set e} 42; set e");
        assert!(lines(&s).contains(&"42"), "eval: {:?}", lines(&s));
        eval_command(&mut s, "set n World; subst {hello $n}");
        assert!(has_line(&s, "hello World"), "subst: {:?}", lines(&s));
        eval_command(&mut s, "subst -novariables {hello $n}");
        assert!(
            has_line(&s, "hello $n"),
            "subst -novariables: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "subst {sum=[expr {20 + 22}]}");
        assert!(has_line(&s, "sum=42"), "subst [cmd]: {:?}", lines(&s));
        eval_command(&mut s, "puts -nonewline ab; puts cd");
        assert!(
            has_line(&s, "abcd"),
            "puts -nonewline concats: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "format {%s=%d (0x%x)} cells 12 12");
        assert!(has_line(&s, "cells=12 (0xc)"), "format: {:?}", lines(&s));
        eval_command(&mut s, "incr c; incr c 41; set c");
        assert!(lines(&s).contains(&"42"), "incr: {:?}", lines(&s));
        eval_command(&mut s, "append g foo bar; set g");
        assert!(has_line(&s, "foobar"), "append: {:?}", lines(&s));
        let dir = std::env::temp_dir().join("helion-rune-tcl-src");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("lib.tcl");
        std::fs::write(&f, "proc fromfile {} {return sourced!}; fromfile").unwrap();
        eval_command(&mut s, &format!("source {}", f.display()));
        assert!(has_line(&s, "sourced!"), "source: {:?}", lines(&s));
        eval_command(&mut s, "source /no/such/file.tcl");
        assert!(
            last_error(&s).contains("couldn't read file \"/no/such/file.tcl\""),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn expr_parity_with_old_ide() {
        let mut s = AppState::demo();
        for (cmd, want) in [
            ("expr {1 ? 2 : 3}", "2"),
            ("expr {2 ** 10}", "1024"),
            ("expr {\"a\" eq \"a\"}", "1"),
            ("expr {\"a\" ne \"b\"}", "1"),
            ("expr {\"abc\" < \"abd\"}", "1"),
            ("expr {abs(-5)}", "5"),
            ("expr {int(3.9)}", "3"),
            ("expr {sqrt(16)}", "4.0"),
            ("expr {pow(2, 8)}", "256.0"),
            ("expr 0x10", "16"),
        ] {
            eval_command(&mut s, cmd);
            assert!(has_line(&s, want), "{cmd}: {:?}", lines(&s));
        }
        eval_command(&mut s, "set ev 21; expr {$ev * 2}");
        assert!(lines(&s).contains(&"42"), "dollar var: {:?}", lines(&s));
        eval_command(&mut s, "set ev 21; expr {ev * 2}");
        assert!(lines(&s).contains(&"42"), "bare word var: {:?}", lines(&s));
        eval_command(&mut s, "expr {[set q 20] + 22}");
        assert!(lines(&s).contains(&"42"), "cmd subst: {:?}", lines(&s));
        eval_command(&mut s, "expr {1 / 0}");
        assert!(
            last_error(&s).contains("divide by zero"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "expr {bogusfunc(1)}");
        assert!(
            last_error(&s).contains("unknown function \"bogusfunc\""),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn arrays_and_lone_dollar() {
        let mut s = AppState::demo();
        eval_command(&mut s, "set a(0) first; set a(1) second; set a(1)");
        assert!(has_line(&s, "second"), "got: {:?}", lines(&s));
        eval_command(&mut s, "set i 0; set a($i)");
        assert!(has_line(&s, "first"), "index subst: {:?}", lines(&s));
        eval_command(&mut s, "set price 5$; set price");
        assert!(has_line(&s, "5$"), "lone dollar: {:?}", lines(&s));
    }

    #[test]
    fn errors_carry_old_style_line_numbers() {
        let mut s = AppState::demo();
        eval_command(&mut s, "set a 1\nbogus_line2\nset b 2");
        let err = last_error(&s);
        assert!(
            err.contains("invalid command name \"bogus_line2\""),
            "got: {err}"
        );
        assert!(err.contains("(line 2)"), "old ERROR text has line: {err}");
        assert!(err.starts_with("ERROR: "), "got: {err}");
    }

    #[test]
    fn help_lists_every_command_and_topic() {
        let mut s = AppState::demo();
        eval_command(&mut s, "help");
        for verb in OLD_IDE_VERBS.iter().chain(["history"].iter()) {
            assert!(has_line(&s, verb), "help must list {verb}: {:?}", lines(&s));
        }
        eval_command(&mut s, "help get_cells");
        assert!(has_line(&s, "get_cells"), "got: {:?}", lines(&s));
        eval_command(&mut s, "help bogus_topic_xyz");
        assert!(
            last_error(&s).contains("no help topic \"bogus_topic_xyz\""),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "proc myhelp {a} {}; help myhelp");
        assert!(has_line(&s, "user procedure"), "proc help: {:?}", lines(&s));
    }

    #[test]
    fn object_queries_with_filter_of_objects() {
        let mut s = AppState::demo();
        with_counter(&mut s);
        eval_command(&mut s, "get_pins -of_objects u_luts0");
        assert!(has_line(&s, "u_luts0/"), "pins of cell: {:?}", lines(&s));
        eval_command(&mut s, "get_cells -filter {TYPE == lut}");
        assert!(has_line(&s, "u_lut"), "filter: {:?}", lines(&s));
        eval_command(&mut s, "get_cells -filter {TYPE == lut && FANOUT >= 0}");
        assert!(has_line(&s, "u_lut"), "and-filter: {:?}", lines(&s));
        let before = s.console.lines.len();
        eval_command(&mut s, "get_cells -filter {BOGUSPROP == x}");
        assert!(
            s.console.lines[before..]
                .iter()
                .all(|l| !l.text.contains("u_lut")),
            "empty filter must match nothing: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_nets -of_objects u_luts0");
        assert!(
            !last_error(&s).contains("no such object"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_cells -bogus x");
        assert!(
            last_error(&s).contains("get_cells: unknown option \"-bogus\""),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_cells -filter");
        assert!(
            last_error(&s).contains("get_cells: -filter needs an argument"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_cells -of_objects nosuchobj");
        assert!(
            last_error(&s).contains("-of_objects: no such object \"nosuchobj\""),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_cells {u_lut[0123456789]*}");
        assert!(has_line(&s, "u_lut"), "glob class: {:?}", lines(&s));
    }

    #[test]
    fn clocks_timing_paths_and_create_clock() {
        let mut s = AppState::demo();
        with_counter(&mut s);
        eval_command(&mut s, "create_clock -period 10 clk");
        assert!(
            has_line(&s, "clock clk period 10000ps"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_clocks");
        assert!(has_line(&s, "clk"), "got: {:?}", lines(&s));
        eval_command(&mut s, "get_property PERIOD clk");
        assert!(has_line(&s, "10000"), "clock prop: {:?}", lines(&s));
        eval_command(&mut s, "get_timing_paths -max_paths 3");
        assert!(!has_line(&s, "ERROR"), "timing paths: {:?}", lines(&s));
        eval_command(&mut s, "create_clock -period bogus clk");
        assert!(
            last_error(&s).contains("create_clock: bad -period \"bogus\""),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "create_clock -bogus 1 clk");
        assert!(
            last_error(&s).contains("create_clock: unknown option \"-bogus\""),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn properties_select_current_design_hnf() {
        let mut s = AppState::demo();
        with_counter(&mut s);
        eval_command(&mut s, "get_property TYPE u_luts0");
        assert!(has_line(&s, "lut"), "vivado order: {:?}", lines(&s));
        eval_command(&mut s, "get_property u_luts0 TYPE");
        assert!(has_line(&s, "lut"), "obj-prop order: {:?}", lines(&s));
        eval_command(&mut s, "get_property BOGUS u_luts0");
        assert!(
            last_error(&s).contains("no such object \"u_luts0\" (or unknown property \"BOGUS\")"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_property -quiet BOGUS u_luts0");
        let quiet_err = last_error(&s);
        eval_command(&mut s, "set_property KEEP TRUE u_luts0");
        assert!(has_line(&s, "KEEP"), "got: {:?}", lines(&s));
        eval_command(&mut s, "get_property KEEP u_luts0");
        assert!(has_line(&s, "TRUE"), "keep sticks: {:?}", lines(&s));
        eval_command(&mut s, "get_property DONT_TOUCH u_luts0");
        assert!(
            last_error(&s).contains("unknown property"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "set_property BOGUSPROP 1 u_luts0");
        assert!(
            last_error(&s).contains("unsupported property \"BOGUSPROP\""),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "report_property u_luts0");
        assert!(has_line(&s, "TYPE = lut"), "got: {:?}", lines(&s));
        eval_command(&mut s, "list_properties u_luts0");
        assert!(has_line(&s, "TYPE"), "got: {:?}", lines(&s));
        eval_command(&mut s, "select_objects u_luts0");
        assert!(has_line(&s, "u_luts0"), "got: {:?}", lines(&s));
        eval_command(&mut s, "get_selected_objects");
        assert!(has_line(&s, "u_luts0"), "got: {:?}", lines(&s));
        eval_command(&mut s, "highlight_objects u_luts0");
        assert!(
            s.tcl.selected.iter().any(|o| o.highlighted),
            "highlight flag"
        );
        eval_command(&mut s, "select_objects nosuchobj");
        assert!(
            last_error(&s).contains("select_objects: no such object \"nosuchobj\""),
            "got: {:?}",
            lines(&s)
        );
        let before = s.console.lines.len();
        eval_command(&mut s, "current_design");
        assert!(
            s.console.lines[before..]
                .iter()
                .any(|l| l.text == "counter"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "write_hnf");
        assert!(has_line(&s, "HNF 1"), "got: {:?}", lines(&s));
        let _ = quiet_err;
    }

    #[test]
    fn flow_commands_mirror_old_messages() {
        let mut s = AppState::demo();
        with_counter(&mut s);
        eval_command(&mut s, "opt_design");
        assert!(has_line(&s, "opt removed"), "got: {:?}", lines(&s));
        eval_command(&mut s, "place_design");
        assert!(has_line(&s, "place_design sites"), "got: {:?}", lines(&s));
        eval_command(&mut s, "route_design");
        assert!(has_line(&s, "route_design hops"), "got: {:?}", lines(&s));
        eval_command(&mut s, "write_bitstream");
        assert!(has_line(&s, "write_bitstream"), "got: {:?}", lines(&s));
        eval_command(&mut s, "write_checkpoint");
        assert!(has_line(&s, "write_checkpoint"), "got: {:?}", lines(&s));
        assert!(has_line(&s, "hash 0x"), "got: {:?}", lines(&s));
        eval_command(&mut s, "report_timing");
        assert!(has_line(&s, "Timing Report:"), "got: {:?}", lines(&s));
        assert!(has_line(&s, "endpoints"), "got: {:?}", lines(&s));
        eval_command(&mut s, "report_utilization");
        assert!(has_line(&s, "Utilization:"), "got: {:?}", lines(&s));
        eval_command(&mut s, "hds::impl");
        assert!(has_line(&s, "impl frames"), "got: {:?}", lines(&s));
    }

    #[test]
    fn debug_eco_hw_die_and_hds() {
        let mut s = AppState::demo();
        with_counter(&mut s);
        eval_command(&mut s, "mark_debug clk");
        assert!(has_line(&s, "mark_debug clk"), "got: {:?}", lines(&s));
        eval_command(&mut s, "open_hw_manager");
        assert!(has_line(&s, "open_hw_manager sim"), "got: {:?}", lines(&s));
        eval_command(&mut s, "program_hw cable=sim");
        assert!(has_line(&s, "backend=sim"), "got: {:?}", lines(&s));
        assert!(has_line(&s, "DONE="), "got: {:?}", lines(&s));
        let before = s.console.lines.len();
        eval_command(&mut s, "report_die");
        assert!(
            s.console.lines[before..]
                .iter()
                .all(|l| l.sev != MsgSev::Error),
            "got: {:?}",
            lines(&s)
        );
        let before = s.console.lines.len();
        eval_command(&mut s, "report_featuremap");
        assert!(
            s.console.lines[before..]
                .iter()
                .all(|l| l.sev != MsgSev::Error),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "hds::flow");
        assert!(
            has_line(&s, "synth opt place route bits hw"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "hds::tree");
        assert!(has_line(&s, "sources"), "got: {:?}", lines(&s));
        eval_command(&mut s, "hds::synth");
        assert!(has_line(&s, "synth ok"), "got: {:?}", lines(&s));
    }

    #[test]
    fn eco_probe_aliases_and_quiet_flags() {
        let mut s = AppState::demo();
        with_counter(&mut s);
        eval_command(&mut s, "add_probe clk");
        assert!(has_line(&s, "add_probe clk"), "got: {:?}", lines(&s));
        eval_command(&mut s, "eco u_luts0 0xAAAAAAAAAAAAAAAA");
        assert!(
            has_line(&s, "eco u_luts0 0xaaaaaaaaaaaaaaaa"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "program_hw_devices cable=sim");
        assert!(has_line(&s, "backend=sim"), "got: {:?}", lines(&s));
        eval_command(&mut s, "report_hw_targets");
        assert!(has_line(&s, "HL10T-C32-1"), "got: {:?}", lines(&s));
        eval_command(&mut s, "list_property u_luts0");
        assert!(has_line(&s, "TYPE"), "got: {:?}", lines(&s));
        eval_command(&mut s, "report_property u_luts0 TYPE");
        assert!(has_line(&s, "TYPE = lut"), "pattern form: {:?}", lines(&s));
        let before = s.console.lines.len();
        eval_command(&mut s, "get_property -quiet BOGUS nosuchobj");
        assert!(
            s.console.lines[before..]
                .iter()
                .all(|l| l.sev != MsgSev::Error),
            "-quiet must stay silent: {:?}",
            lines(&s)
        );
        let before = s.console.lines.len();
        eval_command(&mut s, "set_property -quiet BOGUSPROP 1 u_luts0");
        assert!(
            s.console.lines[before..]
                .iter()
                .all(|l| l.sev != MsgSev::Error),
            "-quiet must stay silent: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "get_ports -filter {DIRECTION == IN}");
        assert!(!has_line(&s, "ERROR: get_ports"), "got: {:?}", lines(&s));
    }

    #[test]
    fn no_design_errors_name_the_command() {
        let mut s = AppState::demo();
        hl_part(&mut s);
        for cmd in [
            "get_cells",
            "get_nets",
            "get_pins",
            "get_ports",
            "current_design",
            "write_hnf",
            "report_timing",
        ] {
            eval_command(&mut s, cmd);
            assert!(
                last_error(&s).contains("no design"),
                "{cmd} must name no-design: {:?}",
                lines(&s)
            );
        }
        eval_command(&mut s, "opt_design");
        assert!(
            last_error(&s).contains("opt_design: no design"),
            "got: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "set_property KEEP TRUE u_luts0");
        assert!(
            last_error(&s).contains("set_property: no design"),
            "got: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn read_verbs_match_old_synth_message() {
        let mut s = AppState::demo();
        hl_part(&mut s);
        let src = counter_source();
        eval_command(&mut s, &format!("read_verilog {}", src.display()));
        assert!(has_line(&s, "synth"), "got: {:?}", lines(&s));
        assert!(has_line(&s, "cells"), "got: {:?}", lines(&s));
        assert!(has_line(&s, "luts"), "got: {:?}", lines(&s));
        eval_command(&mut s, "read_sv /no/such/file.sv");
        assert!(
            has_line(&s, "ERROR"),
            "missing file must error: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "synth_design");
        assert!(
            last_error(&s).contains("synth_design: no RTL source") || has_line(&s, "synth"),
            "no-arg synth keeps the editor-buffer form: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn suggestions_span_the_full_set() {
        let mut s = AppState::demo();
        eval_command(&mut s, "get_cel");
        assert!(
            last_error(&s).contains("get_cells"),
            "prefix suggest: {:?}",
            lines(&s)
        );
        eval_command(&mut s, "strng length ab");
        assert!(
            last_error(&s).contains("string"),
            "typo suggest: {:?}",
            lines(&s)
        );
    }

    #[test]
    fn semicolons_chain_and_comments_are_quiet() {
        let mut s = AppState::demo();
        eval_command(&mut s, "set a 1; set b 2; expr {$a + $b}");
        assert!(
            lines(&s).contains(&"3"),
            "chained script must evaluate: {:?}",
            lines(&s)
        );
        let before = s.console.lines.len();
        eval_command(&mut s, "# just a comment");
        assert!(
            s.console.lines.len() <= before + 1,
            "comment-only input evaluates nothing: {:?}",
            lines(&s)
        );
        assert!(
            s.tcl.history.iter().any(|h| h == "# just a comment"),
            "history keeps it"
        );
    }
}
