// Helion-ST NCO phase accumulator — freq word + MSB carrier (not AXI).
// st_valid loads freq (sel=0) or phase (sel=1); enable: phase += freq.
// carrier = phase MSB (square-wave NCO out). Real FF+adder fabric.
module h_phase_accum (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path
    input  wire        st_valid,
    input  wire [15:0] st_data,
    output wire        st_ready,
    input  wire        sel,       // 0=freq_word 1=phase load
    input  wire        enable,    // accumulate one step
    output reg  [15:0] phase_out,
    output wire        carrier,   // phase MSB
    output wire        st_out_valid
);
    reg [15:0] freq_r;
    reg [15:0] phase_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign carrier      = phase_r[15];
    assign st_out_valid = out_v;

    // Combinational next-phase add → LUT+carry fabric
    wire [15:0] phase_n = phase_r + freq_r;

    always @(posedge clk) begin
        if (!resetn) begin
            freq_r    <= 16'h0001;
            phase_r   <= 16'h0;
            phase_out <= 16'h0;
            out_v     <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                if (sel)
                    phase_r <= st_data;
                else
                    freq_r  <= st_data;
                out_v <= 1'b1;
            end else if (enable) begin
                phase_r   <= phase_n;
                phase_out <= phase_n;
                out_v     <= 1'b1;
            end
        end
    end
endmodule
