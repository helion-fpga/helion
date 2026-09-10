// Helion-ST one CORDIC rotation step — registered x/y/z (not AXI).
// Circular rotation: if z>=0 → x-=y>>i, y+=x>>i, z-=atan; else opposite.
// st_valid+sel loads x(0)/y(1)/z(2)/atan(3); shift_amt = i; enable = one step.
module h_cordic_step (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path
    input  wire        st_valid,
    input  wire [15:0] st_data,
    output wire        st_ready,
    input  wire [1:0]  sel,       // 0=x 1=y 2=z 3=atan_i
    input  wire [3:0]  shift_amt, // iteration index i (0..15)
    input  wire        enable,    // perform one rotation step
    output reg  signed [15:0] x_out,
    output reg  signed [15:0] y_out,
    output reg  signed [15:0] z_out,
    output wire        st_out_valid
);
    reg signed [15:0] x_r;
    reg signed [15:0] y_r;
    reg signed [15:0] z_r;
    reg signed [15:0] atan_r;
    reg               out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Arithmetic right shifts of x/y by i → LUT+carry fabric
    wire signed [15:0] x_shr = x_r >>> shift_amt;
    wire signed [15:0] y_shr = y_r >>> shift_amt;
    wire               z_pos = ~z_r[15];

    wire signed [15:0] x_n = z_pos ? (x_r - y_shr) : (x_r + y_shr);
    wire signed [15:0] y_n = z_pos ? (y_r + x_shr) : (y_r - x_shr);
    wire signed [15:0] z_n = z_pos ? (z_r - atan_r) : (z_r + atan_r);

    always @(posedge clk) begin
        if (!resetn) begin
            x_r    <= 16'sd0;
            y_r    <= 16'sd0;
            z_r    <= 16'sd0;
            atan_r <= 16'sd0;
            x_out  <= 16'sd0;
            y_out  <= 16'sd0;
            z_out  <= 16'sd0;
            out_v  <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (sel)
                    2'd0: x_r    <= st_data;
                    2'd1: y_r    <= st_data;
                    2'd2: z_r    <= st_data;
                    default: atan_r <= st_data;
                endcase
            end else if (enable) begin
                x_r   <= x_n;
                y_r   <= y_n;
                z_r   <= z_n;
                x_out <= x_n;
                y_out <= y_n;
                z_out <= z_n;
                out_v <= 1'b1;
            end
        end
    end
endmodule
