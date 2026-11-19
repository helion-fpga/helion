// Combinational DSP48E1s inferred from `*` by the v2 front end.
//
// LANES lanes each run a 32-bit Galois LFSR (CRC-32 polynomial, seeded per
// lane) and form, every clock, seven products of the operand shapes one
// DSP48E1 multiplier takes with no register in the slice:
//   signed 25 x 18, unsigned 24 x 17, signed * unsigned (unsigned 20 x 17),
//   16 x 16 truncated to 16 bits, signed 20 x 12 into 64 bits, x * 100,
//   signed x * -12345.
// The products are XORed into a 48-bit register that rotates every clock,
// so no register loads a product straight and every product stays a
// combinational DSP (7 per lane). The LEDs show all lanes' registers
// folded to 8 bits: any wrong product bit shows there within a few clocks,
// which is what `x7v verify` compares with the RTL.
//
// Build and check (Helion only):
//   zedbit examples/zedboard_dsp_comb.sv examples/zedboard_dsp_fill.xdc dsp_comb.bit zedboard_dsp_comb
//   cd tools/x7v && python3 -m x7v --board zedboard verify dsp_comb.bit \
//       --rtl ../../examples/zedboard_dsp_comb.sv --top zedboard_dsp_comb \
//       --xdc ../../examples/zedboard_dsp_fill.xdc --cycles 3000
module zedboard_dsp_comb #(
    parameter int LANES = 4
) (
    input  logic       clk,
    input  logic       rst,
    output logic [7:0] led
);
    localparam logic [31:0] POLY = 32'h04C11DB7;

    // Power-on reset: flops start at zero when the board is configured.
    logic [3:0] por = 4'd0;
    always_ff @(posedge clk) if (por != 4'd15) por <= por + 4'd1;
    logic [1:0] rst_sync = 2'b00;
    always_ff @(posedge clk) rst_sync <= {rst_sync[0], rst};
    wire restart = (por != 4'd15) | rst_sync[1];

    logic [48*LANES-1:0] hs;

    for (genvar l = 0; l < LANES; l++) begin : g_lane
        logic [31:0] s = '0;
        logic [47:0] h = '0;
        wire signed [24:0] sa = s[24:0];
        wire signed [17:0] sb = s[31:14];
        wire signed [19:0] s20 = s[19:0];
        wire signed [11:0] s12 = s[31:20];
        wire signed [42:0] p_ss = sa * sb;
        wire        [40:0] p_uu = s[23:0] * s[31:15];
        wire        [47:0] p_mix = s20 * s[31:15];
        wire        [15:0] p_tr = s[15:0] * s[31:16];
        wire signed [63:0] p_wide = s20 * s12;
        wire        [31:0] p_k = s[31:16] * 100;
        wire signed [31:0] p_kn = s20 * -12345;
        wire [47:0] fold = 48'(p_ss) ^ 48'(p_uu) ^ p_mix ^ 48'(p_tr) ^ p_wide[47:0] ^ 48'(p_wide[63:48])
                           ^ 48'(p_k) ^ {p_kn, 16'd0};
        always_ff @(posedge clk) begin
            s <= restart ? 32'(l + 1) * 32'h9E3779B1 | 32'd1 : {s[30:0], 1'b0} ^ (s[31] ? POLY : 32'd0);
            h <= restart ? 48'd0 : {h[46:0], h[47]} ^ fold;
        end
        assign hs[48*l +: 48] = h;
    end

    logic [47:0] x;
    always_comb begin
        x = '0;
        for (int l = 0; l < LANES; l++) x = x ^ hs[48*l +: 48];
    end
    logic [7:0] q = '0;
    always_ff @(posedge clk) q <= x[7:0] ^ x[15:8] ^ x[23:16] ^ x[31:24] ^ x[39:32] ^ x[47:40];
    assign led = q;
endmodule
