// Fills the ZedBoard's block RAM and checks every word of it.
//
// NMEM memories of 1024 x 16, each with its own initial contents (a hash of
// the memory number and the address, set by an initial block). Helion's v2
// front end puts each one on a RAMB18 (HELION_V2_HARD=1), so the default
// 240 memories need 240 RAMB18 = 120 RAMB36 of the Zynq-7020's 140 (86%).
//
// After configuration (a power-on counter, no button needed) the design
// scans the memories three times, all of them in parallel, one address a
// clock:
//   scan 0  reads the initial contents and writes each word back with its
//           bytes swapped;
//   scan 1  reads the swapped words and swaps them back;
//   scan 2  reads the initial contents again.
// Each word read is rotated left by (memory number mod 16), the NMEM words
// of one address are XORed together (three registered stages), and the
// result is shifted into a 32-bit CRC-32-polynomial signature. After each
// scan the signature must equal EXP_INIT (scans 0 and 2) or EXP_SWAP
// (scan 1). The scans leave the memories as they started, so BTNC (rst)
// simply runs the test again. It takes about 3,100 clocks (31 us).
//
// LEDs: LD7 = done, LD6 = every scan matched. Passed: LD0 blinks (about
// 0.75 Hz). Failed: LD0..LD2 are the scans that did not match.
//
// EXP_INIT and EXP_SWAP come from examples/zedboard_bram_fill_expected.py,
// which computes them from the same content formula without this RTL;
// pass its output as parameters when NMEM changes. `--sim` there runs this
// design in Verilator to completion (and again after BTNC), with the right
// and with corrupted values.
//
// Build (Helion only; block RAM inference is the v2 front end's opt-in):
//   HELION_V2_HARD=1 HELION_XILINX_DB=~/.cache/helion/prjxray-db \
//   cargo run --release -p helion-xilinx7 --example zedbit -- \
//       examples/zedboard_bram_fill.sv examples/zedboard_bram_fill.xdc \
//       bram_fill.bit --front=v2
module zedboard_bram_fill #(
    parameter int          NMEM     = 240,
    parameter logic [31:0] EXP_INIT = 32'hB2C304D0,
    parameter logic [31:0] EXP_SWAP = 32'hF8590EEF
) (
    input  logic       clk,
    input  logic       rst,
    output logic [7:0] led
);
    localparam int GRP = 6;                     // words XORed in one LUT level
    localparam int NG  = (NMEM + GRP - 1) / GRP;
    localparam int NG2 = (NG + GRP - 1) / GRP;

    // Initial word `a` of memory `m`.
    function automatic logic [15:0] word0(input int unsigned m, input int unsigned a);
        logic [31:0] h;
        h = (m << 10) | a;
        h = h * 32'h9E3779B1;
        h = h ^ (h >> 16);
        h = h * 32'h85EBCA6B;
        h = h ^ (h >> 13);
        return h[15:0];
    endfunction

    function automatic logic [15:0] rotl16(input logic [15:0] v, input int r);
        logic [31:0] w;
        w = {v, v} << r;
        return w[31:16];
    endfunction

    // Power-on reset: flops start at zero when the board is configured.
    logic [3:0] por = 4'd0;
    always_ff @(posedge clk) if (por != 4'd15) por <= por + 4'd1;
    logic [1:0] rst_sync = 2'b00;
    always_ff @(posedge clk) rst_sync <= {rst_sync[0], rst};
    wire restart = (por != 4'd15) | rst_sync[1];

    // Controller: cnt 0..1023 issues the reads of one scan, then the
    // pipeline drains and the signature is checked at cnt == CHECK.
    localparam logic [10:0] CHECK = 11'd1031;
    logic [1:0]  pass = 2'd0;
    logic [10:0] cnt  = 11'd0;
    logic        done = 1'b0;
    logic [2:0]  fail = 3'd0;
    wire         re   = ~done & ~cnt[10] & ~restart;

    // Read pipeline: q (block RAM output) -> s1 -> s2 -> x -> sig, each
    // stage XORing at most GRP words (a chain of XORs maps one LUT per GRP).
    logic        v1 = 1'b0, v2 = 1'b0, v3 = 1'b0, v4 = 1'b0;
    logic [9:0]  a1;
    logic [16*NMEM-1:0] qrot;
    logic [16*NG-1:0]   fold1, s1;
    logic [16*NG2-1:0]  fold2, s2;
    logic [15:0]        fold3, x;
    logic [31:0]        sig = 32'd0;
    wire                we = v1 & (pass != 2'd2);

    for (genvar m = 0; m < NMEM; m++) begin : g_mem
        logic [15:0] mem [0:1023];
        logic [15:0] q;
        initial for (int a = 0; a < 1024; a++) mem[a] = word0(m, a);
        always_ff @(posedge clk) begin
            if (we) mem[a1] <= {q[7:0], q[15:8]};
            if (re) q <= mem[cnt[9:0]];
        end
        assign qrot[16*m +: 16] = rotl16(q, m % 16);
    end

    always_comb begin
        fold1 = '0;
        for (int m = 0; m < NMEM; m++)
            fold1[16*(m/GRP) +: 16] = fold1[16*(m/GRP) +: 16] ^ qrot[16*m +: 16];
        fold2 = '0;
        for (int g = 0; g < NG; g++)
            fold2[16*(g/GRP) +: 16] = fold2[16*(g/GRP) +: 16] ^ s1[16*g +: 16];
        fold3 = '0;
        for (int g = 0; g < NG2; g++)
            fold3 = fold3 ^ s2[16*g +: 16];
    end

    always_ff @(posedge clk) begin
        v1 <= re & ~restart;
        v2 <= v1 & ~restart;
        v3 <= v2 & ~restart;
        v4 <= v3 & ~restart;
        a1 <= cnt[9:0];
        s1 <= fold1;
        s2 <= fold2;
        x  <= fold3;
        if (restart) begin
            pass <= 2'd0;
            cnt  <= 11'd0;
            done <= 1'b0;
            fail <= 3'd0;
            sig  <= 32'd0;
        end else if (!done) begin
            if (v4) sig <= {sig[30:0], 1'b0} ^ (sig[31] ? 32'h04C11DB7 : 32'd0) ^ {16'd0, x};
            if (cnt == CHECK) begin
                fail[pass] <= sig != (pass == 2'd1 ? EXP_SWAP : EXP_INIT);
                sig  <= 32'd0;
                cnt  <= 11'd0;
                pass <= pass + 2'd1;
                done <= pass == 2'd2;
            end else begin
                cnt <= cnt + 11'd1;
            end
        end
    end

    logic [26:0] beat = 27'd0;
    always_ff @(posedge clk) beat <= beat + 27'd1;

    wire ok = done & (fail == 3'd0);
    assign led = {done, ok, 3'b000, ok ? {2'b00, beat[26]} : fail};
endmodule
