// URAM288 fuzzing template for Helion Job 5 (UltraScale+ bitgen).
//
// One URAM288 per generate iteration, all attributes at their UG974
// defaults, ports tied off in read mode. The per-column driver script
// (one_uram_in_every_uram_column.tcl) places one instance in the
// bottom-most URAM288 site of every URAM column, mirroring the DSP
// per-column job (dsp.v) in the Vivado fuzzing package.
//
// Port and attribute names, widths, allowed values, and defaults are
// from AMD UG974 (Vivado UltraScale Libraries), URAM288 page,
// 2026.1 English. There are no INIT attributes on URAM288 (unlike
// RAMB18E2 INIT_00..INIT_3F): initial memory contents cannot be set
// through this instantiation, so the content encoding is a separate
// probe (Job 5 phase 3), not a variant of this file.
//
// Clock: tied low, following dsp.v in the same package. If Vivado DRC
// rejects a tied-off clock when generating the bitstream, use the
// bram_array.v pattern instead (an FDRE toggling a fake clock).
// Read mode (RDB_WR tied high) with address 0 and all enables as
// listed keeps the block inert: no writes, power-on contents only.

module uram ();

  parameter G_SIZE = 4;

  genvar i;
  generate
    for (i=0; i<G_SIZE; i=i+1) begin : URAM288_gen
      (* DONT_TOUCH = "yes" *)
      URAM288 #(
        // Auto-sleep: latency to enter sleep mode (3..15, default 8).
        .AUTO_SLEEP_LATENCY(8),
        // Average consecutive inactive cycles in SLEEP mode, power estimation (10..100000, default 10).
        .AVG_CONS_INACTIVE_CYCLES(10),
        // Byte-write control (PARITY_INTERLEAVED, PARITY_INDEPENDENT).
        .BWE_MODE_A("PARITY_INTERLEAVED"),
        .BWE_MODE_B("PARITY_INTERLEAVED"),
        // Position in cascade chain (NONE, FIRST, LAST, MIDDLE).
        .CASCADE_ORDER_A("NONE"),
        .CASCADE_ORDER_B("NONE"),
        // Automatic sleep mode (FALSE, TRUE).
        .EN_AUTO_SLEEP_MODE("FALSE"),
        // ECC encoder on read (FALSE, TRUE).
        .EN_ECC_RD_A("FALSE"),
        .EN_ECC_RD_B("FALSE"),
        // ECC decoder on write (FALSE, TRUE).
        .EN_ECC_WR_A("FALSE"),
        .EN_ECC_WR_B("FALSE"),
        // Input pipeline registers (FALSE, TRUE).
        .IREG_PRE_A("FALSE"),
        .IREG_PRE_B("FALSE"),
        // Programmable inversion, all default 1'b0.
        .IS_CLK_INVERTED(1'b0),
        .IS_EN_A_INVERTED(1'b0),
        .IS_EN_B_INVERTED(1'b0),
        .IS_RDB_WR_A_INVERTED(1'b0),
        .IS_RDB_WR_B_INVERTED(1'b0),
        .IS_RST_A_INVERTED(1'b0),
        .IS_RST_B_INVERTED(1'b0),
        // Output pipeline registers (FALSE, TRUE).
        .OREG_A("FALSE"),
        .OREG_B("FALSE"),
        // ECC decoder output registers (FALSE, TRUE).
        .OREG_ECC_A("FALSE"),
        .OREG_ECC_B("FALSE"),
        // Cascade registers (FALSE, TRUE).
        .REG_CAS_A("FALSE"),
        .REG_CAS_B("FALSE"),
        // Reset mode (SYNC, ASYNC).
        .RST_MODE_A("SYNC"),
        .RST_MODE_B("SYNC"),
        // Self-address value (11'h000..11'h7ff).
        .SELF_ADDR_A(11'h000),
        .SELF_ADDR_B(11'h000),
        // Self-address mask (11'h000..11'h7ff).
        .SELF_MASK_A(11'h7ff),
        .SELF_MASK_B(11'h7ff),
        // External CE inputs for output registers (FALSE, TRUE).
        .USE_EXT_CE_A("FALSE"),
        .USE_EXT_CE_B("FALSE")
      )
      URAM288_inst (
        // Cascade inputs: unused, cascade order is NONE.
        .CAS_IN_ADDR_A(23'b0),
        .CAS_IN_ADDR_B(23'b0),
        .CAS_IN_BWE_A(9'b0),
        .CAS_IN_BWE_B(9'b0),
        .CAS_IN_DBITERR_A(1'b0),
        .CAS_IN_DBITERR_B(1'b0),
        .CAS_IN_DIN_A(72'b0),
        .CAS_IN_DIN_B(72'b0),
        .CAS_IN_DOUT_A(72'b0),
        .CAS_IN_DOUT_B(72'b0),
        .CAS_IN_EN_A(1'b0),
        .CAS_IN_EN_B(1'b0),
        .CAS_IN_RDACCESS_A(1'b0),
        .CAS_IN_RDACCESS_B(1'b0),
        .CAS_IN_RDB_WR_A(1'b0),
        .CAS_IN_RDB_WR_B(1'b0),
        .CAS_IN_SBITERR_A(1'b0),
        .CAS_IN_SBITERR_B(1'b0),
        // Cascade outputs: unconnected.
        .CAS_OUT_ADDR_A(),
        .CAS_OUT_ADDR_B(),
        .CAS_OUT_BWE_A(),
        .CAS_OUT_BWE_B(),
        .CAS_OUT_DBITERR_A(),
        .CAS_OUT_DBITERR_B(),
        .CAS_OUT_DIN_A(),
        .CAS_OUT_DIN_B(),
        .CAS_OUT_DOUT_A(),
        .CAS_OUT_DOUT_B(),
        .CAS_OUT_EN_A(),
        .CAS_OUT_EN_B(),
        .CAS_OUT_RDACCESS_A(),
        .CAS_OUT_RDACCESS_B(),
        .CAS_OUT_RDB_WR_A(),
        .CAS_OUT_RDB_WR_B(),
        .CAS_OUT_SBITERR_A(),
        .CAS_OUT_SBITERR_B(),
        // Clock, tied low (see header note on the DRC fallback).
        .CLK(1'b0),
        // Error flags: unconnected.
        .DBITERR_A(),
        .DBITERR_B(),
        .SBITERR_A(),
        .SBITERR_B(),
        .RDACCESS_A(),
        .RDACCESS_B(),
        // Data ports: read mode, address 0, no writes.
        .ADDR_A(23'b0),
        .ADDR_B(23'b0),
        .BWE_A(9'b0),
        .BWE_B(9'b0),
        .DIN_A(72'b0),
        .DIN_B(72'b0),
        .DOUT_A(),
        .DOUT_B(),
        .EN_A(1'b1),
        .EN_B(1'b1),
        .RDB_WR_A(1'b1),
        .RDB_WR_B(1'b1),
        // Error injection: off.
        .INJECT_DBITERR_A(1'b0),
        .INJECT_DBITERR_B(1'b0),
        .INJECT_SBITERR_A(1'b0),
        .INJECT_SBITERR_B(1'b0),
        // Output register clock enables: on (unused while OREG is FALSE).
        .OREG_CE_A(1'b1),
        .OREG_CE_B(1'b1),
        .OREG_ECC_CE_A(1'b1),
        .OREG_ECC_CE_B(1'b1),
        // Resets: not asserted.
        .RST_A(1'b0),
        .RST_B(1'b0),
        // Sleep: awake.
        .SLEEP(1'b0)
      );
    end
  endgenerate

endmodule
