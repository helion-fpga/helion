// Helion 1.0 SV carry8 - 8-bit incrementer on one CARRY8, LED = cnt[7]
//
// The incrementer maps to a single CARRY8 in one CLB slice
// (SLICE_X0Y0, CLEL_R on xcku3p):
//   - CARRY_TYPE = SINGLE_CY8: one 8-bit carry chain.
//   - CI = 1 (CARRY8.CI.V1): the +1 carry-in.
//   - CI_TOP = 0 (ground; UG974: tie to ground when SINGLE_CY8).
//     CI_TOP.V0 is the correct value but is not observable through
//     parse-back (a mux default with no exact complement), so the
//     golden FASM leaves CI_TOP unconfigured; see the design doc.
//   - S[i] = cnt[i] and DI[i] = cnt[i], both from the slice LUTs:
//     each LUT is a buffer of cnt[i] (INIT 0xAAAAAAAAAAAAAAAA), its
//     O6 output drives S[i] (propagate) and its O5 output drives
//     DI[i] (generate) through the DI mux (CARRY8.DIi.<letter>).
//     CO[i] = S[i] ? CI[i] : DI[i] and O[i] = S[i] ^ CI[i], so the
//     chain adds 1: O = cnt + 1, captured by the 8 FFs.
//   - led = cnt[7], the top count bit, as in counter.sv.
// The golden FASM is the placed form of this design; see
// crates/helion-ultrascale/harness/golden/carry8.xcku3p.fasm and
// docs/ultra-bitgen-crew/carry8-golden.md.
module carry8 (
    input logic clk,
    output logic led
);
    logic [7:0] cnt;
    always_ff @(posedge clk) begin
        cnt <= cnt + 1;
    end
    assign led = cnt[7];
endmodule
