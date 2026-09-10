// Helion-ST registered 4:1 mux on 32b + sel (not AXI).
// Loads D0..D3 via Helion-ST bytes (st_valid + byte_sel + which);
// on enable registers q = Dx[sel]. Real FF+LUT fabric.
module h_mux4 (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path (byte-wise into D0..D3)
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire [1:0]  byte_sel, // which byte of selected operand
    input  wire [1:0]  which,    // 0=D0, 1=D1, 2=D2, 3=D3
    input  wire [1:0]  sel,      // mux select
    input  wire        enable,   // compute & register mux result
    output reg  [31:0] q,
    output wire        st_out_valid
);
    reg [31:0] d0_r;
    reg [31:0] d1_r;
    reg [31:0] d2_r;
    reg [31:0] d3_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // 4:1 mux → LUT fabric
    wire [31:0] res =
        (sel == 2'b00) ? d0_r :
        (sel == 2'b01) ? d1_r :
        (sel == 2'b10) ? d2_r : d3_r;

    always @(posedge clk) begin
        if (!resetn) begin
            d0_r  <= 32'h0;
            d1_r  <= 32'h0;
            d2_r  <= 32'h0;
            d3_r  <= 32'h0;
            q     <= 32'h0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (which)
                    2'b00: begin
                        case (byte_sel)
                            2'b00: d0_r[7:0]   <= st_data;
                            2'b01: d0_r[15:8]  <= st_data;
                            2'b10: d0_r[23:16] <= st_data;
                            2'b11: d0_r[31:24] <= st_data;
                        endcase
                    end
                    2'b01: begin
                        case (byte_sel)
                            2'b00: d1_r[7:0]   <= st_data;
                            2'b01: d1_r[15:8]  <= st_data;
                            2'b10: d1_r[23:16] <= st_data;
                            2'b11: d1_r[31:24] <= st_data;
                        endcase
                    end
                    2'b10: begin
                        case (byte_sel)
                            2'b00: d2_r[7:0]   <= st_data;
                            2'b01: d2_r[15:8]  <= st_data;
                            2'b10: d2_r[23:16] <= st_data;
                            2'b11: d2_r[31:24] <= st_data;
                        endcase
                    end
                    2'b11: begin
                        case (byte_sel)
                            2'b00: d3_r[7:0]   <= st_data;
                            2'b01: d3_r[15:8]  <= st_data;
                            2'b10: d3_r[23:16] <= st_data;
                            2'b11: d3_r[31:24] <= st_data;
                        endcase
                    end
                endcase
            end else if (enable) begin
                q     <= res;
                out_v <= 1'b1;
            end
        end
    end
endmodule
