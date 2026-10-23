// t04_ff_reset: FF with asynchronous reset. No counter, no LUT arithmetic.
// Isolates: the Hff SR pin and its polarity, plus the reset input's IOB.
// BTNC is btn[0]; the reset is active high and asynchronous, so pressing BTNC
// clears the pattern immediately regardless of the clock.
// Pass: with BTNC held, LD0 on and LD1 off. Released, LD0/LD1 alternate on
// successive clocks (toggle flop).
module t04_ff_reset (
    input  logic       clk,
    input  logic [4:0] btn,
    output logic [7:0] led
);
    logic q;
    always_ff @(posedge clk or posedge btn[0]) begin
        if (btn[0]) q <= 1'b1;
        else        q <= ~q;
    end
    assign led[0] = q;
    assign led[1] = ~q;
    assign led[7:2] = 6'b0;
endmodule
