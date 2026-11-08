# Diff engine: per-block comparison of reference bits vs emitted bits.
#
# Record format (INTERFACES.md), one JSONL record per block per run:
#   {"block": "LUT_INIT", "design": "blinky", "part": "xcku3p",
#    "diff_bits": 0, "total_bits": 512, "status": "green"}
# Status is green (diff 0), red (diff > 0), or blocked (missing data,
# with a `missing` field naming the exact data). Blocked records carry
# diff_bits null, never zero: a comparison that did not run must not
# look like a pass. Every record also carries the run seed and the
# placement hash, and the coordinate system of its reference bits.
#
# Diff semantics: reference bitset R maps bit positions to required
# values (0 or 1; must-be-0 bits are constrained too). Emitted bitset
# E maps positions to emitted values; a position absent from E reads
# as 0. diff_bits counts positions where the values differ, plus
# positions in E with value 1 that R does not constrain (extra set
# bits). total_bits is |R|.
#
# Greedy queue: blocks ordered by effective size descending, where
# effective size is diff_bits when the comparison ran, else total_bits
# (the size of the work still standing). Ties: red before blocked
# before green, then block name. That ordering IS the crew's work queue.
#
# No em dashes anywhere.

import json
import os

from emitter import NO_EMITTER_MISSING
from fasm import BLOCKS, classify, parse_fasm, parse_init_value
from reference import MissingReference
import roundtrip as roundtrip_mod

# Blocks whose reference bits are absolute FARs once resolved.
ABSOLUTE_BLOCKS = {"LUT_INIT", "FF_INIT", "BRAM_CONTENT", "BRAM_PARITY", "INT_PIP"}

# Blocks the golden designs do not exercise at all (v1 goldens have no
# BRAM, DSP, or URAM instances). Reported blocked, never green.
NOT_EXERCISED = {
    "BRAM_CONTENT": "no BRAM instances in the golden designs",
    "BRAM_PARITY": "no BRAM instances in the golden designs",
    "DSP48E2": "no DSP48E2 instances in the golden designs",
    "URAM288": "no URAM288 instances in the golden designs",
}


def _ff_bel(feature):
    import re
    m = re.match(r"^([A-H])FF2?\.(INIT|SRVAL)$", feature)
    return m.group(1) + ("FF2" if "FF2" in feature else "FF")


def resolve_feature(ref, part, feature):
    """Resolve one golden FASM feature to reference bits.

    Returns (block, bits, coord_system, missing). bits is a dict
    position -> required value. missing is None when the feature
    resolved, else the exact gap. coord_system is one of absolute,
    absolute-provisional, tile-relative.
    """
    block = classify(feature)
    tile, feat, value = feature.tile, feature.feature, feature.value
    try:
        if block == "LUT_INIT":
            import re
            m = re.match(r"^([A-H])6LUT\.INIT\[63:0\]$", feat)
            bel = m.group(1) + "6LUT"
            from fasm import slice_xy
            x, y = slice_xy(tile)
            init = parse_init_value(value)
            return block, ref.resolve_lut(part, x, y, bel, init), "absolute", None
        if block == "FF_INIT":
            from fasm import slice_xy
            x, y = slice_xy(tile)
            if ".INIT" in feat:
                bel = _ff_bel(feat)
                init = int(value)
                if init not in (0, 1):
                    raise MissingReference("FF INIT value %r is not 0 or 1" % value)
                return block, ref.resolve_ff(part, x, y, bel, init), "absolute", None
            if ".SRVAL" in feat:
                bel = _ff_bel(feat)
                srval = int(value)
                if srval not in (0, 1):
                    raise MissingReference("FF SRVAL value %r is not 0 or 1" % value)
                return block, ref.resolve_srval(part, x, y, bel, srval), "absolute", None
            raise MissingReference("unhandled FF feature %r" % feat)
        if block in ("BRAM_CONTENT", "BRAM_PARITY"):
            raise MissingReference(NOT_EXERCISED[block])
        if block == "INT_PIP":
            from fasm import int_xy
            col, y = int_xy(tile)
            bits = ref.resolve_int_pip(part, col, y, feat)
            return block, bits, "absolute-provisional", None
        if block == "IO_CONFIG":
            # Feature string after the tile: e.g. IOB_X0Y0.IOSTANDARD_IN.LVCMOS12
            bits = ref.resolve_hpio(tile, feat)
            return block, bits, "tile-relative", None
        if block == "CLK":
            if ".PIP." in feat:
                bits = ref.resolve_cmt_pip(tile, feat)
                return block, bits, "tile-relative", None
            raise MissingReference(
                "BUFGCE/MMCM config-bit encoding absent from prjuray-db "
                "segbits_cmt_right.db (104 lines, CLK_HROUTE PIPs only) "
                "and BitFiltrator; needs Vivado fuzzing (SPEC plan 2)"
            )
        if block == "DSP48E2":
            raise MissingReference(
                "DSP48E2 config-bit encoding absent: prjuray-db "
                "site_types/site_type_DSP48E2.json documents site pins "
                "only, BitFiltrator ships no DSP data; needs Vivado "
                "fuzzing (SPEC plan 2)"
            )
        if block == "URAM288":
            raise MissingReference(
                "URAM288 encoding absent from both public sources "
                "(prjuray-db and BitFiltrator cover RAMB18/36 only)"
            )
    except MissingReference as e:
        return block, {}, "n/a", e.detail
    raise AssertionError("unhandled block %s" % block)


def compute_diff(ref_bits, emitted_bits):
    """diff_bits between reference and emitted bitsets. See module doc."""
    diff = 0
    for pos, req in ref_bits.items():
        got = emitted_bits.get(pos, 0)
        if got != req:
            diff += 1
    for pos, val in emitted_bits.items():
        if pos not in ref_bits and val == 1:
            diff += 1
    return diff


def emitted_bitset_for(emitter, bit_path, part, ref):
    """Emitted bitsets for an already-emitted bitstream, per block.

    Prefers the emitter `bits` subcommand on the real .bit bytes
    (block-labelled lines). Falls back to parse-mediated derivation
    (emitter parse output fed back through the reference resolver)
    and reports which path ran.
    Returns ({block: bitset}, via, missing). via is 'bits',
    'parse-mediated', or None when blocked.
    """
    per_block = emitter.bits(bit_path, part)
    if per_block is not None:
        return per_block, "bits", None
    parsed = emitter.parse(bit_path, part)
    parsed_features = parse_fasm(parsed, "emitter parse output")
    per_block = {}
    missing = []
    for feat in parsed_features:
        block, bits, _, miss = resolve_feature(ref, part, feat)
        if miss:
            missing.append(miss)
        else:
            per_block.setdefault(block, {}).update(bits)
    if missing:
        return per_block, "parse-mediated", "; ".join(sorted(set(missing)))
    return per_block, "parse-mediated", None


def diff_design(ref, emitter, design, part, fasm_path, seed, placement_hash,
                tier, workdir, reference_bit=None):
    """Full diff for one design on one part. Returns (records, framing)."""
    with open(fasm_path) as f:
        text = f.read()
    features = parse_fasm(text, fasm_path)
    by_block = {}
    for feat in features:
        by_block.setdefault(classify(feat), []).append(feat)

    # Emit once per design when an emitter exists; every block compares
    # against the same bitstream, each against its own block's bits.
    bit_path = None
    emit_error = None
    emitted_per_block = {}
    emitted_via = None
    emit_missing = None
    if emitter is not None:
        bit_path = os.path.join(workdir, "emit.bit")
        try:
            emitter.emit(fasm_path, part, bit_path)
            emitted_per_block, emitted_via, emit_missing = emitted_bitset_for(
                emitter, bit_path, part, ref
            )
        except Exception as e:
            emit_error = str(e)
            bit_path = None

    records = []
    for block in BLOCKS:
        if block == "FRAMING":
            continue
        rec = {
            "block": block,
            "design": design,
            "part": part,
            "seed": seed,
            "placement_hash": placement_hash,
            "tier": tier,
        }
        feats = by_block.get(block, [])
        if not feats:
            # The emitter set bits for a block the design does not use:
            # that is a real difference, not a pass.
            extra = emitted_per_block.get(block, {})
            extra_set = sum(1 for v in extra.values() if v == 1)
            if emitter is not None and emit_error is None and extra_set:
                rec["diff_bits"] = extra_set
                rec["total_bits"] = 0
                rec["status"] = "red"
                rec["coord_system"] = "absolute"
                rec["missing"] = None
                rec["detail"] = (
                    "emitter set %d bits in %s, which the golden design "
                    "does not use" % (extra_set, block)
                )
                records.append(rec)
                continue
            rec["diff_bits"] = None
            rec["total_bits"] = 0
            rec["status"] = "blocked"
            rec["coord_system"] = "n/a"
            rec["missing"] = (
                NOT_EXERCISED.get(block)
                or "no %s features in golden FASM for %s/%s; block not "
                "exercised by this design" % (block, design, part)
            )
            records.append(rec)
            continue
        ref_bits = {}
        missing = []
        coord_systems = set()
        for feat in feats:
            _, bits, coord, miss = resolve_feature(ref, part, feat)
            if miss:
                missing.append(miss)
            else:
                ref_bits.update(bits)
                coord_systems.add(coord)
        rec["total_bits"] = len(ref_bits)
        rec["coord_system"] = (
            sorted(coord_systems)[0] if len(coord_systems) == 1
            else "mixed" if coord_systems else "n/a"
        )
        if missing:
            rec["diff_bits"] = None
            rec["status"] = "blocked"
            rec["missing"] = "; ".join(sorted(set(missing)))
            records.append(rec)
            continue
        if emitter is None:
            rec["diff_bits"] = None
            rec["status"] = "blocked"
            rec["missing"] = NO_EMITTER_MISSING
            records.append(rec)
            continue
        if emit_error is not None:
            rec["diff_bits"] = None
            rec["status"] = "blocked"
            rec["missing"] = "emitter failed to emit: %s" % emit_error
            records.append(rec)
            continue
        if emit_missing:
            rec["diff_bits"] = None
            rec["status"] = "blocked"
            rec["missing"] = emit_missing
            records.append(rec)
            continue
        # Tile-relative reference bits cannot be compared against the
        # emitter's absolute bitset until the tilegrid lands.
        if rec["coord_system"] == "tile-relative":
            rec["diff_bits"] = None
            rec["status"] = "blocked"
            rec["missing"] = (
                "reference bits are tile-relative; absolute FARs need the "
                "%s tilegrid (prjuray-db ships xczu3eg tilegrids only)" % part
            )
            records.append(rec)
            continue
        emitted = emitted_per_block.get(block, {})
        rec["diff_bits"] = compute_diff(ref_bits, emitted)
        rec["status"] = "green" if rec["diff_bits"] == 0 else "red"
        rec["emitted_via"] = emitted_via
        records.append(rec)

    # FRAMING block: round-trip stage.
    rt = roundtrip_mod.run_roundtrip(emitter, fasm_path, part, workdir, design)
    framing = {
        "block": "FRAMING",
        "design": design,
        "part": part,
        "seed": seed,
        "placement_hash": placement_hash,
        "tier": tier,
        "coord_system": "n/a",
    }
    framing["total_bits"] = len(rt["checks_passed"]) + len(rt["checks_failed"])
    framing["diff_bits"] = None if rt["status"] == "blocked" else len(rt["checks_failed"])
    framing["status"] = rt["status"]
    if rt["missing"]:
        framing["missing"] = rt["missing"]
    framing["detail"] = rt["detail"]
    if rt["checks_skipped"]:
        framing["checks_skipped"] = rt["checks_skipped"]
    records.append(framing)
    return records


def greedy_order(records):
    """Order records by effective size descending. That ordering IS the queue."""
    rank = {"red": 0, "blocked": 1, "green": 2}

    def key(rec):
        eff = rec["diff_bits"] if rec["diff_bits"] is not None else rec["total_bits"]
        return (-eff, rank[rec["status"]], rec["block"])

    return sorted(records, key=key)


def format_queue(records, design, part):
    lines = []
    lines.append("greedy queue: %s / %s (largest diff first)" % (design, part))
    lines.append(
        "%-4s %-13s %10s %10s %-8s %s"
        % ("rank", "block", "diff_bits", "total_bits", "status", "missing")
    )
    for i, rec in enumerate(greedy_order(records), 1):
        diff = "-" if rec["diff_bits"] is None else str(rec["diff_bits"])
        missing = (rec.get("missing") or "")[:100]
        lines.append(
            "%-4d %-13s %10s %10d %-8s %s"
            % (i, rec["block"], diff, rec["total_bits"], rec["status"], missing)
        )
    return "\n".join(lines)


def write_jsonl(records, path):
    with open(path, "w") as f:
        for rec in records:
            f.write(json.dumps(rec, sort_keys=True) + "\n")
