// Helion stress design — 16×4-bit incrementers, hierarchy, generate-if, XOR reduce.
// Helion-legal SV (no UNISIM / AXI). LED is the XOR of all incrementer MSBs.
// ~4× the LUT count of examples/counter.sv, with real child instances.
module inc4 (
    input  logic clk,
    output logic msb
);
    logic [3:0] cnt;
    always_ff @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assign msb = cnt[3];
endmodule

module xor4 (
    input  logic a,
    input  logic b,
    input  logic c,
    input  logic d,
    output logic y
);
    assign y = ((a ^ b) ^ (c ^ d));
endmodule

module complex (
    input  logic clk,
    output logic led
);
    logic [15:0] m;
    logic p0;
    logic p1;
    logic p2;
    logic p3;
    logic q0;
    logic q1;
    (* mark_debug = "true" *) logic x;
    generate
        if (1) begin
            inc4 u00 (.clk(clk), .msb(m[0]));
            inc4 u01 (.clk(clk), .msb(m[1]));
            inc4 u02 (.clk(clk), .msb(m[2]));
            inc4 u03 (.clk(clk), .msb(m[3]));
            inc4 u04 (.clk(clk), .msb(m[4]));
            inc4 u05 (.clk(clk), .msb(m[5]));
            inc4 u06 (.clk(clk), .msb(m[6]));
            inc4 u07 (.clk(clk), .msb(m[7]));
            inc4 u08 (.clk(clk), .msb(m[8]));
            inc4 u09 (.clk(clk), .msb(m[9]));
            inc4 u10 (.clk(clk), .msb(m[10]));
            inc4 u11 (.clk(clk), .msb(m[11]));
            inc4 u12 (.clk(clk), .msb(m[12]));
            inc4 u13 (.clk(clk), .msb(m[13]));
            inc4 u14 (.clk(clk), .msb(m[14]));
            inc4 u15 (.clk(clk), .msb(m[15]));
        end
    endgenerate
    xor4 r0 (.a(m[0]), .b(m[1]), .c(m[2]), .d(m[3]), .y(p0));
    xor4 r1 (.a(m[4]), .b(m[5]), .c(m[6]), .d(m[7]), .y(p1));
    xor4 r2 (.a(m[8]), .b(m[9]), .c(m[10]), .d(m[11]), .y(p2));
    xor4 r3 (.a(m[12]), .b(m[13]), .c(m[14]), .d(m[15]), .y(p3));
    xor4 r4 (.a(p0), .b(p1), .c(p2), .d(p3), .y(x));
    assign led = x;
endmodule
