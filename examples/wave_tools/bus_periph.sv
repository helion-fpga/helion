// Peripheral exerciser for the waveform tools (protocol decoders, analog
// display, virtual buses). After `start` an FSM walks through an SPI
// exchange, a UART transmission and an AXI-Lite write plus read-back
// against a one-register slave, while a counter and a triangle ramp run
// freely. Every protocol signal is a port, so the testbench VCD records it.
//
//   SPI   mode 0, MSB first, SCLK = clk/4: master sends A5 3C while the
//         slave answers 5A C3 on MISO.
//   UART  8N1, 4 clocks per bit: "OK".
//   AXI   write 0x10 <- 0xBEEF (OKAY), then read 0x10 -> 0xBEEF.
module bus_periph (
    input  logic        clk,
    input  logic        start,
    output logic [1:0]  state,
    output logic [3:0]  cnt,
    output logic [7:0]  ramp,
    output logic        sclk,
    output logic        mosi,
    output logic        miso,
    output logic        cs_n,
    output logic        uart_tx,
    output logic        s_axi_awvalid,
    output logic        s_axi_awready,
    output logic [7:0]  s_axi_awaddr,
    output logic        s_axi_wvalid,
    output logic        s_axi_wready,
    output logic [15:0] s_axi_wdata,
    output logic        s_axi_bvalid,
    output logic        s_axi_bready,
    output logic [1:0]  s_axi_bresp,
    output logic        s_axi_arvalid,
    output logic        s_axi_arready,
    output logic [7:0]  s_axi_araddr,
    output logic        s_axi_rvalid,
    output logic        s_axi_rready,
    output logic [15:0] s_axi_rdata
);
    localparam [1:0] IDLE = 2'd0, SPI_XFER = 2'd1, UART_SEND = 2'd2, AXI_RW = 2'd3;

    // Free-running 4-bit counter and an 8-bit triangle ramp (step 8).
    logic ramp_down;
    always_ff @(posedge clk) begin
        cnt <= cnt + 4'd1;
        if (ramp_down) begin
            if (ramp == 8'd8) ramp_down <= 1'b0;
            ramp <= ramp - 8'd8;
        end else begin
            if (ramp == 8'd240) ramp_down <= 1'b1;
            ramp <= ramp + 8'd8;
        end
    end

    // SPI: one 16-bit chip-select frame; data changes while SCLK is low.
    logic [1:0]  spi_div;
    logic [3:0]  spi_bit;
    logic [15:0] spi_tx;
    logic [15:0] spi_slave;
    assign sclk = (state == SPI_XFER) & spi_div[1];
    assign mosi = (state == SPI_XFER) & spi_tx[15];
    assign miso = (state == SPI_XFER) & spi_slave[15];
    assign cs_n = (state != SPI_XFER);

    // UART: LSB-first frame {stop, data, start}; the line idles high.
    logic [9:0] u_shift;
    logic [3:0] u_bit;
    logic [1:0] u_div;
    logic       u_second;
    assign uart_tx = (state == UART_SEND) ? u_shift[0] : 1'b1;

    // AXI-Lite master sequence step.
    logic [2:0]  axi_step;
    logic [15:0] reg10;
    assign s_axi_bresp = 2'b00;

    always_ff @(posedge clk) begin
        case (state)
            IDLE: begin
                if (start) begin
                    state     <= SPI_XFER;
                    spi_div   <= 2'd0;
                    spi_bit   <= 4'd0;
                    spi_tx    <= 16'hA53C;
                    spi_slave <= 16'h5AC3;
                end
            end
            SPI_XFER: begin
                spi_div <= spi_div + 2'd1;
                if (spi_div == 2'd3) begin
                    spi_tx    <= {spi_tx[14:0], 1'b0};
                    spi_slave <= {spi_slave[14:0], 1'b0};
                    spi_bit   <= spi_bit + 4'd1;
                    if (spi_bit == 4'd15) begin
                        state    <= UART_SEND;
                        u_shift  <= {1'b1, 8'h4F, 1'b0};
                        u_bit    <= 4'd0;
                        u_div    <= 2'd0;
                        u_second <= 1'b0;
                    end
                end
            end
            UART_SEND: begin
                u_div <= u_div + 2'd1;
                if (u_div == 2'd3) begin
                    if (u_bit == 4'd9) begin
                        if (u_second) begin
                            state    <= AXI_RW;
                            axi_step <= 3'd0;
                        end else begin
                            u_shift  <= {1'b1, 8'h4B, 1'b0};
                            u_bit    <= 4'd0;
                            u_second <= 1'b1;
                        end
                    end else begin
                        u_shift <= {1'b1, u_shift[9:1]};
                        u_bit   <= u_bit + 4'd1;
                    end
                end
            end
            AXI_RW: begin
                case (axi_step)
                    3'd0: begin
                        s_axi_awaddr  <= 8'h10;
                        s_axi_wdata   <= 16'hBEEF;
                        s_axi_awvalid <= 1'b1;
                        s_axi_wvalid  <= 1'b1;
                        s_axi_bready  <= 1'b1;
                        axi_step      <= 3'd1;
                    end
                    3'd1: begin
                        if (s_axi_awvalid & s_axi_awready) begin
                            s_axi_awvalid <= 1'b0;
                            s_axi_wvalid  <= 1'b0;
                            axi_step      <= 3'd2;
                        end
                    end
                    3'd2: begin
                        if (s_axi_bvalid & s_axi_bready) begin
                            s_axi_bready  <= 1'b0;
                            s_axi_araddr  <= 8'h10;
                            s_axi_arvalid <= 1'b1;
                            s_axi_rready  <= 1'b1;
                            axi_step      <= 3'd3;
                        end
                    end
                    3'd3: begin
                        if (s_axi_arvalid & s_axi_arready) begin
                            s_axi_arvalid <= 1'b0;
                            axi_step      <= 3'd4;
                        end
                    end
                    default: begin
                        if (s_axi_rvalid & s_axi_rready) begin
                            s_axi_rready <= 1'b0;
                            state        <= IDLE;
                        end
                    end
                endcase
            end
        endcase
    end

    // AXI-Lite slave: one register at 0x10, ready one cycle after valid.
    always_ff @(posedge clk) begin
        s_axi_awready <= s_axi_awvalid & s_axi_wvalid & ~s_axi_awready & ~s_axi_bvalid;
        s_axi_wready  <= s_axi_awvalid & s_axi_wvalid & ~s_axi_awready & ~s_axi_bvalid;
        if (s_axi_awvalid & s_axi_awready) begin
            if (s_axi_awaddr == 8'h10) reg10 <= s_axi_wdata;
            s_axi_bvalid <= 1'b1;
        end else if (s_axi_bvalid & s_axi_bready) begin
            s_axi_bvalid <= 1'b0;
        end
        s_axi_arready <= s_axi_arvalid & ~s_axi_arready & ~s_axi_rvalid;
        if (s_axi_arvalid & s_axi_arready) begin
            s_axi_rvalid <= 1'b1;
            s_axi_rdata  <= (s_axi_araddr == 8'h10) ? reg10 : 16'h0000;
        end else if (s_axi_rvalid & s_axi_rready) begin
            s_axi_rvalid <= 1'b0;
        end
    end
endmodule
