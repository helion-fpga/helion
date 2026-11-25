// Self-checking testbench for examples/zedboard_oled.sv.
//
// Models the SSD1306 SPI slave: samples SDIN on the rising edge of SCLK,
// assembles bytes, and checks:
//   - power order: VDD on (low) first, RES low then high, VBAT on (low)
//     only after RES goes high, and at least 99 ms from VBAT on to the
//     first SCLK rising edge;
//   - D/C low for all 25 init bytes, high for all 512 display bytes, and
//     constant within each byte;
//   - the 25 init bytes match the expected command list exactly;
//   - the 512 display bytes match the alternating 0xAA/0x55 stripe pattern;
//   - SDIN is stable for at least 100 ns before every SCLK rising edge
//     (the bit-banged link has a full 1 us half period, so this has margin).
// Pass prints PASS and finishes; any failure prints FAIL with the first
// mismatch and finishes.
//
// Run with Verilator:
//   $ verilator --binary -j 1 --top-module zedboard_oled_tb \
//     examples/zedboard_oled_tb.sv examples/zedboard_oled.sv
// or with iverilog:
//   $ iverilog -g2012 -o oled_tb.vvp \
//     examples/zedboard_oled_tb.sv examples/zedboard_oled.sv && vvp oled_tb.vvp
// (the $ keeps Verilator from reading a command line as a directive:
// a comment starting with `verilator` after the // is a Verilator pragma.)
// No simulator is installed on the Linux sandbox, so this is run on the Mac.
`timescale 1ns / 1ps

module zedboard_oled_tb;
    logic clk = 0;
    logic oled_dc, oled_res, oled_sclk, oled_sdin, oled_vbat, oled_vdd, led;

    always #5 clk = ~clk; // 100 MHz

    zedboard_oled dut (
        .clk(clk),
        .oled_dc(oled_dc),
        .oled_res(oled_res),
        .oled_sclk(oled_sclk),
        .oled_sdin(oled_sdin),
        .oled_vbat(oled_vbat),
        .oled_vdd(oled_vdd),
        .led(led)
    );

    // Expected init bytes, in order.
    logic [7:0] init_exp [0:24];
    initial begin
        init_exp[0]  = 8'hae;
        init_exp[1]  = 8'hd5;
        init_exp[2]  = 8'h80;
        init_exp[3]  = 8'ha8;
        init_exp[4]  = 8'h1f;
        init_exp[5]  = 8'hd3;
        init_exp[6]  = 8'h00;
        init_exp[7]  = 8'h40;
        init_exp[8]  = 8'h8d;
        init_exp[9]  = 8'h14;
        init_exp[10] = 8'h20;
        init_exp[11] = 8'h00;
        init_exp[12] = 8'ha1;
        init_exp[13] = 8'hc8;
        init_exp[14] = 8'hda;
        init_exp[15] = 8'h02;
        init_exp[16] = 8'h81;
        init_exp[17] = 8'h8f;
        init_exp[18] = 8'hd9;
        init_exp[19] = 8'hf1;
        init_exp[20] = 8'hdb;
        init_exp[21] = 8'h40;
        init_exp[22] = 8'ha4;
        init_exp[23] = 8'ha6;
        init_exp[24] = 8'haf;
    end

    integer errors = 0;
    integer byte_cnt = 0;  // bytes fully received
    integer bit_cnt = 0;   // bits received in the current byte
    integer wait_us = 0;   // watchdog counter, in microseconds
    logic [7:0] rx_byte = 8'h00;
    logic rx_dc = 1'b0;    // D/C sampled at the first rising edge of a byte
    logic res_was_low = 1'b0;
    time last_sdin_change = 0;
    time vbat_on_time = 0;
    time first_sclk_time = 0;

    always @(oled_sdin) last_sdin_change = $time;
    // RES starts low, so its negedge never fires; sample the level instead.
    always @(posedge clk) if (oled_res === 1'b0) res_was_low = 1'b1;

    // Power order: VDD on while VBAT is still off, RES high before VBAT on.
    initial begin
        wait (oled_vdd === 1'b0);
        if (oled_vbat !== 1'b1) begin
            $display("FAIL: VBAT not off (high) when VDD switched on");
            errors = errors + 1;
        end
        wait (oled_res === 1'b1);
        if (!res_was_low) begin
            $display("FAIL: RES never went low before going high");
            errors = errors + 1;
        end
        if (oled_vbat !== 1'b1) begin
            $display("FAIL: VBAT switched on before RES went high");
            errors = errors + 1;
        end
        wait (oled_vbat === 1'b0);
        vbat_on_time = $time;
    end

    task check_byte(input logic [7:0] b, input logic dc, input integer idx);
        logic [7:0] exp;
        logic exp_dc;
        begin
            if (idx < 25) begin
                exp = init_exp[idx];
                exp_dc = 1'b0;
            end else if (idx < 537) begin
                exp = (((idx - 25) >> 5) & 1) != 0 ? 8'h55 : 8'haa;
                exp_dc = 1'b1;
            end else begin
                $display("FAIL: extra byte %0d received: %h", idx, b);
                errors = errors + 1;
                exp = b;
                exp_dc = dc;
            end
            if (dc !== exp_dc) begin
                $display("FAIL: byte %0d D/C %b, expected %b", idx, dc, exp_dc);
                errors = errors + 1;
            end
            if (b !== exp) begin
                $display("FAIL: byte %0d is %h, expected %h", idx, b, exp);
                errors = errors + 1;
            end
        end
    endtask

    // SSD1306 SPI slave model: sample SDIN on the rising edge of SCLK.
    always @(posedge oled_sclk) begin
        if (first_sclk_time == 0) first_sclk_time = $time;
        if (($time - last_sdin_change) < 100) begin
            $display("FAIL: SDIN changed %0t ns before SCLK rising edge of byte %0d bit %0d",
                     $time - last_sdin_change, byte_cnt, bit_cnt);
            errors = errors + 1;
        end
        if (bit_cnt == 0) begin
            rx_dc = oled_dc;
        end else if (oled_dc !== rx_dc) begin
            $display("FAIL: D/C changed mid-byte %0d", byte_cnt);
            errors = errors + 1;
        end
        rx_byte = {rx_byte[6:0], oled_sdin};
        bit_cnt = bit_cnt + 1;
        if (bit_cnt == 8) begin
            bit_cnt = 0;
            check_byte(rx_byte, rx_dc, byte_cnt);
            byte_cnt = byte_cnt + 1;
        end
    end

    // Main check: wait for led with a 300 ms watchdog, then the totals.
    initial begin
        wait_us = 0;
        while (led !== 1'b1 && wait_us < 300_000) begin
            #1000; // 1 us steps
            wait_us = wait_us + 1;
        end
        if (led !== 1'b1) begin
            $display("FAIL: timeout after 300 ms, led never went high, bytes received %0d", byte_cnt);
            errors = errors + 1;
        end else begin
            #1000;
            if (byte_cnt != 537) begin
                $display("FAIL: led high but byte count %0d, expected 537", byte_cnt);
                errors = errors + 1;
            end
        end
        if (vbat_on_time == 0 || first_sclk_time == 0) begin
            $display("FAIL: VBAT-on or first-SCLK timestamp missing");
            errors = errors + 1;
        end else if ((first_sclk_time - vbat_on_time) < 99_000_000) begin
            $display("FAIL: first SCLK edge only %0t ns after VBAT on, need 100 ms",
                     first_sclk_time - vbat_on_time);
            errors = errors + 1;
        end
        if (errors == 0)
            $display("PASS: oled init and data check, 537 bytes, D/C and setup OK");
        else
            $display("FAIL: oled check, %0d errors", errors);
        $finish;
    end
endmodule
