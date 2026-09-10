// Helion-MM SPI master bit-engine — clk-div, shift reg, busy/done (not Xilinx AXI SPI).
module h_spi_mm (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output wire        sclk,
    output wire        mosi,
    input  wire        miso,
    output wire        cs_n
);
    // 0x00 TX write / RX+status read (bit8=busy, bit9=done)
    // 0x04 divider (low 8 bits), 0x08 ctrl (bit0=cs assert while busy)
    reg [7:0]  shift_r;
    reg [7:0]  rx_r;
    reg [7:0]  div_r;
    reg [7:0]  div_cnt;
    reg [3:0]  bit_idx;
    reg        busy;
    reg        done_r;
    reg        sclk_r;
    reg        mosi_r;
    reg        cs_r;
    reg        phase; // 0=drive, 1=sample edge within bit

    assign mm_ready = 1'b1;
    assign sclk     = sclk_r;
    assign mosi     = mosi_r;
    assign cs_n     = ~cs_r;

    always @(posedge clk) begin
        if (!resetn) begin
            shift_r  <= 8'h00;
            rx_r     <= 8'h00;
            div_r    <= 8'h01;
            div_cnt  <= 8'h00;
            bit_idx  <= 4'h0;
            busy     <= 1'b0;
            done_r   <= 1'b0;
            sclk_r   <= 1'b0;
            mosi_r   <= 1'b0;
            cs_r     <= 1'b0;
            phase    <= 1'b0;
            mm_rdata <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h0: begin
                        if (!busy) begin
                            shift_r <= mm_wdata[7:0];
                            bit_idx <= 4'h0;
                            busy    <= 1'b1;
                            done_r  <= 1'b0;
                            cs_r    <= 1'b1;
                            sclk_r  <= 1'b0;
                            phase   <= 1'b0;
                            mosi_r  <= mm_wdata[7];
                            div_cnt <= div_r;
                        end
                    end
                    4'h4: div_r <= (mm_wdata[7:0] == 8'h0) ? 8'h01 : mm_wdata[7:0];
                    4'h8: begin
                        if (mm_wdata[1]) done_r <= 1'b0;
                        if (!busy && !mm_wdata[0]) cs_r <= 1'b0;
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: mm_rdata <= {22'h0, done_r, busy, rx_r};
                    4'h4: mm_rdata <= {24'h0, div_r};
                    4'h8: mm_rdata <= {30'h0, done_r, cs_r};
                    default: mm_rdata <= 32'h0;
                endcase
            end

            if (busy) begin
                if (div_cnt != 8'h0)
                    div_cnt <= div_cnt - 8'h1;
                else begin
                    div_cnt <= div_r;
                    if (!phase) begin
                        // rising edge: sample MISO into LSB path
                        sclk_r  <= 1'b1;
                        rx_r    <= {rx_r[6:0], miso};
                        phase   <= 1'b1;
                    end else begin
                        // falling edge: shift next MOSI or finish
                        sclk_r <= 1'b0;
                        phase  <= 1'b0;
                        if (bit_idx == 4'd7) begin
                            busy   <= 1'b0;
                            done_r <= 1'b1;
                            cs_r   <= 1'b0;
                            mosi_r <= 1'b0;
                            bit_idx <= 4'h0;
                        end else begin
                            shift_r <= {shift_r[6:0], 1'b0};
                            mosi_r  <= shift_r[6];
                            bit_idx <= bit_idx + 4'h1;
                        end
                    end
                end
            end
        end
    end
endmodule
