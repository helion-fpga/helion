// Helion-ST single MAC FIR tap — registered acc += x*coeff (not AXI).
// st_valid loads x(sel=0) / coeff(sel=1) / clears acc(sel=2 via st_data ignored).
// enable: acc += signed(x)*signed(coeff); out = acc[31:16] (Q15-ish).
module h_fir_tap (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path
    input  wire        st_valid,
    input  wire [15:0] st_data,
    output wire        st_ready,
    input  wire [1:0]  sel,       // 0=x 1=coeff 2=clear_acc 3=load_acc_hi
    input  wire        enable,    // one MAC step
    output reg  signed [15:0] y_out,
    output wire        st_out_valid
);
    reg signed [15:0] x_r;
    reg signed [15:0] coeff_r;
    reg signed [31:0] acc_r;
    reg               out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Combinational MAC product → LUT fabric (no DSP/Xilinx IP)
    wire signed [31:0] prod  = x_r * coeff_r;
    wire signed [31:0] acc_n = acc_r + prod;

    always @(posedge clk) begin
        if (!resetn) begin
            x_r     <= 16'sd0;
            coeff_r <= 16'sd0;
            acc_r   <= 32'sd0;
            y_out   <= 16'sd0;
            out_v   <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (sel)
                    2'd0: x_r     <= st_data;
                    2'd1: coeff_r <= st_data;
                    2'd2: begin
                        acc_r <= 32'sd0;
                        y_out <= 16'sd0;
                    end
                    default: acc_r[31:16] <= st_data;
                endcase
                out_v <= 1'b1;
            end else if (enable) begin
                acc_r <= acc_n;
                y_out <= acc_n[31:16];
                out_v <= 1'b1;
            end
        end
    end
endmodule
