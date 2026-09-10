// Helion-MM single-slot mailbox — write data, sticky full/empty, read clears (not AXI).
// Write (mm_valid && mm_write) stores mm_wdata and sets sticky full (clears empty).
// Read (mm_valid && !mm_write) returns data and clears full (sets empty). Real FF+LUT.
module h_mailbox_mm (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output wire        full,
    output wire        empty,
    output wire        wr_strobe,
    output wire        rd_strobe
);
    reg [31:0] slot_r;
    reg        full_r;
    reg        wr_r;
    reg        rd_r;

    assign mm_ready  = 1'b1;
    assign full      = full_r;
    assign empty     = !full_r;
    assign wr_strobe = wr_r;
    assign rd_strobe = rd_r;

    // mm_addr[3:0]: 0x0=data, 0x4=status {empty,full}
    always @(posedge clk) begin
        if (!resetn) begin
            slot_r   <= 32'h0;
            full_r   <= 1'b0;
            mm_rdata <= 32'h0;
            wr_r     <= 1'b0;
            rd_r     <= 1'b0;
        end else begin
            wr_r <= 1'b0;
            rd_r <= 1'b0;

            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h0: begin
                        slot_r <= mm_wdata;
                        full_r <= 1'b1;
                        wr_r   <= 1'b1;
                    end
                    4'h4: begin
                        // status write bit0 clears full (force empty)
                        if (mm_wdata[0])
                            full_r <= 1'b0;
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: begin
                        mm_rdata <= slot_r;
                        // read clears full → empty sticky
                        full_r   <= 1'b0;
                        rd_r     <= 1'b1;
                    end
                    4'h4: mm_rdata <= {30'h0, !full_r, full_r};
                    default: mm_rdata <= 32'h0;
                endcase
            end
        end
    end
endmodule
