// Helion-MM threshold/compare — MM regs + sticky match/irq-style status (not AXI).
// Addr map (word via mm_addr[3:2]): 0=value, 1=threshold_lo, 2=threshold_hi,
// 3=ctrl/status. ctrl[0]=enable compare, ctrl[1]=clear sticky match.
// Match when value in [lo,hi] inclusive while enable; sticky until clear.
// Real FF+LUT fabric for catalog proof.
module h_compare_mm (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output reg         match,      // sticky match status
    output wire        irq         // irq-style level while match sticky
);
    reg [31:0] value_r;
    reg [31:0] thr_lo;
    reg [31:0] thr_hi;
    reg        enable_r;
    reg        match_r;

    assign mm_ready = 1'b1;
    assign irq      = match_r;

    wire [1:0] sel = mm_addr[3:2];
    wire in_range = (value_r >= thr_lo) && (value_r <= thr_hi);

    always @(posedge clk) begin
        if (!resetn) begin
            value_r  <= 32'h0;
            thr_lo   <= 32'h0;
            thr_hi   <= 32'hFFFF_FFFF;
            enable_r <= 1'b0;
            match_r  <= 1'b0;
            match    <= 1'b0;
            mm_rdata <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (sel)
                    2'b00: value_r <= mm_wdata;
                    2'b01: thr_lo  <= mm_wdata;
                    2'b10: thr_hi  <= mm_wdata;
                    2'b11: begin
                        enable_r <= mm_wdata[0];
                        if (mm_wdata[1])
                            match_r <= 1'b0;
                    end
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (sel)
                    2'b00: mm_rdata <= value_r;
                    2'b01: mm_rdata <= thr_lo;
                    2'b10: mm_rdata <= thr_hi;
                    2'b11: mm_rdata <= {30'h0, match_r, enable_r};
                endcase
            end

            // Continuous compare when enabled — sticky set on in-range
            if (enable_r && in_range)
                match_r <= 1'b1;

            match <= match_r;
        end
    end
endmodule
