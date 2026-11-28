// iCE40-M1 design D1: a free-running counter on the HX8K breakout's 8 LEDs.
// At 12 MHz led[0] toggles about every 11 ms (bit 16 of the count).
module d1_counter (
    input  logic       clk,
    output logic [7:0] led
);
    logic [23:0] count;
    always_ff @(posedge clk) count <= count + 24'd1;
    assign led = count[23:16];
endmodule
