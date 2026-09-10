// Helion-ST sync FIFO — depth 8, width 8 (not AXI stream / not Xilinx FIFO).
module h_sync_fifo (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST write (ingress)
    input  wire       wr_valid,
    input  wire [7:0] wr_data,
    output wire       wr_ready,
    // Helion-ST read (egress)
    output wire       rd_valid,
    output wire [7:0] rd_data,
    input  wire       rd_ready,
    output wire       full,
    output wire       empty
);
    reg [7:0] mem0, mem1, mem2, mem3, mem4, mem5, mem6, mem7;
    reg [2:0] wr_ptr;
    reg [2:0] rd_ptr;
    reg [3:0] count;

    wire do_wr = wr_valid && wr_ready;
    wire do_rd = rd_valid && rd_ready;

    assign empty    = (count == 4'd0);
    assign full     = (count == 4'd8);
    assign wr_ready = !full;
    assign rd_valid = !empty;

    reg [7:0] rd_data_r;
    assign rd_data = rd_data_r;

    always @(*) begin
        case (rd_ptr)
            3'd0: rd_data_r = mem0;
            3'd1: rd_data_r = mem1;
            3'd2: rd_data_r = mem2;
            3'd3: rd_data_r = mem3;
            3'd4: rd_data_r = mem4;
            3'd5: rd_data_r = mem5;
            3'd6: rd_data_r = mem6;
            default: rd_data_r = mem7;
        endcase
    end

    always @(posedge clk) begin
        if (!resetn) begin
            wr_ptr <= 3'd0;
            rd_ptr <= 3'd0;
            count  <= 4'd0;
            mem0 <= 8'h0; mem1 <= 8'h0; mem2 <= 8'h0; mem3 <= 8'h0;
            mem4 <= 8'h0; mem5 <= 8'h0; mem6 <= 8'h0; mem7 <= 8'h0;
        end else begin
            if (do_wr) begin
                case (wr_ptr)
                    3'd0: mem0 <= wr_data;
                    3'd1: mem1 <= wr_data;
                    3'd2: mem2 <= wr_data;
                    3'd3: mem3 <= wr_data;
                    3'd4: mem4 <= wr_data;
                    3'd5: mem5 <= wr_data;
                    3'd6: mem6 <= wr_data;
                    default: mem7 <= wr_data;
                endcase
                wr_ptr <= wr_ptr + 3'd1;
            end
            if (do_rd)
                rd_ptr <= rd_ptr + 3'd1;
            case ({do_wr, do_rd})
                2'b10: count <= count + 4'd1;
                2'b01: count <= count - 4'd1;
                default: count <= count;
            endcase
        end
    end
endmodule
