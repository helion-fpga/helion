// Rung 6, the fill: one design that uses most of the ZedBoard's Zynq-7020.
//
//   block RAM  240 RAMB18 (86%)   zedboard_bram_fill      (its own check, LD7/LD6 there)
//   DSP        192 DSP48E1 (87%)  zedboard_dsp_fill_prims (its own check)
//   LUTs       LANES lanes of 32 LUTs + 32 flops, folded by an XOR tree, so that
//              the whole design is at least 85% of the 53,200 LUTs.
//
// LUT lanes. Lane l holds a 32-bit state that starts at seed(l) (the flops'
// initial values, no reset) and steps NCYC times:
//   next[j] = s[j-1] ^ (s[j+3] & s[j+7]) ^ s[j+12]
// (indices mod 32): four state bits per output bit, one LUT each. (With six
// inputs on every LUT, every LUT input pin of a lane's tiles is in use and
// Helion's router does not finish even 96 lanes: HELION_GAPS.md G75.) A lane only
// moves while its enable is high. The enable is a pulse NCYC clocks long that
// travels down a shift register, one lane per clock, so every lane steps
// exactly NCYC times, no net fans out to more than one lane, and a lane that
// is not enabled holds its state (start-up needs no global reset).
// When the pulse has left the last lane every state is final. The states,
// each rotated left by (l mod 32), are XORed together through a registered
// tree and the result must equal EXP_LUT, which
// examples/zedboard_fill_expected.py computes from the formula above.
//
// LEDs: LD7 all three done, LD6 all three passed, LD0 blinks when passed
// (about 0.75 Hz). On a failure LD1 = block RAM, LD2 = DSP, LD3 = LUT lanes.
// BTNC (rst) re-runs the RAM and DSP tests; the LUT lanes run once after
// configuration.
//
// Build (Helion only):
//   zedbit examples/zedboard_fill.sv,examples/zedboard_bram_fill.sv,examples/zedboard_dsp_fill_prims.sv
//          examples/zedboard_bram_fill.xdc out.bit zedboard_fill
module zedboard_fill #(
    parameter int          LANES   = 960,
    parameter int          NCYC    = 1024,
    parameter logic [31:0] EXP_LUT = 32'h4658B7EB
) (
    input  logic       clk,
    input  logic       rst,
    output logic [7:0] led
);
    logic [7:0] led_ram, led_dsp;
    zedboard_bram_fill      u_ram (.clk(clk), .rst(rst), .led(led_ram));
    zedboard_dsp_fill_prims u_dsp (.clk(clk), .rst(rst), .led(led_dsp));

    localparam int GRP = 6;                     // words XORed in one LUT level
    localparam int NG  = (LANES + GRP - 1) / GRP;
    localparam int NG2 = (NG + GRP - 1) / GRP;
    localparam int NG3 = (NG2 + GRP - 1) / GRP;

    // Start state of lane `l` (never zero).
    function automatic logic [31:0] seed(input int unsigned l);
        logic [31:0] h;
        h = (l + 1) * 32'h9E3779B1;
        h = h ^ (h >> 16);
        h = h * 32'h85EBCA6B;
        h = h ^ (h >> 13);
        return h | 32'd1;
    endfunction

    function automatic logic [31:0] rotl32(input logic [31:0] v, input int r);
        logic [63:0] w;
        w = {v, v} << r;
        return w[63:32];
    endfunction

    function automatic logic [31:0] step(input logic [31:0] s);
        logic [31:0] n;
        for (int j = 0; j < 32; j++)
            n[j] = s[(j + 31) % 32] ^ (s[(j + 3) % 32] & s[(j + 7) % 32]) ^ s[(j + 12) % 32];
        return n;
    endfunction

    // Power-on delay, then the root of the enable pulse: NCYC clocks long.
    localparam int CW = $clog2(NCYC + 1);
    localparam int FW = $clog2(LANES + 16 + 1);
    logic [4:0]      por  = 5'd0;
    logic [CW-1:0]   cnt  = '0;
    logic [FW-1:0]   fin  = '0;
    logic            en0  = 1'b0;
    logic            done = 1'b0, fail = 1'b0;
    logic [LANES-1:0] en  = '0;
    wire running = por[4] & (cnt != CW'(NCYC));
    wire drained = fin == FW'(LANES + 16);

    logic [32*LANES-1:0] rot;
    for (genvar l = 0; l < LANES; l++) begin : g_lane
        logic [31:0] s = seed(l);
        always_ff @(posedge clk) if (en[l]) s <= step(s);
        assign rot[32*l +: 32] = rotl32(s, l % 32);
    end

    // Fold: rot -> f1 -> f2 -> f3 -> sig, each stage XORing at most GRP words.
    logic [32*NG-1:0]  x1, f1;
    logic [32*NG2-1:0] x2, f2;
    logic [32*NG3-1:0] x3, f3;
    logic [31:0]       x4, sig;
    always_comb begin
        x1 = '0;
        for (int l = 0; l < LANES; l++) x1[32*(l/GRP) +: 32] = x1[32*(l/GRP) +: 32] ^ rot[32*l +: 32];
        x2 = '0;
        for (int g = 0; g < NG; g++)    x2[32*(g/GRP) +: 32] = x2[32*(g/GRP) +: 32] ^ f1[32*g +: 32];
        x3 = '0;
        for (int g = 0; g < NG2; g++)   x3[32*(g/GRP) +: 32] = x3[32*(g/GRP) +: 32] ^ f2[32*g +: 32];
        x4 = '0;
        for (int g = 0; g < NG3; g++)   x4 = x4 ^ f3[32*g +: 32];
    end

    always_ff @(posedge clk) begin
        if (!por[4]) por <= por + 5'd1;
        if (running) cnt <= cnt + 1'b1;
        en0 <= running;
        en  <= {en[LANES-2:0], en0};
        f1  <= x1;
        f2  <= x2;
        f3  <= x3;
        sig <= x4;
        // After the root pulse ends, wait for it to leave the last lane and
        // for the fold registers to settle, then check once.
        if (por[4] && cnt == CW'(NCYC) && !drained) fin <= fin + 1'b1;
        if (drained && !done) begin
            done <= 1'b1;
            fail <= sig != EXP_LUT;
        end
    end

    logic [26:0] beat = 27'd0;
    always_ff @(posedge clk) beat <= beat + 27'd1;

    wire ram_done = led_ram[7], ram_ok = led_ram[6];
    wire dsp_done = led_dsp[7], dsp_ok = led_dsp[6];
    wire all_done = done & ram_done & dsp_done;
    wire all_ok   = all_done & ~fail & ram_ok & dsp_ok;
    assign led = {all_done, all_ok, 2'b00,
                  done & fail, dsp_done & ~dsp_ok, ram_done & ~ram_ok, all_ok & beat[26]};
endmodule
