#!/usr/bin/env python3
"""Verify BitFiltrator xczu3eg.json against public prjuray-db sources.

Sources (all license-free):
  BF  = bitfiltrator-ref/resources/devices/xczu3eg.json (MIT)
  PJ  = prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/part.json (CC0)
  TG  = prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/tilegrid.json (CC0)
Writes a report to ~/workspace/us09-device/verify-report.txt and prints
a short PASS/FAIL summary per check.

Section 10 also generates the tilegrid-derived INT column table
(data/xczu3eg_int_columns.json): (FAR row, INT tile X) -> CLB_IO_CLK
column major, from the INT tiles' own baseaddrs. The table is the
ground truth the emitter uses for xczu3eg INT tiles; the generic
provisional rule in bitgen_device.rs keys CLB columns by SLICE_X,
which is not the INT tile X on this part, so the rule is wrong for
INT_X2 and above (see section 10 notes). Regenerate the table with
`--write-int-columns`; the run asserts the table equals the tilegrid
before writing it.

Section 11 does the same for the interface columns
(data/xczu3eg_intf_columns.json): (tile type, FAR row, tile X) ->
CLB_IO_CLK column major, from the INT_INTF and HDIO tiles' own
baseaddrs. Only the four types with tilegrid windows are covered
(INT_INTF_LEFT_TERM_PSS, INT_INTF_R_PCIE4, HDIO_BOT_RIGHT,
HDIO_TOP_RIGHT); INT_INTF_L, INT_INTF_R, INT_INTF_L_PCIE4 and
INT_INTF_RIGHT_TERM_IO record no baseaddr anywhere, so they stay
unresolvable and the emitter must fail loud on them. Regenerate with
`--write-intf-columns`; without the flag the run asserts the vendored
table equals the tilegrid derivation.

This script lives in the repo at
crates/helion-ultrascale/tools/verify_xczu3eg.py. Paths are
overridable with BF_PATH / PJ_PATH / TG_PATH / OUT_PATH /
SIDECAR_PATH env vars; defaults below are the local reference clones,
except the sidecar, which defaults to the crate's data/ directory
next to this script.
"""
import json, collections, re, os, sys
from pathlib import Path

BF = os.environ.get('BF_PATH', '/home/hatch/workspace/helion-ultra-bitstream/bitfiltrator-ref/resources/devices/xczu3eg.json')
PJ = os.environ.get('PJ_PATH', '/home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/part.json')
TG = os.environ.get('TG_PATH', '/home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/tilegrid.json')
OUT = os.environ.get('OUT_PATH', '/home/hatch/workspace/us09-device/verify-report.txt')
SIDECAR = os.environ.get('SIDECAR_PATH', str(Path(__file__).resolve().parent.parent / 'data' / 'xczu3eg_int_columns.json'))
SIDECAR_INTF = os.environ.get('SIDECAR_INTF_PATH', str(Path(__file__).resolve().parent.parent / 'data' / 'xczu3eg_intf_columns.json'))
WRITE_SIDECAR = '--write-int-columns' in sys.argv
WRITE_INTF_SIDECAR = '--write-intf-columns' in sys.argv

bf = json.load(open(BF))
pj = json.load(open(PJ))
tg = json.load(open(TG))

lines = []
def w(s=''):
    lines.append(s)
results = []
def check(name, ok, detail=''):
    results.append((name, ok))
    w(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f' :: {detail}' if detail else ''))

def far_parts(baseaddr_hex):
    v = int(baseaddr_hex, 16)
    return ((v >> 24) & 0x7, (v >> 18) & 0x3f, (v >> 8) & 0x3ff, v & 0xff)

slr = bf['slrs']['SLR0']
rows = slr['rowMajors']

# ---- 1. minors tables vs part.json ----
for r in ['0', '1', '2']:
    buses = pj['rows'][r]['configuration_buses']
    pj_std = [buses['CLB_IO_CLK']['configuration_columns'][str(c)]['frame_count']
              for c in range(len(buses['CLB_IO_CLK']['configuration_columns']))]
    pj_bram = [buses['BLOCK_RAM']['configuration_columns'][str(c)]['frame_count']
               for c in range(len(buses['BLOCK_RAM']['configuration_columns']))]
    bf_std = rows[r]['num_minors_per_std_colMajor']
    bf_bram = rows[r]['num_minors_per_bram_content_colMajor']
    check(f'row {r} std minors == part.json CLB_IO_CLK frame_counts',
          bf_std == pj_std,
          f'len bf={len(bf_std)} pj={len(pj_std)} sums {sum(bf_std)} vs {sum(pj_std)}' +
          ('' if bf_std == pj_std else f' first diffs: {[(i, a, b) for i, (a, b) in enumerate(zip(bf_std, pj_std)) if a != b][:5]}'))
    check(f'row {r} bram minors == part.json BLOCK_RAM frame_counts',
          bf_bram == pj_bram,
          f'len bf={len(bf_bram)} pj={len(pj_bram)} sums {sum(bf_bram)} vs {sum(pj_bram)}')

# ---- 2. header counts vs tilegrid site counts ----
slice_sites = collections.Counter()
bram_tiles = 0
dsp_sites = 0
for name, t in tg.items():
    for s, st in t.get('sites', {}).items():
        slice_sites[st] += 1
    if t.get('type') == 'BRAM':
        bram_tiles += 1
n_slices = slice_sites['SLICEL'] + slice_sites['SLICEM']
check('num_slices == SLICE sites in tilegrid', bf['num_slices'] == n_slices,
      f"bf={bf['num_slices']} tg={n_slices} (SLICEL {slice_sites['SLICEL']} + SLICEM {slice_sites['SLICEM']})")
check('num_luts == slices*8', bf['num_luts'] == n_slices * 8, f"bf={bf['num_luts']}")
check('num_regs == slices*16', bf['num_regs'] == n_slices * 16, f"bf={bf['num_regs']}")
check('num_brams == BRAM tiles in tilegrid', bf['num_brams'] == bram_tiles,
      f"bf={bf['num_brams']} tg={bram_tiles}")
check('num_dsps == DSP48E2 sites in tilegrid', bf['num_dsps'] == slice_sites['DSP48E2'],
      f"bf={bf['num_dsps']} tg={slice_sites['DSP48E2']}")

# ---- 3. IDCODE ----
check('idcode == part.json idcode', slr['idcode'] == f"0x{pj['idcode']:08x}",
      f"bf={slr['idcode']} pj=0x{pj['idcode']:08x}")

# ---- 4. CLB columns from tilegrid ----
# tile name -> (row, slice_x, baseaddr col, tile type) from SLICE sites
CLE_TYPES = {'CLEL_L', 'CLEL_R', 'CLEM', 'CLEM_R'}
clb_derived = {}   # (row, x) -> set of (col, type)
clb_other_types = collections.Counter()
for name, t in tg.items():
    ty = t.get('type')
    bits = t.get('bits', {}).get('CLB_IO_CLK')
    if bits is None:
        continue
    for s, st in t.get('sites', {}).items():
        m = re.fullmatch(r'SLICE_X(\d+)Y(\d+)', s)
        if not m:
            continue
        x, y = int(m.group(1)), int(m.group(2))
        row = y // 60
        blk, r, col, minor = far_parts(bits['baseaddr'])
        if ty not in CLE_TYPES:
            clb_other_types[ty] += 1
        clb_derived.setdefault((row, x), set()).add((col, ty, bits['frames']))
w(f'non-base CLE tile types holding SLICE sites: {dict(clb_other_types)}')
bad = {k: v for k, v in clb_derived.items() if len({(c, t) for c, t, f in v}) != 1}
check('each (row, slice_x) has exactly one (col, tile type) in tilegrid',
      not bad, f'conflicts: {len(bad)} e.g. {dict(list(bad.items())[:3])}')
# frames per CLB column should be 16 everywhere
badf = {k: v for k, v in clb_derived.items() if {f for _, _, f in v} != {16}}
check('every CLB tile carries frames=16', not badf, f'non-16: {len(badf)}')
for r in ['0', '1', '2']:
    ri = int(r)
    bf_clb = {int(k): v for k, v in rows[r]['clb_colMajors'].items()}
    bf_ty = {int(k): v for k, v in rows[r]['clb_tileTypes'].items()}
    tg_map = {}
    for (row, x), s in clb_derived.items():
        if row == ri:
            (col, ty, f) = next(iter(s))
            tg_map[x] = (col, ty)
    check(f'row {r} clb_colMajors == tilegrid',
          all(tg_map.get(x, (None, None))[0] == c for x, c in bf_clb.items()) and set(tg_map) == set(bf_clb),
          f"bf cols={len(bf_clb)} tg cols={len(tg_map)}" +
          ('' if set(tg_map) == set(bf_clb) and all(tg_map[x][0] == c for x, c in bf_clb.items()) else
           f" diffs: {[(x, bf_clb[x], tg_map.get(x)) for x in sorted(set(bf_clb) | set(tg_map)) if tg_map.get(x, (None,))[0] != bf_clb.get(x)][:5]}"))
    check(f'row {r} clb_tileTypes == tilegrid',
          all(tg_map.get(x, (None, None))[1] == t for x, t in bf_ty.items()) and set(tg_map) == set(bf_ty),
          f"bf types={collections.Counter(bf_ty.values())} tg types={collections.Counter(v[1] for v in tg_map.values())}" +
          ('' if all(tg_map.get(x, (None, None))[1] == t for x, t in bf_ty.items()) else
           f" diffs: {[(x, bf_ty[x], tg_map.get(x)) for x in sorted(set(bf_ty) | set(tg_map)) if tg_map.get(x, (None, None))[1] != bf_ty.get(x)][:5]}"))

# ---- 5. BRAM columns from tilegrid ----
bram_derived = {}  # (row, x) -> set of (content_col, reg_col)
for name, t in tg.items():
    if t.get('type') != 'BRAM':
        continue
    bits = t.get('bits', {})
    bram_bits = bits.get('BLOCK_RAM')
    clk_bits = bits.get('CLB_IO_CLK')
    if bram_bits is None or clk_bits is None:
        w(f'BRAM tile {name} missing bits: {list(bits.keys())}')
        continue
    _, _, ccol, _ = far_parts(bram_bits['baseaddr'])
    _, _, rcol, _ = far_parts(clk_bits['baseaddr'])
    for s in t.get('sites', {}):
        m = re.fullmatch(r'RAMB18_X(\d+)Y(\d+)', s)
        if not m:
            continue
        x, y = int(m.group(1)), int(m.group(2))
        row = y // 24
        bram_derived.setdefault((row, x), set()).add((ccol, rcol, bram_bits['frames'], clk_bits['frames']))
bad = {k: v for k, v in bram_derived.items() if len({(c, r) for c, r, _, _ in v}) != 1}
check('each (row, bram_x) has exactly one (content_col, reg_col)',
      not bad, f'conflicts: {len(bad)}')
badf = {k: v for k, v in bram_derived.items() if {f for _, _, f, _ in v} != {256} or {f for _, _, _, f in v} != {6}}
check('every BRAM tile carries BLOCK_RAM frames=256 and CLB_IO_CLK frames=6',
      not badf, f'non-standard: {len(badf)}')
for r in ['0', '1', '2']:
    ri = int(r)
    bf_c = {int(k): v for k, v in rows[r]['bram_content_colMajors'].items()}
    bf_p = {int(k): v for k, v in rows[r]['bram_content_parity_colMajors'].items()}
    bf_r = {int(k): v for k, v in rows[r]['bram_reg_colMajors'].items()}
    tg_map = {}
    for (row, x), s in bram_derived.items():
        if row == ri:
            (c, rr, _, _) = next(iter(s))
            tg_map[x] = (c, rr)
    check(f'row {r} bram_content_colMajors == tilegrid',
          all(tg_map.get(x, (None, None))[0] == c for x, c in bf_c.items()) and set(tg_map) == set(bf_c),
          f"bf={bf_c} tg={ {x: v[0] for x, v in sorted(tg_map.items())} }")
    check(f'row {r} bram_content_parity_colMajors == content table',
          bf_p == bf_c, f'parity={bf_p} content={bf_c}')
    check(f'row {r} bram_reg_colMajors == tilegrid',
          all(tg_map.get(x, (None, None))[1] == c for x, c in bf_r.items()) and set(tg_map) == set(bf_r),
          f"bf={bf_r} tg={ {x: v[1] for x, v in sorted(tg_map.items())} }")

# ---- 6. DSP columns: what do BF values correspond to? ----
# DSP tiles have no bits. List BF dsp_colMajors per row and the
# neighboring tiles (by grid_x) of each DSP tile.
for r in ['0', '1', '2']:
    bf_d = {int(k): v for k, v in rows[r]['dsp_colMajors'].items()}
    w(f"row {r} dsp_colMajors (bf): {bf_d}")
    w(f"row {r} dsp y range: min={rows[r]['min_dsp_y_ofst']} max={rows[r]['max_dsp_y_ofst']}")
# DSP tile grid positions per row
dsp_grid = collections.defaultdict(list)
for name, t in tg.items():
    if t.get('type') == 'DSP':
        for s in t.get('sites', {}):
            m = re.fullmatch(r'DSP48E2_X(\d+)Y(\d+)', s)
            if m:
                dsp_grid[int(m.group(2)) // 24].append((int(m.group(1)), t['grid_x'], t['grid_y']))
for r in sorted(dsp_grid):
    xs = sorted({x for x, _, _ in dsp_grid[r]})
    gxs = sorted({gx for _, gx, _ in dsp_grid[r]})
    w(f'DSP row {r}: logical X values {xs} grid_x values {gxs} ntiles={len(dsp_grid[r])}')
# neighbor columns of DSP tiles: tiles at grid_x +-1, same grid_y
by_grid = {}
for name, t in tg.items():
    if 'grid_x' in t and 'grid_y' in t:
        by_grid[(t['grid_x'], t['grid_y'])] = (name, t)
neigh_cols = collections.Counter()
for name, t in tg.items():
    if t.get('type') != 'DSP':
        continue
    gx, gy = t['grid_x'], t['grid_y']
    for dx in (-1, 1):
        nb = by_grid.get((gx + dx, gy))
        if nb is None:
            continue
        nname, nt = nb
        bits = nt.get('bits', {}).get('CLB_IO_CLK')
        if bits is not None:
            _, _, col, _ = far_parts(bits['baseaddr'])
            neigh_cols[(nt.get('type'), col)] += 1
w(f'DSP neighbor CLB_IO_CLK columns (tile type, col) -> count: {dict(neigh_cols)}')

# ---- 7. columns in part.json with no owning tile in tilegrid ----
owned = collections.defaultdict(set)  # row -> set of cols with a tile baseaddr
for name, t in tg.items():
    bits = t.get('bits', {}).get('CLB_IO_CLK')
    if bits is None:
        continue
    _, r, col, _ = far_parts(bits['baseaddr'])
    owned[r].add(col)
for r in ['0', '1', '2']:
    ri = int(r)
    pj_cols = set(range(len(pj['rows'][r]['configuration_buses']['CLB_IO_CLK']['configuration_columns'])))
    missing = sorted(pj_cols - owned[ri])
    extra = sorted(owned[ri] - pj_cols)
    w(f'row {r}: part.json cols={len(pj_cols)} tilegrid-owned cols={len(owned[ri])} '
      f'missing(no tile)={missing} extra(tile but not in part.json)={extra}')
    if missing:
        buses = pj['rows'][r]['configuration_buses']['CLB_IO_CLK']['configuration_columns']
        w(f'  missing cols frame_counts: {[(c, buses[str(c)]["frame_count"]) for c in missing]}')
    check(f'row {r} every part.json std column owns a tile', not missing,
          f'missing cols: {missing}')
# distinct baseaddr count
n_distinct = sum(len(v) for v in owned.values())
w(f'distinct tilegrid CLB_IO_CLK baseaddrs total: {n_distinct} (300 expected per H1)')

# ---- 8. tileType_siteType_pairs vs tilegrid ----
bf_pairs = {tuple(p) for p in bf['tileType_siteType_pairs']}
tg_pairs = set()
for name, t in tg.items():
    ty = t.get('type')
    for s, st in t.get('sites', {}).items():
        tg_pairs.add((ty, st))
# base types only: strip _RBRK/_TERM_B/_TERM_T suffixes from tile types
def base_ty(ty):
    for suf in ('_RBRK', '_TERM_B', '_TERM_T'):
        if ty.endswith(suf):
            return ty[:-len(suf)]
    return ty
tg_pairs_base = {(base_ty(ty), st) for ty, st in tg_pairs}
w(f'BF pairs: {len(bf_pairs)}, tilegrid pairs (raw): {len(tg_pairs)}, (base types): {len(tg_pairs_base)}')
w(f'BF pairs not in tilegrid(base): {sorted(bf_pairs - tg_pairs_base)}')
w(f'tilegrid(base) pairs not in BF: {sorted(tg_pairs_base - bf_pairs)}')
check('BF tileType_siteType_pairs == tilegrid pairs (base types)',
      bf_pairs == tg_pairs_base,
      f'bf-only={sorted(bf_pairs - tg_pairs_base)} tg-only={sorted(tg_pairs_base - bf_pairs)}')

# ---- 9. frame totals ----
total = 0
for r in ['0', '1', '2']:
    total += sum(rows[r]['num_minors_per_std_colMajor']) + sum(rows[r]['num_minors_per_bram_content_colMajor'])
w(f'BF file total frames: {total}')
w(f'part.json total frames: {sum(sum(c["frame_count"] for c in pj["rows"][r]["configuration_buses"][b]["configuration_columns"].values()) for r in ["0","1","2"] for b in ["CLB_IO_CLK","BLOCK_RAM"])}')
check('BF total frames == part.json total frames', total == 14952, f'total={total}')

npass = sum(1 for _, ok in results if ok)
w('')
w(f'SUMMARY: {npass}/{len(results)} checks passed')
open(OUT, 'w').write('\n'.join(lines) + '\n')
print(f'{npass}/{len(results)} checks passed; report at {OUT}')
for name, ok in results:
    if not ok:
        print(f'  FAIL: {name}')

# ---- 10. INT columns vs tilegrid ground truth (corrected pairing) ----
# The provisional rule (bitgen_device.rs int_col_major) keys CLB columns
# by SLICE_X: for INT tile INT_X<x>Y<y> it looks up the BF file's CLB
# column x and takes col - 1 (CLEL_R) or col + 1 (other types). That
# keying is WRONG on xczu3eg: tile X is not SLICE_X. Tile names share
# the X number only within a tile type's own numbering: CLEL_L_X5Y0
# holds SLICE_X7Y0, CLEM_R_X26Y0 holds SLICE_X39Y0, and two CLE tiles
# can share one (X, Y) pair (CLEL_R_X1Y0 at grid 166 holds SLICE_X2Y0
# while CLEM_X1Y0 at grid 164 holds SLICE_X1Y0). The rule therefore
# matches only INT_X0 and INT_X1 (360 of 5,940 INT tiles) and mispredicts
# the rest (e.g. INT_X2Y0: actual col 7, rule predicts 4).
#
# Correct pairing (BRIEFS/tilegrid-research.md Q5.2): an INT tile serves
# the CLE tile with the SAME tile X (INT_X<n> next to CLE*_X<n>), and
# the type-dependent offset is read off THAT tile's baseaddr col:
# CLEL_R -> INT col = CLE col - 1; CLEL_L/CLEM/CLEM_R -> CLE col + 1.
# The two CLE neighbours of one INT tile (left and right) give the SAME
# INT column, so the pairing is unambiguous: INT col = left CLE col + 1
# = right CLE col - 1. This check verifies that for every plain INT
# tile, and builds the (row, INT tile X) -> column table the emitter
# uses instead of the provisional rule.
def _far(h):
    v = int(h, 16)
    return ((v >> 24) & 7, (v >> 18) & 0x3f, (v >> 8) & 0x3ff, v & 0xff)
# CLE tiles keyed by (tile X, Y): (baseaddr col, type). A (X, Y) pair
# can hold two CLE tiles (left and right neighbours of one INT tile).
cle_by_xy = collections.defaultdict(list)
for _name, _t in tg.items():
    _ty = _t.get('type')
    if _ty not in ('CLEL_L', 'CLEL_R', 'CLEM', 'CLEM_R'):
        continue
    _bits = _t.get('bits', {}).get('CLB_IO_CLK')
    if _bits is None:
        continue
    m = re.fullmatch(r'(CLEL_L|CLEL_R|CLEM|CLEM_R)_X(\d+)Y(\d+)', _name)
    if not m:
        continue
    _blk, _r, _col, _minor = _far(_bits['baseaddr'])
    cle_by_xy[(int(m.group(2)), int(m.group(3)))].append((_col, _ty))
int_tiles = 0
int_ok = 0
int_bad = []
int_nobits_types = collections.Counter()
int_table = {}  # (row, int_x) -> col
int_table_conflicts = 0
for _name, _t in tg.items():
    _ty = _t.get('type')
    if not (_ty == 'INT' or _ty.startswith('INT_')):
        continue
    _bits = _t.get('bits', {}).get('CLB_IO_CLK')
    if _bits is None:
        int_nobits_types[_ty] += 1
        continue
    m = re.fullmatch(r'INT_X(\d+)Y(\d+)', _name)
    if not m:
        continue
    _x, _y = int(m.group(1)), int(m.group(2))
    _row = _y // 60
    _blk, _r, _col, _minor = _far(_bits['baseaddr'])
    int_tiles += 1
    _rowtab = rows[str(_row)]
    _neigh = cle_by_xy.get((_x, _y), [])
    _preds = set()
    for _ccol, _cty in _neigh:
        _preds.add(_ccol - 1 if _cty == 'CLEL_R' else _ccol + 1)
    _minors = _rowtab['num_minors_per_std_colMajor']
    _ok = (len(_preds) >= 1 and _preds == {_col}
           and _r == _row and _minor == 0
           and _col < len(_minors) and _minors[_col] >= 76
           and _bits['frames'] == 76)
    if _ok:
        int_ok += 1
    else:
        int_bad.append((_name, f'preds={sorted(_preds)} actual={_col} row={_r}/{_row} minor={_minor} frames={_bits["frames"]} neigh={_neigh}'))
    _key = (_row, _x)
    if _key in int_table and int_table[_key] != _col:
        int_table_conflicts += 1
    int_table[_key] = _col
lines.append(f'[{{"PASS" if int_ok == int_tiles else "FAIL"}}] INT same-tile-X rule matches tilegrid on every INT tile with bits :: {int_ok}/{int_tiles} ok, bad={len(int_bad)} e.g. {int_bad[:3]}')
lines.append(f'INT tile types with no bits entry (cannot check rule): {dict(int_nobits_types)}')
lines.append(f'INT column table: {len(int_table)} (row, int_x) entries, conflicts={int_table_conflicts}, int_x range {min(x for _, x in int_table)}..{max(x for _, x in int_table)}')
results.append(('INT same-tile-X rule vs tilegrid', int_ok == int_tiles and int_table_conflicts == 0))
# Write the sidecar table (only when asked, and only if the check passed).
if WRITE_SIDECAR:
    assert int_ok == int_tiles and int_table_conflicts == 0, 'refusing to write INT table: tilegrid check failed'
    _cols = {}
    for (_row, _x), _col in sorted(int_table.items()):
        _cols.setdefault(str(_row), {})[str(_x)] = _col
    _doc = {
        'part': 'xczu3eg-sbva484-1-e',
        'source': 'prjuray-db zynqusp/xczu3eg-sbva484-1-e/tilegrid.json (CC0): INT tile CLB_IO_CLK baseaddr per (FAR row, INT tile X)',
        'license': 'CC0-1.0 (prjuray-db); derived table, no AMD content',
        'generated_by': 'crates/helion-ultrascale/tools/verify_xczu3eg.py --write-int-columns (section 10)',
        'columns': _cols,
    }
    open(SIDECAR, 'w').write(json.dumps(_doc, indent=2, sort_keys=True) + '\n')
    lines.append(f'INT column table written to {SIDECAR} ({len(int_table)} entries)')
# DSP check: BF dsp_colMajors == neighbor CLEM/CLEM_R col - 1 per DSP column
dsp_ok = True
dsp_detail = []
for _r in ['0', '1', '2']:
    _bf_d = {int(k): v for k, v in rows[_r]['dsp_colMajors'].items()}
    # neighbor CLEM/R cols from the earlier aggregate: find per DSP logical x
    # the CLEM/R tile immediately right (grid_x+1) of each DSP tile in this row
    _by_grid = {}
    for _n, _t in tg.items():
        if 'grid_x' in _t and 'grid_y' in _t:
            _by_grid[(_t['grid_x'], _t['grid_y'])] = _t
    _pred = {}
    for _n, _t in tg.items():
        if _t.get('type') != 'DSP':
            continue
        for _s in _t.get('sites', {}):
            _mm = re.fullmatch(r'DSP48E2_X(\d+)Y(\d+)', _s)
            if not _mm or int(_mm.group(2)) // 24 != int(_r):
                continue
            _gx, _gy = _t['grid_x'], _t['grid_y']
            for _dx in (1, -1):
                _nb = _by_grid.get((_gx + _dx, _gy))
                if _nb is None:
                    continue
                _bb = _nb.get('bits', {}).get('CLB_IO_CLK')
                if _bb is None:
                    continue
                _b2, _r2, _c2, _m2 = _far(_bb['baseaddr'])
                if _nb.get('type') in ('CLEM', 'CLEM_R'):
                    _pred[int(_mm.group(1))] = _c2 - 1
    _match = _pred == _bf_d
    dsp_ok = dsp_ok and _match
    dsp_detail.append(f"row {_r}: pred={dict(sorted(_pred.items()))} bf={dict(sorted(_bf_d.items()))} match={_match}")
for _d in dsp_detail:
    lines.append(_d)
lines.append(f'[{{"PASS" if dsp_ok else "FAIL"}}] dsp_colMajors == neighbor CLEM/CLEM_R col - 1 :: {dsp_detail}')
results.append(('DSP colMajors vs tilegrid', dsp_ok))
# ---- 11. INT_INTF / HDIO columns vs tilegrid ground truth ----
# Same pattern as section 10, but the key adds the tile type: an
# INT_INTF or HDIO tile name is <TYPE>_X<x>Y<y> with y absolute
# (60 per clock-region row), and the column is the tile's own
# CLB_IO_CLK baseaddr column. Only four types carry tilegrid windows
# (INT_INTF_LEFT_TERM_PSS, INT_INTF_R_PCIE4, HDIO_BOT_RIGHT,
# HDIO_TOP_RIGHT); the other INT_INTF types record no baseaddr, so
# they are listed as uncovered and excluded, never inferred. Every
# windowed tile must agree on one column per (type, row, X), its FAR
# row must equal Y // 60, and its base minor must be 0 (the table
# stores the column; the emitter builds base FAR (0, row, col, 0)).
INTF_TYPES = ('INT_INTF_LEFT_TERM_PSS', 'INT_INTF_R_PCIE4', 'HDIO_BOT_RIGHT', 'HDIO_TOP_RIGHT')
_intf_table = {}  # (tile_type, row, x) -> col
_intf_conflicts = 0
_intf_tiles = 0
_intf_bad = []
_intf_nobits = collections.Counter()
for _name, _t in tg.items():
    _ty = _t.get('type')
    if _ty not in INTF_TYPES and not (_ty or '').startswith('INT_INTF'):
        continue
    if _ty not in INTF_TYPES:
        # An INT_INTF type with no windows: uncovered by design.
        _intf_nobits[_ty] += 1
        continue
    _bits = _t.get('bits', {}).get('CLB_IO_CLK')
    if _bits is None:
        _intf_nobits[_ty] += 1
        continue
    m = re.fullmatch(r'(.+)_X(\d+)Y(\d+)', _name)
    if not m:
        _intf_bad.append((_name, 'unparsable tile name'))
        continue
    _x, _y = int(m.group(2)), int(m.group(3))
    _row = _y // 60
    _blk, _r, _col, _minor = _far(_bits['baseaddr'])
    _intf_tiles += 1
    if not (_blk == 0 and _r == _row and _minor == 0):
        _intf_bad.append((_name, f'row={_r}/{_row} block={_blk} minor={_minor}'))
    _key = (_ty, _row, _x)
    if _key in _intf_table and _intf_table[_key] != _col:
        _intf_conflicts += 1
    _intf_table[_key] = _col
_intf_ok = (len(_intf_bad) == 0 and _intf_conflicts == 0 and _intf_tiles > 0)
lines.append(f"[{'PASS' if _intf_ok else 'FAIL'}] INTF/HDIO same-(type,row,X) rule matches tilegrid on every windowed tile :: {_intf_tiles} tiles, {len(_intf_table)} entries, conflicts={_intf_conflicts}, bad={_intf_bad[:3]}")
lines.append(f'INTF/HDIO tiles with no bits entry (uncovered, fail loud in the emitter): {dict(_intf_nobits)}')
results.append(('INTF/HDIO column table vs tilegrid', _intf_ok))
if WRITE_INTF_SIDECAR:
    assert _intf_ok, 'refusing to write INTF table: tilegrid check failed'
    _cols = {}
    for (_ty, _row, _x), _col in sorted(_intf_table.items()):
        _cols.setdefault(_ty, {}).setdefault(str(_row), {})[str(_x)] = _col
    _doc = {
        'part': 'xczu3eg-sbva484-1-e',
        'source': 'prjuray-db zynqusp/xczu3eg-sbva484-1-e/tilegrid.json (CC0): INT_INTF/HDIO tile CLB_IO_CLK baseaddr per (tile type, FAR row, tile X)',
        'license': 'CC0-1.0 (prjuray-db); derived table, no AMD content',
        'generated_by': 'crates/helion-ultrascale/tools/verify_xczu3eg.py --write-intf-columns (section 11)',
        'columns': _cols,
    }
    open(SIDECAR_INTF, 'w').write(json.dumps(_doc, indent=2, sort_keys=True) + '\n')
    lines.append(f'INTF/HDIO column table written to {SIDECAR_INTF} ({len(_intf_table)} entries)')
elif os.path.exists(SIDECAR_INTF):
    _on_disk = json.load(open(SIDECAR_INTF))
    _disk_cols = {}
    for _ty, _rows in _on_disk.get('columns', {}).items():
        for _row, _xs in _rows.items():
            for _x, _col in _xs.items():
                _disk_cols[(_ty, int(_row), int(_x))] = _col
    _match = (_disk_cols == _intf_table and _on_disk.get('part') == 'xczu3eg-sbva484-1-e')
    lines.append(f"[{'PASS' if _match else 'FAIL'}] vendored INTF/HDIO table equals the tilegrid derivation :: disk={len(_disk_cols)} derived={len(_intf_table)}")
    results.append(('vendored INTF/HDIO table == tilegrid', _match))
_npass = sum(1 for _, _ok in results if _ok)
lines.append('')
lines.append(f'SUMMARY (with INT/DSP): {_npass}/{len(results)} checks passed')
open(OUT, 'w').write('\n'.join(lines) + '\n')
print(f'{_npass}/{len(results)} checks passed (with INT/DSP); report at {OUT}')
for _name, _ok in results:
    if not _ok:
        print(f'  FAIL: {_name}')
