// Helion-ST 2FF pulse synchronizer / toggle-pulse CDC (not AXI, not Xilinx XPM).
// src_clk: rising edge of src_pulse toggles a FF.
// dst_clk: 2FF sync of toggle + XOR edge detect → 1-cycle dst_pulse.
// Helion-native fabric only (no ASYNC_REG XPM / no UNISIM).
module h_cdc_pulse (
    input  wire src_clk,
    input  wire src_resetn,
    input  wire src_pulse,     // single-cycle (or level) pulse in src domain
    input  wire dst_clk,
    input  wire dst_resetn,
    output reg  dst_pulse,     // single-cycle pulse in dst domain
    output wire st_ready,      // Helion-ST ready (always)
    output wire st_out_valid   // pulses with dst_pulse
);
    // --- source domain: toggle on rising edge of src_pulse ---
    reg src_prev;
    reg src_have;
    reg toggle_r;

    wire src_rise = src_have && !src_prev && src_pulse;

    always @(posedge src_clk) begin
        if (!src_resetn) begin
            src_prev  <= 1'b0;
            src_have  <= 1'b0;
            toggle_r  <= 1'b0;
        end else begin
            if (src_rise)
                toggle_r <= ~toggle_r;
            src_prev <= src_pulse;
            src_have <= 1'b1;
        end
    end

    // --- destination domain: 2FF synchronizer + edge detect ---
    reg sync_ff1;
    reg sync_ff2;
    reg sync_ff2_d;

    assign st_ready     = 1'b1;
    assign st_out_valid = dst_pulse;

    always @(posedge dst_clk) begin
        if (!dst_resetn) begin
            sync_ff1   <= 1'b0;
            sync_ff2   <= 1'b0;
            sync_ff2_d <= 1'b0;
            dst_pulse  <= 1'b0;
        end else begin
            sync_ff1   <= toggle_r;
            sync_ff2   <= sync_ff1;
            sync_ff2_d <= sync_ff2;
            // XOR of consecutive synced samples → pulse on toggle edge
            dst_pulse  <= sync_ff2 ^ sync_ff2_d;
        end
    end
endmodule
