// Helion-MM scoreboard / outstanding-id tracker — 4-entry sticky ID CAM (not AXI).
// Distinct from single-slot h_cam_slot. Allocate ID, complete/clear by ID, sticky hit.
// addr 0x0: alloc write (wdata[7:0]=id); 0x4: complete write (clear matching id);
// 0x8: probe key; 0xC: status {count[3:0], hit sticky, full, empty}.
// Real FF+LUT fabric for catalog proof.
module h_scoreboard (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output wire        hit,
    output wire        full,
    output wire        empty,
    output wire [3:0]  count,
    output wire        wr_strobe,
    output wire        cmp_strobe
);
    reg [7:0] id0, id1, id2, id3;
    reg       v0, v1, v2, v3;
    reg       hit_r;
    reg       wr_r;
    reg       cmp_r;

    wire [3:0] occ = {v3, v2, v1, v0};
    wire [2:0] cnt_w = {2'b0, v0} + {2'b0, v1} + {2'b0, v2} + {2'b0, v3};

    assign mm_ready  = 1'b1;
    assign hit       = hit_r;
    assign full      = &occ;
    assign empty     = ~|occ;
    assign count     = {1'b0, cnt_w};
    assign wr_strobe = wr_r;
    assign cmp_strobe = cmp_r;

    always @(posedge clk) begin
        if (!resetn) begin
            id0 <= 8'h0; id1 <= 8'h0; id2 <= 8'h0; id3 <= 8'h0;
            v0  <= 1'b0; v1  <= 1'b0; v2  <= 1'b0; v3  <= 1'b0;
            hit_r    <= 1'b0;
            mm_rdata <= 32'h0;
            wr_r     <= 1'b0;
            cmp_r    <= 1'b0;
        end else begin
            wr_r  <= 1'b0;
            cmp_r <= 1'b0;

            if (mm_valid && mm_write) begin
                case (mm_addr[3:0])
                    4'h0: begin
                        // allocate: take first free slot
                        if (!v0) begin id0 <= mm_wdata[7:0]; v0 <= 1'b1; wr_r <= 1'b1; end
                        else if (!v1) begin id1 <= mm_wdata[7:0]; v1 <= 1'b1; wr_r <= 1'b1; end
                        else if (!v2) begin id2 <= mm_wdata[7:0]; v2 <= 1'b1; wr_r <= 1'b1; end
                        else if (!v3) begin id3 <= mm_wdata[7:0]; v3 <= 1'b1; wr_r <= 1'b1; end
                    end
                    4'h4: begin
                        // complete: clear matching valid id
                        if (v0 && (id0 == mm_wdata[7:0])) v0 <= 1'b0;
                        if (v1 && (id1 == mm_wdata[7:0])) v1 <= 1'b0;
                        if (v2 && (id2 == mm_wdata[7:0])) v2 <= 1'b0;
                        if (v3 && (id3 == mm_wdata[7:0])) v3 <= 1'b0;
                        wr_r <= 1'b1;
                    end
                    4'h8: begin
                        // probe: sticky hit if any valid slot matches
                        cmp_r <= 1'b1;
                        if ((v0 && (id0 == mm_wdata[7:0])) ||
                            (v1 && (id1 == mm_wdata[7:0])) ||
                            (v2 && (id2 == mm_wdata[7:0])) ||
                            (v3 && (id3 == mm_wdata[7:0])))
                            hit_r <= 1'b1;
                    end
                    4'hC: begin
                        if (mm_wdata[0]) hit_r <= 1'b0;
                        if (mm_wdata[1]) begin
                            v0 <= 1'b0; v1 <= 1'b0; v2 <= 1'b0; v3 <= 1'b0;
                        end
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (mm_addr[3:0])
                    4'h0: mm_rdata <= {24'h0, id0};
                    4'h4: mm_rdata <= {24'h0, id1};
                    4'h8: mm_rdata <= {8'h0, id3, id2};
                    4'hC: mm_rdata <= {24'h0, count, hit_r, full, empty, 1'b0};
                    default: mm_rdata <= 32'h0;
                endcase
            end
        end
    end
endmodule
