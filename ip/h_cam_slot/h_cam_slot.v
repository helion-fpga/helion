// Helion-MM tiny CAM/tag compare slot — write tag/data, sticky match (not AXI).
// addr 0x0: tag write/read; 0x4: data write/read; 0x8: probe key write (compare);
// 0xC: status {match sticky}; write status bit0 clears match.
// Real FF+LUT fabric for catalog proof.
module h_cam_slot (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output wire        match,
    output wire        valid_tag,
    output wire        wr_strobe,
    output wire        cmp_strobe
);
    reg [31:0] tag_r;
    reg [31:0] data_r;
    reg        tag_v_r;
    reg        match_r;
    reg        wr_r;
    reg        cmp_r;

    assign mm_ready  = 1'b1;
    assign match     = match_r;
    assign valid_tag = tag_v_r;
    assign wr_strobe = wr_r;
    assign cmp_strobe = cmp_r;

    always @(posedge clk) begin
        if (!resetn) begin
            tag_r    <= 32'h0;
            data_r   <= 32'h0;
            tag_v_r  <= 1'b0;
            match_r  <= 1'b0;
            mm_rdata <= 32'h0;
            wr_r     <= 1'b0;
            cmp_r    <= 1'b0;
        end else begin
            wr_r  <= 1'b0;
            cmp_r <= 1'b0;

            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h0: begin
                        tag_r   <= mm_wdata;
                        tag_v_r <= 1'b1;
                        wr_r    <= 1'b1;
                    end
                    4'h4: begin
                        data_r <= mm_wdata;
                        wr_r   <= 1'b1;
                    end
                    4'h8: begin
                        // probe: sticky match if tag valid and equal
                        cmp_r <= 1'b1;
                        if (tag_v_r && (mm_wdata == tag_r))
                            match_r <= 1'b1;
                    end
                    4'hC: begin
                        if (mm_wdata[0])
                            match_r <= 1'b0;
                        if (mm_wdata[1])
                            tag_v_r <= 1'b0;
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: mm_rdata <= tag_r;
                    4'h4: mm_rdata <= data_r;
                    4'h8: mm_rdata <= tag_r; // last tag (readback)
                    4'hC: mm_rdata <= {30'h0, tag_v_r, match_r};
                    default: mm_rdata <= 32'h0;
                endcase
            end
        end
    end
endmodule
