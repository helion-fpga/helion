// ROOT CAUSE: async-reset mux around a word write.
// Ibex: `if (!rst_ni) pending_dside_accesses_q[i] <= '0;
//        else       pending_dside_accesses_q[i] <= pending_dside_accesses_d[i];`
// lowers to  Mux(Not(rst_ni), 0, Bit(d, i)).
//
// `peel_word_enable` (lib.rs:13794) only accepts a mux whose FALSE branch is
// the array ITSELF (`q <= en ? next : q`). A reset mux has a CONSTANT in the
// true branch and the next word in the false branch, so it returns None and
// the whole memory is refused.
module b_reset_mux_word (
    input  logic       clk,
    input  logic       rst_ni,
    input  logic [1:0] d,
    output logic       o
);
    logic [1:0] q [2];
    always_ff @(posedge clk) begin
      if (!rst_ni) q[0] <= '0;
      else         q[0] <= d;
    end
    assign o = q[0][0];
endmodule
