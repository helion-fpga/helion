// Rung 1 on the board: Ibex (ysyx_ibex, the same core as ibex_obs_wrap) running
// a program from a small ROM at 38.9 MHz, Vivado's own Fmax for this core.
// The ZedBoard's 100 MHz oscillator goes through one MMCM (VCO 700 MHz / 18
// = 38.89 MHz). The program counts and writes the count to the LEDs
// about four times a second:
//
//   30000080  lui  x1, 0x10000      ; LED register address
//   30000084  addi x2, x0, 0
//   30000088  sw   x2, 0(x1)        ; loop: LEDs = count
//   3000008c  lui  x3, 0x200        ; delay
//   30000090  addi x3, x3, -1
//   30000094  bne  x3, x0, -4
//   30000098  addi x2, x2, 1
//   3000009c  jal  x0, -20
//
// Build with VERILATOR defined (ysyx_ibex takes its clock from the pin then):
//   HELION_DEFINES=VERILATOR zedbit examples/ysyx_ibex.sv,examples/zedboard_ibex_soc.sv
//       examples/zedboard_ibex_soc.xdc out.bit zedboard_ibex_soc
module zedboard_ibex_soc (
    input  logic       clk,
    output logic [7:0] led
);
    logic clk_fb, clk_mmcm, clk_sys, locked;
    MMCME2_BASE #(
        .BANDWIDTH("OPTIMIZED"), .CLKIN1_PERIOD(10.0), .DIVCLK_DIVIDE(1),
        .CLKFBOUT_MULT_F(7.0), .CLKFBOUT_PHASE(0.0),
        .CLKOUT0_DIVIDE_F(18.0), .CLKOUT0_DUTY_CYCLE(0.5), .CLKOUT0_PHASE(0.0),
        .REF_JITTER1(0.010), .STARTUP_WAIT("FALSE")
    ) u_mmcm (
        .CLKIN1(clk), .CLKFBIN(clk_fb), .CLKFBOUT(clk_fb), .CLKOUT0(clk_mmcm),
        .LOCKED(locked), .PWRDWN(1'b0), .RST(1'b0)
    );
    BUFG u_bufg (.I(clk_mmcm), .O(clk_sys));

    // Power-on reset: held until LOCKED has been high for two flops plus 16 cycles.
    (* ASYNC_REG = "TRUE" *) logic lock_s1 = 1'b0, lock_s2 = 1'b0;
    logic [4:0] por = 5'd0;
    wire rst = !por[4];
    always_ff @(posedge clk_sys) begin
        lock_s1 <= locked;
        lock_s2 <= lock_s1;
        if (!lock_s2) por <= 5'd0;
        else if (!por[4]) por <= por + 5'd1;
    end

    // Program ROM, word-addressed; everything else is a NOP.
    function automatic logic [31:0] rom(input logic [5:0] a);
        case (a)
            6'd32: rom = 32'h100000B7;
            6'd33: rom = 32'h00000113;
            6'd34: rom = 32'h0020A023;
            6'd35: rom = 32'h002001B7;
            6'd36: rom = 32'hFFF18193;
            6'd37: rom = 32'hFE019EE3;
            6'd38: rom = 32'h00110113;
            6'd39: rom = 32'hFEDFF06F;
            default: rom = 32'h00000013;
        endcase
    endfunction

    logic        arvalid, rready, awvalid, wvalid, wlast, bready;
    logic [31:0] araddr, awaddr;
    logic [63:0] wdata, rdata;
    logic [7:0]  wstrb, arlen, awlen;
    logic [3:0]  arid, awid;
    logic [2:0]  arsize, awsize;
    logic [1:0]  arburst, awburst;

    // Always-ready slave: read data and write response one cycle after the request.
    logic rvalid, bvalid;
    always_ff @(posedge clk_sys) begin
        if (rst) begin
            rvalid <= 1'b0;
            bvalid <= 1'b0;
            led    <= 8'd0;
        end else begin
            rvalid <= arvalid;
            bvalid <= wvalid;
            if (arvalid) rdata <= {32'd0, rom(araddr[7:2])};
            if (wvalid) led <= wdata[7:0];
        end
    end

    ysyx_ibex u_ibex (
        .clock             (clk_sys),
        .reset             (rst),
        .io_master_arready (1'b1),
        .io_master_arvalid (arvalid),
        .io_master_araddr  (araddr),
        .io_master_arid    (arid),
        .io_master_arlen   (arlen),
        .io_master_arsize  (arsize),
        .io_master_arburst (arburst),
        .io_master_rready  (rready),
        .io_master_rvalid  (rvalid),
        .io_master_rresp   (2'b00),
        .io_master_rdata   (rdata),
        .io_master_rlast   (1'b1),
        .io_master_rid     (4'd0),
        .io_master_awready (1'b1),
        .io_master_awvalid (awvalid),
        .io_master_awaddr  (awaddr),
        .io_master_awid    (awid),
        .io_master_awlen   (awlen),
        .io_master_awsize  (awsize),
        .io_master_awburst (awburst),
        .io_master_wready  (1'b1),
        .io_master_wvalid  (wvalid),
        .io_master_wdata   (wdata),
        .io_master_wstrb   (wstrb),
        .io_master_wlast   (wlast),
        .io_master_bready  (bready),
        .io_master_bvalid  (bvalid),
        .io_master_bresp   (2'b00),
        .io_master_bid     (4'd0),
        .io_slave_arvalid  (1'b0),
        .io_slave_araddr   (32'd0),
        .io_slave_arid     (4'd0),
        .io_slave_arlen    (8'd0),
        .io_slave_arsize   (3'd0),
        .io_slave_arburst  (2'd0),
        .io_slave_rready   (1'b0),
        .io_slave_awvalid  (1'b0),
        .io_slave_awaddr   (32'd0),
        .io_slave_awid     (4'd0),
        .io_slave_awlen    (8'd0),
        .io_slave_awsize   (3'd0),
        .io_slave_awburst  (2'd0),
        .io_slave_wvalid   (1'b0),
        .io_slave_wdata    (64'd0),
        .io_slave_wstrb    (8'd0),
        .io_slave_wlast    (1'b0),
        .io_slave_bready   (1'b0),
        .io_interrupt      (1'b0),
        .io_sram0_rdata    (128'd0),
        .io_sram1_rdata    (128'd0),
        .io_sram2_rdata    (128'd0),
        .io_sram3_rdata    (128'd0),
        .io_sram4_rdata    (128'd0),
        .io_sram5_rdata    (128'd0),
        .io_sram6_rdata    (128'd0),
        .io_sram7_rdata    (128'd0)
    );

endmodule
