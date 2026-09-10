// Helion CDC demo — dual clocks with set_clock_groups -exclusive (CDC-16 Info).
// Opens with sibling cdc_exclusive_groups.sdc; report_cdc must paint CDC-16 Info /
// catalog Complete (not Critical / Failed like cdc_cross without groups).
module cdc_exclusive_groups (
    input  logic clk_a,
    input  logic clk_b,
    input  logic d_a,
    output logic q_b
);
    logic q_a;
    always_ff @(posedge clk_a) begin
        q_a <= d_a;
    end
    // Crossing is declared exclusive via SDC; still a raw FF→FF sample in RTL.
    always_ff @(posedge clk_b) begin
        q_b <= q_a;
    end
endmodule
