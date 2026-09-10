// Helion methodology demo — LUT drives an FF CLK (TIMING-10 Warning).
// Opens with sibling timing_lut_as_clock.sdc; report_methodology must emit
// TIMING-10 Warning / catalog Warnings from an honest LUT-on-clock netlist.
// XOR (not clk&en) so synth lowers a real clock LUT instead of CLOCK_GATE skip.
module timing_lut_as_clock (
    input  logic clk,
    input  logic en,
    input  logic d,
    output logic q
);
    logic gated;
    assign gated = clk ^ en;
    always_ff @(posedge gated) begin
        q <= d;
    end
endmodule
