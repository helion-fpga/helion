module h_gpio (
    input  wire clk,
    input  wire resetn,
    input  wire mm_valid,
    input  wire mm_write,
    input  wire [7:0] mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire mm_ready,
    output wire [7:0] gpio
);
    assign mm_ready = 1'b1;
    assign gpio = mm_wdata[7:0];
    always @(posedge clk) if (mm_valid && !mm_write) mm_rdata <= {24'h0, gpio};
endmodule
