module top (input logic clk, input logic [7:0] a, input logic [7:0] b, input logic [2:0] op, output logic [7:0] y, output logic z, output logic lt, output logic [15:0] acc);
    logic [7:0] r;
    always_comb begin
        case (op)
            3'd0: r = a + b;
            3'd1: r = a - b;
            3'd2: r = a & b;
            3'd3: r = a | b;
            3'd4: r = a ^ b;
            3'd5: r = a << b[2:0];
            3'd6: r = a >> b[2:0];
            default: r = ~a;
        endcase
    end
    always_ff @(posedge clk) acc <= acc + {8'b0, r} - {12'b0, op, 1'b0};
    assign y = r;
    assign z = (r == 8'd0);
    assign lt = ($signed(a) < $signed(b));
endmodule
