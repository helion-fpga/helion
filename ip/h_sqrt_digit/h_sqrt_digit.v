// Helion-ST digit-by-digit integer sqrt step — registered rem/root (not AXI).
// Classic non-restoring digit recurrence: bring down 2 bits, trial (root<<2|1).
// st_valid loads radicand; start arms 8 digit steps; enable advances one digit.
module h_sqrt_digit (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path
    input  wire        st_valid,
    input  wire [15:0] st_data,
    output wire        st_ready,
    input  wire        enable,    // one digit step
    input  wire        start,     // begin sqrt of loaded radicand
    output reg  [7:0]  root,
    output reg  [15:0] rem,
    output wire        busy,
    output wire        done,
    output wire        st_out_valid
);
    reg [15:0] rad_r;
    reg [15:0] rem_r;
    reg [7:0]  root_r;
    reg [3:0]  step_r;
    reg        busy_r;
    reg        done_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign busy         = busy_r;
    assign done         = done_r;
    assign st_out_valid = out_v;

    // Bring down next 2 bits of radicand
    wire [17:0] rem_shift = {rem_r, rad_r[15:14]};
    wire [9:0]  trial_sub = {root_r, 2'b01}; // (root<<2)|1
    wire [17:0] trial     = rem_shift - {8'h0, trial_sub};
    wire        ge        = ~trial[17];
    wire [15:0] rem_n     = ge ? trial[15:0] : rem_shift[15:0];
    wire [7:0]  root_n    = ge ? {root_r[6:0], 1'b1} : {root_r[6:0], 1'b0};

    always @(posedge clk) begin
        if (!resetn) begin
            rad_r  <= 16'h0;
            rem_r  <= 16'h0;
            root_r <= 8'h0;
            step_r <= 4'h0;
            busy_r <= 1'b0;
            done_r <= 1'b0;
            root   <= 8'h0;
            rem    <= 16'h0;
            out_v  <= 1'b0;
        end else begin
            out_v  <= 1'b0;
            done_r <= 1'b0;
            if (st_valid) begin
                rad_r <= st_data;
            end else if (start) begin
                rem_r  <= 16'h0;
                root_r <= 8'h0;
                step_r <= 4'd8;
                busy_r <= 1'b1;
                done_r <= 1'b0;
            end else if (enable && busy_r) begin
                rem_r <= rem_n;
                root_r <= root_n;
                rad_r  <= {rad_r[13:0], 2'b00};
                if (step_r <= 4'd1) begin
                    step_r <= 4'h0;
                    busy_r <= 1'b0;
                    done_r <= 1'b1;
                    root   <= root_n;
                    rem    <= rem_n;
                    out_v  <= 1'b1;
                end else begin
                    step_r <= step_r - 4'h1;
                    out_v  <= 1'b1;
                end
            end
        end
    end
endmodule
