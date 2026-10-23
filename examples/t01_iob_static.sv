// t01_iob_static: IOB output-driver test. No clock, no FFs, no LUT logic.
// Isolates: OBUF / IOB output configuration (DRIVE, SLEW, output enable).
// If LD0-LD4 light and LD5 stays dark, the IOB output path works.
// NOTE: LD6/LD7 (U19/U14) sit on RIOB33_SING tiles with no X-Ray segment
// bits, so they are excluded from the pass criterion; they may stay dark
// even when the backend is correct.
module t01_iob_static (
    output logic [7:0] led
);
    assign led[0] = 1'b1;
    assign led[1] = 1'b1;
    assign led[2] = 1'b1;
    assign led[3] = 1'b1;
    assign led[4] = 1'b1;
    assign led[5] = 1'b0;
    assign led[6] = 1'b0;
    assign led[7] = 1'b0;
endmodule
