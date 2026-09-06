//#############################################################################
// Copyright: Zero ASIC. All rights Reserved. (upstream LogikBench max)
// License: MIT (see LICENSE in this directory / LogikBench repository)
//#############################################################################
// Schematic-deepen / phase-d light wrapper: DW=8.
// Upstream DW=16 → ~33k Helion cells / ~5.6M schematic edges (drawing TIMEOUT ≤55s).
// DW=8 → ~4.2k cells; keeps deepen dump under PG cap.
module max #(parameter DW = 8
	     )
   (
    //Inputs
    input [DW-1:0]  a,
    input [DW-1:0]  b,
    //Outputs
    output [DW-1:0] out
    );

   assign out = (a > b) ? a : b;

endmodule
