// Hierarchical blinky — child tog instantiated in top
module tog (
    input  logic clk,
    output logic q
);
    always_ff @(posedge clk) q <= ~q;
endmodule

module hier (
    input  logic clk,
    output logic led
);
    tog u0 (.clk(clk), .q(led));
endmodule
