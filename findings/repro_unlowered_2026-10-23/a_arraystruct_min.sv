// ROOT CAUSE: `word <- other_unpacked_array[const_word]`
//
// `classify_word_src` (crates/helion-sv/src/lib.rs:13817) accepts only
//   - RExpr::Ident / RExpr::Bit of a *packed* (depth-0) signal, or
//   - RExpr::Bit of the *same* array (RExpr::Bit(mem, w))
// A word select of a DIFFERENT unpacked array parses as RExpr::IndexPart and
// returns None, so `ok=false`, no word is planned, and
// `lower_unpacked_clocked_words` leaves the memory in `mem_names`.
// synth_rtl then hard-errors with `bram_not_lowered` (lib.rs:16601-16630).
//
// The element type is irrelevant: `logic [1:0]` fails exactly like a struct.
module a_arraystruct_min (
    input  logic clk,
    input  logic [1:0] w,
    output logic o
);
    logic [1:0] q   [2];
    logic [1:0] dly [2];

    always_ff @(posedge clk) q[0]   <= dly[0];   // <-- IndexPart RHS
    always_ff @(posedge clk) dly[0] <= w;

    assign o = q[0][0];
endmodule
