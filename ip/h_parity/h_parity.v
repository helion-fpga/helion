// Helion-ST registered even/odd parity + sticky error over 32b (not AXI).
// Loads 32b via four Helion-ST bytes (st_valid + byte_sel). On enable
// registers parity = mode? odd : even of din; sticky_err sets when
// computed parity != expect (clear with clear_err). Real FF+LUT fabric.
module h_parity (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST load path (byte-wise into din)
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire [1:0] byte_sel, // which byte of din[31:0] to load
    input  wire       mode,     // 0=even parity, 1=odd parity
    input  wire       expect,   // expected parity bit for sticky compare
    input  wire       clear_err,// clear sticky_err
    input  wire       enable,   // compute & register parity
    output reg        parity,   // registered even/odd parity bit
    output reg        sticky_err,
    output wire       st_out_valid
);
    reg [31:0] din;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Combinational 32b XOR-reduce → LUTs
    wire even_p = ^din;
    wire odd_p  = ~even_p;
    wire par_w  = mode ? odd_p : even_p;

    always @(posedge clk) begin
        if (!resetn) begin
            din        <= 32'h0;
            parity     <= 1'b0;
            sticky_err <= 1'b0;
            out_v      <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (clear_err)
                sticky_err <= 1'b0;
            if (st_valid) begin
                case (byte_sel)
                    2'b00: din[7:0]   <= st_data;
                    2'b01: din[15:8]  <= st_data;
                    2'b10: din[23:16] <= st_data;
                    2'b11: din[31:24] <= st_data;
                endcase
            end else if (enable) begin
                parity <= par_w;
                if (par_w != expect)
                    sticky_err <= 1'b1;
                out_v <= 1'b1;
            end
        end
    end
endmodule
