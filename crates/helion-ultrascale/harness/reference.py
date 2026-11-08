# Tier-1 reference: public segbits databases plus BitFiltrator tables.
#
# Two sources, cross-checked where they overlap:
# - BitFiltrator (EPFL, MIT): resources/archs/ULTRASCALE_PLUS.json
#   (LutLoc, RegLoc, BramMemLoc, BramMemParityLoc) plus
#   resources/devices/xcku3p.json (column majors, tile types, IDCODE).
# - prjuray-db (f4pga, CC0 1.0): zynqusp/segbits_*.db, FASM format
#   <tiletype>.<feature> <frame>_<bit> [!...], where frame is the minor
#   address within the tile's column and bit is the bit offset within
#   the 93-word frame (0..2975). Verified: prjuray CLEL_L.ALUT.INIT[0]
#   11_15 equals BitFiltrator LutLoc pair [0] (minor 11, offset 15),
#   and BRAM INIT_00[0..4] 00_00/00_12/00_24/00_36/00_60 equals
#   BramMemLoc Y0 frame_ofst [0,12,24,36,60]. The two sources agree
#   bit for bit on the mapping; the harness checks that on every run.
#
# Reference bits are returned as dicts {(far, offset): value} for
# absolute coordinates, or {(tile, frame, offset): value} for
# tile-relative coordinates where the target part's tilegrid is missing.
# A bit that must read 0 is present with value 0: it counts as a
# constrained bit, and an emitter that sets it differs.
#
# Nothing here invents an encoding. Any feature with no public encoding
# raises MissingReference naming the exact missing data.
#
# No em dashes anywhere.

import json
import os

HARNESS_DIR = os.path.dirname(os.path.abspath(__file__))
REF_DIR = os.path.join(HARNESS_DIR, "reference")
PRJURAY_DIR = os.path.join(REF_DIR, "prjuray-db", "zynqusp")

FRAME_WORDS = 93
FRAME_BITS = FRAME_WORDS * 32  # 2976

# FAR layout, UltraScale+ (UG570 table 9-21, via BitFiltrator arch_spec.py):
# bits 31:27 reserved, 26:24 block type, 23:18 row, 17:8 column, 7:0 minor.
BLOCK_CLB_IO_CLK = 0
BLOCK_BRAM_CONTENT = 1

NUM_CLB_PER_COLUMN = 60
NUM_BRAM18_PER_COLUMN = 24

# INT column rule, tile-type dependent, from two independent public sources:
# - xczu3eg tilegrid geometry (prjuray-db): a CLEL_R tile has its serving
#   INT tile on the left at column major - 1 (e.g. INT col 35 serves CLB
#   col 36, INT_X11Y60 baseaddr 0x00042300 vs CLEL_R_X11Y60 0x00042400);
#   CLEL_L, CLEM, and CLEM_R tiles have it on the right at + 1.
# - xcku3p device-file minors table: 52 CLB columns have a 76-minor left
#   neighbour, all CLEL_R; 61 have one on the right, all other CLB types;
#   0 overlap, 0 neither.
# PROVISIONAL for xcku3p/xcvu3p: no xcku3p/xcvu3p tilegrid exists in any
# public DB, and the +1 side has never been exercised by a golden (every
# golden INT line is INT_X0Y0 on CLEL_R column 0, where -1 holds).
def int_col_offset_from_clb(tile_type):
    """INT column major offset from the CLB column major, by CLB tile type."""
    return -1 if tile_type == "CLEL_R" else +1

XCKU3P_IDCODE = 0x04A63093


class MissingReference(Exception):
    """A feature has no public encoding. .detail names the exact gap."""

    def __init__(self, detail):
        super().__init__(detail)
        self.detail = detail


def far(block_type, row, col, minor):
    return ((block_type & 0x7) << 24) | ((row & 0x3F) << 18) | ((col & 0x3FF) << 8) | (minor & 0xFF)


def parse_segbits_db(path):
    """Parse one prjuray segbits .db file.

    Returns {feature: [(frame, bit, value), ...]}. A leading '!' on a
    token means the bit must read 0.
    """
    out = {}
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            parts = line.split()
            feature, tokens = parts[0], parts[1:]
            bits = []
            for tok in tokens:
                val = 1
                if tok.startswith("!"):
                    val = 0
                    tok = tok[1:]
                frame_s, bit_s = tok.split("_")
                bits.append((int(frame_s), int(bit_s), val))
            out[feature] = bits
    return out


class Tier1Reference:
    def __init__(self, ref_dir=REF_DIR):
        self.ref_dir = ref_dir
        self._arch = None
        self._devices = {}
        self._segbits = {}

    # ---- loading ----

    def arch(self):
        if self._arch is None:
            with open(os.path.join(self.ref_dir, "ULTRASCALE_PLUS.json")) as f:
                self._arch = json.load(f)
        return self._arch

    def device(self, part):
        if part not in self._devices:
            if part == "xcku3p":
                path = os.path.join(self.ref_dir, "xcku3p.json")
            elif part == "xcvu3p":
                raise MissingReference(
                    "xcvu3p device file not yet generated (device-file "
                    "worker): BitFiltrator ships no xcvu3p.json and "
                    "prjuray-db/zynqusp covers Zynq US+ only; xcku3p is "
                    "the control part until the xcvu3p file lands"
                )
            else:
                raise MissingReference("no device file for part %r" % part)
            if not os.path.exists(path):
                raise MissingReference("device file missing: %s" % path)
            with open(path) as f:
                self._devices[part] = json.load(f)
        return self._devices[part]

    def segbits(self, name):
        """Load zynqusp/segbits_<name>.db (name without prefix/suffix)."""
        if name not in self._segbits:
            path = os.path.join(PRJURAY_DIR, "segbits_%s.db" % name)
            if not os.path.exists(path):
                raise MissingReference("segbits DB missing: %s" % path)
            self._segbits[name] = parse_segbits_db(path)
        return self._segbits[name]

    # ---- device-file helpers (BitFiltrator resolution flow) ----

    def _slr_row(self, part, y, per_column):
        """(relative row major, y offset within the column) for a BEL row."""
        dev = self.device(part)
        slr = dev["slrs"]["SLR0"]
        row_major = y // per_column
        if row_major < slr["min_clock_region_row_idx"] or row_major > slr["max_clock_region_row_idx"]:
            raise MissingReference(
                "Y=%d is outside the clock-region rows of %s" % (y, part)
            )
        return row_major, y % per_column

    def _clb_column(self, part, row_major, x):
        dev = self.device(part)
        row = dev["slrs"]["SLR0"]["rowMajors"][str(row_major)]
        col_majors = row["clb_colMajors"]
        tile_types = row["clb_tileTypes"]
        if str(x) not in col_majors:
            raise MissingReference(
                "CLB column %d has no column major in %s row %d"
                % (x, part, row_major)
            )
        return col_majors[str(x)], tile_types[str(x)]

    # ---- LUT / FF / BRAM via BitFiltrator ----

    def resolve_lut(self, part, slice_x, slice_y, bel, init_value):
        """Reference bits for one LUT INIT: {(far, offset): value}."""
        row_major, y_ofst = self._slr_row(part, slice_y, NUM_CLB_PER_COLUMN)
        col_major, tile_type = self._clb_column(part, row_major, slice_x)
        lut_loc = self.arch()[tile_type]["LutLoc"]["Y_ofst"][str(y_ofst)]
        try:
            minors = lut_loc["minor"][bel]
            ofsts = lut_loc["frame_ofst"][bel]
        except KeyError:
            raise MissingReference(
                "no LutLoc entry for %s %s at Y offset %d"
                % (tile_type, bel, y_ofst)
            )
        if len(minors) != 64 or len(ofsts) != 64:
            raise MissingReference(
                "LutLoc entry for %s %s has %d bits, expected 64"
                % (tile_type, bel, len(minors))
            )
        bits = {}
        for i in range(64):
            f = far(BLOCK_CLB_IO_CLK, row_major, col_major, minors[i])
            bits[(f, ofsts[i])] = (init_value >> i) & 1
        return bits

    def resolve_ff(self, part, slice_x, slice_y, bel, init_value):
        """Reference bits for one FF INIT: {(far, offset): value}.

        Polarity is INVERTED, per BitFiltrator bitstream_state_checker.py
        citing XAPP1230 pg 20: CLB registers are inverted in the frame,
        so INIT=0 stores frame bit 1 and INIT=1 stores frame bit 0.
        prjuray agrees: AFF.INIT.V0 sets the bit, AFF.INIT.V1 clears it.
        """
        row_major, y_ofst = self._slr_row(part, slice_y, NUM_CLB_PER_COLUMN)
        col_major, tile_type = self._clb_column(part, row_major, slice_x)
        reg_loc = self.arch()[tile_type]["RegLoc"]["Y_ofst"][str(y_ofst)]
        try:
            minor = reg_loc["minor"][bel]
            ofst = reg_loc["frame_ofst"][bel]
        except KeyError:
            raise MissingReference(
                "no RegLoc entry for %s %s at Y offset %d"
                % (tile_type, bel, y_ofst)
            )
        f = far(BLOCK_CLB_IO_CLK, row_major, col_major, minor)
        return {(f, ofst): 1 - (init_value & 1)}

    def resolve_srval(self, part, slice_x, slice_y, bel, srval_value):
        """Reference bits for one FF SRVAL: {(far, offset): value}.

        Polarity is INVERTED, same as INIT (XAPP1230 pg 20): SRVAL=0
        stores frame bit 1 and SRVAL=1 stores frame bit 0. prjuray
        agrees: AFF.SRVAL.V0 sets the bit, AFF.SRVAL.V1 clears it.

        Position derives from the INIT RegLoc entry: same frame
        (minor), bit offset delta -1 for A/C/E/G letters, +1 for
        B/D/F/H letters. Verified against the prjuray segbits DBs
        for all 16 FFs on all four CLB tile types (CLEL_L, CLEL_R,
        CLEM, CLEM_R): the derived position equals the .SRVAL.V0
        line, and .SRVAL.V1 is its exact complement at the same
        position (see cross_check_srval).
        """
        if not bel or bel[0] not in "ABCDEFGH":
            raise MissingReference(
                "SRVAL BEL %r is not a CLB flip-flop" % bel
            )
        row_major, y_ofst = self._slr_row(part, slice_y, NUM_CLB_PER_COLUMN)
        col_major, tile_type = self._clb_column(part, row_major, slice_x)
        reg_loc = self.arch()[tile_type]["RegLoc"]["Y_ofst"][str(y_ofst)]
        try:
            minor = reg_loc["minor"][bel]
            ofst = reg_loc["frame_ofst"][bel]
        except KeyError:
            raise MissingReference(
                "no RegLoc entry for %s %s at Y offset %d"
                % (tile_type, bel, y_ofst)
            )
        delta = -1 if bel[0] in "ACEG" else +1
        f = far(BLOCK_CLB_IO_CLK, row_major, col_major, minor)
        return {(f, ofst + delta): 1 - (srval_value & 1)}

    def resolve_bram(self, part, bram_x, bram_y, size_kb, init_bits):
        """Reference bits for one RAMB18 INIT: {(far, offset): value}.

        init_bits is a list of 16384 values (content) or 2048 (parity);
        use resolve_bram_parity for parity. Only 18K is supported:
        BitFiltrator documents RAMB18 only.
        """
        if size_kb != 18:
            raise MissingReference(
                "BRAM size %dK unsupported: BitFiltrator locate_bram "
                "supports 18K only; RAMB36 is two 18K halves and needs "
                "the split documented before it can resolve" % size_kb
            )
        row_major, y_ofst = self._slr_row(part, bram_y, NUM_BRAM18_PER_COLUMN)
        dev = self.device(part)
        row = dev["slrs"]["SLR0"]["rowMajors"][str(row_major)]
        col_majors = row["bram_content_colMajors"]
        if str(bram_x) not in col_majors:
            raise MissingReference(
                "BRAM column %d has no content column major in %s row %d"
                % (bram_x, part, row_major)
            )
        col_major = col_majors[str(bram_x)]
        mem_loc = self.arch()["BRAM"]["BramMemLoc"]["Y_ofst"][str(y_ofst)]
        minors, ofsts = mem_loc["minor"], mem_loc["frame_ofst"]
        if len(init_bits) != len(minors):
            raise MissingReference(
                "BRAM INIT has %d bits, BramMemLoc expects %d"
                % (len(init_bits), len(minors))
            )
        bits = {}
        for i, v in enumerate(init_bits):
            f = far(BLOCK_BRAM_CONTENT, row_major, col_major, minors[i])
            bits[(f, ofsts[i])] = v & 1
        return bits

    def resolve_bram_parity(self, part, bram_x, bram_y, initp_bits):
        row_major, y_ofst = self._slr_row(part, bram_y, NUM_BRAM18_PER_COLUMN)
        dev = self.device(part)
        row = dev["slrs"]["SLR0"]["rowMajors"][str(row_major)]
        col_majors = row["bram_content_parity_colMajors"]
        if str(bram_x) not in col_majors:
            raise MissingReference(
                "BRAM column %d has no parity column major in %s row %d"
                % (bram_x, part, row_major)
            )
        col_major = col_majors[str(bram_x)]
        par_loc = self.arch()["BRAM"]["BramMemParityLoc"]["Y_ofst"][str(y_ofst)]
        minors, ofsts = par_loc["minor"], par_loc["frame_ofst"]
        if len(initp_bits) != len(minors):
            raise MissingReference(
                "BRAM parity INIT has %d bits, BramMemParityLoc expects %d"
                % (len(initp_bits), len(minors))
            )
        bits = {}
        for i, v in enumerate(initp_bits):
            f = far(BLOCK_BRAM_CONTENT, row_major, col_major, minors[i])
            bits[(f, ofsts[i])] = v & 1
        return bits

    # ---- prjuray segbits ----

    def resolve_int_pip(self, part, int_col, int_y, pip_feature):
        """Reference bits for one INT PIP.

        int_col is the logical CLB column index the INT tile serves
        (golden FASM convention, provisional until the xcku3p tilegrid
        lands). Absolute FARs use the tile-type-dependent rule
        (CLEL_R: INT column major = CLB column major - 1; all other
        CLB tile types: + 1), flagged provisional by the caller.
        Returns {(far, offset): value}.
        """
        db = self.segbits("int")
        key = "INT." + pip_feature
        if key not in db:
            raise MissingReference(
                "INT PIP feature %r not in prjuray-db segbits_int.db "
                "(3778 entries); PIP may not exist or the INT PIP "
                "completeness audit is still open" % pip_feature
            )
        row_major, _ = self._slr_row(part, int_y, NUM_CLB_PER_COLUMN)
        col_major, tile_type = self._clb_column(part, row_major, int_col)
        int_col_major = col_major + int_col_offset_from_clb(tile_type)
        bits = {}
        for frame, bit, val in db[key]:
            f = far(BLOCK_CLB_IO_CLK, row_major, int_col_major, frame)
            bits[(f, bit)] = val
        return bits

    def resolve_hpio(self, tile, feature):
        """Reference bits for one HPIO feature, tile-relative.

        Absolute FARs need the HPIO tile's column major, which is
        unknown for xcku3p/xcvu3p (no tilegrid). Returns
        {(tile, frame, offset): value}.
        """
        db = self.segbits("hpio_right")
        key = "HPIO_RIGHT." + feature
        if key not in db:
            raise MissingReference(
                "HPIO feature %r not in prjuray-db segbits_hpio_right.db; "
                "encoding of this IO attribute is unverified" % feature
            )
        return {(tile, frame, bit): val for frame, bit, val in db[key]}

    def resolve_cmt_pip(self, tile, feature):
        """Reference bits for one CMT clock PIP, tile-relative."""
        db = self.segbits("cmt_right")
        key = "CMT_RIGHT." + feature
        if key not in db:
            raise MissingReference(
                "CMT feature %r not in prjuray-db segbits_cmt_right.db "
                "(104 lines, CLK_HROUTE PIPs only)" % feature
            )
        return {(tile, frame, bit): val for frame, bit, val in db[key]}

    # ---- cross-checks (BitFiltrator vs prjuray) ----

    def cross_check_lut(self, part, slice_x, slice_y, bel):
        """Verify BitFiltrator LutLoc equals prjuray CLEL/CLEM INIT bits.

        Returns (checked, mismatches). Raises MissingReference if either
        source lacks the entry.
        """
        row_major, y_ofst = self._slr_row(part, slice_y, NUM_CLB_PER_COLUMN)
        col_major, tile_type = self._clb_column(part, row_major, slice_x)
        lut_loc = self.arch()[tile_type]["LutLoc"]["Y_ofst"][str(y_ofst)]
        minors = lut_loc["minor"][bel]
        ofsts = lut_loc["frame_ofst"][bel]
        db_name = {
            "CLEL_L": "clel_l", "CLEL_R": "clel_r",
            "CLEM": "clem", "CLEM_R": "clem_r",
        }[tile_type]
        db = self.segbits(db_name)
        letter = bel[0]
        mismatches = []
        checked = 0
        for i in range(64):
            key = "%s.%sLUT.INIT[%d]" % (tile_type, letter, i)
            if key not in db:
                raise MissingReference(
                    "prjuray %s lacks %s for cross-check" % (db_name, key)
                )
            prjuray_bits = db[key]
            if len(prjuray_bits) != 1:
                raise MissingReference(
                    "prjuray %s has %d bits, expected exactly 1"
                    % (key, len(prjuray_bits))
                )
            frame, bit, val = prjuray_bits[0]
            if val != 1:
                raise MissingReference(
                    "prjuray %s is a must-be-0 bit, cannot cross-check "
                    "an INIT=1 mapping" % key
                )
            checked += 1
            if frame != minors[i] or bit != ofsts[i]:
                mismatches.append(
                    (i, (frame, bit), (minors[i], ofsts[i]))
                )
        return checked, mismatches

    def cross_check_ff(self, part, slice_x, slice_y, bel):
        """Verify BitFiltrator RegLoc equals prjuray FF INIT.V0 bit.

        BitFiltrator's own u-ray comparator matches the .INIT.V0 line
        (the set bit) against RegLoc. Polarity is inverted (XAPP1230):
        V0 sets the bit, V1 clears it. Returns True when the position
        matches and V0/V1 are complements at that position.
        """
        row_major, y_ofst = self._slr_row(part, slice_y, NUM_CLB_PER_COLUMN)
        col_major, tile_type = self._clb_column(part, row_major, slice_x)
        reg_loc = self.arch()[tile_type]["RegLoc"]["Y_ofst"][str(y_ofst)]
        minor = reg_loc["minor"][bel]
        ofst = reg_loc["frame_ofst"][bel]
        db_name = {
            "CLEL_L": "clel_l", "CLEL_R": "clel_r",
            "CLEM": "clem", "CLEM_R": "clem_r",
        }[tile_type]
        db = self.segbits(db_name)
        key_v0 = "%s.%s.INIT.V0" % (tile_type, bel)
        key_v1 = "%s.%s.INIT.V1" % (tile_type, bel)
        if key_v0 not in db or key_v1 not in db:
            raise MissingReference(
                "prjuray %s lacks %s or %s for cross-check"
                % (db_name, key_v0, key_v1)
            )
        frame0, bit0, val0 = db[key_v0][0]
        frame1, bit1, val1 = db[key_v1][0]
        return (
            (frame0, bit0) == (minor, ofst)
            and (frame1, bit1) == (minor, ofst)
            and val0 == 1 and val1 == 0
        )

    def cross_check_srval(self, part, slice_x, slice_y, bel):
        """Verify the SRVAL delta rule against prjuray .SRVAL.V0/.V1.

        Derives the SRVAL position from the INIT RegLoc entry (same
        frame, offset delta -1 for A/C/E/G, +1 for B/D/F/H) and checks
        it equals the prjuray .SRVAL.V0 set bit, with .SRVAL.V1 the
        exact complement at the same position. Returns True on match.
        """
        row_major, y_ofst = self._slr_row(part, slice_y, NUM_CLB_PER_COLUMN)
        col_major, tile_type = self._clb_column(part, row_major, slice_x)
        reg_loc = self.arch()[tile_type]["RegLoc"]["Y_ofst"][str(y_ofst)]
        minor = reg_loc["minor"][bel]
        ofst = reg_loc["frame_ofst"][bel]
        delta = -1 if bel[0] in "ACEG" else +1
        db_name = {
            "CLEL_L": "clel_l", "CLEL_R": "clel_r",
            "CLEM": "clem", "CLEM_R": "clem_r",
        }[tile_type]
        db = self.segbits(db_name)
        key_v0 = "%s.%s.SRVAL.V0" % (tile_type, bel)
        key_v1 = "%s.%s.SRVAL.V1" % (tile_type, bel)
        if key_v0 not in db or key_v1 not in db:
            raise MissingReference(
                "prjuray %s lacks %s or %s for cross-check"
                % (db_name, key_v0, key_v1)
            )
        frame0, bit0, val0 = db[key_v0][0]
        frame1, bit1, val1 = db[key_v1][0]
        return (
            (frame0, bit0) == (minor, ofst + delta)
            and (frame1, bit1) == (minor, ofst + delta)
            and val0 == 1 and val1 == 0
        )
