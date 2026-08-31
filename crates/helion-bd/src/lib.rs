//! Headless block design: validate + emit SV (no GUI canvas).

use helion_ipxact::IpCore;

#[derive(Clone, Debug)]
pub struct BlockDesign {
    pub name: String,
    pub cores: Vec<IpCore>,
}

#[derive(Clone, Debug)]
pub struct ValidateReport {
    pub ok: bool,
    pub errors: Vec<String>,
}

pub fn validate(bd: &BlockDesign) -> ValidateReport {
    let mut errors = Vec::new();
    if bd.name.is_empty() {
        errors.push("empty name".into());
    }
    for c in &bd.cores {
        if c.bus != "Helion-MM" && c.bus != "Helion-ST" {
            errors.push(format!("{}: bus {} not Helion-MM/ST", c.name, c.bus));
        }
    }
    ValidateReport {
        ok: errors.is_empty(),
        errors,
    }
}

pub fn emit_sv(bd: &BlockDesign) -> Result<String, String> {
    let v = validate(bd);
    if !v.ok {
        return Err(v.errors.join("; "));
    }
    let mut s = format!("module {} (\n  input logic clk,\n  input logic resetn\n);\n", bd.name);
    for c in &bd.cores {
        s.push_str(&format!(
            "  {} #() u_{} (.clk(clk), .resetn(resetn));\n",
            c.name, c.name
        ));
    }
    s.push_str("endmodule\n");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ipxact::{pack_gpio, pack_uart};

    #[test]
    fn validate_and_emit() {
        let bd = BlockDesign {
            name: "sys".into(),
            cores: vec![pack_uart(), pack_gpio()],
        };
        assert!(validate(&bd).ok);
        let sv = emit_sv(&bd).unwrap();
        assert!(sv.contains("module sys"));
        assert!(sv.contains("h_uart"));
        assert!(sv.contains("h_gpio"));
    }
}
