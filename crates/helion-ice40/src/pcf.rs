//! PCF (physical constraints) reader for the HX8K CT256 package.
//!
//! Only `set_io` is understood, in the form nextpnr and icebox_vlog read:
//!
//! ```text
//! set_io [-nowarn] [-pullup yes|no] [-io_std SB_LVCMOS] <port> <pin>
//! ```
//!
//! `<port>` names a scalar port (`clk`) or one bit of a vector port
//! (`led[3]`). Anything this backend cannot honour is an error naming the
//! line: an unknown command, a pull-up (not implemented), an IO standard
//! other than LVCMOS, a pin the package does not have, or a port or pin
//! constrained twice.

use crate::device::ct256_pin_map;
use std::collections::HashMap;

/// One `set_io` constraint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetIo {
    /// Port or port bit, e.g. `clk` or `led[3]`.
    pub port: String,
    /// CT256 package pin, e.g. `J3`.
    pub pin: String,
    /// 1-based source line.
    pub line: usize,
}

/// Parse PCF text. `origin` names the file in errors.
pub fn parse_pcf(text: &str, origin: &str) -> Result<Vec<SetIo>, String> {
    let pins = ct256_pin_map();
    let mut out: Vec<SetIo> = Vec::new();
    let mut by_port: HashMap<String, usize> = HashMap::new();
    let mut by_pin: HashMap<String, usize> = HashMap::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let code = raw.split('#').next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let toks: Vec<&str> = code.split_whitespace().collect();
        let err = |m: String| format!("{origin}:{line}: {m}");
        if toks[0] != "set_io" {
            return Err(err(format!("unsupported PCF command '{}' (only set_io is read)", toks[0])));
        }
        let mut args: Vec<&str> = Vec::new();
        let mut k = 1;
        while k < toks.len() {
            match toks[k] {
                "-nowarn" => k += 1,
                "-pullup" => {
                    match toks.get(k + 1).copied() {
                        Some("no") => {}
                        Some("yes") => return Err(err("set_io -pullup yes: pull-ups are not implemented by the iCE40 backend".into())),
                        other => return Err(err(format!("set_io -pullup expects yes or no, got {other:?}"))),
                    }
                    k += 2;
                }
                "-io_std" => {
                    match toks.get(k + 1).copied() {
                        Some("SB_LVCMOS") => {}
                        other => return Err(err(format!("set_io -io_std {other:?}: only SB_LVCMOS is supported"))),
                    }
                    k += 2;
                }
                t if t.starts_with('-') => return Err(err(format!("unsupported set_io option '{t}'"))),
                t => {
                    args.push(t);
                    k += 1;
                }
            }
        }
        let [port, pin] = args[..] else {
            return Err(err(format!("set_io expects <port> <pin>, got {} argument(s)", args.len())));
        };
        if !pins.contains_key(pin) {
            return Err(err(format!("pin '{pin}' does not exist on the HX8K CT256 package")));
        }
        if let Some(&prev) = by_port.get(port) {
            return Err(err(format!("port '{port}' is already constrained on line {prev}")));
        }
        if let Some(&prev) = by_pin.get(pin) {
            return Err(err(format!("pin '{pin}' is already used on line {prev}")));
        }
        by_port.insert(port.to_string(), line);
        by_pin.insert(pin.to_string(), line);
        out.push(SetIo { port: port.to_string(), pin: pin.to_string(), line });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_set_io_with_options_and_comments() {
        let p = parse_pcf("# board\nset_io clk J3\nset_io -nowarn led[0] B5 # LED D2\nset_io -pullup no -io_std SB_LVCMOS rx B1\n", "t.pcf").unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!((p[1].port.as_str(), p[1].pin.as_str(), p[1].line), ("led[0]", "B5", 3));
    }

    #[test]
    fn refuses_what_it_cannot_honour() {
        for (text, needle) in [
            ("set_io clk Z99\n", "does not exist"),
            ("set_io clk J3\nset_io clk B5\n", "already constrained"),
            ("set_io a J3\nset_io b J3\n", "already used"),
            ("set_io -pullup yes clk J3\n", "pull-ups"),
            ("set_io -io_std SB_LVDS_INPUT clk J3\n", "SB_LVCMOS"),
            ("set_frequency clk 12\n", "unsupported PCF command"),
            ("set_io clk\n", "expects <port> <pin>"),
        ] {
            let e = parse_pcf(text, "t.pcf").unwrap_err();
            assert!(e.contains(needle), "{text:?}: {e}");
            assert!(e.starts_with("t.pcf:"), "{e}");
        }
    }
}
