// Root cause: wide_cone
// A single continuous assign whose cone is wide (many primary inputs).
module e_wide_cone (
    input  logic [63:0] a,
    input  logic [63:0] b,
    input  logic [63:0] c,
    output logic        y
);
    wire [63:0] p = a & b;
    assign y = ^(p ^ c) ^ (a[0] & b[1]);
endmodule
