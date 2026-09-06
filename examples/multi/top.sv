// Top instantiates tog from a separate read_sv file
module top (
    input  logic clk,
    output logic led
);
    tog u0 (.clk(clk), .q(led));
endmodule
