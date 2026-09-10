// Helion-MM SPI slave — shift-in on sclk, sticky RX byte + status (not AXI SPI).
// Mode 0: sample MOSI on rising sclk while cs_n low; MISO shifts on falling.
// MM: 0x00 RX+status (bit8=busy, bit9=done sticky), 0x04 TX preload, 0x08 ctrl clear.
module h_spi_slave (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    input  wire        sclk,
    input  wire        mosi,
    output wire        miso,
    input  wire        cs_n
);
    reg [7:0]  rx_shift;
    reg [7:0]  rx_sticky;
    reg [7:0]  tx_shift;
    reg [7:0]  tx_preload;
    reg [3:0]  bit_cnt;
    reg        busy;
    reg        done_r;
    reg        sclk_d;
    reg        cs_d;
    reg        miso_r;
    reg        active;

    assign mm_ready = 1'b1;
    assign miso     = miso_r;

    wire sclk_rise =  sclk & ~sclk_d;
    wire sclk_fall = ~sclk &  sclk_d;
    wire cs_assert = ~cs_n & cs_d;   // falling cs_n
    wire cs_release = cs_n & ~cs_d;  // rising cs_n

    always @(posedge clk) begin
        if (!resetn) begin
            rx_shift   <= 8'h00;
            rx_sticky  <= 8'h00;
            tx_shift   <= 8'h00;
            tx_preload <= 8'hFF;
            bit_cnt    <= 4'h0;
            busy       <= 1'b0;
            done_r     <= 1'b0;
            sclk_d     <= 1'b0;
            cs_d       <= 1'b1;
            miso_r     <= 1'b0;
            active     <= 1'b0;
            mm_rdata   <= 32'h0;
        end else begin
            sclk_d <= sclk;
            cs_d   <= cs_n;

            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h4: tx_preload <= mm_wdata[7:0];
                    4'h8: begin
                        if (mm_wdata[1]) done_r <= 1'b0;
                        if (mm_wdata[2]) rx_sticky <= 8'h00;
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: mm_rdata <= {22'h0, done_r, busy, rx_sticky};
                    4'h4: mm_rdata <= {24'h0, tx_preload};
                    4'h8: mm_rdata <= {30'h0, done_r, busy};
                    default: mm_rdata <= 32'h0;
                endcase
            end

            if (cs_assert) begin
                active   <= 1'b1;
                busy     <= 1'b1;
                bit_cnt  <= 4'h0;
                rx_shift <= 8'h00;
                tx_shift <= tx_preload;
                miso_r   <= tx_preload[7];
                done_r   <= 1'b0;
            end else if (cs_release) begin
                active <= 1'b0;
                busy   <= 1'b0;
                miso_r <= 1'b0;
            end else if (active && !cs_n) begin
                if (sclk_rise) begin
                    rx_shift <= {rx_shift[6:0], mosi};
                    if (bit_cnt == 4'd7) begin
                        rx_sticky <= {rx_shift[6:0], mosi};
                        done_r    <= 1'b1;
                        busy      <= 1'b0;
                        active    <= 1'b0;
                        bit_cnt   <= 4'h0;
                    end else begin
                        bit_cnt <= bit_cnt + 4'h1;
                    end
                end else if (sclk_fall) begin
                    tx_shift <= {tx_shift[6:0], 1'b0};
                    miso_r   <= tx_shift[6];
                end
            end
        end
    end
endmodule
