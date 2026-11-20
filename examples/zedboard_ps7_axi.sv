// Rung 7 (PS7 + AXI), the PL half: the ARM reads and writes logic registers
// over M_AXI_GP0. The PS7 primitive is instantiated directly (no Vivado IP).
//
// Clock and reset: FCLK_CLK0 (FCLKCLK[0]) goes through one BUFG and clocks
// all the logic and MAXIGP0ACLK. The PS sets its frequency (100 MHz in the
// usual ps7_init; the XDC times it at 100 MHz). FCLK_RESET0_N
// (FCLKRESETN[0]) crosses into the clock through two flops; the register
// block is held in reset until it has been seen high twice.
//
// Register map (M_AXI_GP0 window 0x4000_0000..0x7FFF_FFFF, decoded here):
//   0x4000_0000  MAGIC    read-only, 0x484C_0701 ("HL", rung 7, version 1)
//   0x4000_0004  SCRATCH  read-write, WSTRB byte enables, reset 0
//   0x4000_0008  COUNTER  read-only, FCLK cycles (free-running, no reset)
//                         from 0xFFFF_FC00 at configuration, so every bit
//                         changes within 1,024 cycles and a short
//                         simulation exercises the whole carry chain
//   0x4000_000C  LEDS     read-write, bits [6:0] drive LD0..LD6 (WSTRB[0]),
//                         reset 0; LD7 is a heartbeat (COUNTER bit 26,
//                         about 0.75 Hz at 100 MHz), alive as soon as FCLK
//                         runs, with no software and no reset release.
// Any other address in the window, a write to a read-only register, and
// any burst (ARLEN/AWLEN other than 0, or a W burst longer than one beat)
// answer SLVERR (read data 0); a burst still gets all its beats (RLAST on
// the last) and has all its W beats taken (up to WLAST), so a master never
// hangs.
//
// AXI3 slave rules kept: one read and one write in flight; RID/BID echo
// ARID/AWID; AW and W are taken in either order (W may come first);
// AWREADY/WREADY/ARREADY depend only on this block's state; BVALID and
// RVALID never wait for BREADY/RREADY and hold until taken. WID is not
// read: the GP0 master never interleaves write data.
//
// Unused PS7 pins are left unconnected. Helion ties every unconnected PS7
// fabric input to 0 (interrupts, the other AXI ports' handshakes and
// clocks, EMIO inputs, DDRARB, FCLKCLKTRIGN, ...), as Vivado's PS7 build
// does; the INT-tile default of 1 would otherwise assert all 16 PL
// interrupts. MIO, DDR, PS_CLK, PS_PORB and PS_SRSTB are
// the PS's dedicated package pins, configured by the boot code, not the
// bitstream: they are not connected here.
//
// Build: zedbit examples/zedboard_ps7_axi.sv examples/zedboard_ps7_axi.xdc out.bit zedboard_ps7_axi
// RTL test: examples/zedboard_ps7_axi_tb.sv (with a simulation-only PS7).
module zedboard_ps7_axi (
    output logic [7:0] led
);
    localparam logic [31:0] MAGIC = 32'h484C_0701;
    localparam logic [1:0] OKAY = 2'b00, SLVERR = 2'b10;

    logic [3:0] fclk, fclk_resetn;
    logic clk;
    logic arvalid, arready, awvalid, awready, wvalid, wready, wlast;
    logic bvalid, bready, rvalid, rready, rlast;
    logic [31:0] araddr, awaddr, wdata, rdata;
    logic [11:0] arid, awid, bid, rid;
    logic [3:0] arlen, awlen, wstrb;
    logic [1:0] bresp, rresp;

    PS7 ps7 (
        .FCLKCLK(fclk),
        .FCLKRESETN(fclk_resetn),
        .MAXIGP0ACLK(clk),
        .MAXIGP0ARVALID(arvalid),
        .MAXIGP0ARREADY(arready),
        .MAXIGP0ARADDR(araddr),
        .MAXIGP0ARID(arid),
        .MAXIGP0ARLEN(arlen),
        .MAXIGP0AWVALID(awvalid),
        .MAXIGP0AWREADY(awready),
        .MAXIGP0AWADDR(awaddr),
        .MAXIGP0AWID(awid),
        .MAXIGP0AWLEN(awlen),
        .MAXIGP0WVALID(wvalid),
        .MAXIGP0WREADY(wready),
        .MAXIGP0WDATA(wdata),
        .MAXIGP0WSTRB(wstrb),
        .MAXIGP0WLAST(wlast),
        .MAXIGP0BVALID(bvalid),
        .MAXIGP0BREADY(bready),
        .MAXIGP0BID(bid),
        .MAXIGP0BRESP(bresp),
        .MAXIGP0RVALID(rvalid),
        .MAXIGP0RREADY(rready),
        .MAXIGP0RDATA(rdata),
        .MAXIGP0RID(rid),
        .MAXIGP0RRESP(rresp),
        .MAXIGP0RLAST(rlast)
    );

    BUFG u_bufg (.I(fclk[0]), .O(clk));

    (* ASYNC_REG = "TRUE" *) logic rst_s1 = 1'b0, rst_s2 = 1'b0;
    always_ff @(posedge clk) begin
        rst_s1 <= fclk_resetn[0];
        rst_s2 <= rst_s1;
    end
    logic rst;
    assign rst = ~rst_s2;

    logic [31:0] cnt = 32'hFFFF_FC00;
    always_ff @(posedge clk) cnt <= cnt + 32'd1;

    logic [31:0] scratch;
    logic [6:0] led_reg;
    assign led = {cnt[26], led_reg};

    // Register index of an address, and whether it is one of the four.
    function automatic logic hit(input logic [31:0] a);
        hit = a[31:12] == 20'h40000 && a[11:4] == 8'h00;
    endfunction

    // Write: one address and one (last) data beat held until the response.
    logic aw_held, w_held, w_first, w_single, b_pend;
    logic [11:0] aw_id;
    logic [31:0] aw_addr, w_data;
    logic [3:0] aw_len, w_strb;
    logic [11:0] b_id;
    logic [1:0] b_resp;
    assign awready = ~aw_held & ~rst;
    assign wready = ~w_held & ~rst;
    assign bvalid = b_pend;
    assign bid = b_id;
    assign bresp = b_resp;
    logic do_write, wr_ok;
    assign do_write = aw_held & w_held & ~b_pend;
    assign wr_ok = aw_len == 4'd0 && w_single && hit(aw_addr) && aw_addr[2];
    always_ff @(posedge clk) begin
        if (rst) begin
            aw_held <= 1'b0;
            w_held <= 1'b0;
            w_first <= 1'b1;
            w_single <= 1'b0;
            b_pend <= 1'b0;
            scratch <= 32'd0;
            led_reg <= 7'd0;
        end else begin
            if (awvalid && awready) begin
                aw_held <= 1'b1;
                aw_id <= awid;
                aw_addr <= awaddr;
                aw_len <= awlen;
            end
            if (wvalid && wready) begin
                if (w_first) begin
                    w_data <= wdata;
                    w_strb <= wstrb;
                    w_single <= wlast;
                end
                w_first <= wlast;
                if (wlast) w_held <= 1'b1;
            end
            if (do_write) begin
                aw_held <= 1'b0;
                w_held <= 1'b0;
                b_pend <= 1'b1;
                b_id <= aw_id;
                b_resp <= wr_ok ? OKAY : SLVERR;
                if (wr_ok && !aw_addr[3]) begin
                    if (w_strb[0]) scratch[7:0] <= w_data[7:0];
                    if (w_strb[1]) scratch[15:8] <= w_data[15:8];
                    if (w_strb[2]) scratch[23:16] <= w_data[23:16];
                    if (w_strb[3]) scratch[31:24] <= w_data[31:24];
                end
                if (wr_ok && aw_addr[3] && w_strb[0]) led_reg <= w_data[6:0];
            end else if (bvalid && bready) begin
                b_pend <= 1'b0;
            end
        end
    end

    // Read: one burst at a time; its first beat is read when the address is taken.
    logic r_pend, r_last;
    logic [11:0] r_id;
    logic [31:0] r_data;
    logic [1:0] r_resp;
    logic [3:0] r_left;
    assign arready = ~r_pend & ~rst;
    assign rvalid = r_pend;
    assign rid = r_id;
    assign rdata = r_data;
    assign rresp = r_resp;
    assign rlast = r_last;
    logic [31:0] rd_word;
    always_comb begin
        case (araddr[3:2])
            2'd0: rd_word = MAGIC;
            2'd1: rd_word = scratch;
            2'd2: rd_word = cnt;
            default: rd_word = {25'd0, led_reg};
        endcase
    end
    always_ff @(posedge clk) begin
        if (rst) begin
            r_pend <= 1'b0;
            r_last <= 1'b0;
        end else if (arvalid && arready) begin
            r_pend <= 1'b1;
            r_id <= arid;
            r_left <= arlen;
            r_last <= arlen == 4'd0;
            if (arlen == 4'd0 && hit(araddr)) begin
                r_data <= rd_word;
                r_resp <= OKAY;
            end else begin
                r_data <= 32'd0;
                r_resp <= SLVERR;
            end
        end else if (rvalid && rready) begin
            if (r_last) begin
                r_pend <= 1'b0;
            end else begin
                r_left <= r_left - 4'd1;
                r_last <= r_left == 4'd1;
            end
        end
    end
endmodule
