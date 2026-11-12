// Ibex with every bus output observable through a handful of pins.
//
// The memory bus is answered from a shift register fed by eight data pins,
// so the core executes whatever the pins supply, and all bus outputs are
// folded by XOR into a 16-bit signature register that drives the pads.
// Unlike ibex_pin_wrap, nothing in the core is dead: a tool that drops or
// miswires logic changes the signature.
//
// Build with VERILATOR defined (ysyx_ibex takes its clock from the pin then).
module ibex_obs_wrap (
    input  logic        clk,
    input  logic        rst,
    input  logic [7:0]  din,
    input  logic        ready,
    input  logic        irq,
    output logic [15:0] sig
);
    // Read data: the last eight bytes from the pins.
    logic [63:0] rdata;
    always_ff @(posedge clk) rdata <= {rdata[55:0], din};

    logic        arvalid, rready, awvalid, wvalid, wlast, bready;
    logic [31:0] araddr, awaddr;
    logic [63:0] wdata;
    logic [7:0]  wstrb, arlen, awlen;
    logic [3:0]  arid, awid;
    logic [2:0]  arsize, awsize;
    logic [1:0]  arburst, awburst;

    // One-cycle read and write responses when `ready` allows.
    logic rvalid, bvalid;
    always_ff @(posedge clk) begin
        if (rst) begin
            rvalid <= 1'b0;
            bvalid <= 1'b0;
        end else begin
            rvalid <= arvalid & ready;
            bvalid <= wvalid & ready;
        end
    end

    ysyx_ibex u_ibex (
        .clock             (clk),
        .reset             (rst),
        .io_master_arready (ready),
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
        .io_master_awready (ready),
        .io_master_awvalid (awvalid),
        .io_master_awaddr  (awaddr),
        .io_master_awid    (awid),
        .io_master_awlen   (awlen),
        .io_master_awsize  (awsize),
        .io_master_awburst (awburst),
        .io_master_wready  (ready),
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
        .io_interrupt      (irq),
        .io_sram0_rdata    (128'd0),
        .io_sram1_rdata    (128'd0),
        .io_sram2_rdata    (128'd0),
        .io_sram3_rdata    (128'd0),
        .io_sram4_rdata    (128'd0),
        .io_sram5_rdata    (128'd0),
        .io_sram6_rdata    (128'd0),
        .io_sram7_rdata    (128'd0)
    );

    // Signature: every bus output, folded.
    logic [15:0] fold;
    assign fold = araddr[15:0] ^ araddr[31:16] ^ awaddr[15:0] ^ awaddr[31:16]
                ^ wdata[15:0] ^ wdata[31:16] ^ wdata[47:32] ^ wdata[63:48]
                ^ {wstrb, arlen} ^ {awlen, arid, awid}
                ^ {arsize, awsize, arburst, awburst, arvalid, rready, awvalid, wvalid, wlast, bready};
    always_ff @(posedge clk) begin
        if (rst) sig <= 16'd0;
        else     sig <= {sig[14:0], sig[15]} ^ fold;
    end
endmodule
