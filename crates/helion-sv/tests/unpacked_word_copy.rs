//! Word-to-word copies between *different* unpacked arrays.
//!
//! `q[0] <= dly[0]` — a constant word select of another unpacked array —
//! parses as `RExpr::Bit(other, word)`. `classify_word_src` used to require
//! `other == mem`, so this shape returned `None`, no word was planned, and
//! `lower_unpacked_clocked_words` left the array in `mem_names`. `synth_rtl`
//! then hard-errored with `bram_not_lowered`:
//!
//! ```text
//! synth_sv: 1 memory/memories were not lowered, ... `q`: its access pattern
//! could not be lowered (depth=2, width=2)
//! ```
//!
//! That is the shape Ibex uses for its outstanding-data-side-access tracker
//! (`pending_access_t pending_dside_accesses_q[N]` fed from
//! `pending_dside_accesses_d[N]`), which made `examples/ysyx_ibex.sv` fail to
//! compile. Note the element type is irrelevant: a plain `logic [1:0]` array
//! hit the same refusal, so this was never really an "array of structs" bug.

use helion_ir::CellKind;

fn count_ffs(d: &helion_ir::Design) -> usize {
    d.cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count()
}

/// The smallest shape that used to fail: two distinct unpacked arrays, one
/// word copied between them. Must lower to real flops, not error out.
#[test]
fn cross_array_word_copy_lowers_to_flops() {
    let src = r#"
module cross (input logic clk, input logic [1:0] w, output logic o);
    logic [1:0] q   [2];
    logic [1:0] dly [2];

    always_ff @(posedge clk) q[0]   <= dly[0];
    always_ff @(posedge clk) dly[0] <= w;

    assign o = q[0][0];
endmodule
"#;
    let d = helion_sv::synth_sv(src, "cross.sv").expect("cross-array word copy must lower");
    // `q` is 2 words x 2 bits; the copy drives all 4 flop D pins.
    let ffs = count_ffs(&d);
    assert!(
        ffs >= 4,
        "expected >=4 Hff for the two 2x2 arrays, got {ffs} (cells={})",
        d.cells.len()
    );
    // The refusal marks itself on the design; it must not be present.
    assert_ne!(d.attrs.get("BRAM_NOT_LOWERED"), Some("q"));
    // Every bit of q[0] must have a driver: q_w0_0 and q_w0_1.
    for net in ["q_w0_0", "q_w0_1"] {
        let n = d
            .nets
            .iter()
            .find(|n| n.name == net)
            .unwrap_or_else(|| panic!("missing net {net}"));
        assert!(
            !n.endpoints.is_empty(),
            "net {net} has no endpoints; the copy did not lower"
        );
    }
}

/// The Ibex shape: an array-of-struct written from a sibling array inside a
/// generate-for. Element type is packed `{valid,is_read}` (2 bits).
#[test]
fn struct_array_word_copy_in_generate_lowers() {
    let src = r#"
module struct_arr (input logic clk, input logic d, output logic o);
    localparam int unsigned N = 2;
    typedef struct packed { logic valid; logic is_read; } pending_access_t;

    pending_access_t q[N];
    pending_access_t dly[N];

    for (genvar i = 0; i < N; i++) begin : g_track
      always_ff @(posedge clk) q[i] <= dly[i];
    end

    always_comb dly[0] = {d, 1'b0};
    always_comb dly[1] = '0;

    assign o = q[0].valid & q[1].is_read;
endmodule
"#;
    let d = helion_sv::synth_sv(src, "struct_arr.sv")
        .expect("struct-array word copy must lower");
    assert!(
        count_ffs(&d) >= 4,
        "expected >=4 Hff, cells={}",
        d.cells.len()
    );
    assert_ne!(d.attrs.get("BRAM_NOT_LOWERED"), Some("q"));
}

/// A same-array shift must keep working (no regression from widening the
/// source to another array).
#[test]
fn same_array_word_shift_still_lowers() {
    let src = r#"
module shift (input logic clk, output logic o);
    logic [1:0] q [2];
    always_ff @(posedge clk) q[0] <= q[1];
    assign o = q[0][0];
endmodule
"#;
    let d = helion_sv::synth_sv(src, "shift.sv").expect("same-array shift must still lower");
    assert!(count_ffs(&d) >= 2, "cells={}", d.cells.len());
}

/// A widening copy is NOT this lower: the source is narrower than the target,
/// so it must keep the explicit refusal rather than silently invent a mux.
#[test]
fn narrower_array_source_is_still_refused() {
    let src = r#"
module narrow (input logic clk, output logic o);
    logic [3:0] q [2];
    logic [1:0] n [2];
    always_ff @(posedge clk) q[0] <= n[0];
    assign o = q[0][0];
endmodule
"#;
    let err = helion_sv::synth_sv(src, "narrow.sv")
        .expect_err("a differing element width must stay a named refusal");
    assert!(
        err.contains("not lowered"),
        "expected the bram_not_lowered refusal, got: {err}"
    );
}

/// A synchronous-clear mux around a word write:
/// `if (rst) q[0] <= '0; else q[0] <= d;` parses as `Mux(rst, Const(0), d)`.
/// The constant is in the TRUE arm (the mirror of a hold mux), which used to
/// return `None` from `peel_word_enable` and refuse the whole array. Ibex
/// writes its RVFI/MIP trackers this way.
#[test]
fn reset_mux_word_write_lowers() {
    let src = r#"
module rst_mux (
    input  logic       clk,
    input  logic       rst,
    input  logic [1:0] d,
    output logic       o
);
    logic [1:0] q [2];
    always_ff @(posedge clk) begin
      if (rst) q[0] <= '0;
      else     q[0] <= d;
    end
    assign o = q[0][0];
endmodule
"#;
    let d = helion_sv::synth_sv(src, "rst_mux.sv").expect("reset-muxed word write must lower");
    assert!(
        count_ffs(&d) >= 2,
        "expected >=2 Hff for the 2-bit word, cells={}",
        d.cells.len()
    );
    assert_ne!(d.attrs.get("BRAM_NOT_LOWERED"), Some("q"));
    // The clear must be real logic: at least one LUT carries `rst` into D,
    // so the flop is not left with an unconditional next-state.
    assert!(
        count_ffs(&d) >= 2 && d.cells.iter().any(|c| matches!(c.kind, CellKind::Lut6 { .. })),
        "expected a LUT implementing the clear term, cells={}",
        d.cells.len()
    );
}
