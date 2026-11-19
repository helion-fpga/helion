#!/usr/bin/env python3
"""Generate one Verilog variant per row of uram_variant_table.tsv.

Usage (on the cluster, or anywhere with python3):
    python3 gen_uram_variants.py [table.tsv] [out_dir]

Each variant is a single URAM288 with DONT_TOUCH, all attributes at
the UG974 defaults except the one override named by the table row.
The baseline (v000) is the UG974 defaults with no override. Output
layout:

    out_dir/v000/uram_variant.v
    out_dir/v001/uram_variant.v
    ...
    out_dir/manifest.tsv   (id, attribute, default, variant, verilog)

There are 38 variants for 34 attributes: CASCADE_ORDER_A and
CASCADE_ORDER_B have three variants each (FIRST, MIDDLE, LAST) and
every other attribute has one. The table is the single source of
truth for the count.

The generator never edits a variant between runs: the same file is
the input to every Vivado run for that id, so a diff between two runs
can only come from the one attribute that differs.

Defaults are read from the table itself (the `default` column of the
first row for each attribute), so the table is the single source of
truth. The full attribute list with defaults must match AMD UG974
(2023.1 English) URAM288 "Available Attributes"; the script aborts
if any of the 34 attributes in EXPECTED_ATTRIBUTES is missing from
the table. The port list is the UG974 URAM288 "Port Descriptions"
table, read 2026-11-19: all inputs tied to constants, all outputs
unconnected, clock tied low, enables tied high so the cell is a
live RAM and not optimized away (DONT_TOUCH is set as well).

Self-check: after generating, the script verifies that the baseline
file contains all 34 attributes at their defaults and that each
variant file differs from the baseline in exactly one attribute
line (the `.NAME(value)` line for the overridden attribute). A
variant that differs anywhere else aborts the generation.

No em dashes anywhere.
"""

import sys
from pathlib import Path

EXPECTED_ATTRIBUTES = [
    "AUTO_SLEEP_LATENCY",
    "AVG_CONS_INACTIVE_CYCLES",
    "BWE_MODE_A",
    "BWE_MODE_B",
    "CASCADE_ORDER_A",
    "CASCADE_ORDER_B",
    "EN_AUTO_SLEEP_MODE",
    "EN_ECC_RD_A",
    "EN_ECC_RD_B",
    "EN_ECC_WR_A",
    "EN_ECC_WR_B",
    "IREG_PRE_A",
    "IREG_PRE_B",
    "IS_CLK_INVERTED",
    "IS_EN_A_INVERTED",
    "IS_EN_B_INVERTED",
    "IS_RDB_WR_A_INVERTED",
    "IS_RDB_WR_B_INVERTED",
    "IS_RST_A_INVERTED",
    "IS_RST_B_INVERTED",
    "OREG_A",
    "OREG_B",
    "OREG_ECC_A",
    "OREG_ECC_B",
    "REG_CAS_A",
    "REG_CAS_B",
    "RST_MODE_A",
    "RST_MODE_B",
    "SELF_ADDR_A",
    "SELF_ADDR_B",
    "SELF_MASK_A",
    "SELF_MASK_B",
    "USE_EXT_CE_A",
    "USE_EXT_CE_B",
]

# Port connections: identical in every variant. All inputs tied to
# constants, all outputs unconnected, clock tied low. This is the
# UG974 URAM288 port list. The ties match the branch's phase-1
# template (docs/ultra-bitgen-crew/vivado-fuzzing/job5-uram/tcl/uram.v,
# commit 013044f2) so the sweep baseline is the same design as the
# phase-1 template with G_SIZE=1: BWE 0 (no byte lanes written),
# RDB_WR 1, enables high. The RDB_WR polarity is not asserted here;
# the tie is constant across all variants, so no sweep result
# depends on it.
PORTS = """        .CAS_OUT_ADDR_A(),
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
        .DBITERR_A(),
        .DBITERR_B(),
        .DOUT_A(),
        .DOUT_B(),
        .RDACCESS_A(),
        .RDACCESS_B(),
        .SBITERR_A(),
        .SBITERR_B(),
        .ADDR_A(23'b0),
        .ADDR_B(23'b0),
        .BWE_A(9'b0),
        .BWE_B(9'b0),
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
        .CLK(1'b0),
        .DIN_A(72'b0),
        .DIN_B(72'b0),
        .EN_A(1'b1),
        .EN_B(1'b1),
        .INJECT_DBITERR_A(1'b0),
        .INJECT_DBITERR_B(1'b0),
        .INJECT_SBITERR_A(1'b0),
        .INJECT_SBITERR_B(1'b0),
        .OREG_CE_A(1'b1),
        .OREG_CE_B(1'b1),
        .OREG_ECC_CE_A(1'b1),
        .OREG_ECC_CE_B(1'b1),
        .RDB_WR_A(1'b1),
        .RDB_WR_B(1'b1),
        .RST_A(1'b0),
        .RST_B(1'b0),
        .SLEEP(1'b0)
"""


def render_value(kind, value):
    """Render a table value as a Verilog literal."""
    if kind == "string":
        return '"%s"' % value
    return value


def render_verilog(run_id, kinds, values, overrides):
    """Render one variant module.

    kinds: attribute -> kind ("string", "int", "bits").
    values: attribute -> default value string.
    overrides: attribute -> variant value string (at most one entry).
    """
    lines = []
    lines.append("// Generated by gen_uram_variants.py from uram_variant_table.tsv.")
    lines.append("// Run %s. Do not hand-edit: regenerate from the table." % run_id)
    if overrides:
        for name, value in overrides.items():
            lines.append("// Override: %s = %s (default %s)" % (name, value, values[name]))
    else:
        lines.append("// Baseline: all attributes at UG974 defaults.")
    lines.append("module uram_single ();")
    lines.append("")
    lines.append('  (* DONT_TOUCH = "yes" *)')
    lines.append("  URAM288 #(")
    attr_lines = []
    for name in EXPECTED_ATTRIBUTES:
        value = overrides.get(name, values[name])
        attr_lines.append("    .%s(%s)" % (name, render_value(kinds[name], value)))
    lines.append(",\n".join(attr_lines))
    lines.append("  )")
    lines.append("  URAM288_inst (")
    lines.append(PORTS.rstrip("\n"))
    lines.append("  );")
    lines.append("")
    lines.append("endmodule")
    return "\n".join(lines) + "\n"


def read_table(path):
    """Return (kinds, values, variants).

    kinds/values map attribute to kind/default. variants is a list of
    (run_id, attribute, variant_value).
    """
    rows = []
    with open(path, encoding="utf-8") as f:
        header = ""
        for line in f:
            if line.startswith("#") or not line.strip():
                continue
            header = line
            break
        assert header.startswith("id\tattribute\tkind\tdefault\tvariant"), header
        for line in f:
            line = line.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            assert len(parts) == 5, line
            rows.append(parts)
    kinds = {}
    values = {}
    variants = []
    for run_id, attr, kind, default, variant in rows:
        if attr not in kinds:
            kinds[attr] = kind
            values[attr] = default
        else:
            assert kinds[attr] == kind and values[attr] == default, (
                "conflicting default for %s" % attr
            )
        variants.append((run_id, attr, variant))
    missing = [a for a in EXPECTED_ATTRIBUTES if a not in kinds]
    assert not missing, "table is missing attributes: %s" % ", ".join(missing)
    extra = [a for a in kinds if a not in EXPECTED_ATTRIBUTES]
    assert not extra, "table has unknown attributes: %s" % ", ".join(extra)
    # 38 variants for 34 attributes: CASCADE_ORDER_A/B have three
    # each (FIRST, MIDDLE, LAST), every other attribute has one.
    assert len(variants) == 38, (
        "expected 38 variants, got %d" % len(variants)
    )
    return kinds, values, variants


def write_variant(out_dir, run_id, text):
    vdir = out_dir / run_id
    vdir.mkdir(parents=True, exist_ok=True)
    (vdir / "uram_variant.v").write_text(text, encoding="utf-8")


def check_single_attribute_diff(baseline, variant_text, attr):
    """The variant must differ from the baseline in exactly one
    attribute line: the `.ATTR(` line for the overridden attribute.
    Comment header lines naming the run and the override are allowed
    to differ; they do not affect synthesis.
    """
    base_lines = baseline.splitlines()
    var_lines = variant_text.splitlines()
    assert len(base_lines) == len(var_lines), (
        "line count changed for %s" % attr
    )
    diffs = [i for i, (a, b) in enumerate(zip(base_lines, var_lines)) if a != b]
    attr_diffs = [i for i in diffs if ".%s(" % attr in var_lines[i]]
    comment_diffs = [i for i in diffs if var_lines[i].startswith("//")]
    assert sorted(diffs) == sorted(attr_diffs + comment_diffs) and len(attr_diffs) == 1, (
        "variant for %s differs in %d non-comment lines, expected 1: %s"
        % (attr, len(attr_diffs), [var_lines[i] for i in diffs])
    )


def main(argv):
    table_path = Path(argv[1]) if len(argv) > 1 else Path("uram_variant_table.tsv")
    out_dir = Path(argv[2]) if len(argv) > 2 else Path("variants")
    kinds, values, variants = read_table(table_path)
    out_dir.mkdir(parents=True, exist_ok=True)
    manifest = ["id\tattribute\tdefault\tvariant\tverilog"]
    baseline = render_verilog("v000", kinds, values, {})
    write_variant(out_dir, "v000", baseline)
    manifest.append("v000\t-\t-\t-\tv000/uram_variant.v")
    for run_id, attr, variant in variants:
        text = render_verilog(run_id, kinds, values, {attr: variant})
        check_single_attribute_diff(baseline, text, attr)
        write_variant(out_dir, run_id, text)
        manifest.append("%s\t%s\t%s\t%s\t%s/uram_variant.v" % (
            run_id, attr, values[attr], variant, run_id))
    (out_dir / "manifest.tsv").write_text("\n".join(manifest) + "\n", encoding="utf-8")
    print("wrote %d variants + baseline to %s" % (len(variants), out_dir))
    print("manifest: %s" % (out_dir / "manifest.tsv"))
    print("self-check: every variant differs from the baseline in exactly one attribute line")


if __name__ == "__main__":
    main(sys.argv)
