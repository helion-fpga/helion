"""Parse decoded FASM text.

Input is the FASM the decoder prints (one feature per line) or a
hand-written golden FASM. Both spellings are accepted:
- plain:    SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555
- valued:   SLICE_X0Y0.AFF.INIT = 0
- plain:    SLICE_X0Y0.AFF.INIT.V0
- PIP:      INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1
  (bidirectional BYPASS PIPs may carry a .FWD/.REV suffix;
  see parse_pip for the normalization)

Output: {tile: {feature: value}} where value is an int for valued
features and None for plain ones. INIT vectors keep their full
width; a bare `INIT` with no index list is the single-bit spelling
used by the goldens (AFF.INIT = 0/1).

No em dashes anywhere.
"""
from __future__ import annotations

import re
from collections import defaultdict

_VALUED = re.compile(
    r"^(?P<tile>[A-Za-z0-9_]+)\.(?P<feat>.+?)\s*=\s*(?P<val>\S+)\s*$"
)
_PLAIN = re.compile(r"^(?P<tile>[A-Za-z0-9_]+)\.(?P<feat>.+?)\s*$")
_TILE = re.compile(r"^[A-Za-z0-9_]+$")


def parse_value(text: str) -> int:
    """Parse a FASM value: 0x..., 64'h..., 1'b0, or a bare decimal."""
    text = text.strip().rstrip(";")
    m = re.match(r"^(\d+)'h([0-9a-fA-F_]+)$", text)
    if m:
        return int(m.group(2).replace("_", ""), 16)
    m = re.match(r"^(\d+)'b([01_]+)$", text)
    if m:
        return int(m.group(2).replace("_", ""), 2)
    m = re.match(r"^(\d+)'d(\d+)$", text)
    if m:
        return int(m.group(2))
    if text.lower().startswith("0x"):
        return int(text[2:].replace("_", ""), 16)
    return int(text, 0)


def parse_fasm(text: str | list[str]) -> dict[str, dict[str, int | None]]:
    """Parse FASM text (or a list of FASM lines) into
    {tile: {feature: value or None}}."""
    lines = text if isinstance(text, list) else text.splitlines()
    out: dict[str, dict[str, int | None]] = defaultdict(dict)
    for raw in lines:
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        m = _VALUED.match(line)
        if m:
            out[m.group("tile")][m.group("feat")] = parse_value(m.group("val"))
            continue
        m = _PLAIN.match(line)
        if not m:
            raise ValueError(f"cannot parse FASM line: {raw!r}")
        out[m.group("tile")][m.group("feat")] = None
    return dict(out)


def parse_fasm_file(path: str) -> dict[str, dict[str, int | None]]:
    with open(path) as f:
        return parse_fasm(f.read())


def slice_tiles(tiles: dict[str, dict]) -> dict[str, dict[str, int | None]]:
    """Only the SLICE_XxYy tiles, in sorted order."""
    return {t: f for t, f in sorted(tiles.items()) if t.startswith("SLICE_")}


def lut_depends(init: int, k: int, width: int = 6) -> bool:
    """Does the LUT INIT depend on input bit k (0 = A1)? True when
    flipping bit k changes some output bit."""
    for i in range(1 << width):
        if not (i >> k) & 1 and ((init >> i) & 1) != ((init >> (i | 1 << k)) & 1):
            return True
    return False


def init_value(features: dict[str, int | None], letter: str) -> int | None:
    """The 64-bit INIT of LUT `letter`, or None when the LUT has no
    INIT feature. Accepts the grouped spelling
    `A6LUT.INIT[63:0] = 0x...` and per-bit spellings
    `ALUT.INIT[i]` (valued 0/1 or plain presence meaning 1)."""
    grouped = features.get(f"{letter}6LUT.INIT[63:0]")
    if grouped is not None:
        return grouped
    v = 0
    found = False
    for feat, val in features.items():
        m = re.match(rf"^{letter}LUT\.INIT\[(\d+)\]$", feat)
        if m:
            found = True
            if val is None or val:
                v |= 1 << int(m.group(1))
    return v if found else None


def ff_attr(features: dict[str, int | None], ff: str, attr: str) -> int | None:
    """INIT or SRVAL of FF `ff` (e.g. AFF, AFF2): 0 or 1, or None when
    neither spelling is present. Accepts the valued spelling
    (`AFF.INIT = 0`) and the V0/V1 spelling, where the V suffix IS
    the attribute value: `AFF.INIT.V0` means INIT 0, `AFF.INIT.V1`
    means INIT 1.

    Decided on balance of evidence, NOT settled by a direct Vivado
    known-INIT comparison (see tools/ffmux-check/
    FFMUX_CHECK_REPORT.md): the Xilinx FDRE INIT default is 1'b0
    and 98.5% of used FFs in the 14-pair Vivado corpus read V0;
    the 7-series ZINI analogy (bit set means INIT 0, validated on
    ZedBoard hardware); and emitter-side consistency, where
    bitgen.rs says "V1 is 1", ff_placed_bit is !init
    (bitloc.rs:787-801), and the segbits DB has V0 setting the
    12_02 bit while V1 clears it. The missing datum is a decoded
    Vivado bitstream for a design with known INIT on a
    decoder-supported part. Note the bit-level and value-level
    readings differ: V0 sets the segbits bit, but the bit is
    NOT the value (bit = NOT(INIT)); group attributes (CEUSED,
    SRUSED, CLKINV, SRINV) keep the opposite V convention, V1
    sets, V0 clears, handled by group_attr below."""
    v = features.get(f"{ff}.{attr}")
    if v is not None:
        return 1 if v else 0
    if f"{ff}.{attr}.V0" in features:
        return 0
    if f"{ff}.{attr}.V1" in features:
        return 1
    return None


def parse_pip(feat: str) -> tuple[str, str] | None:
    """Normalized (dst, src) for a PIP feature, or None when the
    feature is not a PIP. Plain `PIP.D.S` means S drives D.
    Bidirectional BYPASS PIPs in decoded Vivado FASM carry a
    direction suffix: `PIP.D.S.FWD` means S drives D (same as
    plain), `PIP.D.S.REV` means D drives S (swapped). The
    segbits DB confirms the semantics, e.g.
    `PIP.BYPASS_E8.INT_NODE_IMUX_5_INT_OUT0.FWD` sets 71_13
    74_24 while `.REV` sets 71_13 and clears 74_24. Only the
    four BYPASS PIPs of an INT tile use these suffixes; any
    other 4-component PIP feature returns None."""
    parts = feat.split(".")
    if len(parts) < 3 or parts[0] != "PIP":
        return None
    if len(parts) == 3:
        return (parts[1], parts[2])
    if len(parts) == 4 and parts[3] == "FWD":
        return (parts[1], parts[2])
    if len(parts) == 4 and parts[3] == "REV":
        return (parts[2], parts[1])
    return None


def group_attr(features: dict[str, int | None], group: str, attr: str) -> int | None:
    """A per-group V0/V1 attribute (CEUSED, SRUSED, CLKINV, SRINV):
    V1 sets it, V0 clears it (INTERFACES.md). None when absent."""
    if f"{group}.{attr}.V1" in features:
        return 1
    if f"{group}.{attr}.V0" in features:
        return 0
    return None


def to_fasm(tiles: dict[str, dict[str, int | None]]) -> list[str]:
    """Canonical FASM lines, sorted, for reporting."""
    out = []
    for tile in sorted(tiles):
        for feat in sorted(tiles[tile]):
            v = tiles[tile][feat]
            if v is None:
                out.append(f"{tile}.{feat}")
            else:
                out.append(f"{tile}.{feat} = 0x{v:x}")
    return out
