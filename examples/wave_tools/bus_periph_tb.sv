// Testbench for bus_periph.sv (Helion testbench-runner subset). Pulses
// `start`, lets the SPI, UART and AXI-Lite phases run, and checks the
// AXI-Lite read-back. With the VCD dump on, the Wave pane's decoders read
// every protocol from this run.
module bus_periph_tb;
    logic        clk = 0;
    logic        start = 0;
    logic [1:0]  state;
    logic [3:0]  cnt;
    logic [7:0]  ramp;
    logic        sclk;
    logic        mosi;
    logic        miso;
    logic        cs_n;
    logic        uart_tx;
    logic        s_axi_awvalid;
    logic        s_axi_awready;
    logic [7:0]  s_axi_awaddr;
    logic        s_axi_wvalid;
    logic        s_axi_wready;
    logic [15:0] s_axi_wdata;
    logic        s_axi_bvalid;
    logic        s_axi_bready;
    logic [1:0]  s_axi_bresp;
    logic        s_axi_arvalid;
    logic        s_axi_arready;
    logic [7:0]  s_axi_araddr;
    logic        s_axi_rvalid;
    logic        s_axi_rready;
    logic [15:0] s_axi_rdata;

    bus_periph dut (
        .clk(clk), .start(start), .state(state), .cnt(cnt), .ramp(ramp),
        .sclk(sclk), .mosi(mosi), .miso(miso), .cs_n(cs_n), .uart_tx(uart_tx),
        .s_axi_awvalid(s_axi_awvalid), .s_axi_awready(s_axi_awready),
        .s_axi_awaddr(s_axi_awaddr), .s_axi_wvalid(s_axi_wvalid),
        .s_axi_wready(s_axi_wready), .s_axi_wdata(s_axi_wdata),
        .s_axi_bvalid(s_axi_bvalid), .s_axi_bready(s_axi_bready),
        .s_axi_bresp(s_axi_bresp), .s_axi_arvalid(s_axi_arvalid),
        .s_axi_arready(s_axi_arready), .s_axi_araddr(s_axi_araddr),
        .s_axi_rvalid(s_axi_rvalid), .s_axi_rready(s_axi_rready),
        .s_axi_rdata(s_axi_rdata)
    );

    always #5 clk = ~clk;

    initial begin
        $display("bus_periph_tb: start");
        repeat (3) @(posedge clk);
        #1;
        start = 1;
        @(posedge clk);
        #1;
        start = 0;
        repeat (190) @(posedge clk);
        #1;
        assert (state == 0) else $error("sequence did not return to IDLE: state=%0d", state);
        assert (s_axi_rdata == 16'hBEEF) else $error("AXI read-back: rdata=%h", s_axi_rdata);
        $display("rdata=%h at %t", s_axi_rdata, $time);
        $finish;
    end
endmodule
