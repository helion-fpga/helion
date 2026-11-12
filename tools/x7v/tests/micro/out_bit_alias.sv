// Per-bit output aliases of register bits and constants (zedboard_blinky shape).
module top(input logic clk, input logic [1:0] a, output logic [4:0] led);
    logic [5:0] cnt = 6'd0;
    always_ff @(posedge clk) cnt <= cnt + {4'd0, a};
    assign led[0] = cnt[5];
    assign led[1] = cnt[4];
    assign led[2] = cnt[3:3];
    assign led[3] = 1'b1;
    assign led[4] = cnt[0];
endmodule
