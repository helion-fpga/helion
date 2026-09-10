// Helion CDC demo — dual clocks with pin-scoped set_false_path (CDC-14 Warning).
// Opens with sibling cdc_partial_false.sdc; report_cdc must paint CDC-14 Warning /
// catalog Warnings (PartialFalsePath), not full False Path / Safe like clock-level FP.
module cdc_partial_false (
    input  logic clk_a,
    input  logic clk_b,
    input  logic d_a,
    output logic q_b
);
    logic q_a;
    always_ff @(posedge clk_a) begin
        q_a <= d_a;
    end
    // Crossing remains partially timed: SDC false-paths only the destination D pin.
    always_ff @(posedge clk_b) begin
        q_b <= q_a;
    end
endmodule
