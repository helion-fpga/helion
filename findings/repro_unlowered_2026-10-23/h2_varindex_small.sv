// same, but depth 8 so it clears MAX_DEPTH=16 -- does the var-index read lower?
module h2_varindex_small (
    input  logic       clk,
    input  logic [7:0] wdata,
    input  logic [2:0] waddr,
    input  logic       we,
    input  logic [2:0] raddr,
    output logic [7:0] rdata
);
    reg [7:0] m [0:7];
    always @(posedge clk) if (we) m[waddr] <= wdata;
    assign rdata = m[raddr];
endmodule
