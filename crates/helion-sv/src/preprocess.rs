//! Verilog/SV preprocessor: `` `define `` / `` `ifdef `` / macro expansion.
//! Unknown `` `FOO `` and `` `FOO(...) `` are dropped so Helion can ingest
//! large cores (Ibex, PicoRV32) that ship with assertion macros.

use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
struct Macro {
    args: Vec<String>,
    body: String,
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
