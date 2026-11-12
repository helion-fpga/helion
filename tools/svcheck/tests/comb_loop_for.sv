module top (input logic [7:0] a, input logic [7:0] b, output logic [3:0] ones, output logic [7:0] rev, output logic [7:0] prio);
    always_comb begin
        ones = '0;
        for (int i = 0; i < 8; i++) ones = ones + {3'b0, a[i]};
    end
    always_comb begin
        for (int i = 0; i < 8; i++) rev[i] = b[7 - i];
    end
    always_comb begin
        prio = '0;
        for (int i = 7; i >= 0; i--) if (a[i] & b[i]) prio = 8'(i) + 8'd1;
    end
endmodule
