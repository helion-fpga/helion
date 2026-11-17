// Fills the ZedBoard's DSP blocks with multiply-accumulators and checks them.
//
// LANES independent lanes (default 192 = 87% of the Zynq-7020's 220
// DSP48E1). Each lane has a 32-bit Galois LFSR (CRC-32 polynomial, seeded
// from a hash of the lane number) and computes, every clock,
//   a <= S[24:0]; b <= S[31:14]; m <= a * b; acc <= acc + m
// with a (25 bits), b (18 bits), m and acc (48 bits) signed: the
// standard multiply-accumulate shape that maps one lane to one DSP48E1
// (AREG = BREG = MREG = PREG = 1; the reset is the DSP's synchronous reset).
// The LFSR stays in the fabric. examples/zedboard_dsp_fill_prims.sv is the
// same design with explicit DSP48E1 instances.
//
// After configuration (a power-on counter, no button needed) the lanes run
// from their seeds with every register cleared. For NCYC clocks every
// lane's acc is rotated left by (lane mod 48), the LANES words are XORed
// together (three registered stages), the 48-bit result x is folded to 32
// bits (x[31:0] ^ x[47:32]) and shifted into a 32-bit CRC-32-polynomial
// signature, which must then equal EXP_SIG. It takes about 1,030 clocks
// (10 us). BTNC (rst) runs the test again.
//
// LEDs: LD7 = done, LD6 = signature matched. Passed: LD0 blinks (about
// 0.75 Hz). Failed: LD0 lit.
//
// EXP_SIG comes from examples/zedboard_dsp_fill_expected.py, which computes
// it from the same formula without this RTL; pass its output as a
// parameter when LANES or NCYC change.
//
// Build (Helion only):
//   HELION_V2_HARD=1 HELION_XILINX_DB=~/.cache/helion/prjxray-db \
//   cargo run --release -p helion-xilinx7 --example zedbit -- \
//       examples/zedboard_dsp_fill.sv examples/zedboard_dsp_fill.xdc \
//       dsp_fill.bit --front=v2
module zedboard_dsp_fill #(
    parameter int          LANES   = 192,
    parameter int          NCYC    = 1024,
    parameter logic [31:0] EXP_SIG = 32'h201901BD
) (
    input  logic       clk,
    input  logic       rst,
    output logic [7:0] led
);
    localparam int GRP = 6;                     // words XORed in one LUT level
    localparam int NG  = (LANES + GRP - 1) / GRP;
    localparam int NG2 = (NG + GRP - 1) / GRP;
    localparam logic [31:0] POLY = 32'h04C11DB7;

    // LFSR seed of lane `l` (never zero).
    function automatic logic [31:0] seed(input int unsigned l);
        logic [31:0] h;
        h = (l + 1) * 32'h9E3779B1;
        h = h ^ (h >> 16);
        h = h * 32'h85EBCA6B;
        h = h ^ (h >> 13);
        return h | 32'd1;
    endfunction

    function automatic logic [47:0] rotl48(input logic [47:0] v, input int r);
        logic [95:0] w;
        w = {v, v} << r;
        return w[95:48];
    endfunction

    // Power-on reset: flops start at zero when the board is configured.
    logic [3:0] por = 4'd0;
    always_ff @(posedge clk) if (por != 4'd15) por <= por + 4'd1;
    logic [1:0] rst_sync = 2'b00;
    always_ff @(posedge clk) rst_sync <= {rst_sync[0], rst};
    wire restart = (por != 4'd15) | rst_sync[1];

    logic [48*LANES-1:0] accrot;

    for (genvar l = 0; l < LANES; l++) begin : g_lane
        logic [31:0]        lfsr;
        logic signed [24:0] a;
        logic signed [17:0] b;
        logic signed [47:0] m;  // the 43-bit product, sign-extended
        logic signed [47:0] acc;
        always_ff @(posedge clk) begin
            if (restart) begin
                lfsr <= seed(l);
                a    <= '0;
                b    <= '0;
                m    <= '0;
                acc  <= '0;
            end else begin
                lfsr <= {lfsr[30:0], 1'b0} ^ (lfsr[31] ? POLY : 32'd0);
                a    <= lfsr[24:0];
                b    <= lfsr[31:14];
                m    <= a * b;
                acc  <= acc + m;
            end
        end
        assign accrot[48*l +: 48] = rotl48(acc, l % 48);
    end

    // Fold pipeline: acc -> s1 -> s2 -> x -> sig, each stage XORing at most
    // GRP words (a chain of XORs maps one LUT per GRP).
    logic [48*NG-1:0]  fold1, s1;
    logic [48*NG2-1:0] fold2, s2;
    logic [47:0]       fold3, x;

    always_comb begin
        fold1 = '0;
        for (int l = 0; l < LANES; l++)
            fold1[48*(l/GRP) +: 48] = fold1[48*(l/GRP) +: 48] ^ accrot[48*l +: 48];
        fold2 = '0;
        for (int g = 0; g < NG; g++)
            fold2[48*(g/GRP) +: 48] = fold2[48*(g/GRP) +: 48] ^ s1[48*g +: 48];
        fold3 = '0;
        for (int g = 0; g < NG2; g++)
            fold3 = fold3 ^ s2[48*g +: 48];
    end

    // Controller: cnt counts clocks from restart; acc is sampled while
    // cnt < NCYC and the signature is checked once the pipeline drains.
    localparam int CW = $clog2(NCYC + 4);
    localparam logic [CW-1:0] CHECK = CW'(NCYC + 3);
    logic [CW-1:0] cnt  = '0;
    logic          v1 = 1'b0, v2 = 1'b0, v3 = 1'b0;
    logic [31:0]   sig  = 32'd0;
    logic          done = 1'b0;
    logic          fail = 1'b0;

    always_ff @(posedge clk) begin
        v1 <= ~restart & ~done & (cnt < CW'(NCYC));
        v2 <= v1 & ~restart;
        v3 <= v2 & ~restart;
        s1 <= fold1;
        s2 <= fold2;
        x  <= fold3;
        if (restart) begin
            cnt  <= '0;
            sig  <= 32'd0;
            done <= 1'b0;
            fail <= 1'b0;
        end else if (!done) begin
            if (v3) sig <= {sig[30:0], 1'b0} ^ (sig[31] ? POLY : 32'd0) ^ x[31:0] ^ {16'd0, x[47:32]};
            if (cnt == CHECK) begin
                fail <= sig != EXP_SIG;
                done <= 1'b1;
            end
            cnt <= cnt + 1'b1;
        end
    end

    logic [26:0] beat = 27'd0;
    always_ff @(posedge clk) beat <= beat + 27'd1;

    wire ok = done & ~fail;
    assign led = {done, ok, 5'b00000, ok ? beat[26] : fail};
endmodule
