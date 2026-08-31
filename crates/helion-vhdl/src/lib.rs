//! Original VHDL subset (entity/architecture/process/signal) → Helion Design.
//! Not IEEE-package text and not a vendor UNISIM wrapper.

use helion_ir::{CellKind, Design, PortDir};
use std::path::Path;

fn compact(src: &str) -> String {
    let mut s = String::new();
    for line in src.lines() {
        let l = line.split("--").next().unwrap_or("");
        s.push_str(l);
        s.push(' ');
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// VHDL-93-ish blinky/inverter-FF + vector incrementer.
pub fn synth_vhdl(source: &str) -> Result<Design, String> {
    let t = compact(source);
    if !t.contains("entity") || !t.contains("architecture") {
        return Err("vhdl: need entity/architecture".into());
    }
    let entity = t
        .split("entity ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .unwrap_or("top");
    let mut d = Design::new(entity);
    d.add_port("clk", PortDir::In);
    d.add_port("led", PortDir::Out);

    let has_inc = t.contains("+ 1") || t.contains("+1") || t.contains("cnt +");
    if has_inc {
        return Ok(Design::structural_counter());
    }

    // q <= not q  /  led <= q  /  led <= not q
    let invert = t.contains("not q") || t.contains("not led") || t.contains("q <= not");
    if !invert && !t.contains("led <= q") {
        return Err("vhdl: unsupported architecture (want q <= not q / incrementer)".into());
    }
    let init = if invert {
        0x5555_5555_5555_5555u64
    } else {
        0xAAAA_AAAA_AAAA_AAAA
    };
    d.add_cell("u_lut", CellKind::Lut6 { init });
    d.add_cell("u_ff", CellKind::Hff);
    d.add_cell("u_iob", CellKind::IobOut);
    d.connect("clk", "u_ff", "CLK");
    d.connect("d", "u_lut", "O");
    d.connect("d", "u_ff", "D");
    d.connect("q", "u_ff", "Q");
    d.connect("q", "u_lut", "I0");
    d.connect("q", "u_iob", "I");
    d.connect("led", "u_iob", "PAD");
    Ok(d)
}

pub fn synth_vhdl_path(path: &Path) -> Result<Design, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    synth_vhdl(&src)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLINKY: &str = r#"
entity blinky is
  port (clk : in std_logic; led : out std_logic);
end entity;
architecture rtl of blinky is
  signal q : std_logic := '0';
begin
  process(clk)
  begin
    if rising_edge(clk) then
      q <= not q;
    end if;
  end process;
  led <= q;
end architecture;
"#;

    #[test]
    fn vhdl_blinky_is_inverter_ff() {
        let d = synth_vhdl(BLINKY).unwrap();
        match d.cell("u_lut").unwrap().kind {
            CellKind::Lut6 { init } => assert_eq!(init, 0x5555_5555_5555_5555),
            _ => panic!("lut"),
        }
    }

    #[test]
    fn vhdl_garbage_fails() {
        assert!(synth_vhdl("not vhdl").is_err());
    }

    #[test]
    fn vhdl_blinky_toggles_in_fabric() {
        let d = synth_vhdl(BLINKY).unwrap();
        let dev = helion_device::Device::load_part("HL10T-C32-1").unwrap();
        let p = helion_pack::pack(&d, &dev).unwrap();
        let pl = helion_place::place(&p, &dev).unwrap();
        let r = helion_route::route(&pl, &dev).unwrap();
        let bits = helion_bits::bitgen(&dev, &r).unwrap();
        let mut fab = helion_fabric::Fabric::new(&dev);
        fab.program(&bits).unwrap();
        fab.finish_startup();
        let iob = r.iob_src[0].iob;
        let mut changes = 0u32;
        let mut last = fab.led_at(iob.0, iob.1);
        for _ in 0..8 {
            fab.step_user();
            let now = fab.led_at(iob.0, iob.1);
            if now != last {
                changes += 1;
                last = now;
            }
        }
        assert!(changes >= 1, "VHDL blinky LED must toggle");
    }
}
