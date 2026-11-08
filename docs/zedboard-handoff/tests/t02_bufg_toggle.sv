// t02_bufg_toggle: BUFGCTRL clock-path test. One toggle FF, no arithmetic.
// Isolates: IBUF -> BUFGCTRL -> global clock network -> FF clock pin.
// A 100 MHz clock toggling one FF yields 50 MHz on LD0, which the eye
// sees as a steady DIM glow (50% duty, far above flicker fusion).
// PASS: LD0 glows dimly (clearly on, clearly not full bright).
// FAIL (clock not reaching FF): LD0 fully dark (q never toggles from 0).
// Uses no CARRY4, no CE, no SR, so a PASS proves the clock path alone.
module t02_bufg_toggle (
    input  logic       clk,
    output logic [7:0] led
);
    logic q;
    always_ff @(posedge clk) begin
        q <= ~q;
    end
    assign led[0] = q;
    assign led[7:1] = 7'b0;
endmodule
