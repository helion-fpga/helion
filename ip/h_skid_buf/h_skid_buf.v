// Helion-ST 1-deep skid buffer — valid/ready style (not AXI, not AXIS).
// Decouples upstream (wr_*) from downstream (rd_*) with one holding register
// so wr_ready can stay high while rd_ready is low for one beat.
// Naming is Helion-ST (wr_/rd_), never AXI/s_axis/m_axis.
module h_skid_buf (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST ingress (upstream)
    input  wire       wr_valid,
    input  wire [7:0] wr_data,
    output wire       wr_ready,
    // Helion-ST egress (downstream)
    output wire       rd_valid,
    output wire [7:0] rd_data,
    input  wire       rd_ready
);
    // buf_valid: skid holding register occupied
    reg       buf_valid;
    reg [7:0] buf_data;
    // pipe_valid: primary output register occupied
    reg       pipe_valid;
    reg [7:0] pipe_data;

    // Upstream can accept when skid is empty (or will drain this cycle)
    assign wr_ready = !buf_valid;
    assign rd_valid = pipe_valid;
    assign rd_data  = pipe_data;

    wire do_wr   = wr_valid && wr_ready;
    wire do_rd   = rd_valid && rd_ready;
    // Load pipe from skid when pipe frees and skid has data
    wire load_pipe_from_buf = buf_valid && (!pipe_valid || do_rd);
    // Load pipe from wr when pipe free/draining and skid empty
    wire load_pipe_from_wr  = do_wr && (!pipe_valid || do_rd) && !buf_valid;
    // Park into skid when pipe is full and not draining
    wire load_buf_from_wr   = do_wr && pipe_valid && !do_rd;

    always @(posedge clk) begin
        if (!resetn) begin
            buf_valid  <= 1'b0;
            buf_data   <= 8'h0;
            pipe_valid <= 1'b0;
            pipe_data  <= 8'h0;
        end else begin
            // Skid holding register
            if (load_buf_from_wr) begin
                buf_valid <= 1'b1;
                buf_data  <= wr_data;
            end else if (load_pipe_from_buf) begin
                buf_valid <= 1'b0;
            end

            // Primary pipe register
            if (load_pipe_from_wr) begin
                pipe_valid <= 1'b1;
                pipe_data  <= wr_data;
            end else if (load_pipe_from_buf) begin
                pipe_valid <= 1'b1;
                pipe_data  <= buf_data;
            end else if (do_rd) begin
                pipe_valid <= 1'b0;
            end
        end
    end
endmodule
