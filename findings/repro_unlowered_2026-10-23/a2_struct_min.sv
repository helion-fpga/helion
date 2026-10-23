// ROOT CAUSE (same as a_arraystruct_min), in the shape Ibex actually uses:
// array-of-struct with a generate-for, `q[i] <= dly[i]`.
// Element type `pending_access_t` is packed{valid,is_read} = 2 bits; depth 2.
// That is far under MAX_DEPTH=16 / MAX_WIDTH=32 / MAX_BITS=128 -- the caps are
// NOT the reason. The RHS word select is.
module a2_struct_min (
    input  logic clk,
    input  logic d,
    output logic o
);
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
