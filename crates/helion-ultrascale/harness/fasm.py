# FASM parsing, canonical serialization, and block classification.
#
# FASM text format (INTERFACES.md): one line per configured feature.
#   # comment lines start with #
#   <tile>.<feature> [= <value>]
# <tile> is the Vivado tile name (e.g. INT_X12Y45, SLICE_X23Y45).
# <feature> is the segbits feature string exactly as it appears in the
# segbits DBs (e.g. PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1, A6LUT.INIT[63:0],
# AFF.INIT). INIT values expand MSB-first across the listed bits
# (bit 0 = LSB of the value).
#
# No em dashes anywhere. Never fabricate: the parser rejects lines it
# cannot understand instead of guessing what they mean.

import re

BLOCKS = [
    "LUT_INIT",
    "FF_INIT",
    "BRAM_CONTENT",
    "BRAM_PARITY",
    "INT_PIP",
    "IO_CONFIG",
    "CLK",
    "DSP48E2",
    "URAM288",
    "FRAMING",
]

_TILE_RE = re.compile(r"^[A-Za-z0-9_]+_X\d+Y\d+$")
_FEATURE_RE = re.compile(r"^[A-Za-z0-9_\.\[\]:]+$")
_HEX_RE = re.compile(r"^0[xX][0-9a-fA-F]+$")
_LUT_INIT_RE = re.compile(r"^([A-H])6LUT\.INIT\[63:0\]$")
_FF_INIT_RE = re.compile(r"^([A-H])FF2?\.INIT$")
_FF_SRVAL_RE = re.compile(r"^([A-H])FF2?\.SRVAL$")
_SLICE_RE = re.compile(r"^SLICE_X(\d+)Y(\d+)$")
_INT_RE = re.compile(r"^INT_X(\d+)Y(\d+)$")
_BRAM_RE = re.compile(r"^RAMB(18|36)_X(\d+)Y(\d+)$")


class FasmError(ValueError):
    pass


class Feature:
    """One parsed FASM line: tile, feature, optional value string."""

    def __init__(self, tile, feature, value=None, line_no=0):
        self.tile = tile
        self.feature = feature
        self.value = value
        self.line_no = line_no

    def __repr__(self):
        if self.value is None:
            return "%s.%s" % (self.tile, self.feature)
        return "%s.%s = %s" % (self.tile, self.feature, self.value)

    def __eq__(self, other):
        return (
            isinstance(other, Feature)
            and self.tile == other.tile
            and self.feature == other.feature
            and self.value == other.value
        )

    def __hash__(self):
        return hash((self.tile, self.feature, self.value))


def normalize_value(value):
    """Canonical form for a FASM value string.

    Hex values become lowercase 0x with no redundant leading zeros
    (0x0 stays 0x0). Plain integers stay decimal. Anything else is
    returned unchanged.
    """
    if value is None:
        return None
    v = value.strip()
    if _HEX_RE.match(v):
        n = int(v, 16)
        return "0x%x" % n
    return v


def parse_fasm(text, source="<string>"):
    """Parse FASM text into a list of Feature, in file order.

    Raises FasmError on any line that is not a comment, blank, or a
    well-formed feature line. Never guesses.
    """
    features = []
    for line_no, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "=" in line:
            left, value = line.split("=", 1)
            value = normalize_value(value)
            if value is None or value == "":
                raise FasmError(
                    "%s:%d: empty value in line %r" % (source, line_no, raw)
                )
        else:
            left, value = line, None
        left = left.strip()
        if "." not in left:
            raise FasmError(
                "%s:%d: no tile.feature separator in line %r"
                % (source, line_no, raw)
            )
        tile, feature = left.split(".", 1)
        tile = tile.strip()
        feature = feature.strip()
        if not _TILE_RE.match(tile):
            raise FasmError(
                "%s:%d: bad tile name %r in line %r"
                % (source, line_no, tile, raw)
            )
        if not _FEATURE_RE.match(feature) or not feature:
            raise FasmError(
                "%s:%d: bad feature %r in line %r"
                % (source, line_no, feature, raw)
            )
        features.append(Feature(tile, feature, value, line_no))
    return features


def serialize_fasm(features):
    """Canonical serialization: one line per feature, in given order."""
    return "\n".join(repr(f) for f in features) + ("\n" if features else "")


def canonical_fasm(text, source="<string>"):
    """Parse then re-serialize: the canonical form used for comparisons."""
    return serialize_fasm(parse_fasm(text, source))


def parse_init_value(value, width=64):
    """Parse an INIT value string to an int. Hex 0x... or decimal."""
    if value is None:
        raise FasmError("INIT feature with no value")
    v = value.strip()
    if _HEX_RE.match(v):
        n = int(v, 16)
    elif v.isdigit():
        n = int(v, 10)
    else:
        raise FasmError("INIT value %r is not hex or decimal" % value)
    if n >= (1 << width):
        raise FasmError("INIT value %r exceeds %d bits" % (value, width))
    return n


def classify(feature):
    """Map a parsed Feature to its block in the registry.

    Returns one of BLOCKS (never FRAMING: framing has no FASM features).
    Raises FasmError for a feature that matches no known block pattern.
    """
    tile, feat = feature.tile, feature.feature
    if _LUT_INIT_RE.match(feat):
        return "LUT_INIT"
    if _FF_INIT_RE.match(feat) or _FF_SRVAL_RE.match(feat):
        return "FF_INIT"
    if ".ABCDFF." in feat or ".ABCFF." in feat or feat.endswith(".CEUSED.V0") or feat.endswith(".CEUSED.V1"):
        return "FF_INIT"
    if "INITP" in feat or "INIT_P" in feat:
        if "BRAM" in feat or "RAMB" in tile or "RAMB" in feat:
            return "BRAM_PARITY"
    if "INIT" in feat and ("BRAM" in feat or "RAMB" in tile or "RAMB" in feat):
        return "BRAM_CONTENT"
    if "DSP48E2" in tile or "DSP48E2" in feat:
        return "DSP48E2"
    if "URAM" in tile or "URAM" in feat:
        return "URAM288"
    if "BUFGCE" in feat or "BUFGCTRL" in feat or "MMCM" in feat or "PLL" in feat:
        return "CLK"
    if tile.startswith("CMT_") and (feat.startswith("PIP.") or ".PIP." in feat):
        return "CLK"
    if feat.startswith("PIP.") or ".PIP." in feat:
        return "INT_PIP"
    if "IOB" in feat or tile.startswith("HPIO") or tile.startswith("HDIO"):
        return "IO_CONFIG"
    raise FasmError(
        "feature %s.%s matches no block in the registry"
        % (tile, feat)
    )


def slice_xy(tile):
    m = _SLICE_RE.match(tile)
    if not m:
        raise FasmError("tile %r is not a SLICE tile" % tile)
    return int(m.group(1)), int(m.group(2))


def int_xy(tile):
    m = _INT_RE.match(tile)
    if not m:
        raise FasmError("tile %r is not an INT tile" % tile)
    return int(m.group(1)), int(m.group(2))


def bram_xy(tile):
    m = _BRAM_RE.match(tile)
    if not m:
        raise FasmError("tile %r is not a RAMB tile" % tile)
    return int(m.group(1)), int(m.group(2)), int(m.group(3))
