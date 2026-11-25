// Self-checking testbench for examples/zedboard_hdmi.sv.
//
// I2C side: an ADV7511 slave model samples SDA on each SCL rising edge,
// frames bytes between START and STOP, and checks
//   - every transaction is exactly 3 bytes: 0x72 (address 0x39, write),
//     register, value, and the 41 (register, value) pairs arrive in the
//     order of the expected list below;
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
//   - led[0] stays low until the last STOP and is high after it.
// Video side: samples HD-DE/HSYNC/VSYNC/D on each HD-CLK rising edge and
// checks
//   - HD-CLK period 40 ns (25 MHz);
//   - no video output changes within 15 ns of an HD-CLK rising edge;
//   - 800 clocks per line, HSYNC low for 96 clocks starting 16 clocks
//     after the 640-clock DE run ends;
//   - 525 lines per frame, VSYNC low for 2 lines, 480 DE lines per frame;
//   - vertical porches: the first DE line starts 35 lines (sync 2 + back
//     porch 33) after VSYNC falls, and VSYNC falls 10 lines (front porch)
//     after the last DE line ends; lines are counted by HSYNC falls;
//   - every active pixel carries {C, Y} of its colour bar with Cb on even
//     and Cr on odd pixels; blanking carries 0x8010;
//   - led[1] (heartbeat) toggles.
// Pass prints PASS and finishes; any failure prints FAIL.
//
// Run with Verilator:
//   $ verilator --binary --timing --top-module zedboard_hdmi_tb \
//     examples/zedboard_hdmi_tb.sv examples/zedboard_hdmi.sv
`timescale 1ns / 1ps

module zedboard_hdmi_tb;
    logic clk = 0;
    logic hd_clk, hd_de, hd_hsync, hd_vsync, hd_scl, hd_sda;
    logic [15:0] hd_d;
    logic [1:0] led;

    always #5 clk = ~clk; // 100 MHz

    zedboard_hdmi dut (
        .clk(clk), .hd_clk(hd_clk), .hd_d(hd_d), .hd_de(hd_de),
        .hd_hsync(hd_hsync), .hd_vsync(hd_vsync),
        .hd_scl(hd_scl), .hd_sda(hd_sda), .led(led)
    );

    // Expected ADV7511 writes {register, value}, in order. See the header
    // of zedboard_hdmi.sv for where each one comes from.
    localparam int N_WR = 41;
    logic [15:0] exp_wr [0:N_WR-1] = '{
        16'hd6c0, 16'h4110,
        16'h9803, 16'h9ae0, 16'h9c30, 16'h9d61, 16'ha2a4, 16'ha3a4,
        16'he0d0, 16'hf900, 16'h5502,
        16'h1501, 16'h1638, 16'h4808, 16'hba60,
        16'h1a24, 16'h18c7, 16'h1934, 16'h1bad, 16'h1c00, 16'h1d00,
        16'h1e1c, 16'h1f1b, 16'h201d, 16'h21dc, 16'h2204, 16'h23ad,
        16'h241f, 16'h2524, 16'h2601, 16'h2735, 16'h2800, 16'h2900,
        16'h2a04, 16'h2bad, 16'h2c08, 16'h2d7c, 16'h2e1b, 16'h2f77,
        16'h1a04,
        16'haf14
    };

    // Bar colours {Cb, Cr, Y}, BT.709 limited range (matches the BT.709
    // CSC table the design loads; see the zedboard_hdmi.sv header).
    logic [23:0] bars [0:7] = '{
        24'h8080eb, 24'h108adb, 24'h9a10bc, 24'h2a1aad,
        24'hd6e64e, 24'h66f03f, 24'hf07620, 24'h808010
    };

    integer errors = 0;

    // ---------------- I2C slave model ----------------
    integer n_txn = 0, nbits = 0, nbytes = 0;
    logic [8:0] sh = 0;
    logic [7:0] txb [0:2];
    logic in_txn = 0;
    realtime t_scl_rise = 0, t_scl_fall = 0, t_sda_chg = 0, t_start = 0, t_stop = -1e9;
    realtime t_last_stop = 0;

    task fail(input string msg);
        begin
            if (errors < 20) $display("FAIL: %s (t=%0t)", msg, $realtime);
            errors = errors + 1;
        end
    endtask

    always @(hd_sda) begin
        // Skip the time-0 X to 1 step of the line's initial value.
        if (hd_scl === 1'b1 && $realtime > 0) begin
            if (hd_sda === 1'b0) begin
                // START
                if ($realtime - t_stop < 4700) fail("bus free time < 4.7 us");
                if (led[0] !== 1'b0) fail("led[0] high before the last write");
                in_txn = 1; nbits = 0; nbytes = 0; t_start = $realtime;
            end else begin
                // STOP. The SCL pulse that carries it was sampled as one
                // stray bit, so a complete write ends with nbits == 1.
                if ($realtime - t_scl_rise < 4000) fail("STOP setup < 4.0 us");
                if (!in_txn) fail("STOP outside a transaction");
                else if (nbits != 1 || nbytes != 3) fail($sformatf("transaction %0d has %0d bytes + %0d bits", n_txn, nbytes, nbits));
                else begin
                    if (txb[0] !== 8'h72) fail($sformatf("txn %0d address byte %h, want 72", n_txn, txb[0]));
                    if (n_txn >= N_WR) fail("extra transaction");
                    else if ({txb[1], txb[2]} !== exp_wr[n_txn])
                        fail($sformatf("txn %0d wrote %h=%h, want %h=%h", n_txn, txb[1], txb[2],
                                       exp_wr[n_txn][15:8], exp_wr[n_txn][7:0]));
                end
                n_txn = n_txn + 1;
                in_txn = 0; t_stop = $realtime; t_last_stop = $realtime;
            end
        end
        // tVD;DAT / tVD;ACK max (UM10204 Table 11): a change while SCL is low
        // must come within 3.45 us of the SCL fall.
        if (hd_scl === 1'b0 && in_txn && $realtime - t_scl_fall > 3450)
            fail("SDA changed > 3.45 us after SCL fell (tVD;DAT)");
        t_sda_chg = $realtime;
    end

    always @(posedge hd_scl) begin
        t_scl_rise = $realtime;
        if (in_txn) begin
            if ($realtime - t_sda_chg < 250) fail("data setup < 250 ns");
            if ($realtime - t_scl_fall < 4700) fail("SCL low < 4.7 us");
            sh = {sh[7:0], hd_sda};
            nbits = nbits + 1;
            if (nbits == 9) begin
                n_bytes_all = n_bytes_all + 1;
                if (sh[0] !== 1'b0) fail($sformatf("ACK slot high after byte %0d of txn %0d", nbytes, n_txn));
                if (nbytes < 3) txb[nbytes] = sh[8:1];
                nbytes = nbytes + 1;
                nbits = 0;
            end
        end
    end

    always @(negedge hd_scl) begin
        if (in_txn && nbits == 0 && nbytes == 0 && ($realtime - t_start) < 4000)
            fail("START hold < 4.0 us");
        if (in_txn && ($realtime - t_scl_rise) < 4000 && !(nbits == 0 && nbytes == 0))
            fail("SCL high < 4.0 us");
        t_scl_fall = $realtime;
    end

    // SDA hold after SCL falls, and the ACK hand-over after the 8th bit.
    integer n_bytes_all = 0, n_ack_handover = 0;
    realtime t_fall_chk;
    always @(negedge hd_scl) if (in_txn) begin
        t_fall_chk = $realtime;
        #40;
        if (t_sda_chg >= t_fall_chk) fail("SDA changed < 40 ns after SCL fell");
        if (nbits == 8) begin
            #60;
            if (hd_sda !== 1'b0) fail($sformatf("SDA not low 100 ns after the 8th SCL fall of byte %0d, txn %0d", nbytes, n_txn));
            else n_ack_handover = n_ack_handover + 1;
        end
    end

    // ---------------- Video checker ----------------
    realtime t_vid_chg = 0, t_clk_rise = 0;
    always @(hd_d or hd_de or hd_hsync or hd_vsync) begin
        if ($realtime > 100 && $realtime - t_clk_rise < 15) fail("video output changed < 15 ns after HD-CLK rise");
        t_vid_chg = $realtime;
    end

    integer clk_n = 0;          // HD-CLK rising edges seen
    integer line_clk = 0;       // clocks since the last HSYNC falling edge
    integer hs_low = 0, de_run = 0, since_de = -1, x = 0;
    integer lines_in_frame = 0, de_lines = 0, vs_low_clks = 0, frames = 0;
    integer line_had_de = 0;
    logic prev_hs = 1, prev_vs = 1, prev_de = 0;
    integer synced = 0;         // set after the first HSYNC falling edge
    integer vsynced = 0;        // set after the first VSYNC falling edge
    integer first_de = -1, last_de = -1; // lines_in_frame at DE line starts

    always @(posedge hd_clk) begin
        if (clk_n > 0 && ($realtime - t_clk_rise) != 40) fail($sformatf("HD-CLK period %0t", $realtime - t_clk_rise));
        if ($realtime - t_vid_chg < 15) fail("video output changed < 15 ns before HD-CLK rise");
        t_clk_rise = $realtime;
        clk_n = clk_n + 1;

        // Pixel data.
        if (hd_de === 1'b1) begin
            if (synced && vsynced) begin
                logic [23:0] c;
                c = bars[x / 80];
                if (hd_d !== {(x % 2) ? c[15:8] : c[23:16], c[7:0]})
                    fail($sformatf("pixel x=%0d line %0d data %h", x, lines_in_frame, hd_d));
            end
            x = x + 1;
        end else if (hd_d !== 16'h8010) begin
            fail($sformatf("blanking data %h", hd_d));
        end

        // DE runs.
        if (hd_de === 1'b1 && prev_de !== 1'b1) begin
            de_run = 1; line_had_de = 1;
            if (first_de < 0) first_de = lines_in_frame;
            last_de = lines_in_frame;
        end
        else if (hd_de === 1'b1) de_run = de_run + 1;
        if (hd_de !== 1'b1 && prev_de === 1'b1) begin
            if (de_run != 640) fail($sformatf("DE run %0d, want 640", de_run));
            since_de = 0;
            x = 0;
        end else if (since_de >= 0) since_de = since_de + 1;

        // HSYNC.
        line_clk = line_clk + 1;
        if (hd_hsync === 1'b0 && prev_hs === 1'b1) begin
            if (synced && line_clk != 800) fail($sformatf("line length %0d, want 800", line_clk));
            if (synced && since_de != 16 && line_had_de) fail($sformatf("HSYNC %0d clocks after DE, want 16", since_de));
            if (line_had_de) de_lines = de_lines + 1;
            line_had_de = 0;
            synced = 1; line_clk = 0; hs_low = 1;
            lines_in_frame = lines_in_frame + 1;
        end else if (hd_hsync === 1'b0) hs_low = hs_low + 1;
        if (hd_hsync === 1'b1 && prev_hs === 1'b0 && synced && hs_low != 96)
            fail($sformatf("HSYNC low %0d clocks, want 96", hs_low));

        // VSYNC.
        if (hd_vsync === 1'b0 && prev_vs === 1'b1) begin
            if (vsynced) begin
                if (lines_in_frame != 525) fail($sformatf("frame has %0d lines, want 525", lines_in_frame));
                if (de_lines != 480) fail($sformatf("frame has %0d DE lines, want 480", de_lines));
                // VSYNC falls at a line start, before that line's HSYNC, so
                // lines_in_frame counts the 2 sync lines and the back porch
                // by the time the first DE line starts.
                if (first_de != 35) fail($sformatf("first DE line %0d lines after VSYNC fall, want 35 (2 + 33)", first_de));
                if (lines_in_frame - last_de - 1 != 10)
                    fail($sformatf("front porch %0d lines, want 10", lines_in_frame - last_de - 1));
                frames = frames + 1;
            end
            vsynced = 1; lines_in_frame = 0; de_lines = 0; vs_low_clks = 1;
            first_de = -1; last_de = -1;
        end else if (hd_vsync === 1'b0) vs_low_clks = vs_low_clks + 1;
        if (hd_vsync === 1'b1 && prev_vs === 1'b0 && vsynced && vs_low_clks != 1600)
            fail($sformatf("VSYNC low %0d clocks, want 1600", vs_low_clks));

        prev_hs = hd_hsync; prev_vs = hd_vsync; prev_de = hd_de;
    end

    logic beat0;
    initial begin
        #200_000;
        beat0 = led[1];
        // Two full frames are about 33.6 ms; the I2C init takes about 24 ms.
        #600_000_000;
        if (n_txn != N_WR) fail($sformatf("%0d I2C writes, want %0d", n_txn, N_WR));
        if (led[0] !== 1'b1) fail("led[0] (init done) not high");
        if (frames < 30) fail($sformatf("only %0d full frames checked", frames));
        if (led[1] === beat0) fail("heartbeat led[1] never toggled");
        if (n_ack_handover != n_bytes_all || n_bytes_all == 0)
            fail($sformatf("%0d ACK hand-overs checked for %0d bytes", n_ack_handover, n_bytes_all));
        if (errors == 0)
            $display("PASS: hdmi %0d I2C writes, %0d ACK hand-overs, %0d frames checked, last STOP at %0.3f ms",
                     n_txn, n_ack_handover, frames, t_last_stop / 1.0e6);
        else
            $display("FAIL: hdmi check, %0d errors", errors);
        $finish;
    end
endmodule
