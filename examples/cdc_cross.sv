// Helion CDC demo — dual clocks with unsynchronized FF→FF data crossing (no 2FF sync).
// Opens with sibling cdc_cross.sdc; report_cdc must paint CDC-10 Critical / catalog Failed.
module cdc_cross (
    input  logic clk_a,
    input  logic clk_b,
    input  logic d_a,
    output logic q_b
);
    logic q_a;
    always_ff @(posedge clk_a) begin
        q_a <= d_a;
    end
    // Unsynchronized cross: capture FF on clk_b samples launch FF on clk_a directly.
    always_ff @(posedge clk_b) begin
        q_b <= q_a;
    end
endmodule
