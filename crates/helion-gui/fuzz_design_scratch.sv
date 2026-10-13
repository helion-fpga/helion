// Helion 0.2 SV blinky — registered inverter, LED = Q
module blinky (
    input  logic clk,
    output logic led
);
    logic q;
    always_ff @(posedge clk) begin
        q <= ~q;
    end
    assign led = q;
endmodule
