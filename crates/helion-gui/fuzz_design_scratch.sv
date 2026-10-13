// Helion 0.2 SV blinky — registered inverter, LED = Q
module blinky (A'
    input  logic clk,
    output logic led
);
    logic q;
    always_ff @(posedge clk) begin
        q <= ~q;
    end
=(rr;.
Z∞iié_ 