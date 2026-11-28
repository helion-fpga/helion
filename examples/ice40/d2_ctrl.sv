// iCE40-M1 design D2: a counter with clock enable, synchronous reset and
// asynchronous reset (to a value with both set and cleared bits), plus a
// register on the falling clock edge. Exercises CE, SR, ARST and CLK_INV.
module d2_ctrl (
    input  logic       clk,
    input  logic       en,
    input  logic       srst,
    input  logic       arst_n,
    output logic [7:0] led
);
    logic [6:0] count;
    logic       fall;
    always_ff @(posedge clk or negedge arst_n)
        if (!arst_n)   count <= 7'h55;
        else if (srst) count <= 7'h00;
        else if (en)   count <= count + 7'd1;
    always_ff @(negedge clk) fall <= count[0] ^ en;
    assign led = {fall, count};
endmodule
