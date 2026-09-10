// Helion-ST registered 16→4 priority encoder + valid (not AXI).
// Samples req[15:0] on st_valid; on enable encodes highest-priority set bit
// (MSB wins) into code[3:0] with valid sticky. Real FF+LUT fabric.
module h_prio_enc (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST request path
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire        hi_byte,  // 0=req[7:0], 1=req[15:8]
    input  wire        enable,   // compute & register encode
    output reg  [3:0]  code,     // binary index of winning request
    output reg         valid,    // 1 if any request was set
    output wire        st_out_valid
);
    reg [15:0] req_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // MSB-first priority encode → LUT fabric
    wire       any = |req_r;
    wire [3:0] enc =
        req_r[15] ? 4'd15 :
        req_r[14] ? 4'd14 :
        req_r[13] ? 4'd13 :
        req_r[12] ? 4'd12 :
        req_r[11] ? 4'd11 :
        req_r[10] ? 4'd10 :
        req_r[9]  ? 4'd9  :
        req_r[8]  ? 4'd8  :
        req_r[7]  ? 4'd7  :
        req_r[6]  ? 4'd6  :
        req_r[5]  ? 4'd5  :
        req_r[4]  ? 4'd4  :
        req_r[3]  ? 4'd3  :
        req_r[2]  ? 4'd2  :
        req_r[1]  ? 4'd1  :
        req_r[0]  ? 4'd0  :
                    4'd0;

    always @(posedge clk) begin
        if (!resetn) begin
            req_r <= 16'h0;
            code  <= 4'h0;
            valid <= 1'b0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                if (hi_byte)
                    req_r[15:8] <= st_data;
                else
                    req_r[7:0]  <= st_data;
            end else if (enable) begin
                code  <= enc;
                valid <= any;
                out_v <= 1'b1;
            end
        end
    end
endmodule
