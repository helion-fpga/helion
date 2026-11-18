//! CMT legal ranges must match the part's speed grade (HELION_GAPS.md G58).
//!
//! `pack` has no part in scope, so it must apply the *slowest* 7-Series
//! grade's ceilings: MMCM VCO 600..=1200 MHz, PLL VCO 800..=1600 MHz. It used
//! to apply the -3 ceilings (1600 / 2133) to every part, so a design asking
//! for a VCO between the -1 and -3 maxima packed cleanly and then had no legal
//! configuration on a -1 part like the ZedBoard's xc7z020clg484-1.

use helion_backend::Backend;
use helion_ir::{intern, CellKind, Design};
use helion_xilinx7::backend::Xilinx7Backend;

fn cmt_design(kind: &str, mult: &str, out_div: &str) -> Design {
    let mut d = Design::new("cmt");
    d.add_cell(
        "u_cmt",
        CellKind::BlackBox {
            module: intern(kind),
        },
    );
    d.set_cell_attr("u_cmt", "CLKIN1_PERIOD", "10.0").unwrap(); // 100 MHz
    d.set_cell_attr("u_cmt", "DIVCLK_DIVIDE", "1").unwrap();
    let (mult_attr, div_attr) = if kind == "PLLE2_ADV" {
        ("CLKFBOUT_MULT", "CLKOUT0_DIVIDE")
    } else {
        ("CLKFBOUT_MULT_F", "CLKOUT0_DIVIDE_F")
    };
    d.set_cell_attr("u_cmt", mult_attr, mult).unwrap();
    d.set_cell_attr("u_cmt", div_attr, out_div).unwrap();
    d.connect("clk_in", "u_cmt", "CLKIN1");
    d.connect("fb", "u_cmt", "CLKFBOUT");
    d.connect("fb", "u_cmt", "CLKFBIN");
    d.connect("out", "u_cmt", "CLKOUT0");
    d
}

#[test]
fn mmcm_vco_ceiling_is_the_slowest_grade() {
    let backend = Xilinx7Backend::open("xc7a35t").unwrap();

    // 1400 MHz VCO: legal on a -2 (<=1440) and -3 (<=1600) part, illegal on -1.
    // pack has no part, so it must refuse.
    let err = backend
        .pack(&cmt_design("MMCME2_ADV", "14.0", "7.0"))
        .expect_err("1400 MHz MMCM VCO must be refused: it is illegal on a -1 part");
    assert!(
        err.contains("VCO frequency") && err.contains("1200"),
        "refusal should name the -1 ceiling 1200 MHz, got: {err}"
    );

    // Exactly at the -1 ceiling is legal.
    backend
        .pack(&cmt_design("MMCME2_ADV", "12.0", "6.0"))
        .expect("1200 MHz MMCM VCO is legal on every grade");

    // 1000 MHz, the common case, stays legal.
    backend
        .pack(&cmt_design("MMCME2_ADV", "10.0", "5.0"))
        .expect("1000 MHz MMCM VCO is legal");
}

#[test]
fn pll_vco_ceiling_is_the_slowest_grade() {
    let backend = Xilinx7Backend::open("xc7a35t").unwrap();

    // 1800 MHz VCO: legal on -2 (<=1866) and -3 (<=2133), illegal on -1.
    let err = backend
        .pack(&cmt_design("PLLE2_ADV", "18", "9"))
        .expect_err("1800 MHz PLL VCO must be refused: it is illegal on a -1 part");
    assert!(
        err.contains("VCO frequency") && err.contains("1600"),
        "refusal should name the -1 ceiling 1600 MHz, got: {err}"
    );

    // Exactly at the -1 ceiling is legal.
    backend
        .pack(&cmt_design("PLLE2_ADV", "16", "8"))
        .expect("1600 MHz PLL VCO is legal on every grade");
}
