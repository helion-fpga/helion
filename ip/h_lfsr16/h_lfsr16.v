// Helion-ST registered 16-bit LFSR/xorshift PRBS — distinct from 8b h_lfsr (not AXI).
// xorshift16: x ^= x<<7; x ^= x>>9; x ^= x<<8. Seed via ST bytes; enable advances.
module h_lfsr16 (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST seed path
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire        hi_byte,  // which byte of 16b seed
    input  wire        enable,   // advance PRBS
    output reg  [15:0] prbs,
    output wire        st_out_valid
);
    reg [15:0] state_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // xorshift16 combinational step → LUT fabric
    wire [15:0] x1 = state_r ^ (state_r << 7);
    wire [15:0] x2 = x1 ^ (x1 >> 9);
    wire [15:0] x3 = x2 ^ (x2 << 8);
    wire [15:0] next_state = (x3 == 16'h0) ? 16'h1 : x3;

    wire [15:0] seed_hi = {st_data, state_r[7:0]};
    wire [15:0] seed_hi_nz = (seed_hi == 16'h0) ? 16'h1 : seed_hi;

    always @(posedge clk) begin
        if (!resetn) begin
            state_r <= 16'hACE1;
            prbs    <= 16'hACE1;
            out_v   <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                if (hi_byte)
                    state_r <= seed_hi_nz;
                else
                    state_r[7:0] <= st_data;
            end else if (enable) begin
                state_r <= next_state;
                prbs    <= next_state;
                out_v   <= 1'b1;
            end
        end
    end
endmodule
