// Helion-ST restoring divider step engine — registered quotient/remainder (not AXI).
// Load dividend (sel=0) / divisor (sel=1) via ST; enable runs one restoring step.
// rem <<= 1 | next_dividend_bit; trial = rem - div; if trial>=0 accept else restore.
module h_div_restoring (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path
    input  wire        st_valid,
    input  wire [15:0] st_data,
    output wire        st_ready,
    input  wire        sel,       // 0=dividend 1=divisor
    input  wire        enable,    // one restoring step
    input  wire        start,     // re-init step count from loaded values
    output reg  [15:0] quotient,
    output reg  [15:0] remainder,
    output wire        busy,
    output wire        done,
    output wire        st_out_valid
);
    reg [15:0] dividend_r;
    reg [15:0] divisor_r;
    reg [15:0] rem_r;
    reg [15:0] quot_r;
    reg [4:0]  step_r;
    reg        busy_r;
    reg        done_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign busy         = busy_r;
    assign done         = done_r;
    assign st_out_valid = out_v;

    // Combinational trial subtract → LUT fabric
    wire [16:0] shifted = {rem_r[14:0], dividend_r[15]};
    wire [16:0] trial   = shifted - {1'b0, divisor_r};
    wire        ge      = ~trial[16];
    wire [15:0] rem_n   = ge ? trial[15:0] : shifted[15:0];
    wire [15:0] quot_n  = {quot_r[14:0], ge};

    always @(posedge clk) begin
        if (!resetn) begin
            dividend_r <= 16'h0;
            divisor_r  <= 16'h1;
            rem_r      <= 16'h0;
            quot_r     <= 16'h0;
            step_r     <= 5'h0;
            busy_r     <= 1'b0;
            done_r     <= 1'b0;
            quotient   <= 16'h0;
            remainder  <= 16'h0;
            out_v      <= 1'b0;
        end else begin
            out_v  <= 1'b0;
            done_r <= 1'b0;
            if (st_valid) begin
                if (sel)
                    divisor_r <= (st_data == 16'h0) ? 16'h1 : st_data;
                else
                    dividend_r <= st_data;
            end else if (start) begin
                rem_r  <= 16'h0;
                quot_r <= 16'h0;
                step_r <= 5'd16;
                busy_r <= 1'b1;
                done_r <= 1'b0;
            end else if (enable && busy_r) begin
                rem_r      <= rem_n;
                quot_r     <= quot_n;
                dividend_r <= {dividend_r[14:0], 1'b0};
                if (step_r <= 5'd1) begin
                    step_r    <= 5'h0;
                    busy_r    <= 1'b0;
                    done_r    <= 1'b1;
                    quotient  <= quot_n;
                    remainder <= rem_n;
                    out_v     <= 1'b1;
                end else begin
                    step_r <= step_r - 5'h1;
                    out_v  <= 1'b1;
                end
            end
        end
    end
endmodule
