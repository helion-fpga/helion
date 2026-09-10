// Helion-MM UART RX — baud-div bit sampler, start/data/stop, sticky byte (not AXI UART).
// Distinct from h_uart (TX). Samples rx through mid-bit baud counter; sticky rx_valid.
module h_uart_rx (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    input  wire        rx,
    output wire        rx_valid,
    output wire        framing_err
);
    // addr 0x00: RX data read (clears sticky) / status {ferr,valid,busy}
    // addr 0x04: baud divider (low 8 bits; 0→1)
    localparam [1:0] S_IDLE  = 2'd0;
    localparam [1:0] S_START = 2'd1;
    localparam [1:0] S_DATA  = 2'd2;
    localparam [1:0] S_STOP  = 2'd3;

    reg [7:0]  div_r;
    reg [7:0]  div_cnt;
    reg [1:0]  state;
    reg [2:0]  bit_idx;
    reg [7:0]  shift_r;
    reg [7:0]  byte_r;
    reg        valid_r;
    reg        busy_r;
    reg        ferr_r;
    reg        rx_sync0;
    reg        rx_sync1;

    assign mm_ready    = 1'b1;
    assign rx_valid    = valid_r;
    assign framing_err = ferr_r;

    // 2FF sync of async RX line → real flops
    always @(posedge clk) begin
        if (!resetn) begin
            rx_sync0 <= 1'b1;
            rx_sync1 <= 1'b1;
        end else begin
            rx_sync0 <= rx;
            rx_sync1 <= rx_sync0;
        end
    end

    always @(posedge clk) begin
        if (!resetn) begin
            div_r    <= 8'h01;
            div_cnt  <= 8'h00;
            state    <= S_IDLE;
            bit_idx  <= 3'h0;
            shift_r  <= 8'h00;
            byte_r   <= 8'h00;
            valid_r  <= 1'b0;
            busy_r   <= 1'b0;
            ferr_r   <= 1'b0;
            mm_rdata <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h0: begin
                        // status write: bit0 clears valid, bit1 clears ferr
                        if (mm_wdata[0]) valid_r <= 1'b0;
                        if (mm_wdata[1]) ferr_r  <= 1'b0;
                    end
                    4'h4: div_r <= (mm_wdata[7:0] == 8'h0) ? 8'h01 : mm_wdata[7:0];
                    default: ;
                endcase
            end
            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: begin
                        mm_rdata <= {16'h0, byte_r, 5'h0, ferr_r, valid_r, busy_r};
                        // sticky clear on data read
                        valid_r <= 1'b0;
                    end
                    4'h4: mm_rdata <= {24'h0, div_r};
                    default: mm_rdata <= 32'h0;
                endcase
            end

            case (state)
                S_IDLE: begin
                    busy_r <= 1'b0;
                    if (!rx_sync1) begin
                        // start-bit falling edge → wait half baud then sample
                        state   <= S_START;
                        busy_r  <= 1'b1;
                        div_cnt <= {1'b0, div_r[7:1]}; // ~half period
                        bit_idx <= 3'h0;
                    end
                end
                S_START: begin
                    if (div_cnt != 8'h0)
                        div_cnt <= div_cnt - 8'h1;
                    else begin
                        // mid-start sample: must still be low
                        if (rx_sync1) begin
                            state  <= S_IDLE;
                            busy_r <= 1'b0;
                        end else begin
                            state   <= S_DATA;
                            div_cnt <= div_r;
                            bit_idx <= 3'h0;
                        end
                    end
                end
                S_DATA: begin
                    if (div_cnt != 8'h0)
                        div_cnt <= div_cnt - 8'h1;
                    else begin
                        div_cnt <= div_r;
                        shift_r <= {rx_sync1, shift_r[7:1]};
                        if (bit_idx == 3'd7) begin
                            state   <= S_STOP;
                            bit_idx <= 3'h0;
                        end else
                            bit_idx <= bit_idx + 3'h1;
                    end
                end
                default: begin // S_STOP
                    if (div_cnt != 8'h0)
                        div_cnt <= div_cnt - 8'h1;
                    else begin
                        if (!rx_sync1)
                            ferr_r <= 1'b1;
                        byte_r  <= shift_r;
                        valid_r <= 1'b1;
                        busy_r  <= 1'b0;
                        state   <= S_IDLE;
                    end
                end
            endcase
        end
    end
endmodule
