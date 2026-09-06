//#############################################################################
// Copyright: Zero ASIC. All rights Reserved. (upstream LogikBench ln)
// License: MIT (see LICENSE in this directory / LogikBench repository)
//#############################################################################
// Schematic-deepen / phase-d light wrapper: DW=8, QW=4 (Q4.4).
// Upstream DW=16/QW=8 → ~17k Helion cells / ~21M edges (drawing TIMEOUT ≤55s).
// DW=8/QW=4 → ~8.5k cells; poly coeffs scaled for Q4.4 (÷16 from Q8.8).
module ln #(parameter DW = 8, // total width
            parameter QW = 4   // fractional bits, Q(DW-QW).QW
            )
   (
    //Inputs
    input signed [DW-1:0]  x,  // operand, must be > 0
    //Outputs
    output signed [DW-1:0] out // ln(x)  (0 for x <= 0)
    );

   // Same structure as upstream; coeffs rescaled for Q4.4 (Q8.8 ÷ 16).
   localparam signed [DW:0] A1  = 23;   // ~369/16
   localparam signed [DW:0] A2  = -10;  // ~-166/16
   localparam signed [DW:0] A3  = 3;    // ~52/16
   localparam signed [DW:0] LN2 = 11;   // ~177/16

   wire [DW-1:0]	    ux;
   reg [$clog2(DW):0]	    p;
   integer		    i;
   wire signed [DW:0]	    e;
   wire [DW-1:0]	    mant;
   wire [QW-1:0]	    u;
   wire signed [QW:0]	    us;
   wire signed [DW+3:0]	    q2, q1, q0, lg;
   wire signed [2*DW-1:0]   l2;
   wire signed [2*DW-1:0]   lnv;

   assign ux = x;

   always @(*) begin
      p = 0;
      for (i = 0; i < DW; i = i + 1)
        if (ux[i])
          p = i[$clog2(DW):0];
   end

   assign e     = $signed({1'b0, p}) - QW;
   assign mant  = (p >= QW) ? (ux >> (p - QW)) : (ux << (QW - p));
   assign u     = mant[QW-1:0];
   assign us    = {1'b0, u};

   assign q2 = A3;
   assign q1 = ((q2 * us) >>> QW) + A2;
   assign q0 = ((q1 * us) >>> QW) + A1;
   assign lg = (q0 * us) >>> QW;

   assign l2  = (e <<< QW) + lg;
   assign lnv = (l2 * LN2) >>> QW;

   assign out = (x <= 0) ? {DW{1'b0}} : lnv[DW-1:0];

endmodule
