// Helion-MM UART — registered TX shift + status (not AXI UART).
module h_uart (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output wire        tx
);
    // addr 0x00: TX data write / status read (bit0=busy)
    // addr 0x04: divider (low 8 bits used)
    reg [7:0]  tx_shift;
    reg [3:0]  bit_idx;
    reg        busy;
    reg        tx_r;
    reg [7:0]  div_r;
    reg [7:0]  div_cnt;

    assign mm_ready = 1'b1;
    assign tx = tx_r;

    always @(posedge clk) begin
        if (!resetn) begin
            tx_shift <= 8'h00;
            bit_idx  <= 4'h0;
            busy     <= 1'b0;
            tx_r     <= 1'b1;
            div_r    <= 8'h01;
            div_cnt  <= 8'h00;
            mm_rdata <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h0: begin
                        if (!busy) begin
                            tx_shift <= mm_wdata[7:0];
                            bit_idx  <= 4'h0;
                            busy     <= 1'b1;
                            tx_r     <= 1'b0; // start bit
                            div_cnt  <= div_r;
                        end
                    end
                    4'h4: div_r <= (mm_wdata[7:0] == 8'h0) ? 8'h01 : mm_wdata[7:0];
                    default: ;
                endcase
            end
            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: mm_rdata <= {31'h0, busy};
                    4'h4: mm_rdata <= {24'h0, div_r};
                    default: mm_rdata <= 32'h0;
                endcase
            end
            if (busy) begin
                if (div_cnt != 8'h0)
                    div_cnt <= div_cnt - 8'h1;
                else begin
                    div_cnt <= div_r;
                    if (bit_idx < 4'd8) begin
                        tx_r     <= tx_shift[0];
                        tx_shift <= {1'b0, tx_shift[7:1]};
                        bit_idx  <= bit_idx + 4'h1;
                    end else begin
                        tx_r  <= 1'b1; // stop
                        busy  <= 1'b0;
                        bit_idx <= 4'h0;
                    end
                end
            end
        end
    end
endmodule
