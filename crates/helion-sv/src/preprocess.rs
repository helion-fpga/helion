//! Verilog/SV preprocessor: `` `define `` / `` `ifdef `` / macro expansion.
//! Unknown `` `FOO `` and `` `FOO(...) `` are dropped so Helion can ingest
//! large cores (Ibex, PicoRV32) that ship with assertion macros.

use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
struct Macro {
    args: Vec<String>,
    body: String,
}

use std::path::{Path, PathBuf};

/// Expand `` `include "file" `` from the including file's directory and
/// obvious include dirs (`include/`, parent `include/`). Quoted form only.
/// Missing files are left in place so the preprocessor can still drop the
/// directive; a named diagnostic is printed.
pub fn expand_includes(source: &str, base: &Path) -> String {
    expand_includes_depth(source, base, &mut Vec::new(), 0)
}

fn expand_includes_depth(
    source: &str,
    base: &Path,
    seen: &mut Vec<PathBuf>,
    depth: usize,
) -> String {
    if depth > 8 {
        return source.to_string();
    }
    let mut out = String::new();
    let mut rest = source;
    while let Some(idx) = rest.find("`include") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + "`include".len()..];
        let trimmed = after.trim_start();
        let ws = after.len() - trimmed.len();
        let (path_s, consumed) = if let Some(q) = trimmed.strip_prefix('"') {
            if let Some(end) = q.find('"') {
                (q[..end].to_string(), ws + 1 + end + 1)
            } else {
                out.push_str("`include");
                rest = after;
                continue;
            }
        } else if let Some(q) = trimmed.strip_prefix('<') {
            if let Some(end) = q.find('>') {
                (q[..end].to_string(), ws + 1 + end + 1)
            } else {
                out.push_str("`include");
                rest = after;
                continue;
            }
        } else {
            out.push_str("`include");
            rest = after;
            continue;
        };
        rest = &after[consumed..];
        if let Some(nl) = rest.find('\n') {
            // keep newline for line structure; drop the rest of the include line
            rest = &rest[nl..];
        }
        match resolve_include(base, &path_s) {
            Some(found) => {
                if seen.iter().any(|p| p == &found) {
                    eprintln!(
                        "diagnostic skip_include file={} why=include cycle; not a LUT",
                        found.display()
                    );
                } else {
                    seen.push(found.clone());
                    let text = std::fs::read_to_string(&found).unwrap_or_default();
                    let child_base = found.parent().unwrap_or(base);
                    out.push_str(&expand_includes_depth(&text, child_base, seen, depth + 1));
                    out.push('\n');
                }
            }
            None => {
                eprintln!(
                    "diagnostic skip_include file={} base={} why=include not found in same dir or include path; not a LUT",
                    path_s,
                    base.display()
                );
            }
        }
    }
    out.push_str(rest);
    out
}

fn resolve_include(base: &Path, spec: &str) -> Option<PathBuf> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let mut cands = Vec::new();
    cands.push(base.join(spec));
    if let Some(name) = Path::new(spec).file_name() {
        cands.push(base.join(name));
        cands.push(base.join("include").join(name));
        if let Some(parent) = base.parent() {
            cands.push(parent.join("include").join(name));
            cands.push(parent.join(spec));
        }
    }
    cands.push(base.join("include").join(spec));
    cands.into_iter().find(|p| p.is_file())
}

/// Expand `` `define `` / `` `ifdef `` and strip leftover backticks.
pub fn preprocess_sv(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut defines: HashMap<String, Macro> = HashMap::new();
    // (parent_emitting, this_branch_on)
    let mut stack: Vec<(bool, bool)> = vec![(true, true)];
    let mut out = String::new();

    fn emitting(stack: &[(bool, bool)]) -> bool {
        stack.last().map(|(p, t)| *p && *t).unwrap_or(true)
    }

    while i < chars.len() {
        if chars[i] == '`' {
            let start = i;
            i += 1;
            let name = read_ident(&chars, &mut i);
            if name.is_empty() {
                continue;
            }
            skip_ws_not_nl(&chars, &mut i);
            match name.as_str() {
                "define" => {
                    let dname = read_ident(&chars, &mut i);
                    let (args, body) = read_define_body(&chars, &mut i);
                    if emitting(&stack) && !dname.is_empty() {
                        defines.insert(dname, Macro { args, body });
                    }
                }
                "undef" => {
                    let dname = read_ident(&chars, &mut i);
                    skip_to_eol(&chars, &mut i);
                    if emitting(&stack) {
                        defines.remove(&dname);
                    }
                }
                "ifdef" | "ifndef" => {
                    let dname = read_ident(&chars, &mut i);
                    skip_to_eol(&chars, &mut i);
                    let parent = emitting(&stack);
                    let present = defines.contains_key(&dname);
                    let take = if name == "ifdef" { present } else { !present };
                    stack.push((parent, parent && take));
                }
                "elsif" | "elif" => {
                    let dname = read_ident(&chars, &mut i);
                    skip_to_eol(&chars, &mut i);
                    if let Some((parent, was)) = stack.pop() {
                        let present = defines.contains_key(&dname);
                        stack.push((parent, parent && !was && present));
                    }
                }
                "else" => {
                    skip_to_eol(&chars, &mut i);
                    if let Some((parent, was)) = stack.pop() {
                        stack.push((parent, parent && !was));
                    }
                }
                "endif" => {
                    skip_to_eol(&chars, &mut i);
                    if stack.len() > 1 {
                        stack.pop();
                    }
                }
                "include" | "timescale" | "resetall" | "default_nettype" | "line"
                | "unconnected_drive" | "nounconnected_drive" | "celldefine"
                | "endcelldefine" | "pragma" | "begin_keywords" | "end_keywords" => {
                    skip_to_eol(&chars, &mut i);
                }
                _ => {
                    i = start + 1; // after backtick
                    let _ = read_ident(&chars, &mut i);
                    if emitting(&stack) {
                        if let Some(m) = defines.get(&name).cloned() {
                            let text = expand_macro(&m, &chars, &mut i);
                            out.push_str(&text);
                        } else {
                            skip_opt_args(&chars, &mut i);
                        }
                    } else {
                        skip_opt_args(&chars, &mut i);
                    }
                }
            }
            continue;
        }
        if emitting(&stack) {
            out.push(chars[i]);
        } else if chars[i] == '\n' {
            out.push('\n');
        }
        i += 1;
    }
    out
}

fn read_ident(chars: &[char], i: &mut usize) -> String {
    let mut s = String::new();
    while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '_') {
        s.push(chars[*i]);
        *i += 1;
    }
    s
}

fn skip_ws_not_nl(chars: &[char], i: &mut usize) {
    while *i < chars.len() && chars[*i].is_whitespace() && chars[*i] != '\n' {
        *i += 1;
    }
}

fn skip_to_eol(chars: &[char], i: &mut usize) {
    while *i < chars.len() && chars[*i] != '\n' {
        *i += 1;
    }
}

fn read_define_body(chars: &[char], i: &mut usize) -> (Vec<String>, String) {
    let mut args = Vec::new();
    if *i < chars.len() && chars[*i] == '(' {
        *i += 1;
        loop {
            let start = *i;
            skip_ws_not_nl(chars, i);
            if *i >= chars.len() || chars[*i] == '\n' {
                break;
            }
            if chars[*i] == ')' {
                *i += 1;
                break;
            }
            let a = read_ident(chars, i);
            if a.is_empty() {
                // Non-ident in the arg list (`define FOO(1+2)`). Consume to the
                // matching ')' so i never stalls.
                let mut depth = 1i32;
                while *i < chars.len() && chars[*i] != '\n' && depth > 0 {
                    match chars[*i] {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    *i += 1;
                }
                break;
            }
            args.push(a);
            skip_ws_not_nl(chars, i);
            if *i < chars.len() && chars[*i] == '=' {
                *i += 1;
                let mut d = 0i32;
                while *i < chars.len() && chars[*i] != '\n' {
                    match chars[*i] {
                        '(' => d += 1,
                        ')' if d == 0 => break,
                        ')' => d -= 1,
                        ',' if d == 0 => break,
                        _ => {}
                    }
                    *i += 1;
                }
            }
            skip_ws_not_nl(chars, i);
            if *i < chars.len() && chars[*i] == ',' {
                *i += 1;
                continue;
            }
            if *i < chars.len() && chars[*i] == ')' {
                *i += 1;
                break;
            }
            if *i == start {
                *i += 1;
            }
        }
    }
    skip_ws_not_nl(chars, i);
    let mut body = String::new();
    while *i < chars.len() && chars[*i] != '\n' {
        if chars[*i] == '\\' && chars.get(*i + 1) == Some(&'\n') {
            *i += 2;
            body.push(' ');
            continue;
        }
        body.push(chars[*i]);
        *i += 1;
    }
    (args, body.trim().to_string())
}

fn skip_opt_args(chars: &[char], i: &mut usize) {
    skip_ws_not_nl(chars, i);
    if *i < chars.len() && chars[*i] == '(' {
        let mut depth = 0i32;
        while *i < chars.len() {
            match chars[*i] {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    *i += 1;
                    if depth <= 0 {
                        break;
                    }
                    continue;
                }
                _ => {}
            }
            *i += 1;
        }
    }
}

fn expand_macro(m: &Macro, chars: &[char], i: &mut usize) -> String {
    if m.args.is_empty() {
        return m.body.clone();
    }
    skip_ws_not_nl(chars, i);
    let mut vals: Vec<String> = Vec::new();
    if *i < chars.len() && chars[*i] == '(' {
        *i += 1;
        let mut cur = String::new();
        let mut depth = 1i32;
        while *i < chars.len() && depth > 0 {
            let c = chars[*i];
            *i += 1;
            if c == '(' {
                depth += 1;
                cur.push(c);
            } else if c == ')' {
                depth -= 1;
                if depth == 0 {
                    if !cur.is_empty() {
                        vals.push(cur.trim().to_string());
                    }
                    break;
                }
                cur.push(c);
            } else if c == ',' && depth == 1 {
                vals.push(cur.trim().to_string());
                cur.clear();
            } else {
                cur.push(c);
            }
        }
    }
    let mut body = m.body.clone();
    for (a, v) in m.args.iter().zip(vals.iter()) {
        body = body.replace(a, v);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ifdef_keeps_defined_branch() {
        let s = preprocess_sv(
            "`define FOO\n`ifdef FOO\nwire a;\n`else\nwire b;\n`endif\n",
        );
        assert!(s.contains("wire a;"), "{s}");
        assert!(!s.contains("wire b;"), "{s}");
    }

    #[test]
    fn unknown_macro_invocation_dropped() {
        let s = preprocess_sv("`ASSERT(foo, bar)\nassign led = q;\n");
        assert!(s.contains("assign led = q;"), "{s}");
        assert!(!s.contains("ASSERT"), "{s}");
    }

    #[test]
    fn object_like_define_expands() {
        let s = preprocess_sv("`define N 4\nlogic [`N:0] q;\n");
        assert!(s.contains("logic [4:0] q;"), "{s}");
    }

    #[test]
    fn function_like_non_ident_args_do_not_hang() {
        let s = preprocess_sv("`define FOO(1+2)\nlogic x;\n");
        assert!(s.contains("logic x;"), "{s}");
        assert!(!s.contains('`'), "{s}");
    }

    #[test]
    fn object_like_paren_body_not_args() {
        let s = preprocess_sv("`define BAR (4)\nlogic [3:0] q;\nassign q = `BAR;\n");
        assert!(s.contains("assign q = (4);") || s.contains("assign q = (4)"), "{s}");
        assert!(!s.contains('`'), "{s}");
    }
}
