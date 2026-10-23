// t02_input_passthrough: input IOB + combinational LUT path, no clock.
// Isolates: IBUF -> LUT -> OBUF. A design with a *data* input used to be
// unbuildable (the pad net had no driver), so this is the test that proves
// inputs work at all.
// Pass: each LED mirrors its button. BTNC..BTNR are btn[0..4], LD0..LD4.
module t02_input_passthrough (
    input  logic [4:0] btn,
    output logic [7:0] led
);
    assign led[4:0] = btn;
    assign led[7:5] = 3'b0;
endmodule
