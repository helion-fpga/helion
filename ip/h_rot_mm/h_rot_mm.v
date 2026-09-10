// Helion-MM programmed rotate left/right by N on 32b data (not AXI).
// Addr map (word via mm_addr[3:2]): 0=data (r/w), 1=amount (w, low 5b),
// 2=ctrl (w: [0]=dir 0=left/1=right, [1]=go pulse sticky until consumed),
// 3=result (r). On go: result <= rotate(data, amount[4:0]). Real FF+LUT fabric.
module h_rot_mm (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output reg  [31:0] result,
    output wire        busy
);
    reg [31:0] data_r;
    reg [4:0]  amt_r;
    reg        dir_r;   // 0=left, 1=right
    reg        do_rot;

    assign mm_ready = 1'b1;
    assign busy     = do_rot;

    wire [1:0] sel = mm_addr[3:2];

    // Barrel rotate via shift+OR — denser LUT fabric than single-bit shift
    wire [31:0] d = data_r;
    wire [4:0]  n = amt_r;
    wire [31:0] rol =
          ({32{n == 5'd0}}  & d)
        | ({32{n == 5'd1}}  & {d[30:0], d[31]})
        | ({32{n == 5'd2}}  & {d[29:0], d[31:30]})
        | ({32{n == 5'd3}}  & {d[28:0], d[31:29]})
        | ({32{n == 5'd4}}  & {d[27:0], d[31:28]})
        | ({32{n == 5'd5}}  & {d[26:0], d[31:27]})
        | ({32{n == 5'd6}}  & {d[25:0], d[31:26]})
        | ({32{n == 5'd7}}  & {d[24:0], d[31:25]})
        | ({32{n == 5'd8}}  & {d[23:0], d[31:24]})
        | ({32{n == 5'd9}}  & {d[22:0], d[31:23]})
        | ({32{n == 5'd10}} & {d[21:0], d[31:22]})
        | ({32{n == 5'd11}} & {d[20:0], d[31:21]})
        | ({32{n == 5'd12}} & {d[19:0], d[31:20]})
        | ({32{n == 5'd13}} & {d[18:0], d[31:19]})
        | ({32{n == 5'd14}} & {d[17:0], d[31:18]})
        | ({32{n == 5'd15}} & {d[16:0], d[31:17]})
        | ({32{n == 5'd16}} & {d[15:0], d[31:16]})
        | ({32{n == 5'd17}} & {d[14:0], d[31:15]})
        | ({32{n == 5'd18}} & {d[13:0], d[31:14]})
        | ({32{n == 5'd19}} & {d[12:0], d[31:13]})
        | ({32{n == 5'd20}} & {d[11:0], d[31:12]})
        | ({32{n == 5'd21}} & {d[10:0], d[31:11]})
        | ({32{n == 5'd22}} & {d[9:0],  d[31:10]})
        | ({32{n == 5'd23}} & {d[8:0],  d[31:9]})
        | ({32{n == 5'd24}} & {d[7:0],  d[31:8]})
        | ({32{n == 5'd25}} & {d[6:0],  d[31:7]})
        | ({32{n == 5'd26}} & {d[5:0],  d[31:6]})
        | ({32{n == 5'd27}} & {d[4:0],  d[31:5]})
        | ({32{n == 5'd28}} & {d[3:0],  d[31:4]})
        | ({32{n == 5'd29}} & {d[2:0],  d[31:3]})
        | ({32{n == 5'd30}} & {d[1:0],  d[31:2]})
        | ({32{n == 5'd31}} & {d[0],    d[31:1]});
    wire [31:0] ror =
          ({32{n == 5'd0}}  & d)
        | ({32{n == 5'd1}}  & {d[0],    d[31:1]})
        | ({32{n == 5'd2}}  & {d[1:0],  d[31:2]})
        | ({32{n == 5'd3}}  & {d[2:0],  d[31:3]})
        | ({32{n == 5'd4}}  & {d[3:0],  d[31:4]})
        | ({32{n == 5'd5}}  & {d[4:0],  d[31:5]})
        | ({32{n == 5'd6}}  & {d[5:0],  d[31:6]})
        | ({32{n == 5'd7}}  & {d[6:0],  d[31:7]})
        | ({32{n == 5'd8}}  & {d[7:0],  d[31:8]})
        | ({32{n == 5'd9}}  & {d[8:0],  d[31:9]})
        | ({32{n == 5'd10}} & {d[9:0],  d[31:10]})
        | ({32{n == 5'd11}} & {d[10:0], d[31:11]})
        | ({32{n == 5'd12}} & {d[11:0], d[31:12]})
        | ({32{n == 5'd13}} & {d[12:0], d[31:13]})
        | ({32{n == 5'd14}} & {d[13:0], d[31:14]})
        | ({32{n == 5'd15}} & {d[14:0], d[31:15]})
        | ({32{n == 5'd16}} & {d[15:0], d[31:16]})
        | ({32{n == 5'd17}} & {d[16:0], d[31:17]})
        | ({32{n == 5'd18}} & {d[17:0], d[31:18]})
        | ({32{n == 5'd19}} & {d[18:0], d[31:19]})
        | ({32{n == 5'd20}} & {d[19:0], d[31:20]})
        | ({32{n == 5'd21}} & {d[20:0], d[31:21]})
        | ({32{n == 5'd22}} & {d[21:0], d[31:22]})
        | ({32{n == 5'd23}} & {d[22:0], d[31:23]})
        | ({32{n == 5'd24}} & {d[23:0], d[31:24]})
        | ({32{n == 5'd25}} & {d[24:0], d[31:25]})
        | ({32{n == 5'd26}} & {d[25:0], d[31:26]})
        | ({32{n == 5'd27}} & {d[26:0], d[31:27]})
        | ({32{n == 5'd28}} & {d[27:0], d[31:28]})
        | ({32{n == 5'd29}} & {d[28:0], d[31:29]})
        | ({32{n == 5'd30}} & {d[29:0], d[31:30]})
        | ({32{n == 5'd31}} & {d[30:0], d[31]});
    wire [31:0] rot_w = dir_r ? ror : rol;

    always @(posedge clk) begin
        if (!resetn) begin
            data_r   <= 32'h0;
            amt_r    <= 5'h0;
            dir_r    <= 1'b0;
            do_rot   <= 1'b0;
            result   <= 32'h0;
            mm_rdata <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (sel)
                    2'b00: data_r <= mm_wdata;
                    2'b01: amt_r  <= mm_wdata[4:0];
                    2'b10: begin
                        dir_r <= mm_wdata[0];
                        if (mm_wdata[1])
                            do_rot <= 1'b1;
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (sel)
                    2'b00: mm_rdata <= data_r;
                    2'b01: mm_rdata <= {27'h0, amt_r};
                    2'b10: mm_rdata <= {30'h0, do_rot, dir_r};
                    2'b11: mm_rdata <= result;
                endcase
            end

            if (do_rot) begin
                result <= rot_w;
                do_rot <= 1'b0;
            end
        end
    end
endmodule
