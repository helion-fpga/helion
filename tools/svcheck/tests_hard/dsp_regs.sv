// Registers around inferred DSP48E1s: the shapes the slice takes (A, B, M
// and P registers, accumulate, clock enable, synchronous reset to 0) and
// the ones that must stay flops in the fabric. Each shape multiplies its
// own slice of `x` and `y`, so no two share a product.
module dsp_regs (
    input  logic               clk, arst, rst, en, en2,
    input  logic        [255:0] x,
    input  logic        [255:0] y,
    output logic signed [42:0] q_p,     // P
    output logic signed [42:0] q_ce,    // P with a clock enable
    output logic signed [42:0] q_rst,   // P with a synchronous reset that wins over the enable
    output logic signed [42:0] q_ab,    // A, B and P
    output logic signed [47:0] q_mp,    // M and P
    output logic signed [47:0] q_acc,   // P <= P + A * B
    output logic signed [47:0] q_mac,   // A, B, M, P, accumulate, reset and enable
    output logic        [31:0] q_u,     // unsigned 16 x 16 into 32 bits, A and B with their own enables
    output logic        [15:0] q_tr,    // truncated product accumulated in 16 bits
    output logic        [31:0] q_async, // asynchronous reset: stays a flop
    output logic        [31:0] q_init,  // power-up value 5: stays a flop
    output logic        [31:0] q_one,   // reset to 1: stays a flop
    output logic        [31:0] q_under, // reset under the enable: stays a flop
    output logic        [31:0] q_neg,   // falling edge: stays a flop
    output logic        [31:0] q_sh,    // product also used combinationally: stays a flop
    output logic        [31:0] o_sh,
    output logic        [31:0] q_mo,    // M read elsewhere too: it is P, the next stage a flop
    output logic        [31:0] o_mo,
    output logic signed [63:0] q_wacc,  // accumulator wider than 48 bits: the add stays in the fabric
    output logic        [31:0] q_ain,   // A register also read elsewhere: kept in the fabric as well
    output logic        [15:0] o_ain
);
    function automatic logic signed [24:0] sa(input int k); return x[25*k +: 25]; endfunction
    function automatic logic signed [17:0] sb(input int k); return y[18*k +: 18]; endfunction
    function automatic logic [15:0] ua(input int k); return x[16*k +: 16]; endfunction
    function automatic logic [15:0] ub(input int k); return y[16*k +: 16]; endfunction

    always_ff @(posedge clk) q_p <= sa(0) * sb(0);
    always_ff @(posedge clk) if (en) q_ce <= sa(1) * sb(1);
    always_ff @(posedge clk) if (rst) q_rst <= '0; else if (en) q_rst <= sb(2) * sa(2);

    logic signed [24:0] ar;
    logic signed [17:0] br;
    always_ff @(posedge clk) begin
        ar   <= sa(3);
        br   <= sb(3);
        q_ab <= ar * br;
    end

    logic signed [47:0] m1;
    always_ff @(posedge clk) begin
        m1   <= sa(4) * sb(4);
        q_mp <= m1;
    end

    always_ff @(posedge clk) q_acc <= q_acc + sa(5) * sb(5);

    logic signed [24:0] ma;
    logic signed [17:0] mb;
    logic signed [47:0] mm;
    always_ff @(posedge clk) begin
        if (rst) begin
            ma <= '0; mb <= '0; mm <= '0; q_mac <= '0;
        end else if (en) begin
            ma    <= sa(6);
            mb    <= sb(6);
            mm    <= ma * mb;
            q_mac <= q_mac + mm;
        end
    end

    logic [15:0] uc, ud;
    always_ff @(posedge clk) begin
        if (en) uc <= ua(0);
        if (en2) ud <= ub(0);
        q_u <= uc * ud;
    end

    always_ff @(posedge clk) q_tr <= q_tr + ua(1) * ub(1);

    always_ff @(posedge clk or posedge arst) if (arst) q_async <= '0; else q_async <= ua(2) * ub(2);

    initial q_init = 32'd5;
    always_ff @(posedge clk) q_init <= ua(3) * ub(3);

    always_ff @(posedge clk) if (rst) q_one <= 32'd1; else q_one <= ua(4) * ub(4);

    always_ff @(posedge clk) if (en) begin if (rst) q_under <= '0; else q_under <= ua(5) * ub(5); end

    always_ff @(negedge clk) q_neg <= ua(6) * ub(6);

    assign o_sh = ua(7) * ub(7);
    always_ff @(posedge clk) q_sh <= ua(7) * ub(7);

    logic [31:0] mo;
    always_ff @(posedge clk) begin
        mo   <= ua(8) * ub(8);
        q_mo <= mo;
    end
    assign o_mo = mo;

    always_ff @(posedge clk) q_wacc <= q_wacc + sa(7) * sb(7);

    logic [15:0] ai;
    always_ff @(posedge clk) begin
        ai    <= ua(9);
        q_ain <= ai * ub(9);
    end
    assign o_ain = ai;
endmodule
