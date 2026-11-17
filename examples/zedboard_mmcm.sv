// Rung 4 (clocking): the ZedBoard's 100 MHz oscillator (Y9) into one
// MMCME2_BASE making three clocks, each on its own BUFG:
//   clk_a  50 MHz         (VCO 1000 MHz / 20)
//   clk_b 125 MHz         (VCO / 8)
//   clk_c  25 MHz, +50.625 degrees (VCO / 40; 5 5/8 VCO periods late)
// The MMCM's LOCKED gates a power-on reset in each domain (two-flop
// synchronizer, then a 16-cycle hold). Domain A sends a 32-bit LFSR
// sequence to domain B over a toggle handshake (req and ack each cross
// through two flops; the data is stable while req is in flight). Domain B
// runs the same LFSR and compares every word it receives.
//
// LEDs: LD0/LD1/LD2 heartbeat of clk_a/clk_b/clk_c (about 1.5, 1.9, 1.5 Hz),
// LD3..LD5 bits 25..23 of B's received-word count (they count, about a
// second per step), LD6 cross-domain check passed (at least 2^16 words
// received, none wrong),
// LD7 MMCM locked.
// Pmods JA and JB (dbg) carry fast-changing state of all three domains, so
// a co-simulation of a few thousand cycles sees every domain and the
// crossing work (the LEDs change only every fraction of a second):
// JA = {clk_b count[3:0], clk_a count[3:0]}, JB = {error, words[2:0],
// clk_c count[3:0]}. Nothing needs to be plugged in.
// Build: zedbit examples/zedboard_mmcm.sv examples/zedboard_mmcm.xdc out.bit zedboard_mmcm
module zedboard_mmcm (
    input  logic        clk,
    output logic [7:0]  led,
    output logic [15:0] dbg
);
    logic clk_fb, clk_a_mmcm, clk_b_mmcm, clk_c_mmcm, locked;
    logic clk_a, clk_b, clk_c;

    MMCME2_BASE #(
        .BANDWIDTH("OPTIMIZED"),
        .CLKIN1_PERIOD(10.0),
        .DIVCLK_DIVIDE(1),
        .CLKFBOUT_MULT_F(10.0),
        .CLKFBOUT_PHASE(0.0),
        .CLKOUT0_DIVIDE_F(20.0),
        .CLKOUT0_DUTY_CYCLE(0.5),
        .CLKOUT0_PHASE(0.0),
        .CLKOUT1_DIVIDE(8),
        .CLKOUT1_DUTY_CYCLE(0.5),
        .CLKOUT1_PHASE(0.0),
        .CLKOUT2_DIVIDE(40),
        .CLKOUT2_DUTY_CYCLE(0.5),
        .CLKOUT2_PHASE(50.625),
        .REF_JITTER1(0.010),
        .STARTUP_WAIT("FALSE")
    ) u_mmcm (
        .CLKIN1(clk),
        .CLKFBIN(clk_fb),
        .CLKFBOUT(clk_fb),
        .CLKOUT0(clk_a_mmcm),
        .CLKOUT1(clk_b_mmcm),
        .CLKOUT2(clk_c_mmcm),
        .LOCKED(locked),
        .PWRDWN(1'b0),
        .RST(1'b0)
    );

    BUFG u_bufg_a (.I(clk_a_mmcm), .O(clk_a));
    BUFG u_bufg_b (.I(clk_b_mmcm), .O(clk_b));
    BUFG u_bufg_c (.I(clk_c_mmcm), .O(clk_c));

    // Power-on reset per domain: held until LOCKED has been seen high for
    // two flops plus 16 cycles of that domain's clock.
    logic rst_a, rst_b, rst_c;
    por u_por_a (.clk(clk_a), .locked(locked), .rst(rst_a));
    por u_por_b (.clk(clk_b), .locked(locked), .rst(rst_b));
    por u_por_c (.clk(clk_c), .locked(locked), .rst(rst_c));

    // Heartbeats.
    logic [24:0] cnt_a;
    logic [25:0] cnt_b;
    logic [23:0] cnt_c;
    always_ff @(posedge clk_a) cnt_a <= rst_a ? 25'd0 : cnt_a + 25'd1;
    always_ff @(posedge clk_b) cnt_b <= rst_b ? 26'd0 : cnt_b + 26'd1;
    always_ff @(posedge clk_c) cnt_c <= rst_c ? 24'd0 : cnt_c + 24'd1;

    // Domain A: sender. A new word goes out once the previous one's ack
    // has come back.
    logic [31:0] lfsr_a, data_a;
    logic req_a, busy_a;
    (* ASYNC_REG = "TRUE" *) logic ack_s1, ack_s2;
    logic ack_b;
    always_ff @(posedge clk_a) begin
        ack_s1 <= ack_b;
        ack_s2 <= ack_s1;
        if (rst_a) begin
            lfsr_a <= 32'h1;
            data_a <= 32'h0;
            req_a  <= 1'b0;
            busy_a <= 1'b0;
        end else if (!busy_a) begin
            data_a <= lfsr_a;
            lfsr_a <= next(lfsr_a);
            req_a  <= ~req_a;
            busy_a <= 1'b1;
        end else if (ack_s2 == req_a) begin
            busy_a <= 1'b0;
        end
    end

    // Domain B: receiver and checker.
    (* ASYNC_REG = "TRUE" *) logic req_s1, req_s2;
    logic [31:0] expect_b, got_b;
    logic [25:0] words_b;
    logic err_b, have_b, many_b;
    // The capture flops have no reset, so the data goes flop to flop.
    always_ff @(posedge clk_b) if (req_s2 != ack_b) got_b <= data_a; // stable: A holds it until this ack returns
    always_ff @(posedge clk_b) begin
        req_s1 <= req_a;
        req_s2 <= req_s1;
        if (rst_b) begin
            ack_b    <= 1'b0;
            expect_b <= 32'h1;
            have_b   <= 1'b0;
            words_b  <= 26'd0;
            err_b    <= 1'b0;
            many_b   <= 1'b0;
        end else begin
            have_b <= 1'b0;
            if (req_s2 != ack_b) begin
                have_b <= 1'b1;
                ack_b  <= req_s2;
            end
            if (have_b) begin
                if (got_b != expect_b) err_b <= 1'b1;
                expect_b <= next(expect_b);
                words_b <= words_b + 26'd1;
                if (words_b[16]) many_b <= 1'b1;
            end
        end
    end

    // x^32 + x^22 + x^2 + x + 1, Galois form.
    function automatic logic [31:0] next(input logic [31:0] s);
        next = {1'b0, s[31:1]} ^ (s[0] ? 32'h80200003 : 32'h0);
    endfunction

    assign led[0] = cnt_a[24];
    assign led[1] = cnt_b[25];
    assign led[2] = cnt_c[23];
    assign led[5:3] = words_b[25:23];
    assign led[6] = many_b & ~err_b;
    assign led[7] = locked;
    assign dbg = {err_b, words_b[2:0], cnt_c[3:0], cnt_b[3:0], cnt_a[3:0]};
endmodule

module por (
    input  logic clk,
    input  logic locked,
    output logic rst
);
    (* ASYNC_REG = "TRUE" *) logic s1 = 1'b0, s2 = 1'b0;
    logic [4:0] hold = 5'd0;
    always_ff @(posedge clk) begin
        s1 <= locked;
        s2 <= s1;
        if (!s2) hold <= 5'd0;
        else if (!hold[4]) hold <= hold + 5'd1;
    end
    assign rst = ~hold[4];
endmodule
