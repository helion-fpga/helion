//#############################################################################
// Copyright: Zero ASIC. All rights Reserved.
// Author: Andreas Olofsson
// License:  MIT (see LICENSE file in LogikBench repository)
//#############################################################################
// Corpus wrapper: upstream DW=64 needs 64 IOB outs; HL10T-C32-1 has user_io=32.
// DW=16 fits the part and still exercises multi-IOB comb pack/place (a637b85).

module bin2gray #(parameter DW = 16
                  )
   (
    input [DW-1:0]  in, // binary input
    output [DW-1:0] out // gray encoded output
    );
   assign out = in ^ (in >> 1);

endmodule
