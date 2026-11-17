// The rung 2 block RAM fill (examples/zedboard_bram_fill.sv, 240 RAMB18)
// clocked at 25 MHz: the board's 100 MHz oscillator divided by 4 with two
// toggle flops, the same way zedboard_pico_soc.sv makes clk_half. Same RAM
// logic and LEDs (LD7 done, LD6 all passed); only the clock is slower, for
// while Helion's routing can't meet 100 MHz on this design (HELION_GAPS.md G38).
// Build: zedbit examples/zedboard_bram_fill_25mhz.sv,examples/zedboard_bram_fill.sv
//        examples/zedboard_bram_fill.xdc out.bit zedboard_bram_fill_25mhz
module zedboard_bram_fill_25mhz (
    input  logic       clk,
    input  logic       rst,
    output logic [7:0] led
);
    logic clk_half = 1'b0;
    logic clk_quarter = 1'b0;
    always_ff @(posedge clk) clk_half <= ~clk_half;
    always_ff @(posedge clk_half) clk_quarter <= ~clk_quarter;
    zedboard_bram_fill u_fill (.clk(clk_quarter), .rst(rst), .led(led));
endmodule
