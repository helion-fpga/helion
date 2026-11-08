// t05_ff_sr: FF synchronous-reset test (needs BTNC button).
// Isolates: FF SRUSEDMUX / SR pin configuration.
// q toggles freely; while BTNC is held, synchronous reset forces q to 0.
// PASS: LD0 dim (toggling) normally; pressing BTNC makes LD0 go dark
//   and stay dark while held; releasing resumes dim toggling.
// FAIL (SR ignored): pressing BTNC has no effect on LD0.
// LD1 mirrors btn[0] so the observer can confirm the button reads at all.
// BUTTON CAVEAT: same as t04. If LD1 does not respond to BTNC, this test
// is INCONCLUSIVE (not FAIL).
// NOTE: uses a synchronous reset (inside the posedge block) because the
// Helion backend's async-reset lowering is unverified. Sync reset exercises
// the SR pin path without depending on async support.
module t05_ff_sr (
    input  logic       clk,
    input  logic [4:0] btn,
    output logic [7:0] led
);
    logic q;
    always_ff @(posedge clk) begin
        if (btn[0]) q <= 1'b0;
        else       q <= ~q;
    end
    assign led[0] = q;
    assign led[1] = btn[0];  // button sanity mirror
    assign led[7:2] = 6'b0;
endmodule
