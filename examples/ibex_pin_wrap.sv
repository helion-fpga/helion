// FM-HEL-TOP: pin-reduced full-core wrap around ysyx_ibex.
// Keeps the Ibex fabric (child elaborated) but board pins are clk + led only.
// LED is FF-driven (honest IOB / DRC ROUTE-3). AXI/SRAM ports tied internally.
// Part: HL10T-C32-1. No async reset on the wrap (IMUX-safe heartbeat).
// Honesty: not a functional SoC bring-up; probe pin + fabric config under cap.

module ibex_pin_wrap (
    input  logic clk,
    output logic led
);
    logic reset;
    assign reset = 1'b0;

    // Heartbeat FF → board-facing LED (driven IOB); mark_debug for Probe/ILA.
    (* mark_debug = "true" *) logic [7:0] hb;
    always_ff @(posedge clk) begin
        hb  <= hb + 8'd1;
        led <= hb[7];
    end

    // Tied AXI / SRAM / interrupt (Ibex RTL mentions OK; not a Helion AXI product).
    logic        io_master_arready;
    logic        io_master_arvalid;
    logic [31:0] io_master_araddr;
    logic [3:0]  io_master_arid;
    logic [7:0]  io_master_arlen;
    logic [2:0]  io_master_arsize;
    logic [1:0]  io_master_arburst;
    logic        io_master_rready;
    logic        io_master_rvalid;
    logic [1:0]  io_master_rresp;
    logic [63:0] io_master_rdata;
    logic        io_master_rlast;
    logic [3:0]  io_master_rid;
    logic        io_master_awready;
    logic        io_master_awvalid;
    logic [31:0] io_master_awaddr;
    logic [3:0]  io_master_awid;
    logic [7:0]  io_master_awlen;
    logic [2:0]  io_master_awsize;
    logic [1:0]  io_master_awburst;
    logic        io_master_wready;
    logic        io_master_wvalid;
    logic [63:0] io_master_wdata;
    logic [7:0]  io_master_wstrb;
    logic        io_master_wlast;
    logic        io_master_bready;
    logic        io_master_bvalid;
    logic [1:0]  io_master_bresp;
    logic [3:0]  io_master_bid;

    logic        io_slave_arready;
    logic        io_slave_arvalid;
    logic [31:0] io_slave_araddr;
    logic [3:0]  io_slave_arid;
    logic [7:0]  io_slave_arlen;
    logic [2:0]  io_slave_arsize;
    logic [1:0]  io_slave_arburst;
    logic        io_slave_rready;
    logic        io_slave_rvalid;
    logic [1:0]  io_slave_rresp;
    logic [63:0] io_slave_rdata;
    logic        io_slave_rlast;
    logic [3:0]  io_slave_rid;
    logic        io_slave_awready;
    logic        io_slave_awvalid;
    logic [31:0] io_slave_awaddr;
    logic [3:0]  io_slave_awid;
    logic [7:0]  io_slave_awlen;
    logic [2:0]  io_slave_awsize;
    logic [1:0]  io_slave_awburst;
    logic        io_slave_wready;
    logic        io_slave_wvalid;
    logic [63:0] io_slave_wdata;
    logic [7:0]  io_slave_wstrb;
    logic        io_slave_wlast;
    logic        io_slave_bready;
    logic        io_slave_bvalid;
    logic [1:0]  io_slave_bresp;
    logic [3:0]  io_slave_bid;

    logic        io_interrupt;
    logic [5:0]  io_sram0_addr, io_sram1_addr, io_sram2_addr, io_sram3_addr;
    logic [5:0]  io_sram4_addr, io_sram5_addr, io_sram6_addr, io_sram7_addr;
    logic        io_sram0_cen, io_sram1_cen, io_sram2_cen, io_sram3_cen;
    logic        io_sram4_cen, io_sram5_cen, io_sram6_cen, io_sram7_cen;
    logic        io_sram0_wen, io_sram1_wen, io_sram2_wen, io_sram3_wen;
    logic        io_sram4_wen, io_sram5_wen, io_sram6_wen, io_sram7_wen;
    logic [127:0] io_sram0_wmask, io_sram1_wmask, io_sram2_wmask, io_sram3_wmask;
    logic [127:0] io_sram4_wmask, io_sram5_wmask, io_sram6_wmask, io_sram7_wmask;
    logic [127:0] io_sram0_wdata, io_sram1_wdata, io_sram2_wdata, io_sram3_wdata;
    logic [127:0] io_sram4_wdata, io_sram5_wdata, io_sram6_wdata, io_sram7_wdata;
    logic [127:0] io_sram0_rdata, io_sram1_rdata, io_sram2_rdata, io_sram3_rdata;
    logic [127:0] io_sram4_rdata, io_sram5_rdata, io_sram6_rdata, io_sram7_rdata;

    assign io_master_arready = 1'b0;
    assign io_master_rvalid  = 1'b0;
    assign io_master_rresp   = 2'b0;
    assign io_master_rdata   = 64'b0;
    assign io_master_rlast   = 1'b0;
    assign io_master_rid     = 4'b0;
    assign io_master_awready = 1'b0;
    assign io_master_wready  = 1'b0;
    assign io_master_bvalid  = 1'b0;
    assign io_master_bresp   = 2'b0;
    assign io_master_bid     = 4'b0;
    assign io_slave_arvalid  = 1'b0;
    assign io_slave_araddr   = 32'b0;
    assign io_slave_arid     = 4'b0;
    assign io_slave_arlen    = 8'b0;
    assign io_slave_arsize   = 3'b0;
    assign io_slave_arburst  = 2'b0;
    assign io_slave_rready   = 1'b0;
    assign io_slave_awvalid  = 1'b0;
    assign io_slave_awaddr   = 32'b0;
    assign io_slave_awid     = 4'b0;
    assign io_slave_awlen    = 8'b0;
    assign io_slave_awsize   = 3'b0;
    assign io_slave_awburst  = 2'b0;
    assign io_slave_wvalid   = 1'b0;
    assign io_slave_wdata    = 64'b0;
    assign io_slave_wstrb    = 8'b0;
    assign io_slave_wlast    = 1'b0;
    assign io_slave_bready   = 1'b0;
    assign io_interrupt      = 1'b0;
    assign io_sram0_rdata = 128'b0;
    assign io_sram1_rdata = 128'b0;
    assign io_sram2_rdata = 128'b0;
    assign io_sram3_rdata = 128'b0;
    assign io_sram4_rdata = 128'b0;
    assign io_sram5_rdata = 128'b0;
    assign io_sram6_rdata = 128'b0;
    assign io_sram7_rdata = 128'b0;

    ysyx_ibex u_ibex (
        .clock(clk),
        .reset(reset),
        .io_master_arready(io_master_arready),
        .io_master_arvalid(io_master_arvalid),
        .io_master_araddr(io_master_araddr),
        .io_master_arid(io_master_arid),
        .io_master_arlen(io_master_arlen),
        .io_master_arsize(io_master_arsize),
        .io_master_arburst(io_master_arburst),
        .io_master_rready(io_master_rready),
        .io_master_rvalid(io_master_rvalid),
        .io_master_rresp(io_master_rresp),
        .io_master_rdata(io_master_rdata),
        .io_master_rlast(io_master_rlast),
        .io_master_rid(io_master_rid),
        .io_master_awready(io_master_awready),
        .io_master_awvalid(io_master_awvalid),
        .io_master_awaddr(io_master_awaddr),
        .io_master_awid(io_master_awid),
        .io_master_awlen(io_master_awlen),
        .io_master_awsize(io_master_awsize),
        .io_master_awburst(io_master_awburst),
        .io_master_wready(io_master_wready),
        .io_master_wvalid(io_master_wvalid),
        .io_master_wdata(io_master_wdata),
        .io_master_wstrb(io_master_wstrb),
        .io_master_wlast(io_master_wlast),
        .io_master_bready(io_master_bready),
        .io_master_bvalid(io_master_bvalid),
        .io_master_bresp(io_master_bresp),
        .io_master_bid(io_master_bid),
        .io_slave_arready(io_slave_arready),
        .io_slave_arvalid(io_slave_arvalid),
        .io_slave_araddr(io_slave_araddr),
        .io_slave_arid(io_slave_arid),
        .io_slave_arlen(io_slave_arlen),
        .io_slave_arsize(io_slave_arsize),
        .io_slave_arburst(io_slave_arburst),
        .io_slave_rready(io_slave_rready),
        .io_slave_rvalid(io_slave_rvalid),
        .io_slave_rresp(io_slave_rresp),
        .io_slave_rdata(io_slave_rdata),
        .io_slave_rlast(io_slave_rlast),
        .io_slave_rid(io_slave_rid),
        .io_slave_awready(io_slave_awready),
        .io_slave_awvalid(io_slave_awvalid),
        .io_slave_awaddr(io_slave_awaddr),
        .io_slave_awid(io_slave_awid),
        .io_slave_awlen(io_slave_awlen),
        .io_slave_awsize(io_slave_awsize),
        .io_slave_awburst(io_slave_awburst),
        .io_slave_wready(io_slave_wready),
        .io_slave_wvalid(io_slave_wvalid),
        .io_slave_wdata(io_slave_wdata),
        .io_slave_wstrb(io_slave_wstrb),
        .io_slave_wlast(io_slave_wlast),
        .io_slave_bready(io_slave_bready),
        .io_slave_bvalid(io_slave_bvalid),
        .io_slave_bresp(io_slave_bresp),
        .io_slave_bid(io_slave_bid),
        .io_interrupt(io_interrupt),
        .io_sram0_addr(io_sram0_addr),
        .io_sram0_cen(io_sram0_cen),
        .io_sram0_wen(io_sram0_wen),
        .io_sram0_wmask(io_sram0_wmask),
        .io_sram0_wdata(io_sram0_wdata),
        .io_sram0_rdata(io_sram0_rdata),
        .io_sram1_addr(io_sram1_addr),
        .io_sram1_cen(io_sram1_cen),
        .io_sram1_wen(io_sram1_wen),
        .io_sram1_wmask(io_sram1_wmask),
        .io_sram1_wdata(io_sram1_wdata),
        .io_sram1_rdata(io_sram1_rdata),
        .io_sram2_addr(io_sram2_addr),
        .io_sram2_cen(io_sram2_cen),
        .io_sram2_wen(io_sram2_wen),
        .io_sram2_wmask(io_sram2_wmask),
        .io_sram2_wdata(io_sram2_wdata),
        .io_sram2_rdata(io_sram2_rdata),
        .io_sram3_addr(io_sram3_addr),
        .io_sram3_cen(io_sram3_cen),
        .io_sram3_wen(io_sram3_wen),
        .io_sram3_wmask(io_sram3_wmask),
        .io_sram3_wdata(io_sram3_wdata),
        .io_sram3_rdata(io_sram3_rdata),
        .io_sram4_addr(io_sram4_addr),
        .io_sram4_cen(io_sram4_cen),
        .io_sram4_wen(io_sram4_wen),
        .io_sram4_wmask(io_sram4_wmask),
        .io_sram4_wdata(io_sram4_wdata),
        .io_sram4_rdata(io_sram4_rdata),
        .io_sram5_addr(io_sram5_addr),
        .io_sram5_cen(io_sram5_cen),
        .io_sram5_wen(io_sram5_wen),
        .io_sram5_wmask(io_sram5_wmask),
        .io_sram5_wdata(io_sram5_wdata),
        .io_sram5_rdata(io_sram5_rdata),
        .io_sram6_addr(io_sram6_addr),
        .io_sram6_cen(io_sram6_cen),
        .io_sram6_wen(io_sram6_wen),
        .io_sram6_wmask(io_sram6_wmask),
        .io_sram6_wdata(io_sram6_wdata),
        .io_sram6_rdata(io_sram6_rdata),
        .io_sram7_addr(io_sram7_addr),
        .io_sram7_cen(io_sram7_cen),
        .io_sram7_wen(io_sram7_wen),
        .io_sram7_wmask(io_sram7_wmask),
        .io_sram7_wdata(io_sram7_wdata),
        .io_sram7_rdata(io_sram7_rdata)
    );
endmodule
