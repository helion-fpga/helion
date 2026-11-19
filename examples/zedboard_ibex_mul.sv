// The board's Ibex SoC (zedboard_ibex_soc.sv) running a multiply test, so a
// bitstream check exercises the core's multiplier (RV32MFast: one 17x17
// signed product, a DSP48E1). The counter program never multiplies.
//
//   30000080  lui    x1, 0x10000      ; LED register address
//   30000084  lui    x2, 0x9e378
//   30000088  addi   x2, x2, -1607    ; a = 0x9e3779b9
//   3000008c  lui    x3, 0x7f4a8
//   30000090  addi   x3, x3, 1985     ; b = 0x7f4a87c1
//   30000094  mul    x4, x2, x3       ; loop
//   30000098  mulh   x5, x2, x3
//   3000009c  mulhu  x6, x2, x3
//   300000a0  mulhsu x7, x2, x3
//   300000a4  xor    x8, x4, x5       ; x8 = x4 ^ x5 ^ x6 ^ x7, folded to a byte
//   300000a8  xor    x8, x8, x6
//   300000ac  xor    x8, x8, x7
//   300000b0  srli   x9, x8, 16
//   300000b4  xor    x8, x8, x9
//   300000b8  srli   x9, x8, 8
//   300000bc  xor    x8, x8, x9
//   300000c0  sw     x8, 0(x1)        ; LEDs
//   300000c4  add    x2, x2, x4       ; a += low product
//   300000c8  xor    x3, x3, x5       ; b = (b ^ signed high product) + 53
//   300000cc  addi   x3, x3, 53
//   300000d0  jal    x0, -60
//
// The first LED values are 00001010 10101111 11001100 01101000 10000100.
// Build and check as zedboard_ibex_soc, with this file added and top
// zedboard_ibex_mul.
module zedboard_ibex_mul (
    input  logic       clk,
    output logic [7:0] led
);
    zedboard_ibex_soc #(.MUL_TEST(1'b1)) u_soc (.clk(clk), .led(led));
endmodule
