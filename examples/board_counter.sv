// An 8-bit counter with synchronous reset and enable, for bringing up a board
// (tools/x7v/tests/boards.sh builds and verifies it for every board).
module board_counter (
    input  logic       clk,
    input  logic       rst,
    input  logic       en,
    output logic [7:0] led
);
    logic [7:0] q = 8'd0;
    always_ff @(posedge clk) begin
        if (rst)     q <= 8'd0;
        else if (en) q <= q + 8'd1;
    end
    assign led = q;
endmodule
