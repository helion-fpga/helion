// Self-checking testbench for examples/zedboard_audio.sv.
//
// I2C side: an ADAU1761 slave model samples SDA on each SCL rising edge,
// frames bytes between START and STOP, and checks
//   - the 20 transactions match the expected byte list below exactly
//     (address byte 0x76, register address MSB 0x40, LSB, data);
//   - the ACK slot after every byte is low (the master drives it low);
//   - SDA never changes while SCL is high, except for START and STOP;
//   - SDA does not change within 40 ns after an SCL fall, and the master
//     drives the ACK level (low) by 100 ns after SCL falls on the 8th bit
//     of each byte: a slave may start its ACK 0 ns after that fall
//     (UM10204 tHD;DAT min), so a 1 held longer fights it;
//   - every SDA change while SCL is low comes within 3.45 us of the SCL
//     fall (UM10204 tVD;DAT and tVD;ACK max);
//   - standard-mode timing (NXP UM10204 Rev. 7.0, Table 11): START hold >= 4.0 us,
//     SCL low >= 4.7 us, SCL high >= 4.0 us, data setup >= 250 ns,
//     STOP setup >= 4.0 us, bus free between STOP and START >= 4.7 us;
//   - at least 20 ms pass between the PLL write and the next write;
//   - led[0] stays low until the last STOP and is high after it;
//   - AC-ADR0 and AC-ADR1 are high.
// Audio side: checks
//   - MCLK period 80 ns, BCLK period 340 ns;
//   - LRCLK and SDATA change only while BCLK is low, at least 150 ns
//     before the next BCLK rising edge;
//   - every half frame is 32 BCLKs; LRCLK low = left;
//   - each channel word is the delay bit 0, a 24-bit sample, 7 zero bits;
//     the sample is +0x100000 or -0x100000, left equals right, and the
//     sign flips every 23 frames (999 Hz at fs = 100 MHz / 2176);
//   - led[1] (heartbeat) toggles.
// Pass prints PASS and finishes; any failure prints FAIL.
//
// Run with Verilator:
//   $ verilator --binary --timing --top-module zedboard_audio_tb \
//     examples/zedboard_audio_tb.sv examples/zedboard_audio.sv
`timescale 1ns / 1ps

module zedboard_audio_tb;
    logic clk = 0;
    logic ac_mclk, ac_bclk, ac_lrclk, ac_sdata, ac_adr0, ac_adr1, ac_scl, ac_sda;
    logic [1:0] led;

    always #5 clk = ~clk; // 100 MHz

    zedboard_audio dut (
        .clk(clk), .ac_mclk(ac_mclk), .ac_bclk(ac_bclk), .ac_lrclk(ac_lrclk),
        .ac_sdata(ac_sdata), .ac_adr0(ac_adr0), .ac_adr1(ac_adr1),
        .ac_scl(ac_scl), .ac_sda(ac_sda), .led(led)
    );

    // Expected transactions after the 0x76 0x40 prefix, flattened; txn_len
    // gives the number of bytes (register LSB + data) in each one.
    localparam int N_TXN = 20;
    localparam int N_EXP = 45;
    int txn_len [0:N_TXN-1] = '{2, 7, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2};
    logic [7:0] exp_b [0:N_EXP-1] = '{
        8'h00, 8'h0e,                                  // R0 clock control, core off, PLL
        8'h02, 8'h00, 8'h11, 8'h00, 8'h0d, 8'h19, 8'h01, // R1 PLL M=17 N=13 R=3 X=1 frac, enable
        8'h00, 8'h0f,                                  // R0 core clock on
        8'h15, 8'h00,                                  // serial port 0: slave
        8'h16, 8'h00,                                  // serial port 1: 64 BCLK, I2S delay
        8'h17, 8'h00,                                  // converter 0: fs
        8'hf8, 8'h00,                                  // serial sampling rate: fs
        8'h1c, 8'h21,                                  // left playback mixer
        8'h1e, 8'h41,                                  // right playback mixer
        8'h23, 8'he7, 8'h24, 8'he7,                    // headphone L/R
        8'h25, 8'he7, 8'h26, 8'he7,                    // line out L/R
        8'h29, 8'h03,                                  // playback power
        8'h2a, 8'h03,                                  // DAC enable
        8'hf2, 8'h01,                                  // serial input route
        8'hf9, 8'h7f,                                  // clock enable 0
        8'hfa, 8'h03,                                  // clock enable 1
        8'h36, 8'h00, 8'h36, 8'h03                     // dejitter re-init (slave)
    };

    integer errors = 0;

    task fail(input string msg);
        begin
            if (errors < 20) $display("FAIL: %s (t=%0t)", msg, $realtime);
            errors = errors + 1;
        end
    endtask

    // ---------------- I2C slave model ----------------
    integer n_txn = 0, nbits = 0, nbytes = 0, exp_pos = 0;
    logic [8:0] sh = 0;
    logic [7:0] txb [0:15];
    logic in_txn = 0;
    realtime t_scl_rise = 0, t_scl_fall = 0, t_sda_chg = 0, t_start = 0, t_stop = -1e9;
    realtime t_pll_stop = 0, t_after_pll = 0;

    always @(ac_sda) begin
        // Skip the time-0 X to 1 step of the line's initial value.
        if (ac_scl === 1'b1 && $realtime > 0) begin
            if (ac_sda === 1'b0) begin
                // START
                if ($realtime - t_stop < 4700) fail("bus free time < 4.7 us");
                if (led[0] !== 1'b0) fail("led[0] high before the last write");
                if (n_txn == 2) t_after_pll = $realtime;
                in_txn = 1; nbits = 0; nbytes = 0; t_start = $realtime;
            end else begin
                // STOP. The SCL pulse that carries it was sampled as one
                // stray bit, so a complete write ends with nbits == 1.
                if ($realtime - t_scl_rise < 4000) fail("STOP setup < 4.0 us");
                if (!in_txn) fail("STOP outside a transaction");
                else if (n_txn >= N_TXN) fail("extra transaction");
                else if (nbits != 1 || nbytes != txn_len[n_txn] + 2)
                    fail($sformatf("txn %0d has %0d bytes + %0d bits, want %0d bytes",
                                   n_txn, nbytes, nbits - 1, txn_len[n_txn] + 2));
                else begin
                    if (txb[0] !== 8'h76) fail($sformatf("txn %0d address byte %h, want 76", n_txn, txb[0]));
                    if (txb[1] !== 8'h40) fail($sformatf("txn %0d register MSB %h, want 40", n_txn, txb[1]));
                    for (int i = 0; i < txn_len[n_txn]; i++)
                        if (txb[2 + i] !== exp_b[exp_pos + i])
                            fail($sformatf("txn %0d byte %0d is %h, want %h", n_txn, i, txb[2 + i], exp_b[exp_pos + i]));
                    exp_pos = exp_pos + txn_len[n_txn];
                end
                if (n_txn == 1) t_pll_stop = $realtime;
                n_txn = n_txn + 1;
                in_txn = 0; t_stop = $realtime;
            end
        end
        // tVD;DAT / tVD;ACK max (UM10204 Table 11): a change while SCL is low
        // must come within 3.45 us of the SCL fall.
        if (ac_scl === 1'b0 && in_txn && $realtime - t_scl_fall > 3450)
            fail("SDA changed > 3.45 us after SCL fell (tVD;DAT)");
        t_sda_chg = $realtime;
    end

    always @(posedge ac_scl) begin
        t_scl_rise = $realtime;
        if (in_txn) begin
            if ($realtime - t_sda_chg < 250) fail("data setup < 250 ns");
            if ($realtime - t_scl_fall < 4700) fail("SCL low < 4.7 us");
            sh = {sh[7:0], ac_sda};
            nbits = nbits + 1;
            if (nbits == 9) begin
                n_bytes_all = n_bytes_all + 1;
                if (sh[0] !== 1'b0) fail($sformatf("ACK slot high after byte %0d of txn %0d", nbytes, n_txn));
                if (nbytes < 16) txb[nbytes] = sh[8:1];
                nbytes = nbytes + 1;
                nbits = 0;
            end
        end
    end

    always @(negedge ac_scl) begin
        if (in_txn && nbits == 0 && nbytes == 0 && ($realtime - t_start) < 4000)
            fail("START hold < 4.0 us");
        if (in_txn && ($realtime - t_scl_rise) < 4000 && !(nbits == 0 && nbytes == 0))
            fail("SCL high < 4.0 us");
        t_scl_fall = $realtime;
    end

    // SDA hold after SCL falls, and the ACK hand-over after the 8th bit.
    integer n_bytes_all = 0, n_ack_handover = 0;
    realtime t_fall_chk;
    always @(negedge ac_scl) if (in_txn) begin
        t_fall_chk = $realtime;
        #40;
        if (t_sda_chg >= t_fall_chk) fail("SDA changed < 40 ns after SCL fell");
        if (nbits == 8) begin
            #60;
            if (ac_sda !== 1'b0) fail($sformatf("SDA not low 100 ns after the 8th SCL fall of byte %0d, txn %0d", nbytes, n_txn));
            else n_ack_handover = n_ack_handover + 1;
        end
    end

    // ---------------- I2S checker ----------------
    realtime t_mclk = 0, t_bclk = 0, t_data_chg = 0;
    integer n_mclk = 0, n_bclk = 0;

    always @(posedge ac_mclk) begin
        if (n_mclk > 0 && ($realtime - t_mclk) != 80) fail($sformatf("MCLK period %0t", $realtime - t_mclk));
        t_mclk = $realtime; n_mclk = n_mclk + 1;
    end

    always @(ac_lrclk or ac_sdata) begin
        if ($realtime > 0) begin
            if (ac_bclk !== 1'b0) fail("LRCLK/SDATA changed while BCLK high");
            t_data_chg = $realtime;
        end
    end

    integer half_bits = 0, frames = 0, run = 0, flips = 0;
    logic prev_lr = 0;
    logic [31:0] w = 0;
    logic [23:0] left = 0, prev_sample = 0;
    integer synced = 0;
    logic have_left = 0;

    task check_word(input logic lr_was, input logic [31:0] wd);
        logic [23:0] s;
        begin
            s = wd[30:7];
            if (wd[31] !== 1'b0) fail("delay bit not 0");
            if (wd[6:0] !== 7'd0) fail("padding bits not 0");
            if (s !== 24'h100000 && s !== 24'hf00000) fail($sformatf("sample %h", s));
            if (lr_was == 1'b0) begin
                left = s;
                have_left = 1;
            end else if (have_left) begin
                if (s !== left) fail($sformatf("right %h != left %h", s, left));
                frames = frames + 1;
                if (frames > 1 && s !== prev_sample) begin
                    if (flips > 0 && run != 23) fail($sformatf("tone half period %0d frames, want 23", run));
                    flips = flips + 1;
                    run = 1;
                end else begin
                    run = run + 1;
                end
                prev_sample = s;
            end
        end
    endtask

    always @(posedge ac_bclk) begin
        if (n_bclk > 0 && ($realtime - t_bclk) != 340) fail($sformatf("BCLK period %0t", $realtime - t_bclk));
        if ($realtime - t_data_chg < 150) fail("LRCLK/SDATA setup < 150 ns");
        t_bclk = $realtime; n_bclk = n_bclk + 1;
        if (ac_lrclk !== prev_lr) begin
            // The bit sampled now is slot 0 of the new half frame.
            if (synced) begin
                if (half_bits != 32) fail($sformatf("half frame of %0d BCLKs", half_bits));
                check_word(prev_lr, w);
            end
            synced = 1;
            half_bits = 0;
        end
        w = {w[30:0], ac_sdata};
        half_bits = half_bits + 1;
        prev_lr = ac_lrclk;
    end

    logic beat0;
    initial begin
        #200_000;
        beat0 = led[1];
        if (ac_adr0 !== 1'b1 || ac_adr1 !== 1'b1) fail("AC-ADR0/1 not high");
        #400_000_000;
        if (n_txn != N_TXN) fail($sformatf("%0d I2C writes, want %0d", n_txn, N_TXN));
        if (exp_pos != N_EXP) fail($sformatf("%0d register bytes checked, want %0d", exp_pos, N_EXP));
        if (t_after_pll - t_pll_stop < 20_000_000) fail($sformatf("PLL lock wait %0t, want >= 20 ms", t_after_pll - t_pll_stop));
        if (led[0] !== 1'b1) fail("led[0] (init done) not high");
        if (flips < 100) fail($sformatf("only %0d tone flips seen", flips));
        if (led[1] === beat0) fail("heartbeat led[1] never toggled");
        if (n_ack_handover != n_bytes_all || n_bytes_all == 0)
            fail($sformatf("%0d ACK hand-overs checked for %0d bytes", n_ack_handover, n_bytes_all));
        if (errors == 0)
            $display("PASS: audio %0d I2C writes, %0d ACK hand-overs, PLL wait %0.3f ms, %0d frames, %0d tone half periods, last STOP at %0.3f ms",
                     n_txn, n_ack_handover, (t_after_pll - t_pll_stop) / 1.0e6, frames, flips, t_stop / 1.0e6);
        else
            $display("FAIL: audio check, %0d errors", errors);
        $finish;
    end
endmodule
