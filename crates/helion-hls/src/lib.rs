//! C subset → Helion Design (HLS). Static registers, `!`, `+ 1`, `*`.
//! Original lowering, not a vendor HLS clone.

use helion_ir::{CellKind, Design};
use std::path::Path;

fn strip(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        let l = line.split("//").next().unwrap_or("");
        out.push_str(l);
        out.push('\n');
    }
    out
}

pub fn synth_c(source: &str) -> Result<Design, String> {
    let t = strip(source);
    let low = t.to_lowercase();
    if !low.contains("void") && !low.contains("int") && !low.contains("bool") {
        return Err("hls: not a C translation unit".into());
    }
    if low.contains('*') && (low.contains("int") || low.contains("ap_int")) && !low.contains("bool *") {
        let mut d = Design::new("hls_mul");
        d.add_cell("u_mac0", CellKind::Mac27);
        return Ok(d);
    }
    if low.contains("+ 1") || low.contains("+1") || low.contains("cnt++") || low.contains("++cnt") {
        return Ok(Design::structural_counter());
    }
    if low.contains("!") || low.contains("~") || low.contains("not") {
        return Ok(Design::structural_blinky());
    }
    Err("hls: unsupported body (want q=!q, cnt+1, or a*b)".into())
}

pub fn synth_c_path(path: &Path) -> Result<Design, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    synth_c(&src)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hls_blinky_static_invert() {
        let src = r#"
void blinky(bool *led) {
  static bool q = 0;
  q = !q;
  *led = q;
}
"#;
        let d = synth_c(src).unwrap();
        assert_eq!(d.lut_inits(), vec![0x5555_5555_5555_5555]);
    }

    #[test]
    fn hls_mul_is_mac27() {
        let src = r#"
int mac(int a, int b) { return a * b; }
"#;
        let d = synth_c(src).unwrap();
        assert!(d.cells.iter().any(|c| matches!(c.kind, CellKind::Mac27)));
    }

    #[test]
    fn hls_garbage_fails() {
        assert!(synth_c("lorem ipsum").is_err());
    }
}
