//! Preprocessor: `` `define ``, `` `ifdef `` and friends, `` `include ``.
//!
//! The output keeps the line structure of the input (a directive or a
//! skipped region leaves empty lines), and an included file is bracketed
//! by `` `line `` markers, so every message can name the real file and line.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Lines are numbered `file * FILE_STRIDE + line` inside the front end.
pub const FILE_STRIDE: u32 = 10_000_000;

#[derive(Clone)]
struct Macro {
    args: Option<Vec<(String, Option<String>)>>,
    body: String,
}

pub struct Pre {
    defines: HashMap<String, Macro>,
    dirs: Vec<PathBuf>,
    /// File names by index (index 0 is the top file).
    pub files: Vec<String>,
    depth: u32,
}

fn is_id_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}
fn is_id(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

impl Pre {
    pub fn new(dirs: Vec<PathBuf>, defines: &[(String, String)]) -> Self {
        let mut d = HashMap::new();
        for (k, v) in defines {
            d.insert(k.clone(), Macro { args: None, body: v.clone() });
        }
        Pre { defines: d, dirs, files: Vec::new(), depth: 0 }
    }

    fn find_include(&self, name: &str, from: &Path) -> Option<PathBuf> {
        let mut cands = vec![from.join(name)];
        for d in &self.dirs {
            cands.push(d.join(name));
            cands.push(d.join("include").join(name));
        }
        cands.into_iter().find(|p| p.is_file())
    }

    /// Expand macros in one line of text (no directives).
    fn expand(&self, text: &str, depth: u32, file: &str, line: u32) -> Result<String, String> {
        if !text.contains('`') {
            return Ok(text.to_string());
        }
        if depth > 64 {
            return Err(format!("{file}:{line}: macro expansion is recursive"));
        }
        let b = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'"' {
                // Strings are copied verbatim.
                let st = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i = (i + 1).min(b.len());
                out.push_str(&text[st..i]);
                continue;
            }
            if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
                out.push_str(&text[i..]);
                break;
            }
            if b[i] != b'`' {
                let st = i;
                while i < b.len() && b[i] != b'`' && b[i] != b'"' && !(b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/') {
                    i += 1;
                }
                out.push_str(&text[st..i]);
                continue;
            }
            // `` `` `` joins tokens, `` `" `` is a quote inside a macro body.
            if i + 1 < b.len() && b[i + 1] == b'`' {
                i += 2;
                continue;
            }
            if i + 1 < b.len() && b[i + 1] == b'"' {
                out.push('"');
                i += 2;
                continue;
            }
            let st = i + 1;
            let mut j = st;
            while j < b.len() && is_id(b[j]) {
                j += 1;
            }
            let name = &text[st..j];
            let Some(m) = self.defines.get(name) else {
                if matches!(name, "timescale" | "default_nettype" | "resetall" | "celldefine" | "endcelldefine" | "line" | "pragma" | "begin_keywords" | "end_keywords" | "nounconnected_drive" | "unconnected_drive") {
                    // Compiler directives without effect on the logic: drop the rest of the line.
                    break;
                }
                return Err(format!("{file}:{line}: macro `{name} is not defined"));
            };
            i = j;
            let body = match &m.args {
                None => m.body.clone(),
                Some(formals) => {
                    let mut k = i;
                    while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
                        k += 1;
                    }
                    if k >= b.len() || b[k] != b'(' {
                        return Err(format!("{file}:{line}: macro `{name} needs arguments"));
                    }
                    k += 1;
                    let mut actuals: Vec<String> = Vec::new();
                    let mut cur = String::new();
                    let mut depth_p = 0i32;
                    let mut closed = false;
                    while k < b.len() {
                        let c = b[k];
                        if c == b'"' {
                            let s0 = k;
                            k += 1;
                            while k < b.len() && b[k] != b'"' {
                                if b[k] == b'\\' {
                                    k += 1;
                                }
                                k += 1;
                            }
                            k = (k + 1).min(b.len());
                            cur.push_str(&text[s0..k]);
                            continue;
                        }
                        match c {
                            b'(' | b'[' | b'{' => depth_p += 1,
                            b')' | b']' | b'}' if depth_p > 0 => depth_p -= 1,
                            b')' => {
                                closed = true;
                                k += 1;
                                break;
                            }
                            b',' if depth_p == 0 => {
                                actuals.push(cur.trim().to_string());
                                cur.clear();
                                k += 1;
                                continue;
                            }
                            _ => {}
                        }
                        cur.push(c as char);
                        k += 1;
                    }
                    if !closed {
                        return Err(format!("{file}:{line}: unterminated arguments of macro `{name}"));
                    }
                    if !(formals.is_empty() && cur.trim().is_empty()) {
                        actuals.push(cur.trim().to_string());
                    }
                    i = k;
                    let mut map: HashMap<&str, String> = HashMap::new();
                    for (fi, (fname, dflt)) in formals.iter().enumerate() {
                        let v = match actuals.get(fi) {
                            Some(a) if !a.is_empty() => a.clone(),
                            _ => match dflt {
                                Some(d) => d.clone(),
                                None => String::new(),
                            },
                        };
                        map.insert(fname.as_str(), v);
                    }
                    // Substitute formal names in the body.
                    let bb = m.body.as_bytes();
                    let mut sub = String::with_capacity(m.body.len());
                    let mut p = 0;
                    while p < bb.len() {
                        if bb[p] == b'"' && !(p > 0 && bb[p - 1] == b'`') {
                            let s0 = p;
                            p += 1;
                            while p < bb.len() && bb[p] != b'"' {
                                if bb[p] == b'\\' {
                                    p += 1;
                                }
                                p += 1;
                            }
                            p = (p + 1).min(bb.len());
                            sub.push_str(&m.body[s0..p]);
                        } else if is_id_start(bb[p]) && !(p > 0 && is_id(bb[p - 1])) {
                            let s0 = p;
                            while p < bb.len() && is_id(bb[p]) {
                                p += 1;
                            }
                            let id = &m.body[s0..p];
                            match map.get(id) {
                                Some(v) => sub.push_str(v),
                                None => sub.push_str(id),
                            }
                        } else {
                            sub.push(bb[p] as char);
                            p += 1;
                        }
                    }
                    sub
                }
            };
            out.push_str(&self.expand(&body, depth + 1, file, line)?);
        }
        Ok(out)
    }

    /// Expand `first`, taking following lines while a macro call's
    /// arguments are still open. Returns how many extra lines were used.
    fn expand_joined(&self, first: &str, lines: &[&str], li: &mut usize, name: &str, lineno: u32, out: &mut String) -> Result<u32, String> {
        let mut text = first.to_string();
        let mut extra = 0;
        loop {
            match self.expand(&text, 0, name, lineno) {
                Ok(x) => {
                    out.push_str(&x);
                    return Ok(extra);
                }
                Err(e) if e.contains("unterminated arguments") && *li < lines.len() && extra < 4096 => {
                    // Drop a line comment before joining.
                    if let Some(c) = text.find("//") {
                        text.truncate(c);
                    }
                    text.push(' ');
                    text.push_str(lines[*li].trim_end_matches('\r'));
                    *li += 1;
                    extra += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Preprocess `text` of file `name`; appends to `out`.
    pub fn run(&mut self, text: &str, name: &str, out: &mut String) -> Result<(), String> {
        let file_ix = self.files.len();
        self.files.push(name.to_string());
        if self.depth > 0 {
            out.push_str(&format!("`line 1 {file_ix}\n"));
        }
        self.depth += 1;
        if self.depth > 64 {
            return Err(format!("{name}: includes nest too deeply"));
        }
        let dir = Path::new(name).parent().unwrap_or(Path::new(".")).to_path_buf();
        // (parent emitting, this branch taken, some branch already taken)
        let mut stack: Vec<(bool, bool, bool)> = Vec::new();
        let emitting = |st: &Vec<(bool, bool, bool)>| st.last().map(|(p, t, _)| *p && *t).unwrap_or(true);
        let lines: Vec<&str> = text.split('\n').collect();
        let mut li = 0usize;
        let mut in_block_comment = false;
        while li < lines.len() {
            let lineno = li as u32 + 1;
            let raw = lines[li].trim_end_matches('\r');
            li += 1;
            // Block comments pass through untouched (directives inside them are text).
            if in_block_comment {
                if raw.contains("*/") {
                    in_block_comment = false;
                }
                out.push_str(raw);
                out.push('\n');
                continue;
            }
            let trimmed = raw.trim_start();
            if !trimmed.starts_with('`') {
                if emitting(&stack) {
                    let extra = self.expand_joined(raw, &lines, &mut li, name, lineno, out)?;
                    if let Some(p) = raw.rfind("/*") {
                        if !raw[p..].contains("*/") && !raw[..p].contains("//") {
                            in_block_comment = true;
                        }
                    }
                    for _ in 0..extra {
                        out.push('\n');
                    }
                }
                out.push('\n');
                continue;
            }
            let rest = &trimmed[1..];
            let dname: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            let arg = rest[dname.len()..].trim();
            // The argument without a trailing comment.
            let arg_nc = arg.split("//").next().unwrap_or("").trim();
            match dname.as_str() {
                "ifdef" | "ifndef" => {
                    let id = arg_nc.split_whitespace().next().unwrap_or("");
                    let on = self.defines.contains_key(id) == (dname == "ifdef");
                    stack.push((emitting(&stack), on, on));
                    out.push('\n');
                }
                "elsif" => {
                    let id = arg_nc.split_whitespace().next().unwrap_or("");
                    let Some(top) = stack.last_mut() else { return Err(format!("{name}:{lineno}: `elsif without `ifdef")) };
                    let on = !top.2 && self.defines.contains_key(id);
                    top.1 = on;
                    top.2 |= on;
                    out.push('\n');
                }
                "else" => {
                    let Some(top) = stack.last_mut() else { return Err(format!("{name}:{lineno}: `else without `ifdef")) };
                    top.1 = !top.2;
                    top.2 = true;
                    out.push('\n');
                }
                "endif" => {
                    if stack.pop().is_none() {
                        return Err(format!("{name}:{lineno}: `endif without `ifdef"));
                    }
                    out.push('\n');
                }
                "define" => {
                    // Collect continuation lines.
                    let mut body_text = arg.to_string();
                    let mut extra = 0;
                    while body_text.trim_end().ends_with('\\') && li < lines.len() {
                        let t = body_text.trim_end();
                        body_text = format!("{} {}", &t[..t.len() - 1], lines[li].trim_end_matches('\r'));
                        li += 1;
                        extra += 1;
                    }
                    if emitting(&stack) {
                        let bt = body_text.as_bytes();
                        let mut p = 0;
                        while p < bt.len() && is_id(bt[p]) {
                            p += 1;
                        }
                        let mname = body_text[..p].to_string();
                        if mname.is_empty() {
                            return Err(format!("{name}:{lineno}: `define without a name"));
                        }
                        let mut args = None;
                        if p < bt.len() && bt[p] == b'(' {
                            let close = body_text[p..].find(')').ok_or_else(|| format!("{name}:{lineno}: bad macro arguments"))? + p;
                            let mut list = Vec::new();
                            for a in body_text[p + 1..close].split(',') {
                                let a = a.trim();
                                if a.is_empty() {
                                    continue;
                                }
                                match a.split_once('=') {
                                    Some((n, d)) => list.push((n.trim().to_string(), Some(d.trim().to_string()))),
                                    None => list.push((a.to_string(), None)),
                                }
                            }
                            args = Some(list);
                            p = close + 1;
                        }
                        let mut body = body_text[p..].to_string();
                        if let Some(c) = body.find("//") {
                            body.truncate(c);
                        }
                        self.defines.insert(mname, Macro { args, body: body.trim().to_string() });
                    }
                    for _ in 0..=extra {
                        out.push('\n');
                    }
                }
                "undef" => {
                    if emitting(&stack) {
                        self.defines.remove(arg_nc.split_whitespace().next().unwrap_or(""));
                    }
                    out.push('\n');
                }
                "include" => {
                    if emitting(&stack) {
                        let fname = arg_nc.trim_matches(|c| c == '"' || c == '<' || c == '>');
                        let Some(path) = self.find_include(fname, &dir) else {
                            return Err(format!("{name}:{lineno}: cannot find include file \"{fname}\""));
                        };
                        let inc = std::fs::read_to_string(&path).map_err(|e| format!("{name}:{lineno}: {}: {e}", path.display()))?;
                        self.run(&inc, &path.display().to_string(), out)?;
                        // Back in this file, at the next line.
                        out.push_str(&format!("`line {} {file_ix}\n", lineno + 1));
                    } else {
                        out.push('\n');
                    }
                }
                _ => {
                    // A macro call at the start of a line, or a directive without effect.
                    if emitting(&stack) {
                        let extra = self.expand_joined(raw, &lines, &mut li, name, lineno, out)?;
                        for _ in 0..extra {
                            out.push('\n');
                        }
                    }
                    out.push('\n');
                }
            }
        }
        if !stack.is_empty() {
            return Err(format!("{name}: `ifdef without `endif"));
        }
        self.depth -= 1;
        Ok(())
    }
}

/// Rewrite the internal `line N` of a message as `file:line`.
pub fn decode_lines(msg: &str, files: &[String]) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(p) = rest.find("line ") {
        let after = &rest[p + 5..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || (p > 0 && rest.as_bytes()[p - 1].is_ascii_alphanumeric()) {
            out.push_str(&rest[..p + 5]);
            rest = after;
            continue;
        }
        let n: u64 = digits.parse().unwrap_or(0);
        let (f, l) = ((n / FILE_STRIDE as u64) as usize, n % FILE_STRIDE as u64);
        out.push_str(&rest[..p]);
        match files.get(f) {
            Some(name) if files.len() > 1 => out.push_str(&format!("{name}:{l}")),
            _ => out.push_str(&format!("line {l}")),
        }
        rest = &after[digits.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditionals_and_macros_keep_lines() {
        let src = "`define W 8\n`define MAX(a, b) ((a) > (b) ? (a) : (b))\n`ifdef FOO\nwire x;\n`else\nwire [`W-1:0] y = `MAX(p, q + 1);\n`endif\n";
        let mut p = Pre::new(vec![], &[]);
        let mut out = String::new();
        p.run(src, "t.sv", &mut out).unwrap();
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(lines[5], "wire [8-1:0] y = ((p) > (q + 1) ? (p) : (q + 1));");
        assert!(lines[3].is_empty() && lines[0].is_empty());
        let mut p = Pre::new(vec![], &[("FOO".into(), String::new())]);
        let mut out = String::new();
        p.run(src, "t.sv", &mut out).unwrap();
        assert_eq!(out.split('\n').nth(3), Some("wire x;"));
    }

    #[test]
    fn messages_name_the_file() {
        let files = vec!["top.sv".to_string(), "inc.svh".to_string()];
        assert_eq!(decode_lines("line 10000012: bad thing at line 3", &files), "inc.svh:12: bad thing at top.sv:3");
        assert_eq!(decode_lines("line 7: x", &["a.sv".to_string()]), "line 7: x");
    }
}
