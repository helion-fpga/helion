//#############################################################################
// Copyright: Zero ASIC. All rights Reserved. (upstream LogikBench muladdc)
// License: MIT (see LICENSE in this directory / LogikBench repository)
//#############################################################################
// Schematic-deepen / phase-d light wrapper: DW=8, OW=20.
// Upstream DW=16/OW=40 → ~22k Helion cells / ~5.6M edges (drawing TIMEOUT ≤55s).
// DW=8/OW=20 → ~5.2k cells; keeps deepen dump under PG cap.
module muladdc #(parameter DW = 8,
                 parameter OW = 20
                 )
   (
    input signed [DW-1:0]  a_re,
    input signed [DW-1:0]  a_im,
    input signed [DW-1:0]  b_re,
    input signed [DW-1:0]  b_im,
    input signed [OW-1:0]  c_re,
    input signed [OW-1:0]  c_im,
    output signed [OW-1:0] out_re,
    output signed [OW-1:0] out_im
    );

   wire signed [2*DW-1:0] prod_re;
   wire signed [2*DW-1:0] prod_im;

   assign prod_re[2*DW-1:0] = (a_re * b_re) - (a_im * b_im);
   assign prod_im[2*DW-1:0] = (a_re * b_im) + (a_im * b_re);

   assign out_re = c_re + prod_re;
   assign out_im = c_im + prod_im;

endmodule
