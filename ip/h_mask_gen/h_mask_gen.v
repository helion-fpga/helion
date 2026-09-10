// Helion-ST registered bitmask generator from width/offset (not AXI).
// Loads width[5:0] and offset[4:0] via Helion-ST (st_valid + which);
// on enable registers mask = ((1<<width)-1) << offset (width>=32 → all-1s
// then shifted). Real FF+LUT fabric.
module h_mask_gen (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire        which,    // 0=load width[5:0], 1=load offset[4:0]
    input  wire        enable,   // compute & register mask
    output reg  [31:0] mask,
    output wire        st_out_valid
);
    reg [5:0] width_r;
    reg [4:0] offset_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Build contiguous ones of length width, then shift by offset → LUTs
    // width==0 → 0; width>=32 → 32'hFFFF_FFFF before shift
    wire [31:0] ones =
          ({32{width_r == 6'd0}}  & 32'h0)
        | ({32{width_r == 6'd1}}  & 32'h0000_0001)
        | ({32{width_r == 6'd2}}  & 32'h0000_0003)
        | ({32{width_r == 6'd3}}  & 32'h0000_0007)
        | ({32{width_r == 6'd4}}  & 32'h0000_000F)
        | ({32{width_r == 6'd5}}  & 32'h0000_001F)
        | ({32{width_r == 6'd6}}  & 32'h0000_003F)
        | ({32{width_r == 6'd7}}  & 32'h0000_007F)
        | ({32{width_r == 6'd8}}  & 32'h0000_00FF)
        | ({32{width_r == 6'd9}}  & 32'h0000_01FF)
        | ({32{width_r == 6'd10}} & 32'h0000_03FF)
        | ({32{width_r == 6'd11}} & 32'h0000_07FF)
        | ({32{width_r == 6'd12}} & 32'h0000_0FFF)
        | ({32{width_r == 6'd13}} & 32'h0000_1FFF)
        | ({32{width_r == 6'd14}} & 32'h0000_3FFF)
        | ({32{width_r == 6'd15}} & 32'h0000_7FFF)
        | ({32{width_r == 6'd16}} & 32'h0000_FFFF)
        | ({32{width_r == 6'd17}} & 32'h0001_FFFF)
        | ({32{width_r == 6'd18}} & 32'h0003_FFFF)
        | ({32{width_r == 6'd19}} & 32'h0007_FFFF)
        | ({32{width_r == 6'd20}} & 32'h000F_FFFF)
        | ({32{width_r == 6'd21}} & 32'h001F_FFFF)
        | ({32{width_r == 6'd22}} & 32'h003F_FFFF)
        | ({32{width_r == 6'd23}} & 32'h007F_FFFF)
        | ({32{width_r == 6'd24}} & 32'h00FF_FFFF)
        | ({32{width_r == 6'd25}} & 32'h01FF_FFFF)
        | ({32{width_r == 6'd26}} & 32'h03FF_FFFF)
        | ({32{width_r == 6'd27}} & 32'h07FF_FFFF)
        | ({32{width_r == 6'd28}} & 32'h0FFF_FFFF)
        | ({32{width_r == 6'd29}} & 32'h1FFF_FFFF)
        | ({32{width_r == 6'd30}} & 32'h3FFF_FFFF)
        | ({32{width_r == 6'd31}} & 32'h7FFF_FFFF)
        | ({32{width_r >= 6'd32}} & 32'hFFFF_FFFF);

    wire [4:0] off = offset_r;
    wire [31:0] shifted =
          ({32{off == 5'd0}}  & ones)
        | ({32{off == 5'd1}}  & {ones[30:0], 1'b0})
        | ({32{off == 5'd2}}  & {ones[29:0], 2'b0})
        | ({32{off == 5'd3}}  & {ones[28:0], 3'b0})
        | ({32{off == 5'd4}}  & {ones[27:0], 4'b0})
        | ({32{off == 5'd5}}  & {ones[26:0], 5'b0})
        | ({32{off == 5'd6}}  & {ones[25:0], 6'b0})
        | ({32{off == 5'd7}}  & {ones[24:0], 7'b0})
        | ({32{off == 5'd8}}  & {ones[23:0], 8'b0})
        | ({32{off == 5'd9}}  & {ones[22:0], 9'b0})
        | ({32{off == 5'd10}} & {ones[21:0], 10'b0})
        | ({32{off == 5'd11}} & {ones[20:0], 11'b0})
        | ({32{off == 5'd12}} & {ones[19:0], 12'b0})
        | ({32{off == 5'd13}} & {ones[18:0], 13'b0})
        | ({32{off == 5'd14}} & {ones[17:0], 14'b0})
        | ({32{off == 5'd15}} & {ones[16:0], 15'b0})
        | ({32{off == 5'd16}} & {ones[15:0], 16'b0})
        | ({32{off == 5'd17}} & {ones[14:0], 17'b0})
        | ({32{off == 5'd18}} & {ones[13:0], 18'b0})
        | ({32{off == 5'd19}} & {ones[12:0], 19'b0})
        | ({32{off == 5'd20}} & {ones[11:0], 20'b0})
        | ({32{off == 5'd21}} & {ones[10:0], 21'b0})
        | ({32{off == 5'd22}} & {ones[9:0],  22'b0})
        | ({32{off == 5'd23}} & {ones[8:0],  23'b0})
        | ({32{off == 5'd24}} & {ones[7:0],  24'b0})
        | ({32{off == 5'd25}} & {ones[6:0],  25'b0})
        | ({32{off == 5'd26}} & {ones[5:0],  26'b0})
        | ({32{off == 5'd27}} & {ones[4:0],  27'b0})
        | ({32{off == 5'd28}} & {ones[3:0],  28'b0})
        | ({32{off == 5'd29}} & {ones[2:0],  29'b0})
        | ({32{off == 5'd30}} & {ones[1:0],  30'b0})
        | ({32{off == 5'd31}} & {ones[0],    31'b0});

    always @(posedge clk) begin
        if (!resetn) begin
            width_r  <= 6'h0;
            offset_r <= 5'h0;
            mask     <= 32'h0;
            out_v    <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                if (!which)
                    width_r  <= st_data[5:0];
                else
                    offset_r <= st_data[4:0];
            end else if (enable) begin
                mask  <= shifted;
                out_v <= 1'b1;
            end
        end
    end
endmodule
