//! `always_ff @(posedge clk or negedge rst_n)` is an ASYNCHRONOUS reset.
//!
//! The parser used to keep only the first edge of the sensitivity list, so
//! the reset term was folded into the D cone and the emitted Hff carried no
//! reset pin at all: the reset silently became synchronous (it only took
//! effect at a clock edge, and a held clock meant a flop that never reset).
//! These tests pin the reset onto the flop's async `SR` pin instead.

use helion_ir::CellKind;
use helion_sv::synth_sv;

fn ff_with_q<'a>(d: &'a helion_ir::Design, q: &str) -> &'a helion_ir::Cell {
    d.cells
        .iter()
        .find(|c| matches!(c.kind, CellKind::Hff) && d.net_on(&c.name, "Q") == Some(q))
        .unwrap_or_else(|| panic!("no Hff drives Q net {q}; cells={:?}", d.cells))
}

#[test]
fn async_reset_reaches_the_flop_sr_pin() {
    let d = synth_sv(
        r#"module t(input logic clk, input logic rst_n, input logic d, output logic q);
             always_ff @(posedge clk or negedge rst_n)
               if (!rst_n) q <= 1'b0;
               else        q <= d;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");

    let ff = ff_with_q(&d, "q");
    let sr = d
        .net_on(&ff.name, "SR")
        .expect("async-reset flop must have an SR pin");
    assert_eq!(
        sr, "rst_n",
        "SR must be driven by the sensitivity-list reset term"
    );
}

#[test]
fn sync_reset_still_has_no_sr_pin() {
    // A reset inside the body with no reset edge in the sensitivity list is a
    // synchronous reset: it belongs in D, and must NOT acquire an SR pin.
    let d = synth_sv(
        r#"module t(input logic clk, input logic rst_n, input logic d, output logic q);
             always_ff @(posedge clk)
               if (!rst_n) q <= 1'b0;
               else        q <= d;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");

    let ff = ff_with_q(&d, "q");
    assert!(
        d.net_on(&ff.name, "SR").is_none(),
        "a synchronous reset must not be turned into an async SR pin"
    );
    // The reset must still be present in the D cone: reachable from rst_n by
    // walking LUT outputs forward until the flop's D net is found.
    let rst_net = d.nets.iter().find(|n| n.name == "rst_n").expect("rst_n net");
    let mut frontier: Vec<String> = rst_net.endpoints.iter().map(|e| e.cell.clone()).collect();
    let mut seen: std::collections::HashSet<String> = frontier.iter().cloned().collect();
    let dnet = d.net_on(&ff.name, "D").expect("D net");
    let mut reaches = false;
    while let Some(cell) = frontier.pop() {
        let Some(o) = d.net_on(&cell, "O") else { continue };
        if o == dnet {
            reaches = true;
            break;
        }
        for n in d.nets.iter().filter(|n| n.name == o) {
            for e in &n.endpoints {
                if seen.insert(e.cell.clone()) {
                    frontier.push(e.cell.clone());
                }
            }
        }
    }
    assert!(reaches, "sync reset must be computed in the D cone");
}

#[test]
fn reset_polarity_is_recorded() {
    let d = synth_sv(
        r#"module t(input logic clk, input logic rst, input logic d, output logic q);
             always_ff @(posedge clk or posedge rst)
               if (rst) q <= 1'b0;
               else     q <= d;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");
    let ff = ff_with_q(&d, "q");
    assert_eq!(
        d.net_on(&ff.name, "SR"),
        Some("rst"),
        "active-high async reset term must also reach SR"
    );
    assert_ne!(
        d.attrs.get("ASYNC_RESET_ACTIVE_LOW"),
        Some("1"),
        "an active-high reset must not be flagged active-low"
    );
}

#[test]
fn active_low_reset_is_flagged() {
    let d = synth_sv(
        r#"module t(input logic clk, input logic rst_n, input logic d, output logic q);
             always_ff @(posedge clk or negedge rst_n)
               if (!rst_n) q <= 1'b0;
               else        q <= d;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");
    assert_eq!(d.attrs.get("ASYNC_RESET_ACTIVE_LOW"), Some("1"));
}

#[test]
fn multi_bit_async_reset_covers_every_bit() {
    let d = synth_sv(
        r#"module t(input logic clk, input logic rst_n, input logic [3:0] d, output logic [3:0] q);
             always_ff @(posedge clk or negedge rst_n)
               if (!rst_n) q <= 4'b0;
               else        q <= d;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");

    for bit in 0..4 {
        let q = format!("q_{bit}");
        let ff = ff_with_q(&d, &q);
        assert_eq!(
            d.net_on(&ff.name, "SR"),
            Some("rst_n"),
            "{q}'s flop must carry the async reset"
        );
    }
}
