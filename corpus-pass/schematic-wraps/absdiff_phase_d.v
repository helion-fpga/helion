//#############################################################################
// Copyright: Zero ASIC. All rights Reserved. (upstream LogikBench absdiff)
// License: MIT (see LICENSE in this directory / LogikBench repository)
//#############################################################################
// Schematic-deepen / phase-d light wrapper: DW=8.
// Upstream DW=16 → ~36k Helion cells (schematic edges explode under ≤55s cap).
// DW=8 → ~5.0k cells; keeps deepen dump under PG cap.
module absdiff #(parameter DW = 8
	         )
   (
    //Inputs
    input [DW-1:0]  a,
    input [DW-1:0]  b,
    //Outputs
    output [DW-1:0] out
    );

   assign out = (a > b) ? (a - b) : (b - a);

endmodule
