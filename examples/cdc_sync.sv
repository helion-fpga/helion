// Helion CDC demo — dual clocks with a 2FF synchronizer on the crossing.
// Opens with sibling cdc_sync.sdc; report_cdc must paint CDC-1 Warning /
// catalog Warnings (not CDC-10 Critical / Failed like cdc_cross).
module cdc_sync (
    input  logic clk_a,
    input  logic clk_b,
    input  logic d_a,
    output logic q_b
);
    logic q_a;
    logic sync0;
    always_ff @(posedge clk_a) begin
        q_a <= d_a;
    end
    // 2FF synchronizer on clk_b: first stage samples async, second settles.
    always_ff @(posedge clk_b) begin
        sync0 <= q_a;
        q_b <= sync0;
    end
endmodule
