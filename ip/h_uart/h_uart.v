// Helion-MM UART (catalog). Not Xilinx AXI UART.
module h_uart (
    input  wire clk,
    input  wire resetn,
    input  wire mm_valid,
    input  wire mm_write,
    input  wire [7:0] mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire mm_ready,
    output wire tx
);
    assign mm_ready = 1'b1;
    assign tx = 1'b1;
    always @(posedge clk) if (mm_valid && !mm_write) mm_rdata <= 32'h0;
endmodule
