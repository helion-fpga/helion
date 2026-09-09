// FM-HEL-CONT: pin-reduced wrap around public SERV (bit-serial RISC-V).
// Board pins are clk + led only. LED is FF-driven (honest IOB).
// Wishbone-ish ibus/dbus, extension, and RF ports tied for fabric bring-up.
// Instantiates serv_top (CPU) — not a counter clone. Soft-hold.
// Part: HL10T-C32-1.

module serv_pin_wrap (
    input  logic clk,
    output logic led
);
    logic [7:0] hb;
    always_ff @(posedge clk) begin
        hb  <= hb + 8'd1;
        led <= hb[7];
    end

    logic        i_rst;
    logic        i_timer_irq;
    logic        rf_rreq;
    logic        rf_wreq;
    logic        rf_ready;
    logic [4:0]  wreg0;
    logic [4:0]  wreg1;
    logic        wen0;
    logic        wen1;
    logic        wdata0;
    logic        wdata1;
    logic [4:0]  rreg0;
    logic [4:0]  rreg1;
    logic        rdata0;
    logic        rdata1;
    logic [31:0] o_ibus_adr;
    logic        o_ibus_cyc;
    logic [31:0] i_ibus_rdt;
    logic        i_ibus_ack;
    logic [31:0] o_dbus_adr;
    logic [31:0] o_dbus_dat;
    logic [3:0]  o_dbus_sel;
    logic        o_dbus_we;
    logic        o_dbus_cyc;
    logic [31:0] i_dbus_rdt;
    logic        i_dbus_ack;
    logic [2:0]  o_ext_funct3;
    logic        i_ext_ready;
    logic [31:0] i_ext_rd;
    logic [31:0] o_ext_rs1;
    logic [31:0] o_ext_rs2;
    logic        o_mdu_valid;

    assign i_rst = 1'b0;
    assign i_timer_irq = 1'b0;
    // RF stub: always ready, x0-style zero reads (WITH_CSR=0 → 5-bit reg idx).
    assign rf_ready = 1'b1;
    assign rdata0 = 1'b0;
    assign rdata1 = 1'b0;
    // Idle bus: rdata = ADDI x0,x0,0; never ack (honest tied fabric).
    assign i_ibus_rdt = 32'h0000_0013;
    assign i_ibus_ack = 1'b0;
    assign i_dbus_rdt = 32'h0;
    assign i_dbus_ack = 1'b0;
    assign i_ext_rd = 32'h0;
    assign i_ext_ready = 1'b0;

    // WITH_CSR=0 so RF port widths are [4:0] / 1-bit data (W=1).
    serv_top #(
        .WITH_CSR (0),
        .W (1),
        .PRE_REGISTER (1),
        .RESET_STRATEGY ("MINI"),
        .COMPRESSED (0),
        .ALIGN (0),
        .MDU (0),
        .DEBUG (1'b0)
    ) u_serv (
        .clk (clk),
        .i_rst (i_rst),
        .i_timer_irq (i_timer_irq),
        .o_rf_rreq (rf_rreq),
        .o_rf_wreq (rf_wreq),
        .i_rf_ready (rf_ready),
        .o_wreg0 (wreg0),
        .o_wreg1 (wreg1),
        .o_wen0 (wen0),
        .o_wen1 (wen1),
        .o_wdata0 (wdata0),
        .o_wdata1 (wdata1),
        .o_rreg0 (rreg0),
        .o_rreg1 (rreg1),
        .i_rdata0 (rdata0),
        .i_rdata1 (rdata1),
        .o_ibus_adr (o_ibus_adr),
        .o_ibus_cyc (o_ibus_cyc),
        .i_ibus_rdt (i_ibus_rdt),
        .i_ibus_ack (i_ibus_ack),
        .o_dbus_adr (o_dbus_adr),
        .o_dbus_dat (o_dbus_dat),
        .o_dbus_sel (o_dbus_sel),
        .o_dbus_we (o_dbus_we),
        .o_dbus_cyc (o_dbus_cyc),
        .i_dbus_rdt (i_dbus_rdt),
        .i_dbus_ack (i_dbus_ack),
        .o_ext_funct3 (o_ext_funct3),
        .i_ext_ready (i_ext_ready),
        .i_ext_rd (i_ext_rd),
        .o_ext_rs1 (o_ext_rs1),
        .o_ext_rs2 (o_ext_rs2),
        .o_mdu_valid (o_mdu_valid)
    );
endmodule
