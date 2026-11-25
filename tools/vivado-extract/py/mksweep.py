#!/usr/bin/env python3
"""Write the fuzz.tcl sweep for rtl/us_sink.v (UltraScale+).

  mksweep.py us [--dsp-table T] [--uram-table T] > sweep.tsv

Built-in rows cover LUT INIT, FDRE, CARRY8, RAMB36E2, IO, MMCME4 and BUFGCE.
--dsp-table / --uram-table take an attribute table with columns
`id attribute kind default variant` (the format of the DSP48E2 and URAM288
variant tables in the UltraScale+ bitgen crew docs) and add one row per variant.
Output columns: id kind pattern property value.
"""
import argparse

# Values must differ from the base cell (RAMB36E2 defaults: DOx_REG 1, WRITE_MODE NO_CHANGE;
# opt_design turns CLOCK_DOMAINS into COMMON), or the variant measures nothing.
US_BUILTIN = [
    # (cell or port, pattern, property, [values])
    ("cell", "lut_i", "INIT", ["64'h0000000000000001", "64'h0000000000000002", "64'h0000000080000000",
                               "64'h0000000100000000", "64'h8000000000000000"]),
    ("cell", "ff_i", "INIT", ["1'b1"]),
    ("cell", "ff_i", "IS_C_INVERTED", ["1'b1"]),
    ("cell", "ff_i", "IS_R_INVERTED", ["1'b1"]),
    ("cell", "ff_i", "IS_D_INVERTED", ["1'b1"]),
    ("cell", "carry_i", "CARRY_TYPE", ["DUAL_CY4"]),
    ("cell", "bram_i", "DOA_REG", ["0"]),
    ("cell", "bram_i", "DOB_REG", ["0"]),
    ("cell", "bram_i", "WRITE_MODE_A", ["READ_FIRST", "WRITE_FIRST"]),
    ("cell", "bram_i", "WRITE_MODE_B", ["READ_FIRST"]),
    ("cell", "bram_i", "INIT_A", ["36'h000000001"]),
    ("cell", "bram_i", "SRVAL_A", ["36'h000000001"]),
    ("cell", "bram_i", "RDADDRCHANGEA", ["TRUE"]),
    ("cell", "bram_i", "CLOCK_DOMAINS", ["INDEPENDENT"]),
    ("cell", "bram_i", "SLEEP_ASYNC", ["TRUE"]),
    ("cell", "bram_i", "EN_ECC_READ", ["TRUE"]),
    ("cell", "bram_i", "IS_CLKARDCLK_INVERTED", ["1'b1"]),
    ("cell", "bram_i", "IS_ENARDEN_INVERTED", ["1'b1"]),
    ("cell", "bram_i", "INIT_00", ["256'h1"]),
    ("cell", "bram_i", "INITP_00", ["256'h1"]),
    ("port", "io0", "IOSTANDARD", ["LVCMOS12", "LVCMOS15", "SSTL12", "HSTL_I_18", "POD12"]),
    ("port", "io0", "DRIVE", ["4", "8"]),
    ("port", "io0", "SLEW", ["FAST"]),
    ("port", "io0", "PULLTYPE", ["PULLUP", "PULLDOWN", "KEEPER"]),
    ("cell", "mmcm_i", "CLKFBOUT_MULT_F", ["11.000", "10.125"]),
    ("cell", "mmcm_i", "CLKOUT0_DIVIDE_F", ["11.000", "10.125"]),
    ("cell", "mmcm_i", "CLKOUT0_PHASE", ["45.000"]),
    ("cell", "mmcm_i", "CLKOUT0_DUTY_CYCLE", ["0.400"]),
    ("cell", "mmcm_i", "CLKOUT1_DIVIDE", ["4"]),
    ("cell", "mmcm_i", "BANDWIDTH", ["HIGH", "LOW"]),
    ("cell", "mmcm_i", "STARTUP_WAIT", ["TRUE"]),
    ("cell", "mmcm_i", "IS_RST_INVERTED", ["1'b1"]),
    ("cell", "bufg_i", "CE_TYPE", ["ASYNC"]),
    ("cell", "bufg_i", "IS_CE_INVERTED", ["1'b1"]),
    ("cell", "bufg_i", "IS_I_INVERTED", ["1'b1"]),
]


def table_rows(path, cell, prefix):
    for line in open(path):
        if line.startswith("#") or not line.strip():
            continue
        f = line.rstrip("\n").split("\t")
        if f[0] == "id" or f[1] == "-":
            continue
        yield (f"{prefix}_{f[0]}", "cell", cell, f[1], f[4])


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("family", choices=["us"])
    ap.add_argument("--dsp-table")
    ap.add_argument("--uram-table")
    a = ap.parse_args()
    rows = []
    for kind, pat, prop, vals in US_BUILTIN:
        for i, v in enumerate(vals):
            rows.append((f"{pat.removesuffix('_i')}_{prop.lower()}_{i}", kind, pat, prop, v))
    if a.dsp_table:
        rows += table_rows(a.dsp_table, "dsp_i", "dsp")
    if a.uram_table:
        rows += table_rows(a.uram_table, "uram_i", "uram")
    ids = [r[0] for r in rows]
    assert len(ids) == len(set(ids)), "duplicate sweep ids"
    print("# id\tkind\tpattern\tproperty\tvalue")
    for r in rows:
        print("\t".join(r))


if __name__ == "__main__":
    main()
