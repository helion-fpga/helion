//! HELIONLIB — original primitives (not UNISIM).

pub const LUT6: &str = r#"
module LUT6 #(parameter [63:0] INIT = 64'h0) (
  input I0, I1, I2, I3, I4, I5, output O
);
  assign O = INIT[{I5,I4,I3,I2,I1,I0}];
endmodule
"#;

pub const HFF: &str = r#"
module HFF (input D, CLK, CE, SR, output reg Q);
  always @(posedge CLK) if (SR) Q <= 1'b0; else if (CE) Q <= D;
endmodule
"#;

pub const MAC27: &str = r#"
module MAC27 (input clk, input [26:0] a, b, input [47:0] c, output reg [47:0] p);
  always @(posedge clk) p <= (a * b) + c;
endmodule
"#;

pub fn cell_names() -> &'static [&'static str] {
    &["LUT6", "HFF", "HCARRY", "MAC27", "BRAM18", "HIDDR", "HODDR", "HBUFQ", "HSTARTUP"]
}

/// Map `p <= a * b + c` onto HELIONLIB MAC27 (site primitive).
pub fn map_muladd_to_mac27() -> helion_ir::CellKind {
    helion_ir::CellKind::Mac27
}

pub fn muladd_design() -> helion_ir::Design {
    let mut d = helion_ir::Design::new("muladd");
    d.add_cell("u_mac", map_muladd_to_mac27());
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helionlib_smoke_no_unisim() {
        let names = cell_names();
        assert!(names.contains(&"LUT6"));
        assert!(names.contains(&"HFF"));
        assert!(names.contains(&"MAC27"));
        assert!(!names.iter().any(|n| n.contains("FDRE") || *n == "LUT6_2"));
        assert!(LUT6.contains("module LUT6"));
        assert!(matches!(
            map_muladd_to_mac27(),
            helion_ir::CellKind::Mac27
        ));
        let hb1 = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../ip/h_rv32_hb1/h_rv32_hb1.v");
        let src = std::fs::read_to_string(&hb1).expect("HB1 wrap");
        assert!(src.contains("picorv32"));
        assert!(src.contains("mm_addr"));
        let lic = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/picorv32/LICENSE");
        let l = std::fs::read_to_string(lic).unwrap();
        assert!(l.contains("ISC"));
        assert!(!l.to_uppercase().contains("GENERAL PUBLIC LICENSE"));
        let pico = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/picorv32/picorv32.v");
        let pv = std::fs::read_to_string(&pico).unwrap();
        assert!(
            pv.lines().count() > 2000,
            "expected vendored YosysHQ PicoRV32, got {} lines",
            pv.lines().count()
        );
        assert!(pv.contains("Copyright (C) 2015"));
    }

    #[test]
    fn muladd_packs_on_dsp_part() {
        use helion_device::Device;
        use helion_pack::pack;
        use helion_place::place;
        let dev = Device::load_part("HL10T-DSP1").unwrap();
        let d = muladd_design();
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.macs.len(), 1);
        let pl = place(&p, &dev).unwrap();
        assert_eq!(pl.mac_sites[0].kind, helion_device::SiteKind::Dsp);
        assert!(Device::load_part("HL10T-C32-1").unwrap().dsp_sites().next().is_none());
    }
}
