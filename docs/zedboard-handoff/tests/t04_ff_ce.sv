// t04_ff_ce: FF clock-enable test, differential (needs BTNC button).
// Isolates: FF CEUSEDMUX / CE pin configuration.
// q_on has CE tied high and must toggle (LD0 dim at 50 MHz).
// q_off has CE tied low and must HOLD at 0 (LD1 dark).
// If the backend ignores CE (the known bug), q_off will toggle too and
// LD1 will glow dimly instead of staying dark -> FAIL.
// LD2 mirrors btn[0] so the observer can confirm the button reads at all.
// BUTTON CAVEAT: btn[] is Bank 34, whose Vcco follows the J18 Vadj jumper
// (factory default 1.8V) while the XDC says LVCMOS33. If J18 is at 1.8V,
// BTNC may not read; then LD2 will not respond and this test is
// INCONCLUSIVE (not FAIL). The harness must check LD2 first.
module t04_ff_ce (
    input  logic       clk,
    input  logic [4:0] btn,
    output logic [7:0] led
);
    logic q_on, q_off;
    logic ce_vcc, ce_gnd;
    assign ce_vcc = 1'b1;
    assign ce_gnd = 1'b0;
    always_ff @(posedge clk) begin
        if (ce_vcc) q_on  <= ~q_on;
        if (ce_gnd) q_off <= ~q_off;
    end
    assign led[0] = q_on;    // expect DIM (toggles)
    assign led[1] = q_off;   // expect DARK (holds 0)
    assign led[2] = btn[0];  // button sanity mirror
    assign led[7:3] = 5'b0;
endmodule
