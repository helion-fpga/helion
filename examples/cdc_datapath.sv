// Helion CDC demo — dual clocks with set_max_delay -datapath_only (CDC-13 Warning).
// Opens with sibling cdc_datapath.sdc; report_cdc must paint CDC-13 Warning /
// catalog Warnings (TimedDatapath), not Critical / Failed like cdc_cross.
module cdc_datapath (
    input  logic clk_a,
    input  logic clk_b,
    input  logic d_a,
    output logic q_b
);
    logic q_a;
    always_ff @(posedge clk_a) begin
        q_a <= d_a;
    end
    // Crossing is timed datapath-only via SDC; still a raw FF→FF sample in RTL.
    always_ff @(posedge clk_b) begin
        q_b <= q_a;
    end
endmodule
