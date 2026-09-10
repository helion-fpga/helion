// Helion-ST registered clamp of 32b value into [lo,hi] inclusive (not AXI).
// Loads value/lo/hi via Helion-ST bytes (st_valid + byte_sel + which);
// on enable registers q = max(lo, min(hi, value)). Real FF+LUT fabric.
module h_clamp (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path (byte-wise into value / lo / hi)
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire [1:0]  byte_sel, // which byte of selected word
    input  wire [1:0]  which,    // 0=value, 1=lo, 2=hi
    input  wire        enable,   // compute & register clamped result
    output reg  [31:0] q,
    output wire        st_out_valid
);
    reg [31:0] val_r;
    reg [31:0] lo_r;
    reg [31:0] hi_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Unsigned clamp → LUT fabric
    wire [31:0] lo_eff = (lo_r <= hi_r) ? lo_r : hi_r;
    wire [31:0] hi_eff = (lo_r <= hi_r) ? hi_r : lo_r;
    wire below = (val_r < lo_eff);
    wire above = (val_r > hi_eff);
    wire [31:0] res = below ? lo_eff : (above ? hi_eff : val_r);

    always @(posedge clk) begin
        if (!resetn) begin
            val_r <= 32'h0;
            lo_r  <= 32'h0;
            hi_r  <= 32'hFFFF_FFFF;
            q     <= 32'h0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (which)
                    2'b00: begin
                        case (byte_sel)
                            2'b00: val_r[7:0]   <= st_data;
                            2'b01: val_r[15:8]  <= st_data;
                            2'b10: val_r[23:16] <= st_data;
                            2'b11: val_r[31:24] <= st_data;
                        endcase
                    end
                    2'b01: begin
                        case (byte_sel)
                            2'b00: lo_r[7:0]   <= st_data;
                            2'b01: lo_r[15:8]  <= st_data;
                            2'b10: lo_r[23:16] <= st_data;
                            2'b11: lo_r[31:24] <= st_data;
                        endcase
                    end
                    2'b10: begin
                        case (byte_sel)
                            2'b00: hi_r[7:0]   <= st_data;
                            2'b01: hi_r[15:8]  <= st_data;
                            2'b10: hi_r[23:16] <= st_data;
                            2'b11: hi_r[31:24] <= st_data;
                        endcase
                    end
                    default: ;
                endcase
            end else if (enable) begin
                q     <= res;
                out_v <= 1'b1;
            end
        end
    end
endmodule
