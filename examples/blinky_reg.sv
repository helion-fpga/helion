// Faithful SystemVerilog equivalent of the Vivado oracle source
// ~/CDA4253_A1/blinky.vhd (GT-blinky.bit, md5 3f6d7cfa8a5db206e5cf6fd0e3568324).
// Copied from work/ref/blinky_reg.sv in the handoff package. Pin map matches
// examples/zedboard.xdc: clk on Y9, LD0 on T22.
//
// The VHDL registers LD0 INSIDE the clocked process:
//
//     if rising_edge(clk) then
//         count := count + 1;
//         LD0 <= count(24);      -- registered, not combinational
//     end if;
//
// The reference has 26 carry-chain flops (25 counter bits + the registered
// output). A combinational LD0 assign would be a different design.
module blinky (
    input  wire clk,
    output reg  LD0
);
    reg [24:0] count = 25'd0;

    always @(posedge clk) begin
        count <= count + 1'b1;
        LD0   <= count[24];
    end
endmodule
