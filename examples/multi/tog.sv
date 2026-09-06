// Child toggle — multi-file project honesty (CovertEDA/Vivado-class sources list)
module tog (
    input  logic clk,
    output logic q
);
    always_ff @(posedge clk) q <= ~q;
endmodule
