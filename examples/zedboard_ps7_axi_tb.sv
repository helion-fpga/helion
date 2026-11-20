// RTL test of examples/zedboard_ps7_axi.sv. Simulation only: this file
// holds a PS7 boundary model (FCLK, FCLK_RESET0_N and an AXI3 master on
// M_AXI_GP0, with only the ports the design connects) and a BUFG; never
// give it to synthesis.
//
//   Icarus:    iverilog -g2012 -o tb examples/zedboard_ps7_axi.sv examples/zedboard_ps7_axi_tb.sv && vvp tb
//   With Verilator 5: --binary --timing -Wno-fatal --top-module zedboard_ps7_axi_tb examples/zedboard_ps7_axi.sv examples/zedboard_ps7_axi_tb.sv,
//   then obj_dir/Vzedboard_ps7_axi_tb
//
// The master acts at FCLK edges on the slave's signals as they were half
// a cycle before, and drives its own 1 ns after the edge. Monitors check the slave's side of the
// protocol on every edge: a VALID once raised stays until taken with its
// payload unchanged, READY stays low in reset. Prints PASS or FAIL and the
// error count.
`timescale 1ns / 1ps

module BUFG (input I, output O);
    assign O = I;
endmodule

module PS7 (
    output logic [3:0] FCLKCLK,
    output logic [3:0] FCLKRESETN,
    input  logic MAXIGP0ACLK,
    output logic MAXIGP0ARVALID,
    input  logic MAXIGP0ARREADY,
    output logic [31:0] MAXIGP0ARADDR,
    output logic [11:0] MAXIGP0ARID,
    output logic [3:0] MAXIGP0ARLEN,
    output logic MAXIGP0AWVALID,
    input  logic MAXIGP0AWREADY,
    output logic [31:0] MAXIGP0AWADDR,
    output logic [11:0] MAXIGP0AWID,
    output logic [3:0] MAXIGP0AWLEN,
    output logic MAXIGP0WVALID,
    input  logic MAXIGP0WREADY,
    output logic [31:0] MAXIGP0WDATA,
    output logic [3:0] MAXIGP0WSTRB,
    output logic MAXIGP0WLAST,
    input  logic MAXIGP0BVALID,
    output logic MAXIGP0BREADY,
    input  logic [11:0] MAXIGP0BID,
    input  logic [1:0] MAXIGP0BRESP,
    input  logic MAXIGP0RVALID,
    output logic MAXIGP0RREADY,
    input  logic [31:0] MAXIGP0RDATA,
    input  logic [11:0] MAXIGP0RID,
    input  logic [1:0] MAXIGP0RRESP,
    input  logic MAXIGP0RLAST
);
    integer errors = 0;
    integer cyc = 0;
    integer ar_cyc = 0; // cycle of the last AR handshake
    // The master runs on FCLK itself, not on MAXIGP0ACLK (FCLK after the
    // design's BUFG): it samples at FCLK's edge, before any flop of the
    // design has moved, and drives 1 ns after it, so neither simulator's
    // ordering of the derived clock can race it.
    logic clk;
    assign clk = FCLKCLK[0];

    initial begin
        FCLKCLK = 4'b0;
        FCLKRESETN = 4'b0;
        MAXIGP0ARVALID = 0; MAXIGP0AWVALID = 0; MAXIGP0WVALID = 0;
        MAXIGP0BREADY = 0; MAXIGP0RREADY = 0;
        MAXIGP0ARADDR = 0; MAXIGP0ARID = 0; MAXIGP0ARLEN = 0;
        MAXIGP0AWADDR = 0; MAXIGP0AWID = 0; MAXIGP0AWLEN = 0;
        MAXIGP0WDATA = 0; MAXIGP0WSTRB = 0; MAXIGP0WLAST = 0;
    end
    always #5 FCLKCLK[0] = ~FCLKCLK[0]; // 100 MHz
    always @(posedge clk) cyc <= cyc + 1;
    // The slave's outputs half a cycle before each FCLK edge: what the
    // design's flops and this master see at that edge. The master reads
    // these, not the live ports, so no simulator's ordering of the edge
    // against the design's own flops can race it.
    logic s_awready, s_wready, s_arready, s_bvalid, s_rvalid, s_rlast, s_bready, s_rready;
    logic [11:0] s_bid, s_rid;
    logic [1:0] s_bresp, s_rresp;
    logic [31:0] s_rdata;
    always @(posedge clk) if (MAXIGP0ARVALID && s_arready) ar_cyc <= cyc;

    task automatic fail(input string what);
        errors = errors + 1;
        $display("FAIL at %0t: %s", $time, what);
    endtask

    task automatic expect32(input string what, input logic [31:0] got, input logic [31:0] want);
        if (got !== want) fail($sformatf("%s: got %h, want %h", what, got, want));
    endtask

    // Monitor of the slave's side: a VALID not taken at the edge is still
    // there, with the same payload, half a cycle after it.
    always @(negedge clk) begin
        if (FCLKRESETN[0] && s_bvalid && !s_bready && !(MAXIGP0BVALID === 1'b1 && {MAXIGP0BID, MAXIGP0BRESP} === {s_bid, s_bresp}))
            fail("BVALID dropped or B payload changed before BREADY");
        if (FCLKRESETN[0] && s_rvalid && !s_rready
            && !(MAXIGP0RVALID === 1'b1 && {MAXIGP0RID, MAXIGP0RRESP, MAXIGP0RLAST, MAXIGP0RDATA} === {s_rid, s_rresp, s_rlast, s_rdata}))
            fail("RVALID dropped or R payload changed before RREADY");
        if (FCLKRESETN[0] && (MAXIGP0BVALID === 1'bx || MAXIGP0RVALID === 1'bx)) fail("BVALID or RVALID unknown out of reset");
        {s_awready, s_wready, s_arready, s_bvalid, s_rvalid, s_rlast} = {MAXIGP0AWREADY, MAXIGP0WREADY, MAXIGP0ARREADY, MAXIGP0BVALID, MAXIGP0RVALID, MAXIGP0RLAST};
        {s_bid, s_rid, s_bresp, s_rresp, s_rdata} = {MAXIGP0BID, MAXIGP0RID, MAXIGP0BRESP, MAXIGP0RRESP, MAXIGP0RDATA};
        {s_bready, s_rready} = {MAXIGP0BREADY, MAXIGP0RREADY};
    end

    // Every drive happens 1 ns after an edge (blocking, so a task ending
    // and the next one starting in the same instant cannot reorder them).
    task automatic reset_pl(input integer cycles);
        @(posedge clk);
        #1 FCLKRESETN = 4'b0;
        // The design sees it two flops later.
        repeat (3) @(posedge clk);
        repeat (cycles) begin
            @(posedge clk);
            if (s_arready || s_awready || s_wready) fail("READY high in reset");
            if (s_bvalid || s_rvalid) fail("VALID high in reset");
        end
        #1 FCLKRESETN = 4'hF;
        repeat (3) @(posedge clk);
    endtask

    // One beat on each channel; `delay` idle cycles first. VALID drops 1 ns
    // after the handshake, so a send that follows at once raises it again
    // before the next edge (back to back).
    task automatic aw_send(input logic [11:0] id, input logic [31:0] addr, input logic [3:0] len, input integer delay);
        repeat (delay) @(posedge clk);
        #1 {MAXIGP0AWVALID, MAXIGP0AWID, MAXIGP0AWADDR, MAXIGP0AWLEN} = {1'b1, id, addr, len};
        @(posedge clk);
        while (!s_awready) @(posedge clk);
        #1 MAXIGP0AWVALID = 0;
    endtask

    task automatic w_send(input logic [31:0] data, input logic [3:0] strb, input logic last, input integer delay);
        repeat (delay) @(posedge clk);
        #1 {MAXIGP0WVALID, MAXIGP0WDATA, MAXIGP0WSTRB, MAXIGP0WLAST} = {1'b1, data, strb, last};
        @(posedge clk);
        while (!s_wready) @(posedge clk);
        #1 MAXIGP0WVALID = 0;
    endtask

    task automatic ar_send(input logic [11:0] id, input logic [31:0] addr, input logic [3:0] len, input integer delay);
        repeat (delay) @(posedge clk);
        #1 {MAXIGP0ARVALID, MAXIGP0ARID, MAXIGP0ARADDR, MAXIGP0ARLEN} = {1'b1, id, addr, len};
        @(posedge clk);
        while (!s_arready) @(posedge clk);
        #1 MAXIGP0ARVALID = 0;
    endtask

    // Take one response; BREADY/RREADY rise `delay` cycles after VALID is
    // seen (0: READY is already high before VALID).
    task automatic b_recv(input integer delay, output logic [11:0] id, output logic [1:0] resp);
        integer n;
        logic done;
        n = 0;
        done = 0;
        #1 MAXIGP0BREADY = (delay == 0);
        while (!done) begin
            @(posedge clk);
            if (s_bvalid && MAXIGP0BREADY) begin
                id = s_bid;
                resp = s_bresp;
                done = 1;
                #1 MAXIGP0BREADY = 0;
            end else if (s_bvalid) begin
                n = n + 1;
                if (n >= delay) #1 MAXIGP0BREADY = 1;
            end
        end
    endtask

    task automatic r_recv(input integer delay, output logic [11:0] id, output logic [31:0] data, output logic [1:0] resp, output logic last);
        integer n;
        logic done;
        n = 0;
        done = 0;
        #1 MAXIGP0RREADY = (delay == 0);
        while (!done) begin
            @(posedge clk);
            if (s_rvalid && MAXIGP0RREADY) begin
                id = s_rid;
                data = s_rdata;
                resp = s_rresp;
                last = s_rlast;
                done = 1;
                #1 MAXIGP0RREADY = 0;
            end else if (s_rvalid) begin
                n = n + 1;
                if (n >= delay) #1 MAXIGP0RREADY = 1;
            end
        end
    endtask

    // A single-beat write: AW after `aw_delay` cycles, W after `w_delay`,
    // BREADY `b_delay` cycles after BVALID. Checks BID.
    task automatic write(input logic [31:0] addr, input logic [31:0] data, input logic [3:0] strb, input logic [11:0] id,
                         input integer aw_delay, input integer w_delay, input integer b_delay, output logic [1:0] resp);
        logic [11:0] bid;
        fork
            aw_send(id, addr, 4'd0, aw_delay);
            w_send(data, strb, 1'b1, w_delay);
            b_recv(b_delay, bid, resp);
        join
        if (bid !== id) fail($sformatf("BID %h for AWID %h", bid, id));
    endtask

    task automatic read(input logic [31:0] addr, input logic [11:0] id, input integer r_delay, output logic [31:0] data, output logic [1:0] resp);
        logic [11:0] rid;
        logic last;
        fork
            ar_send(id, addr, 4'd0, 0);
            r_recv(r_delay, rid, data, resp, last);
        join
        if (rid !== id) fail($sformatf("RID %h for ARID %h", rid, id));
        if (last !== 1'b1) fail("RLAST low on a single-beat read");
    endtask

    task automatic write_ok(input logic [31:0] addr, input logic [31:0] data, input logic [3:0] strb);
        logic [1:0] resp;
        write(addr, data, strb, 12'h0F0, 0, 0, 0, resp);
        if (resp !== 2'b00) fail($sformatf("write %h: BRESP %b, want OKAY", addr, resp));
    endtask

    task automatic read_expect(input logic [31:0] addr, input logic [31:0] want, input logic [1:0] want_resp);
        logic [31:0] data;
        logic [1:0] resp;
        read(addr, 12'h70F, 0, data, resp);
        if (resp !== want_resp) fail($sformatf("read %h: RRESP %b, want %b", addr, resp, want_resp));
        expect32($sformatf("read %h", addr), data, want);
    endtask
endmodule

module zedboard_ps7_axi_tb;
    logic [7:0] led;
    zedboard_ps7_axi dut (.led(led));

    localparam logic [31:0] MAGIC = 32'h4000_0000, SCRATCH = 32'h4000_0004, COUNTER = 32'h4000_0008, LEDS = 32'h4000_000C;
    localparam logic [1:0] OKAY = 2'b00, SLVERR = 2'b10;

    logic [31:0] data, c1, c2;
    logic [1:0] resp, r1, r2;
    logic [11:0] id, i1, i2;
    logic last;
    integer a1, i, j, k, t_aw, t_w, t_b;

    initial begin
        dut.ps7.reset_pl(10);

        // Registers after reset.
        dut.ps7.read_expect(MAGIC, 32'h484C_0701, OKAY);
        dut.ps7.read_expect(SCRATCH, 32'h0, OKAY);
        dut.ps7.read_expect(LEDS, 32'h0, OKAY);
        if (led[6:0] !== 7'h0) dut.ps7.fail("LEDs not 0 after reset");

        // RID echoes ARID.
        dut.ps7.read(MAGIC, 12'hABC, 0, data, resp);
        dut.ps7.expect32("MAGIC with id ABC", data, 32'h484C_0701);

        // Full write; then partial strobes, with AW and W in both orders and
        // a slow BREADY.
        dut.ps7.write(SCRATCH, 32'hDEAD_BEEF, 4'hF, 12'h5A5, 0, 0, 0, resp);
        if (resp !== OKAY) dut.ps7.fail("scratch write not OKAY");
        dut.ps7.read_expect(SCRATCH, 32'hDEAD_BEEF, OKAY);
        dut.ps7.write(SCRATCH, 32'h1122_3344, 4'b0101, 12'h001, 3, 0, 0, resp); // W first
        dut.ps7.read_expect(SCRATCH, 32'hDE22_BE44, OKAY);
        dut.ps7.write(SCRATCH, 32'hAA00_0000, 4'b1000, 12'hFFF, 0, 4, 5, resp); // AW first, BREADY late
        if (resp !== OKAY) dut.ps7.fail("strobed write not OKAY");
        dut.ps7.read_expect(SCRATCH, 32'hAA22_BE44, OKAY);
        dut.ps7.write_ok(SCRATCH, 32'hFFFF_FFFF, 4'b0000);
        dut.ps7.read_expect(SCRATCH, 32'hAA22_BE44, OKAY);
        dut.ps7.write_ok(SCRATCH, 32'h0000_7700, 4'b0010);
        dut.ps7.read_expect(SCRATCH, 32'hAA22_7744, OKAY);

        // LED register: 7 bits, byte lane 0 only.
        dut.ps7.write_ok(LEDS, 32'hFFFF_FFD5, 4'b0001);
        dut.ps7.read_expect(LEDS, 32'h0000_0055, OKAY);
        if (led[6:0] !== 7'h55) dut.ps7.fail($sformatf("LEDs %h, want 55", led[6:0]));
        dut.ps7.write_ok(LEDS, 32'h0000_0000, 4'b1110);
        dut.ps7.read_expect(LEDS, 32'h0000_0055, OKAY);

        // Counter: two reads differ by the cycles between their AR handshakes.
        dut.ps7.read(COUNTER, 12'h010, 0, c1, resp);
        a1 = dut.ps7.ar_cyc;
        // Free-running from configuration: the count is the start plus the edges before the read.
        if (c1 !== 32'hFFFF_FC00 + a1) dut.ps7.fail($sformatf("counter read %0d at cycle %0d", c1, a1));
        repeat (17) @(posedge dut.clk);
        dut.ps7.read(COUNTER, 12'h011, 2, c2, resp);
        if (c2 - c1 !== dut.ps7.ar_cyc - a1) dut.ps7.fail($sformatf("counter moved %0d in %0d cycles", c2 - c1, dut.ps7.ar_cyc - a1));
        if (resp !== OKAY) dut.ps7.fail("counter read not OKAY");

        // Heartbeat: LD7 is counter bit 26 (deposit the counter just below it).
        #1 dut.cnt = 32'h03FF_FFF8; // between edges, not racing the flop's own update
        @(posedge dut.clk);
        if (led[7] !== 1'b0) dut.ps7.fail("LD7 high below bit 26");
        repeat (10) @(posedge dut.clk);
        #1;
        if (led[7] !== 1'b1) dut.ps7.fail("LD7 did not rise with counter bit 26");

        // Read-only registers refuse writes; unmapped addresses answer SLVERR.
        dut.ps7.write(MAGIC, 32'h0, 4'hF, 12'h020, 0, 0, 0, resp);
        if (resp !== SLVERR) dut.ps7.fail("write to MAGIC not SLVERR");
        dut.ps7.read_expect(MAGIC, 32'h484C_0701, OKAY);
        dut.ps7.write(COUNTER, 32'h0, 4'hF, 12'h021, 0, 0, 0, resp);
        if (resp !== SLVERR) dut.ps7.fail("write to COUNTER not SLVERR");
        dut.ps7.read_expect(32'h4000_0010, 32'h0, SLVERR);
        dut.ps7.read_expect(32'h4000_0FFC, 32'h0, SLVERR);
        dut.ps7.read_expect(32'h4000_1004, 32'h0, SLVERR);
        dut.ps7.read_expect(32'h7FFF_FFFC, 32'h0, SLVERR);
        dut.ps7.write(32'h4000_1004, 32'h1234_5678, 4'hF, 12'h022, 0, 0, 0, resp);
        if (resp !== SLVERR) dut.ps7.fail("write to an unmapped address not SLVERR");
        dut.ps7.read_expect(SCRATCH, 32'hAA22_7744, OKAY);

        // A 4-beat read burst: four SLVERR beats, RLAST on the last only.
        fork
            dut.ps7.ar_send(12'h3C3, SCRATCH, 4'd3, 0);
            for (i = 0; i < 4; i = i + 1) begin
                dut.ps7.r_recv(i, id, data, resp, last);
                if (id !== 12'h3C3 || resp !== SLVERR || data !== 32'h0 || last !== (i == 3))
                    dut.ps7.fail($sformatf("burst beat %0d: id %h resp %b data %h last %b", i, id, resp, data, last));
            end
        join
        // A 2-beat write burst: both beats taken, one SLVERR, no write.
        fork
            dut.ps7.aw_send(12'h3C4, SCRATCH, 4'd1, 0);
            begin
                dut.ps7.w_send(32'h0BAD_0BAD, 4'hF, 1'b0, 0);
                dut.ps7.w_send(32'h0BAD_0BAD, 4'hF, 1'b1, 2);
            end
            begin
                dut.ps7.b_recv(1, id, resp);
                if (id !== 12'h3C4 || resp !== SLVERR) dut.ps7.fail($sformatf("write burst: BID %h BRESP %b", id, resp));
            end
        join
        dut.ps7.read_expect(SCRATCH, 32'hAA22_7744, OKAY);

        // Back to back: four writes and then four reads with no idle cycles
        // on the master side and READY held high.
        fork
            for (j = 0; j < 4; j = j + 1) dut.ps7.aw_send(12'h100 + j, j[0] ? LEDS : SCRATCH, 4'd0, 0);
            for (k = 0; k < 4; k = k + 1) dut.ps7.w_send(32'hC0DE_0000 + k, 4'hF, 1'b1, 0);
            for (i = 0; i < 4; i = i + 1) begin
                dut.ps7.b_recv(0, id, resp);
                if (id !== 12'h100 + i || resp !== OKAY) dut.ps7.fail($sformatf("back-to-back write %0d: BID %h BRESP %b", i, id, resp));
            end
        join
        fork
            for (j = 0; j < 4; j = j + 1) dut.ps7.ar_send(12'h200 + j, (j == 0) ? MAGIC : (j == 1) ? SCRATCH : (j == 2) ? LEDS : 32'h4000_0020, 4'd0, 0);
            for (i = 0; i < 4; i = i + 1) begin
                dut.ps7.r_recv(0, id, data, resp, last);
                if (id !== 12'h200 + i || last !== 1'b1) dut.ps7.fail($sformatf("back-to-back read %0d: RID %h RLAST %b", i, id, last));
                case (i)
                    0: dut.ps7.expect32("back-to-back MAGIC", data, 32'h484C_0701);
                    1: dut.ps7.expect32("back-to-back SCRATCH", data, 32'hC0DE_0002);
                    2: dut.ps7.expect32("back-to-back LEDS", data, 32'h0000_0003 & 32'h7F);
                    default: if (resp !== SLVERR) dut.ps7.fail("back-to-back unmapped read not SLVERR");
                endcase
            end
        join
        if (led[6:0] !== 7'h03) dut.ps7.fail($sformatf("LEDs %h after back-to-back writes, want 03", led[6:0]));

        // Two reads back to back while RREADY holds the first one off.
        fork
            for (j = 0; j < 2; j = j + 1) dut.ps7.ar_send(12'h300 + j, j ? LEDS : MAGIC, 4'd0, 0);
            for (k = 0; k < 2; k = k + 1) begin
                dut.ps7.r_recv(3, id, data, resp, last);
                if (id !== 12'h300 + k) dut.ps7.fail($sformatf("held-off read %0d: RID %h", k, id));
                dut.ps7.expect32("held-off read", data, k ? 32'h0000_0003 : 32'h484C_0701);
            end
        join

        // A read and a write in flight together; a slow RREADY.
        fork
            dut.ps7.write(SCRATCH, 32'h600D_F00D, 4'hF, 12'h0AA, 1, 0, 2, resp);
            begin
                dut.ps7.read(MAGIC, 12'h0BB, 7, data, resp);
                dut.ps7.expect32("MAGIC during a write", data, 32'h484C_0701);
            end
        join
        dut.ps7.read_expect(SCRATCH, 32'h600D_F00D, OKAY);

        // W may come first: a W offered 60 cycles before its AW is taken
        // at once, not held until the AW arrives.
        fork
            begin dut.ps7.aw_send(12'hC01, SCRATCH, 4'd0, 60); t_aw = dut.ps7.cyc; end
            begin dut.ps7.w_send(32'h1357_9BDF, 4'b1001, 1'b1, 0); t_w = dut.ps7.cyc; end
            begin dut.ps7.b_recv(0, id, resp); t_b = dut.ps7.cyc; end
        join
        if (!(t_w + 50 < t_aw)) dut.ps7.fail($sformatf("W taken at cycle %0d, AW at %0d: W waited for AW", t_w, t_aw));
        if (!(t_b > t_aw) || id !== 12'hC01 || resp !== OKAY) dut.ps7.fail($sformatf("W first: B at %0d (AW %0d), BID %h BRESP %b", t_b, t_aw, id, resp));
        dut.ps7.read_expect(SCRATCH, 32'h130D_F0DF, OKAY);

        // BREADY held off 200 cycles while a second write is taken: the
        // first response holds (the monitor checks it) and is not lost.
        fork
            begin dut.ps7.aw_send(12'hD01, SCRATCH, 4'd0, 0); dut.ps7.aw_send(12'hD02, LEDS, 4'd0, 0); t_aw = dut.ps7.cyc; end
            begin dut.ps7.w_send(32'h2468_ACE0, 4'hF, 1'b1, 0); dut.ps7.w_send(32'h0000_002A, 4'h1, 1'b1, 0); t_w = dut.ps7.cyc; end
            begin dut.ps7.b_recv(200, i1, r1); t_b = dut.ps7.cyc; dut.ps7.b_recv(0, i2, r2); end
        join
        if (i1 !== 12'hD01 || r1 !== OKAY || i2 !== 12'hD02 || r2 !== OKAY) dut.ps7.fail($sformatf("held B: %h/%b then %h/%b", i1, r1, i2, r2));
        if (!(t_aw < t_b && t_w < t_b)) dut.ps7.fail($sformatf("held B: second AW/W at %0d/%0d, not before the first B at %0d", t_aw, t_w, t_b));
        dut.ps7.read_expect(SCRATCH, 32'h2468_ACE0, OKAY);
        dut.ps7.read_expect(LEDS, 32'h0000_002A, OKAY);

        // A PL reset clears the writable registers.
        dut.ps7.reset_pl(4);
        dut.ps7.read_expect(SCRATCH, 32'h0, OKAY);
        dut.ps7.read_expect(LEDS, 32'h0, OKAY);
        dut.ps7.read_expect(MAGIC, 32'h484C_0701, OKAY);

        if (dut.ps7.errors == 0) $display("PASS: zedboard_ps7_axi (%0d cycles)", dut.ps7.cyc);
        else $display("FAIL: zedboard_ps7_axi, %0d errors", dut.ps7.errors);
        $finish;
    end

    initial begin
        #2_000_000;
        $display("FAIL: timeout (a channel hung)");
        $finish;
    end
endmodule
