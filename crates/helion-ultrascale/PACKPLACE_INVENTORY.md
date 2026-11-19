# Pack/place promotion inventory (GAP-F2/F3, session 18)

Branch: `fm/ultra-bitgen-packplace`. Base: `fm/ultra-bitstream` 07d0d7d.
Scope: promote the ported staging pack/place code in
`crates/helion-ultrascale` to production on the real xcku3p device file.
No merges. ZedBoard out of scope.

## 1. Port status (verified by diff, not assumed)

The ported files are byte-identical to staging
(`fm/ultra-packplace:staging/ultrascale-packplace/src/`) except for the
integration worker's documented corrections:

- `pack.rs`: header corrected from "Two slices per CLB" (7-series) to
  "One slice per CLB (UG574)". Clippy-driven iterator cleanup in
  `can_place_ff` (`values().flatten()`). One empty-doc-comment fix.
- `place.rs`: one rustfmt reflow only.
- `legality.rs`: rustfmt reflows only.

No semantic drift from staging. The integration worker's port is the
starting point; the defects below are staging defects carried over.

## 2. What is production-ready as-is

- `pack_ultra` (pack.rs): greedy LUT/FF/CARRY8 grouping, 8 LUTs / 16 FFs /
  1 CARRY8 per slice, SLICEM-only memory LUTs rejected on SLICEL,
  control-set limits enforced at pack time, illegal FF pack fails loud
  with the cell name. Slice-kind counts count slices, not LUTs.
- `place_ultra` (place.rs): CARRY8 chains placed first in vertically
  contiguous slices (constant X, Y ascending), SLICEM-required slices
  only on Memory sites, first-fit in grid order, no free site fails
  loud. `seeded_order` (SplitMix64 + Fisher-Yates) is a pure function:
  same seed gives same order, seed 0 is identity.
- `check_legality` (legality.rs): capacity, SLICEM, control-set,
  duplicate-site, and CARRY8-chain contiguity checks, all returning
  structured `LegalityViolation` values.
- `grid.rs`: `UltraGrid` for `UltraScaleDevice`, with a deterministic
  sort of the HashMap tile iteration. Site names follow Vivado shape
  (`SLICE_X{x}Y{y}`).
- Tests on the port: unit tests in pack/place/legality modules plus
  `tests/corpus_legality.rs`, which synthesizes blinky and counter with
  helion-sv, packs, places on a fake 100x100 grid, and asserts zero
  legality violations. (The staging suite also covered serv_alu; its RTL
  is absent from this worktree, so that case was dropped, not skipped
  silently.)

## 3. Defects found by reading (each gets a fix + regression test)

Status after session 18 pack/place work (branch fm/ultra-bitgen-packplace):
D1 FIXED (d47de949), D2-D6 FIXED (595329a0), D7 FIXED as a comment
(50ff650b). Renderer and DeviceFileGrid added (e31ee0fd, 77e538a6).
Final verification below in section 5.

- D1 (place.rs, chain phase): chain column search groups sites into a
  `HashMap<i32, ...>` (`by_x`) and iterates it directly, so which column
  a CARRY8 chain lands on depends on HashMap iteration order. Same seed
  can place the same chain on different columns across runs. This breaks
  the deterministic-seed contract. Fix: sort columns by X before
  scanning. Regression test: place the same chain twice, assert
  identical sites.
- D2 (legality.rs): a placed entry with an out-of-range `packed_idx` is
  silently skipped (`continue`), so a corrupt placed list can pass.
  Fix: emit a violation instead of skipping.
- D3 (legality.rs): a placed site unknown to the grid defaults to
  `SliceKind::Logic` instead of failing loud. A typo in a site name
  would pass SLICEM checks. Fix: emit a violation for unknown sites.
- D4 (legality.rs): the `MultipleCarry8` variant is never produced; the
  checker never counts CARRY8 per slice. Fix: count and report it.
- D5 (legality.rs): coverage is not checked. A packed slice that is
  never placed (dropped by the placer) passes silently. Fix: emit a
  violation for every unplaced packed slice.
- D6 (legality.rs): chain contiguity sorts by Y only; it never verifies
  the placed order matches the `carry_prev` / `carry_next` chain order.
  A chain placed in reverse Y order would still sort clean. Fix: verify
  order follows the chain links, or record as a checker gap if the fix
  is not verifiable from the placed data.
- D7 (pack.rs): the "place FFs in the same slice as their driving LUT"
  comment overclaims. The code scans slices in order with no LUT-driver
  affinity. Comment fix only; affinity is a QoR item, not correctness.
- D8 (pack.rs): packed slice names are `SLICE_X0Y{idx}`, which look like
  real site names but are only packed-slice IDs, renamed at place time.
  Naming hazard, not a bug. No rename planned; the placed-netlist
  renderer must use placed sites, never packed names.
- D9 (place.rs): the seed only shuffles processing order; site choice is
  first-fit in grid order, so small designs may place identically under
  different seeds. Weak seed effect, not nondeterminism. Recorded, not
  fixed here; the multiseed worker (GAP-F6) owns seed sensitivity.
- D10 (pack.rs): CARRY8 slices never receive LUTs (LUT packing only
  fills the current slice, and carry slices are pushed before LUT
  packing starts). They can receive FFs. Legal, but wastes LUT slots.
  QoR note, not a correctness fix.

## 4. Gaps that are not code defects (named, with unblockers)

- G-F2a Real tilegrid: there is no xcku3p tilegrid in any public source.
  `UltraScaleDevice` (device.rs) is a logical model (known G2: invented
  120x200 grid for xcku3p, hard-block columns every 12, SLICEM ratio
  `(x+y)%3==0`, all model choices). `grid.rs` tile names are
  provisional (`CLBLL_L` uniformly, no L/R half distinction). Placing on
  it is placing on a model, not on the device. Unblocker for ground
  truth: Q6 Vivado PinPlanning probe (GAP-B2). Mitigation in this
  session: a `DeviceFileGrid` built from the vendored BitFiltrator
  `xcku3p.json` column tables (file truth: 4 FAR rows, 113 CLB column
  majors, 452 CLB columns, 60 slices per column, 27,120 slices, exactly
  the file's `num_slices`). Its X coordinate is the logical CLB column
  index, which matches the golden blinky FASM (`SLICE_X0Y0` = row major
  0, CLB column 0, tile type CLEL_R), but the logical-column-to-Vivado-X
  mapping is provisional until the probe lands. The file's own
  `num_slices` (27,120) contradicts AMD's 20,340 by exactly one clock
  row; the grid uses file truth and says so.
- G-F2b Hard-block packing: `pack_ultra` takes only LUTs, FFs, and
  CARRY8 chains. There are no BRAM/DSP/URAM pack inputs and no
  hard-block assignments in `UltraPacked`, so legal packing against
  real BRAM/DSP/URAM columns is unimplemented, not just unverified.
  Needs the synth mapper (GAP-F1) to emit those cell kinds first.
  Device-file data available now: BRAM content columns (40 across
  rows) and DSP columns (76) are enumerable from `xcku3p.json`; URAM
  column majors are NOT in the device file schema (no `uram_colMajors`
  field in `RowMajorRaw`), so URAM column legality has no file source
  at all. This session exposes the BRAM/DSP column listings for the
  future pack step and records URAM columns as missing data.
- G-F2c Control-set limits: `ControlSetLimits` defaults (max_clk 1,
  max_sr 1, max_ce 2, max_ff 16, max_lut 8) are parameterized pending
  exact UG574 confirmation. The code already says so. No value is
  treated as verified here.
- G-F2d Timing-driven packing is phase 2, out of scope for this
  session. `timing_weight` is accepted and unused.
- G-F3a Placement legality vs IO/DSP/BRAM columns: `place_ultra` only
  places CLB slices. IO, DSP, and BRAM placement are not modeled, so
  no legality check against those columns exists yet. Blocked on G-F2b.
- G-F8 End-to-end: pack/place output must feed the router. The
  router-to-PIP translator is the route worker's item (GAP-F4, US-09).
  This session adds a deterministic placed-netlist renderer as the
  handoff artifact (site + BEL assignments, sorted by site) and does
  not touch the route branch.

## 5. Verification plan for this session

- Baseline: `cargo test -j1 -p helion-ultrascale --lib --offline` on the
  untouched base (log /tmp/pp-baseline-lib.log, a predecessor's run
  that finished after this worker started): 82 passed, 0 failed.
- One fix per commit, full crate lib test re-run after each fix, no
  regressions. Results: D1 run 83 passed; renderer run 84 passed;
  legality run 89 passed (all five new fail-loud tests green);
  DeviceFileGrid run 92 passed (all three new grid tests green);
  integration run (`--test packplace_xcku3p`) 4 passed, 0 failed.
- New tests: D1 determinism, D2/D3/D4/D5/D6 fail-loud legality,
  `DeviceFileGrid` counts (27,120 slice sites, 452 CLB columns, 208
  CLEM columns, 40 BRAM columns, 76 DSP columns, all from the vendored
  `xcku3p.json`), blinky + counter pack/place/legality on the
  device-file grid with rendered placed netlists, CARRY8 carried through
  pack and place on the counter (chain preserved, vertically
  contiguous).
- Blinky netlist shape (from examples/blinky.sv, registered inverter):
  1 LUT (INIT 0x5555555555555555, per the golden FASM) + 1 FF.
  Counter netlist shape (from examples/counter.sv, 4-bit incrementer):
  4 LUTs + 4 FFs + 1 CARRY8 chain of length 1. These are post-synth
  shapes for the pack/place test; the real synth mapping is GAP-F1's
  job, and `tests/corpus_legality.rs` already covers the helion-sv
  synth-derived path on the fake grid.
