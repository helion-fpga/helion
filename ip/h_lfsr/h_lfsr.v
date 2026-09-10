// Helion-ST programmable-tap LFSR PRBS generator — enable/load/seed (not AXI).
// Fibonacci LFSR: feedback = ^(state & tap_mask); shift left, insert feedback at LSB.
// st_valid+st_data loads seed when load=1, else loads tap_mask.
module h_lfsr (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST config path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire       load,     // 1=load seed from st_data; 0=load taps
    input  wire       enable,   // advance LFSR
    output reg  [7:0] prbs,
    output wire       st_out_valid
);
    reg [7:0] state_r;
    reg [7:0] taps_r;
    reg       out_v;

    wire feedback = ^(state_r & taps_r);

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    always @(posedge clk) begin
        if (!resetn) begin
            state_r <= 8'hA5;   // non-zero default seed
            taps_r  <= 8'h8E;   // example taps (x^8+x^6+x^5+x^4+1 style mask)
            prbs    <= 8'hA5;
            out_v   <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                if (load) begin
                    // Zero seed forced to 1 so LFSR cannot lock at 0
                    state_r <= (st_data == 8'h00) ? 8'h01 : st_data;
                    prbs    <= (st_data == 8'h00) ? 8'h01 : st_data;
                end else begin
                    taps_r <= (st_data == 8'h00) ? 8'h01 : st_data;
                end
            end else if (enable) begin
                state_r <= {state_r[6:0], feedback};
                prbs    <= {state_r[6:0], feedback};
                out_v   <= 1'b1;
            end
        end
    end
endmodule
