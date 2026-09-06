//#############################################################################
// Copyright: Zero ASIC. All rights Reserved. (upstream LogikBench clamp)
// License: MIT (see LICENSE in this directory / LogikBench repository)
//#############################################################################
// Schematic-deepen / phase-d light wrapper: DW=8.
// Upstream DW=16 → ~66k Helion cells (monster-band; drawing TIMEOUT risk).
// DW=8 → ~8.3k cells; keeps deepen dump under PG cap.
module clamp #(parameter DW = 8
               )
   (
    //Inputs
    input signed [DW-1:0]  a,  // value to clamp
    input signed [DW-1:0]  lo, // lower bound (inclusive)
    input signed [DW-1:0]  hi, // upper bound (inclusive)
    //Outputs
    output signed [DW-1:0] out // a clamped to [lo, hi]
    );

   // Saturate/clip a signed value into the inclusive range [lo, hi].
   assign out = (a < lo) ? lo :
                (a > hi) ? hi : a;

endmodule
