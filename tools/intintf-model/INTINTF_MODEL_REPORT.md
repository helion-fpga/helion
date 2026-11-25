# INT_INTF tile model lane report

Lane branch: fm/ultra-bitgen-intintf-model, code tip c8099849 (impl commit
on n66fix base 4adaeb35). No merges into fm/ultra-bitstream,
fm/zedboard-merge, or master. No critic dispatched by this lane; Critic
Session 51 is the main agent's to dispatch.

## 1. Outcome

The INT_INTF tile model gap from US-01 is closed. DSP and BRAM pins now
edge into the routing graph through INT_INTF tiles: all 850 counted pins
(472 DSP, 131 RAMB181, 81 RAMBFIFO36, 166 RAMBFIFO18) are edged, wire
verified against tileconn.json re-parsed from disk with 0 contradictions.
Real routes are proven in tests: LUT to DSP input, LUT to BRAM input, and
DSP output to slice FF, each through exactly one stitch and at most one
INT_INTF PIP.

What this lane does NOT deliver: no DSP/BRAM design flows end to end yet.
Three independent, evidenced blockers remain (section 9): no DSP/BRAM
placement, no DSP config-bit encoding and no INT_INTF L/R segbits DBs, and
xczu3eg STA GAP-F5. Nothing was invented to work around them; the lane
delivers the graph, the site-indexed registration, the stitch model, the
translator support, and the wire verification only.

## 2. The fix (6 files, +1735/-213)

- `crates/helion-ultrascale/src/wire_db.rs` (+437/-87): `IntTile` gains a
  `tile_type` field (`new()` defaults to `INT`; new `new_typed`). New
  `TileTypeTable` per tile type; `from_json_typed` builds a graph from one
  tile JSON per type (node ids contiguous per tile, `tile_of` a binary
  search over per-tile bases). `from_json` keeps INT-only behavior by
  delegating with a single INT table. Continuation lookup is scoped per
  tile type, so an INT tile and an INT_INTF tile can share name
  coordinates. New `add_stitch(a, b)` (bidirectional same-node edge
  between two tiles, canonical pair recorded, both fanout lists re-sorted
  for N48 determinism), `is_stitch`, `stitch_count`. New error variants
  `UnknownTileType`, `BadStitch`; `WrongTileType` now carries found plus
  expected. Two new unit tests (mixed tables, stitch rules).
- `crates/helion-ultrascale/src/tileconn.rs` (+385/-117): `PinWire`
  gains `partner_type`. `TileconnDb` gains `intf_edges` (pins whose
  unique partner is an INT_INTF wire) and `intf_stitches` (per INT_INTF
  type: intf wire, INT wire, grid delta). The load records an INT_INTF
  edge when a pin has exactly one INT_INTF partner and no plain-INT
  partner; plain-INT precedence is unchanged. `resolve_int_tile` became
  `resolve_partner_tile`, returning tile, wire, and tile type, failing
  loud only for a partner that is neither INT nor INT_INTF*. New
  `parse_named_tile(name, type)` replaces `parse_int_tile_name`
  (INT_X13Y0 and INT_INTF_R_X13Y0 are both (13, 0) in name
  coordinates). New `TileconnDb::intf_tile_json(type)` loads
  `tile_type_INT_INTF_*.json` at runtime from the DB root, never
  vendored. `from_db_and_sites` instantiates partner tiles with their
  types, adds each INT_INTF tile's stitch-partner INT tile, expands
  around INT tiles only (INT_INTF tiles have no span wires), builds with
  `from_json_typed`, and adds one stitch edge per pair. Stitch deltas
  are applied to tilegrid GRID coordinates (via an inverted
  `tile_by_grid` map), not name coordinates: name (13, 0) is grid
  (217, 185) for INT_X13Y0.
- `crates/helion-ultrascale/tests/tileconn_route.rs` (+556/-0): 5 new
  tests on the real DB (skip without `HELION_TILECONN_DB_ROOT`).
- `crates/helion-ultrascale/tests/intintf_model.rs` (new, 334 lines):
  3 fixture tests, no DB root needed. Synthetic tileconn, tilegrid, and
  tile-type files mirror real xczu3eg geometry at small scale (DSP-like
  tile with two sites sharing pin names, INT_INTF_R-like tile with
  LOGIC_OUTS pass-through PIPs, stitch pairs into the INT tile).
- `crates/helion-ultrascale/src/route_to_pip.rs` (+14/-9): a cross-tile
  pair is accepted when it is a span continuation OR a stitch. Stitches
  count under `continuations` (both program no PIP). `TranslatedRoute`
  is unchanged.
- `crates/helion-ultrascale/src/fabric.rs` (+9/-0): `WireDb` gains a
  default `is_stitch` returning false, so placeholder and mock graphs
  keep compiling; `TileconnWireDb` delegates it.

## 3. Edged-pin counts (all 850 counted pins now edged)

| Site type | Pins in template | Edged | Spot checks (verbatim from tileconn) |
| --- | --- | --- | --- |
| DSP48E2 (2 sites) | 864 | 472 | X1Y0 A0 to INT_INTF_R_X13Y0 IMUX_FT0_1; P0 to LOGIC_OUTS_R2; X1Y1 A0 to INT_INTF_R_X13Y1 IMUX_FT0_24 |
| RAMB181 | 203 | 131 | ADDRAU1 to INT_INTF_L_X12Y0 IMUX_FT0_13; DOAU0 to INT_INTF_L_X12Y2 LOGIC_OUTS_L27 |
| RAMBFIFO36 | 533 | 81 | ADDRAL0 to INT_INTF_L_X12Y0 IMUX_FT0_8 |
| RAMBFIFO18 | 242 | 166 | DOAL0 to INT_INTF_L_X12Y0 LOGIC_OUTS_L0 |
| Total | 1842 | 850 | |

Pin direction split of the 850: 586 input pins whose INT_INTF wire
stitches straight to an INT wire (no PIP needed: DSP 342, RAMB181 95,
RAMBFIFO36 53, RAMBFIFO18 96); 264 output pins whose partner wire is a
LOGIC_OUTS PIP source (DSP 130, RAMB181 36, RAMBFIFO36 28, RAMBFIFO18
70); 0 orphans. Real routes proven in tests: LUT O6 to DSP A0
(INT_X13Y0 LOGIC_OUTS_W0 to INT_INTF_R_X13Y0 IMUX_FT0_1, 1 stitch,
0 INT_INTF PIPs, every INT PIP in `segbits_int.db`); LUT O6 to BRAM
ADDRAU1 (INT_X12Y0 LOGIC_OUTS_E0 to INT_INTF_L_X12Y0 IMUX_FT0_13,
1 stitch, 0 INT_INTF PIPs); DSP P0 to slice FF D (INT_INTF_R_X13Y0
LOGIC_OUTS_R2 to LOGIC_OUTS_L2, exactly 1 INT_INTF PIP, then 1 stitch
into INT_X13Y0).

## 4. Wire verification (US-01 standard, re-parsed from disk)

Re-derived by `tileconn_intintf_wire_verification` against
tileconn.json re-parsed from disk (not the loader's own index):

- 850 pin edges checked, 0 contradictions.
- 1,878 distinct INT_INTF to INT stitch pairs checked (intf wire in
  its tile JSON, INT wire in the INT tile JSON), 0 contradictions.
- 864 INT_INTF PIPs across all `tile_type_INT_INTF_*.json` files
  (src/dst in wire list, 0 pseudo), 0 contradictions.
- Coverage counting is unchanged (the existing
  `tileconn_real_counts_and_coverage` test still passes with its
  original numbers).

## 5. Baseline (W-BASELINE)

Base tip 4adaeb35, worktree clean, nothing committed. Release build exit
0 in 54m 16s cold (helion 13,047,648 bytes md5
b377790b9e3fa7c040e0e07940bc4881; helion-bitgen 5,740,048 bytes md5
b004155e3acb385f15e8815b9464cac6). Suites 465/0/1, 92/0/1, checkers 100
OK in 108.605s (4th attempt; see section 10). 12 flows exit 0 with
numbers identical to the n66fix lane; 12 verify-bits PASS.

Gap numbers re-derived two ways, agreeing value-for-value with US-01:
an independent Python probe over the raw DB JSON (tileconn 81,640 entries
/ 658,732 pairs) and the existing `tileconn_real_counts_and_coverage`
Rust test. DSP48E2: 864 pins, 668 any-partner, 0 plain-INT, 472
INT_INTF, 0 ambiguous. RAMB181: 203/203/0/131. RAMBFIFO36:
533/88/0/81. RAMBFIFO18: 242/242/0/166. CLE: 106/106/104 and
108/108/106, 0 INT_INTF partners. No-edge confirmation: edges inserted
= 0 for all four DSP/BRAM site types on the base (the loader only edges
a pin with exactly one plain-INT partner; these have none). INT_INTF
tile type files: 24 in `~/workspace/prjuray-db-ref/zynqusp/tile_types/`
(of 158 total): 6 substantive (INT_INTF_L, INT_INTF_L_PCIE4,
INT_INTF_R, INT_INTF_R_PCIE4, INT_INTF_LEFT_TERM_PSS,
INT_INTF_RIGHT_TERM_IO) plus 18 small RBRK/TERM_B/TERM_T stubs.

## 6. Impl acceptance (W-IMPL, branch tip c8099849)

Release build exit 0. Suites helion-ultrascale 475/0/1 (465 base plus 10
new: 2 wire_db unit, 3 fixture, 5 real-DB), helion-cli release 92/0/1,
US-09 checkers 100 OK in 114.168s. 12 flows exit 0, numbers identical
to n66fix; 12 verify-bits PASS (800/0 each, counter16 80000/0),
decoder built from the impl branch. Fail-before proof: all 8 new
integration tests failed on base for the right reasons (NoPath,
EmptyTiles, 0-of-850); all pass with the fix.

## 7. Coordinator acceptance (merged branch e603d489, own runs)

Release build exit 0 in 50m 33s (helion 13,095,296 bytes md5
de5a6c2cb6c519db1878f908314d3fe4; helion-bitgen 5,738,240 bytes md5
a8db26fd44c463d3d470f077b4a55d83). Suites helion-ultrascale 475/0/1,
helion-cli release 92/0/1, US-09 checkers 100 OK in 126.097s. 12 flows
exit 0 with numbers identical to baseline, impl, and the n66fix lane
(chain16 8570/362/4cf99124 twice, shift32 8690/289/a7a070dd twice,
counter12 7370/752/b18112d3, counter8 s0 8570/363/2902d6f0, s1
8570/342/01cb7f90, counters2x4 8450/307/730fcc93, counter5 9650/82/
6fe2c80a, counter 9650/58/4c99a51f, blinky 9650/4/d82b70c8 unchanged,
counter16 5750/1278/a45c6efd). 12 verify-bits PASS (800/0 each,
counter16 80000/0), decoder built from the merged branch, absolute
workdirs, `--top` passed. No flow number moved.

## 8. Stash proof both sides (coordinator, baseline worktree)

- fix.patch (branch diff 4adaeb35..e603d489) sha256
  d0d9d488ef6a664e943338564d60215acf13b28c9aef9038ac83432749ea0c60,
  byte-identical to the regenerated branch diff. Full patch applied on
  base: intintf_model 3/3 pass, tileconn_route (intintf filter) 5/5
  pass, wire_db (mixed filter) 3/3 pass (2 new plus 1 pre-existing).
- testonly.patch (test files only) sha256
  f5b54f94b1146c952b2bf9d24fe27042f3ecaca603ebf2b70500ff03cdf92c7e.
  Test-only on base: exit 101, intintf_model 0 passed 3 failed,
  tileconn_route 0 passed 5 failed (wire_db filter passes its 1
  pre-existing test; the 2 new wire_db unit tests live in wire_db.rs
  itself, so the test-only patch contains no lib change to fail).
  Baseline worktree restored clean at 4adaeb35 afterward.

## 9. Honest limits: no DSP/BRAM design flows end to end yet

The routing graph model is done and proven, but a DSP/BRAM design still
cannot flow end to end, for three independent, already-evidenced
reasons:

1. No DSP/BRAM placement. `pack.rs` / `place.rs` contain zero mentions
   of DSP48E2 or RAMB; `UltraPlaced` carries only slices. The CLI flow
   therefore never places a DSP/BRAM site, so `load_route_db` stays
   CLE-only and no DSP/BRAM net ever reaches this graph. Placement is
   follow-up work.
2. No DSP config-bit encoding and no INT_INTF L/R segbits. `dsp.rs`
   documents the search: no public source encodes DSP48E2 config bits,
   prjuray-db ships no `segbits_dsp*.db`, and the emitter refuses DSP
   features (`Dsp48e2Blocked`). Separately, no `segbits_int_intf_l.db`
   or `segbits_int_intf_r.db` exists, so an output-pin route (264 of
   850 pins) translates to an INT_INTF LOGIC_OUTS PIP FASM line that
   bitgen would refuse. Input-pin routes (586 of 850) need no INT_INTF
   PIP and are emittable in principle. Both are data gaps (B1 class),
   not model gaps.
3. xczu3eg STA GAP-F5 (unsupported part) still stands for timing.

Proven on the measured design set only. The wire verification covers
the xczu3eg tileconn DB as shipped; no Vivado check.

## 10. Incidents

- Runtime restart killed the coordinator suites script after the
  ultrascale suite (475/0/1 stood) and partway through the CLI suite
  (50 passed, no summary). The CLI suite was re-run from scratch:
  92/0/1. The partial log was overwritten by the re-run; the restart
  and the re-run are recorded here, not hidden.
- The coordinator verify script copied stale port-map paths from the
  n66 lane script for counter8 and counter5
  (`verify-counter8/counter8-port-map-AQ.json`,
  `verify-counter5/counter5-port-map-AQ.json`); those files do not
  exist. The three runs failed loud FileNotFoundError before any
  verification ran. The correct paths are
  `routepack-logs/impl/counter8-port-map-AQ.json` and
  `routepack-logs/impl/counter5-port-map-AQ.json`, the ones the impl
  worker used. The script was fixed and the three verify-bits re-run:
  all PASS 800/0. This is N65 recurring (coord scripts copying fixture
  paths instead of sharing one list), not a lane defect and not a new
  nit.
- Baseline checkers were SIGKILLed 4 times at 87-97 dots (exit 137)
  while the sibling impl worker ran heavy concurrent jobs; the 4th
  attempt in an idle window passed 100 OK. A first suites-driver
  invocation was SIGTERMed before any suite started; re-run detached.
- Impl incidents (from the impl report): fixture tile type not starting
  with `INT_INTF` (renamed to the real `INT_INTF_R`); fixture grid
  collision causing nondeterministic failure (DSP column moved east);
  stitch deltas applied to name coordinates instead of tilegrid grid
  coordinates, a real implementation bug fixed in the lane (name (13, 0)
  is grid (217, 185) for INT_X13Y0); first combined suites run
  OOM-killed the checkers at 77/100, solo re-run 100 OK; `cargo fmt
  --check` disagrees with repo style crate-wide, so no blanket fmt was
  run (see section 11).

## 11. Formatting

rustfmt --check (edition 2024, the workspace edition) on the changed
files, lane vs base diff-hunk counts: fabric.rs 0/0, route_to_pip.rs
2/2, tileconn.rs 14/17, wire_db.rs 0/0, tileconn_route.rs 8/8,
intintf_model.rs 0 (new file). Zero new deviations: every lane count
is equal to or lower than the base count (tileconn.rs drops 17 to 14;
the lane's edits rewrote pre-existing deviating regions).

## 12. New nits

None. The two bugs found during the work (fixture grid collision;
name-vs-grid stitch deltas) were fixed in the lane, not filed as nits.
N65 recurred in the coordinator scripts (section 10) and stays open.

## 13. Carried

N65, N51, N53, N52, N54, N55, N56, N59, N60, N61, N62, N63, IOB
blocked on B1, known-INIT datum, documented xczu3eg timing except the
single BRAM field, 54,907 non-band bits, N16, N9, N34, N37, N38,
A-CTRL, A-F7F8. Fabric delays still blocked on Vivado report_timing
plus write_sdf; Tier 2 waits for Vivado. No critic dispatched by this
lane; main agent dispatches Critic Session 51.
