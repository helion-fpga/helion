# Round-trip and framing stage.
#
# Round-trip check (INTERFACES.md): parse_bitstream(emit_bitstream(f))
# == f exactly, for every golden FASM. A mismatch is a FRAMING red,
# not a warning.
#
# The comparison runs on COMPLETE FASM (canonical_roundtrip_fasm),
# not raw text. The real emitter's parse-back renders exactly one
# .V0/.V1 line per FF in the device for DBs with the pair structure
# (bitgen-core session 2, exact-complement rule), so a partial FASM
# would fail the check even when the emitter is exactly correct.
# The canonical form therefore:
# - normalizes FF INIT/SRVAL spellings to the canonical DB
#   vocabulary .V0/.V1 (valued `AFF.INIT = 0` becomes `AFF.INIT.V0`;
#   the V suffix names the value, bit = NOT(value) per the resolved
#   polarity, researcher session 1),
# - normalizes Verilog-style values (`64'h...`, `1'b0`) to the
#   golden's spelling (lowercase 0x, decimal),
# - completes every FF in the design with exactly one INIT line and
#   exactly one SRVAL line: an FF with no INIT line in the input
#   gets the default .V0 (the Xilinx default FF INIT is 0), and an
#   FF with no SRVAL line gets the default .V0 (the Xilinx default
#   SRVAL is 0). The emitter does not invent defaults; the harness
#   completes the comparison form. The design's FF list comes from
#   the golden design definition (gen_golden).
# - sorts the lines, so the emitter's parse-back order does not
#   matter; the comparison is on the set of lines.
#
# The comparison is scoped to the design's tiles (diff-harness
# session 4, CTO ruling): the real emitter's parse-back renders
# lines for every tile in the device model, so parse-back lines on
# tiles the design does not occupy are dropped before comparing.
# The design's tile set is the set of tiles in the golden FASM
# itself (SLICE_X0Y0, INT_X0Y0, HPIO_RIGHT_X0Y0 for the v1
# goldens). The per-block bits diff still catches stray set bits
# anywhere, so scoping the FRAMING comparison loses no coverage.
# The comparison is also scoped to the design's features
# (diff-harness session 5, CTO ruling): the real emitter's
# parse-back renders lines for unused BELs and attribute pairs on
# the design's own tile (16 INIT.V1 + 16 SRVAL.V1 +
# CEUSED/CLKINV/SRINV/SRUSED/MODE/SYNC/CARRY_TYPE/DI complement
# lines per untouched CLEL_L tile, counted from the real
# segbits_clel_l.db: 57 complement lines per tile, re-verified
# this session by implementing the exact-complement rule over the
# DB). Those lines are not in the completed golden. Both sides are
# therefore filtered after canonicalization to the design's feature
# set: the golden's features plus the completed FF INIT/SRVAL
# defaults. A feature is keyed by (tile, attribute): the .V0/.V1
# suffix is the value, not part of the key, so a wrong value on a
# design feature still fails the check. The per-block bits diff
# still catches stray set bits anywhere, so the scoping loses no
# coverage. Completing the golden tile-wide with attribute defaults
# was the alternative; the CTO ruled feature scoping.
#
# Independent byte checks on the emitted .bit (no emitter code involved):
# - sync word 0xAA995566 present
# - IDCODE matches the part (xcku3p 0x04a63093 from the device file;
#   xcvu3p IDCODE is not in any public source yet, so that check is
#   skipped and the skip is recorded, not silently passed)
#
# With no emitter, FRAMING is blocked with the missing piece named.
# A harness that reports green on a check it did not run is a FAIL.
#
# No em dashes anywhere.

import os
import re

from emitter import EmitterError, NO_EMITTER_MISSING
from fasm import FasmError, Feature, parse_fasm, serialize_fasm
from gen_golden import design_ff_locations
from reference import XCKU3P_IDCODE

SYNC_WORD = b"\xaa\x99\x55\x66"

_FF_ATTR_RE = re.compile(r"^([A-H])FF2?\.(INIT|SRVAL)$")
_FF_V_RE = re.compile(r"^([A-H])FF2?\.(INIT|SRVAL)\.V([01])$")
_VERILOG_HEX_RE = re.compile(r"^(\d+)'h([0-9a-fA-F]+)$")
_VERILOG_BIN_RE = re.compile(r"^(\d+)'b([01]+)$")
_VERILOG_DEC_RE = re.compile(r"^(\d+)'d(\d+)$")
_HEX_RE = re.compile(r"^0[xX][0-9a-fA-F]+$")


def _ff_attr_value(value, source, line_no):
    """Parse an FF INIT/SRVAL value string to 0 or 1. Fail loud."""
    if value is None:
        raise FasmError(
            "%s:%d: FF attribute with no value" % (source, line_no)
        )
    v = value.strip()
    n = None
    if v in ("0", "1"):
        n = int(v)
    elif _HEX_RE.match(v):
        n = int(v, 16)
    else:
        m = _VERILOG_BIN_RE.match(v) or _VERILOG_DEC_RE.match(v)
        if m:
            n = int(m.group(2), 2 if "'b" in v else 10)
        else:
            m = _VERILOG_HEX_RE.match(v)
            if m:
                n = int(m.group(2), 16)
    if n not in (0, 1):
        raise FasmError(
            "%s:%d: FF attribute value %r is not 0 or 1"
            % (source, line_no, value)
        )
    return n


def _normalize_value(value):
    """Canonical spelling for a non-FF value string.

    Verilog-style values become the golden's spelling: `64'h...`
    (what the real emitter's parse-back renders, uppercase hex)
    becomes lowercase 0x with no redundant leading zeros; `N'b...`
    and `N'd...` become decimal. Anything else is returned
    unchanged (parse_fasm already normalized 0x hex).
    """
    if value is None:
        return None
    v = value.strip()
    m = _VERILOG_HEX_RE.match(v)
    if m:
        return "0x%x" % int(m.group(2), 16)
    m = _VERILOG_BIN_RE.match(v)
    if m:
        return str(int(m.group(2), 2))
    m = _VERILOG_DEC_RE.match(v)
    if m:
        return str(int(m.group(2), 10))
    return v


def canonical_roundtrip_fasm(text, design, source="<string>", tiles=None):
    """Canonical form for the round-trip comparison. See module doc.

    Parses the text, normalizes spellings, completes every FF in
    the design with exactly one INIT line and exactly one SRVAL
    line (missing FFs get the default .V0 for both), sorts the
    lines, and serializes. When `tiles` is given, lines on tiles
    outside that set are dropped before completing: that is the
    parse-back side, scoped to the tiles the design occupies.
    Raises FasmError on malformed input or an unknown design, like
    canonical_fasm. This is NOT the placement-hash canonical form
    (fasm.py): changing that would invalidate the golden hashes, so
    the two stay separate.
    """
    try:
        locations = design_ff_locations(design)
    except KeyError:
        raise FasmError("unknown design %r" % design)
    features = parse_fasm(text, source)
    if tiles is not None:
        features = [f for f in features if f.tile in tiles]
    out = []
    has_init = set()
    has_srval = set()
    for feat in features:
        m = _FF_ATTR_RE.match(feat.feature)
        if m:
            # Valued FF INIT/SRVAL: the V suffix names the value.
            # A bare AFF.INIT (value None) fails here in
            # _ff_attr_value: no value selects V0 or V1, never guess.
            bel = feat.feature.split(".")[0]
            n = _ff_attr_value(feat.value, source, feat.line_no)
            out.append(Feature(
                feat.tile, "%s.%s.V%d" % (bel, m.group(2), n),
                None, feat.line_no,
            ))
            if m.group(2) == "INIT":
                has_init.add((feat.tile, bel))
            else:
                has_srval.add((feat.tile, bel))
            continue
        if _FF_V_RE.match(feat.feature):
            if feat.value is not None:
                raise FasmError(
                    "%s:%d: .V0/.V1 feature takes no value in line %r"
                    % (source, feat.line_no, repr(feat))
                )
            out.append(feat)
            bel = feat.feature.split(".")[0]
            if ".INIT." in feat.feature:
                has_init.add((feat.tile, bel))
            elif ".SRVAL." in feat.feature:
                has_srval.add((feat.tile, bel))
            continue
        out.append(Feature(
            feat.tile, feat.feature,
            _normalize_value(feat.value), feat.line_no,
        ))
    for tile, bel in locations:
        if (tile, bel) not in has_init:
            out.append(Feature(tile, "%s.INIT.V0" % bel, None, 0))
        if (tile, bel) not in has_srval:
            out.append(Feature(tile, "%s.SRVAL.V0" % bel, None, 0))
    out.sort(key=repr)
    return serialize_fasm(out)


def _feature_key(feat):
    """Identity of a canonical feature for the feature-scoped comparison.

    The key is (tile, attribute): for an FF .V0/.V1 line the key is
    (tile, bel.attr), the .V0/.V1 suffix stripped, because the suffix
    IS the value (V0 = value 0, V1 = value 1). Two lines for the same
    attribute with different V suffixes are the same feature with
    different values, so a wrong value on a design feature survives
    the scope and still fails the check. Every other feature keys on
    its full feature string; its value is compared separately.
    """
    m = _FF_V_RE.match(feat.feature)
    if m:
        bel = feat.feature.split(".")[0]
        return (feat.tile, "%s.%s" % (bel, m.group(2)))
    return (feat.tile, feat.feature)


def roundtrip_feature_keys(text, source="<canonical>"):
    """Feature-key set of a canonical FASM text. See _feature_key."""
    return {_feature_key(f) for f in parse_fasm(text, source)}


def scope_fasm_to_feature_keys(text, keys, source="<canonical>"):
    """Canonical FASM text restricted to the given feature-key set.

    Parses, drops every line whose feature key is not in `keys`,
    sorts, and serializes. Used after canonicalization to scope the
    FRAMING comparison to the design's feature set (session 5, CTO
    ruling): the golden's features plus the completed FF INIT/SRVAL
    defaults. Lines the parse-back renders for unused BELs and
    attribute pairs on the design's own tile are dropped here; the
    per-block bits diff still catches stray set bits anywhere.
    Scoping a text to its own key set is a no-op by construction.
    """
    kept = [f for f in parse_fasm(text, source) if _feature_key(f) in keys]
    kept.sort(key=repr)
    return serialize_fasm(kept)


def check_bit_bytes(bit_path, part):
    """Independent checks on raw bitstream bytes.

    Returns (passed, failed, skipped) lists of check names.
    """
    passed, failed, skipped = [], [], []
    with open(bit_path, "rb") as f:
        data = f.read()
    if SYNC_WORD in data:
        passed.append("sync-word-present")
    else:
        failed.append("sync-word-present")
    if part == "xcku3p":
        idcode = XCKU3P_IDCODE.to_bytes(4, "big")
        if idcode in data:
            passed.append("idcode-0x04a63093-present")
        else:
            failed.append("idcode-0x04a63093-present")
    else:
        skipped.append(
            "idcode-present (no public IDCODE for %s yet)" % part
        )
    return passed, failed, skipped


def run_roundtrip(emitter, fasm_path, part, workdir, design):
    """Run the round-trip stage for one golden FASM.

    Returns a dict with status (green/red/blocked), checks run, and
    the missing reason when blocked.

    Never raises: an emitter failure is a FRAMING record, not a
    traceback. A run that dies on a broken emitter loses every
    record, including the blocked records for combos that never ran
    (critic session 5, FLAG 2). Failure kinds, in order:
    - golden FASM itself malformed: blocked, the golden is at fault,
      not the emitter.
    - emitter subprocess failure (emit/parse exit nonzero, not found,
      timeout): blocked, the check could not run; mirrors the
      emit_error path in diff_engine.diff_design.
    - emitter parse output is not valid FASM: FRAMING red. The
      emitter broke the parse contract, the same as a corrupt parse
      (valid FASM, wrong value), which is already a red. One failed
      check, the error named in the detail.
    - anything else (e.g. emit exited 0 but wrote no .bit file):
      blocked, the exception named. The check did not run.

    Both sides are compared in complete FASM
    (canonical_roundtrip_fasm): the golden's unspecified FF INITs
    and SRVALs complete to .V0, and the parse-back's .V0/.V1
    spellings are compared as values. The parse-back side is
    additionally scoped to the design's tiles: lines on tiles the
    golden does not occupy are dropped before comparing, because
    the real emitter's parse-back renders lines for every tile in
    the device model. Both sides are then scoped to the design's
    feature set (session 5, CTO ruling): the golden's features
    plus the completed FF INIT/SRVAL defaults. A feature is keyed
    by (tile, attribute), the .V0/.V1 suffix stripped, so a wrong
    value on a design feature still fails the check, while lines
    the parse-back renders for unused BELs and attribute pairs on
    the design's own tile are dropped. The per-block bits diff
    still catches stray set bits anywhere, so the scoping loses no
    coverage. A faithful emitter reproduces a partial golden clean:
    both sides complete identically. The real emitter renders .V1
    for an unmentioned FF (bit 0), so a partial golden still fails
    against it, as a precise .V0 vs .V1 value mismatch: the emitted
    bitstream genuinely encodes INIT=1 where the design default
    is INIT=0. A genuinely wrong INIT or SRVAL value fails the
    same way. Only a complete golden can go green against the real
    emitter, which is the contract: round-trip exactness requires
    complete FASM.
    """
    if emitter is None:
        return {
            "status": "blocked",
            "missing": NO_EMITTER_MISSING,
            "checks_passed": [],
            "checks_failed": [],
            "checks_skipped": [],
            "detail": "round-trip not run: no emitter",
        }
    with open(fasm_path) as f:
        golden_text = f.read()
    try:
        golden_canon = canonical_roundtrip_fasm(golden_text, design, fasm_path)
        # The design's feature set is the completed golden's
        # features. Scoping the golden to its own keys is a no-op
        # by construction; it is done here so both sides go through
        # the same filter.
        golden_keys = roundtrip_feature_keys(golden_canon, fasm_path)
        golden_canon = scope_fasm_to_feature_keys(
            golden_canon, golden_keys, fasm_path)
    except FasmError as e:
        return {
            "status": "blocked",
            "missing": "golden FASM is not valid FASM: %s" % e,
            "checks_passed": [],
            "checks_failed": [],
            "checks_skipped": [],
            "detail": "round-trip not run: golden FASM invalid",
        }
    # The design's tiles are the tiles in the golden FASM itself.
    # The parse-back side is scoped to them; see module doc.
    design_tiles = {f.tile for f in parse_fasm(golden_text, fasm_path)}
    bit_path = os.path.join(workdir, "roundtrip.bit")
    try:
        emitter.emit(fasm_path, part, bit_path)
        parsed_text = emitter.parse(bit_path, part)
        parsed_canon = canonical_roundtrip_fasm(
            parsed_text, design, "emitter parse output",
            tiles=design_tiles)
        parsed_canon = scope_fasm_to_feature_keys(
            parsed_canon, golden_keys, "emitter parse output")
        passed, failed, skipped = check_bit_bytes(bit_path, part)
    except FasmError as e:
        return {
            "status": "red",
            "missing": None,
            "checks_passed": [],
            "checks_failed": ["roundtrip-parse(emit(f))==f"],
            "checks_skipped": [],
            "detail": "emitter parse output is not valid FASM: %s" % e,
        }
    except EmitterError as e:
        return {
            "status": "blocked",
            "missing": "emitter failed during round-trip: %s" % e,
            "checks_passed": [],
            "checks_failed": [],
            "checks_skipped": [],
            "detail": "round-trip not run: emitter failed",
        }
    except Exception as e:  # noqa: BLE001 - the run must never die here
        return {
            "status": "blocked",
            "missing": "round-trip failed: %s: %s" % (type(e).__name__, e),
            "checks_passed": [],
            "checks_failed": [],
            "checks_skipped": [],
            "detail": "round-trip not run: unexpected failure",
        }
    if parsed_canon == golden_canon:
        passed.append("roundtrip-parse(emit(f))==f")
    else:
        failed.append("roundtrip-parse(emit(f))==f")
    status = "green" if not failed else "red"
    return {
        "status": status,
        "missing": None,
        "checks_passed": passed,
        "checks_failed": failed,
        "checks_skipped": skipped,
        "detail": "round-trip %s: %d passed, %d failed, %d skipped"
        % (status, len(passed), len(failed), len(skipped)),
    }
