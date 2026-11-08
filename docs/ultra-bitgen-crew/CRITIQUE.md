# Critique log

Verdict log for the UltraScale+ bitgen crew. PASS (verified), FAIL (defects
with file:line), FLAG (needs CTO decision). Every verdict names the exact
check that caught it. Vague unease is not a finding.

## Baseline check 2026-11-07 ~16:50 EST (critic-interfaces, first pass)

No worker deliverables exist yet. All seven worker worktrees are at base
aee77e3, clean, zero commits past base, zero changed files. Verified with
`git rev-parse` and `git status --porcelain` in each of
helion-ultra-bitgen-{core,device,diff,int,integ,lut,segbits}. There is
nothing to check for interface drift, scope, or provenance yet. This entry
records the baseline so later verdicts have a fixed starting point.

### PASS: no interface drift (check: INTERFACES.md vs all worker branches)

No code changed on any worker branch, so no FASM, segbits API, frame
addressing, diff report shape, or block-name deviation is possible yet.
Contracts on record for future checks: FASM `<tile>.<feature>` lines with
INIT expanded MSB-first; SegbitsDb `lookup(tile_type, feature)` returning
`&[BitRef]`; FAR block types 0 CLB_IO_CLK / 1 BRAM_CONTENT, 93-word
frames; diff report JSONL with `green` / `red` / `blocked` status and
blocks ordered by diff_bits descending; block registry LUT_INIT, FF_INIT,
BRAM_CONTENT, BRAM_PARITY, INT_PIP, IO_CONFIG, CLK, DSP48E2, URAM288,
FRAMING.

### PASS: provenance baseline (check: vendored data and reference clones)

- BitFiltrator reference clone at
  `~/workspace/helion-ultra-bitstream/bitfiltrator-ref/`: LICENSE verified
  MIT, Copyright (c) 2022 EPFL VLSC, full MIT text present. Its JSON may
  be vendored once segbits-vendor records the source and license.
- No RapidWright `.dat` file exists in any bitgen worktree: `find` for
  `*.dat` / `*rapidwright*` returned zero files across
  helion-ultra-bitgen-* and helion-ultra-bitstream.
- No vendored segbits data exists yet: `crates/helion-ultrascale/data/`
  does not exist in the base worktree.
- `/tmp/prjuray-db` is gone, as COORD.md states. No worker has
  re-cloned or vendored it yet.

### PASS: netlist_interchange.rs cleared, not a provenance violation (check: FPGA Interchange / RapidWright ingestion)

`crates/helion-proj/src/netlist_interchange.rs` (1929 lines, pre-existing
base code, present identically in all worktrees) converts only Yosys JSON,
BLIF/EBLIF, EDIF, and structural UNISIM Verilog netlists. It ingests no
FPGA Interchange DB and no RapidWright data, and it is not referenced by
`crates/helion-ultrascale` (no reference in its src or Cargo.toml). The
filename match on "interchange" is a generic netlist-format converter, not
an encumbered device database. No action needed.

### FLAG: prjuray-db CC0 verification still pending (needs segbits-vendor, then CTO sign-off)

segbits-vendor has not yet re-cloned prjuray-db. Before any segbits file
is vendored under `crates/helion-ultrascale/data/`, the clone's CC0 1.0
license file must be verified and the source plus license recorded in the
file header or a LICENSES note, per the brief. This is a watch item, not
a defect: nothing has been vendored yet.

### Scope note

No worker has produced anything, so no scope check is possible yet. The
roster in COORD.md still lists every worker as "spawning" with agent TBD.
When deliverables land, each will be checked against its lane and its
out-of-scope list in its brief.

## 2026-11-07 critic-correctness, base review (aee77e3)

Scope: no worker deliverable exists yet. All seven fm/ultra-bitgen-*
worktrees are clean at aee77e3 with zero commits, so this round reviews
the base only: scout report.md plus crates/helion-ultrascale. No block
goes green. No diff harness exists to re-run.

### PASS

- P1 base tip. `git rev-parse fm/ultra-bitstream` returns
  aee77e3460d88509f2880880a9ba38b77a6e7b40, matching SPEC.md.
- P2 crate tests, re-run by critic. Command:
  `cargo test -p helion-ultrascale -j2` in
  ~/workspace/helion-ultra-bitstream. Result: 10 unit tests plus 2
  integration tests pass, 0 fail, exit 0. Matches the 12-test claim in
  commit 7a4146f.
- P3 FAR layout and frame size. Verified against
  bitfiltrator-ref/src/arch_spec.py UltraScalePlusSpec and
  src/frame_spec.py FarBlockType: reserved 31:27, block type 26:24,
  row 23:18, column 17:8, minor 7:0, frame size 93 words,
  CLB_IO_CLK = 0, BRAM_CONTENT = 1. report.md FAR section and
  INTERFACES.md match exactly.
- P4 LUT/FF/BRAM arch pointers, spot-checked in
  bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json with python3
  json load. CLEL_L/CLEL_R/CLEM/CLEM_R LutLoc: 60 Y_ofst entries,
  8 LUT names, 64 frame_ofst and 64 minor entries each. RegLoc:
  60 Y_ofst entries, 16 FF names (AFF..HFF2), one int frame_ofst
  and minor per FF. BRAM BramMemLoc: 24 Y_ofst entries, 16384
  frame/minor entries per Y. BramMemParityLoc: 24 Y_ofst entries,
  2048 per Y. Report counts confirmed on a per-Y basis; note the
  report phrase "16384 bits" is per BRAM site, 393216 entries per
  full BRAM column.
- P5 xcku3p control part exists, xcvu3p device file missing.
  bitfiltrator-ref/resources/devices/ has 34 files including
  xcku3p.json and no xcvu3p.json. Confirms the report and SPEC.
- P6 derived counts arithmetic, hand-checked. 25.3 * 1024 / 36 =
  719.6 rounds to 720 RAMB36E2. 90.0 * 1024 / 288 = 320 URAM288
  exactly. 394000 / 8 = 49250 CLB slices. The passing count tests
  agree.

### FAIL

- F1 Bel::name() is not injective, so it cannot identify a BEL.
  crates/helion-ultrascale/src/bel.rs:139, the fallback arm
  `_ => Some(self.kind.unisim().to_string())`, returns the same
  string for every index of a kind. Concrete cases: F7Mux index 0
  and index 1 both name "F7MUX"; F8Mux index 0 and 1 both name
  "F8MUX"; Ramb18e2 index 0 and 1 both name "RAMB18E2", and
  device.rs:162-163 creates both RAMB18 BELs on every BRAM tile.
  Any consumer that keys on name, including the FASM handoff in
  INTERFACES.md which names BELs per line, cannot tell these apart.
  Fix: give each indexed BEL a distinct name (F7AMUX style per
  UG574, RAMB18_X/Y or _A/_B), plus a test asserting name
  uniqueness within a tile. Blocks LUT_FF_BRAM naming until fixed.
- F2 "zero clippy warnings" claim is stale. Re-run by critic:
  `cargo clippy -p helion-ultrascale -j2 --all-targets` emits 1
  warning: useless use of `vec!` in
  crates/helion-ultrascale/tests/smoke_place.rs:43
  (`let placements = vec![...]` with a fixed two-element literal).
  Minor, test-only, but the claim in commit 7a4146f no longer holds
  on --all-targets. Fix: use an array literal and re-run clippy.

### FLAG (cannot verify, needs CTO decision or missing data)

- G1 prjuray-db claims are unverifiable right now. /tmp/prjuray-db
  does not exist (confirmed: `ls /tmp/prjuray-db` reports no such
  file or directory) and no vendored copy exists in the repo. The
  report.md numbers 3778 INT PIP lines, 1560 HPIO lines, 104 CMT
  lines, and the site-type JSON filenames cannot be re-checked until
  segbits-vendor re-clones and vendors the DBs. No INT_PIP verdict
  is possible before that.
- G2 device.rs grid is invented, not a Vivado device file.
  crates/helion-ultrascale/src/device.rs build_xcvu3p: 144x400 grid,
  hard-block columns every 12, CLB tiles filled row-major, SLICEM
  ratio by `(x + y) % 3 == 0`, bottom rows all Io tiles. The code
  documents this as logical, and the aggregate counts are right, but
  the grid carries no clb_colMajors, bram_colMajors, FAR row/col
  mapping, IDCODE, or config order required by the INTERFACES.md
  device-file schema. It must not be consumed by bitgen-core as
  ground truth. CTO to confirm the device-file worker builds the
  real xcvu3p.json from xcku3p plus Vivado and does not port this
  grid.
- G3 BRAM tile double-models capacity. device.rs creates one
  Ramb36e2 plus two Ramb18e2 BELs per BRAM tile, implying 1440
  RAMB18 halves alongside 720 RAMB36, with no test asserting the
  RAMB18 count. A RAMB36 split into two RAMB18 is a real mode, but
  the model counts both the whole and its halves at once. CTO to
  rule the intended accounting before lut-ff-bram consumes it.
- G4 LUTRAM "same bits" claim is only partly verified. The paper
  abstract (pdftotext of bitfiltrator-ref/fpl22-bitfiltrator-paper.pdf)
  confirms LUTRAM contents are in BitFiltrator scope, but the cited
  section V-B text stating LUTRAM uses the same INIT bits was not
  confirmed line-level in the two-column PDF extract. Treat as
  covered at abstract level only until someone cites the V-B lines.
- G5 no worker deliverables, no diff harness, no segbits vendor, no
  bitgen-core. Nothing is diffable yet, so no block verdict beyond
  this base review is possible. Tier 2 stays blocked on the Vivado
  license server, carried from Oct 22.
- G6 housekeeping, no verdict impact. My own `cargo test` run
  modified Cargo.lock in the base worktree; I restored it with
  `git checkout -- Cargo.lock`. Worktree is clean again apart from
  the pre-existing untracked bitfiltrator-ref/ clone, which SPEC
  says stays untracked.

## 2026-11-07 critic-correctness, session 2: int-pip deliverable (6bb30b6)

Scope: commit 6bb30b6 on fm/ultra-bitgen-int (worktree
~/workspace/helion-ultra-bitgen-int, base aee77e3). Files:
crates/helion-ultrascale/src/segbits.rs (375 lines, SegbitsDb trait
plus FileSegbitsDb loader), src/int_pip.rs (361 lines, PIP
emission), tests/int_pip.rs (290 lines, routed-design verification),
src/lib.rs (module wiring only), Cargo.lock (4 lines). Worktree
clean at 6bb30b6. No data/ dir, nothing vendored, no file outside
the crate touched.

### PASS: INT_PIP tier-1 emission verified bit-exact (block may go green at tier 1)

- P1 tests, re-run by critic. Command:
  `cargo test -j2 -p helion-ultrascale` in
  ~/workspace/helion-ultra-bitgen-int (via nohup setsid, log
  /tmp/critic-int-test.log). Result: 22 unit plus 2 int_pip
  integration plus 2 smoke = 26 passed, 0 failed, exit 0. Matches
  the worker's claimed breakdown exactly.
- P2 BOUNCE hand-check, verified against the DB by critic.
  Reference clone ~/workspace/prjuray-db-ref is at affbc5e, COPYING
  verified CC0 1.0 Universal. DB line:
  `INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 64_00 !66_00 !68_00 !72_00
  !72_01 !74_00`, exactly the set bit 64_00 and zero bits 66_00,
  68_00, 72_00, 72_01, 74_00 the worker claims.
- P3 full DB, verified by critic with an independent Python pass
  over zynqusp/segbits_int.db: 3778 data lines, 3778 unique
  features, zero duplicates; frame range 0..75, bit range 0..47,
  confirming the worker's correction that the bit index is NOT
  0..32. The test's independent second parser agrees with the
  loader on every line (test full_db_every_line_parses_and_resolves
  passed in my re-run).
- P4 routed design, verified by critic with an independent Python
  BFS (file-order adjacency, visited on enqueue) over the same DB:
  the BFS path from BOUNCE_E_0_FT1 to NN12_END7 reproduces
  EXPECTED_PATH exactly (11 hops), and summing the 11 DB lines
  gives 11 set bits and 48 zero bits, exactly as claimed. Every
  emitted bit is checked by independent lookup in the test, never
  by re-reading the emission's own output.
- P5 scope: the worker stayed in its lane. It vendored nothing
  (`crates/helion-ultrascale/data/` absent, zero files), used the
  reference clone for verification only, and the test locates the
  DB via HELION_SEGBITS_INT_DB, then the reference clone, then the
  vendored path, failing loud and naming all three paths if none
  exists. The loader never reads /tmp on its own.
- P6 interfaces: the SegbitsDb trait and BitRef match
  INTERFACES.md exactly, field for field and signature for
  signature (`BitRef { frame: u32, bit: u8, must_be_zero: bool }`,
  `lookup(&self, tile_type: &str, feature: &str) ->
  Option<&[BitRef]>`, `tile_types(&self) -> &[String]`).
  FASM lines are `<tile>.PIP.<src>.<dst>` per the FASM contract,
  and PIP feature strings match the DB vocabulary exactly.
- P7 fail-loud behavior: negative tests prove a bogus PIP lands
  on the missing list with tile/src/dst/design and stops
  emission, and a zero bit reading 1 fires ZeroBitSet naming
  tile, feature, frame, and bit. Malformed lines, duplicate
  features, and unreadable files all fail loud in the loader.
- P8 hygiene: no em dashes in segbits.rs, int_pip.rs,
  tests/int_pip.rs, lib.rs, or the COORD.md int-pip entry
  (grep exit 1 on all). Cargo.lock change is only the
  helion-ultrascale package entry, legitimate lock churn from
  building the crate's tests. Clippy re-run by critic:
  `cargo clippy -j2 -p helion-ultrascale --all-targets` emits
  exactly 1 warning, the pre-existing useless `vec!` in
  tests/smoke_place.rs:43 (F2, already fixed on
  fm/ultra-bitgen-device); zero new warnings from this commit.
  The new files are fmt-clean.

Tier 2 (Vivado bitstream) is not covered by this verdict and stays
blocked on the license server, unchanged since Oct 22.

Correction (minor, not a defect): the worker's "cargo fmt clean"
claim is overstated as a blanket statement. `cargo fmt -p
helion-ultrascale -- --check` in this worktree shows one diff in
tests/smoke_place.rs:67, a file this commit does not touch. I
verified the line exists identically at base aee77e3, so the diff
is pre-existing base churn, not this worker's. The new files are
clean. No action on this commit.

### FLAG: merge sequencing with segbits-vendor (needs orchestrator, not a defect in this commit)

Both fm/ultra-bitgen-int (6bb30b6) and fm/ultra-bitgen-segbits
(uncommitted staged state) add src/segbits.rs and modify
src/lib.rs. I diffed the two segbits.rs files: they are
byte-identical (0 diff lines), so segbits-vendor IS reusing the
int-pip loader verbatim, exactly as COORD.md directs, not
duplicating it. But the merge needs an explicit order: one branch
lands the shared file first, the other rebases onto it, or the
integration worker resolves the identical addition. Do not blind
merge both. Also note segbits-vendor has an untracked
tests/int_pip_audit.rs in its worktree; the orchestrator should
confirm it does not duplicate tests/int_pip.rs here.

Observation, not a verdict on that branch: segbits-vendor commit
886102a vendored the DBs under data/ with CC0 verified from
COPYING and SHA-256 checks, and I verified the vendored
segbits_int.db header is 4 `#` comment lines the loader skips,
with data lines byte-identical to upstream (same SHA-256). The
int-pip test's vendored path will therefore resolve after the
merge. The base-review G1 FLAG (prjuray-db unverifiable) is closed
in substance by that commit; formal G1 closure belongs to the
segbits-vendor deliverable review, not this one.

## 2026-11-07 critic-correctness, session 3: segbits-vendor deliverable (c8509e6)

Scope: branch fm/ultra-bitgen-segbits tip c8509e6 (worktree
~/workspace/helion-ultra-bitgen-segbits, base aee77e3), two commits:
886102a (vendor prjuray-db segbits DBs under
crates/helion-ultrascale/data/, PROVENANCE.md, INT_PIP_AUDIT.md)
and c8509e6 (port FileSegbitsDb plus SegbitsDb trait from
fm/ultra-bitgen-int). Verdict is on the committed tip only. At
review time the worktree also held uncommitted modifications
(`M src/segbits.rs`, `M tests/segbits.rs`); those are not reviewed
here, they are the in-progress fix for the FAIL below.

### FAIL: committed loader cannot load 3 of the 5 vendored segbits DBs

- F1 the defect. Committed `src/segbits.rs`, `parse_bit_token`:
  the bit is parsed as u32, then `if bit_n > u8::MAX as u32`
  returns Err ("bit token ... has bit ... outside 0..256"). The
  vendored DBs contain bit offsets far above 255. My own scan of
  the vendored files (tokens after `#` headers skipped):
  segbits_hpio_right.db max bit 1359, segbits_hdio_top_right.db
  max bit 1390, segbits_hdio_bot_right.db max bit 1310. (INT max
  47, CMT max 151, both loadable.)
- Execution proof, not code reading alone. In a detached
  worktree at exactly c8509e6 I added a probe test calling
  `FileSegbitsDb::from_file` on the vendored files (probe file
  removed afterwards; the worker's worktree was never touched):
  `HPIO_LOAD_FAILED: segbits line 241 is malformed (bit token
  "!02_259" has bit 259 outside 0..256)` and
  `HDIO_TOP_LOAD_FAILED: segbits line 23 is malformed (bit token
  "01_341" has bit 341 outside 0..256)`. The committed loader
  fails loud on the vendored HPIO/HDIO data instead of loading
  it.
- The committed code admits it. `tests/segbits.rs` (committed)
  carries a NOTE: only INT and CMT are loaded whole; HPIO and
  HDIO "contain bit offsets above 255 (max 1390), which the
  ported loader rejects under its u8 bit type; loading them whole
  is covered by the u16 widening commit, which adds its
  regression test." That commit does not exist. The brief's
  acceptance criterion is "every segbits lookup in the pipeline
  resolves from vendored repo data"; a lookup against HPIO/HDIO
  data cannot resolve through the committed loader at all.
- Repair, all small and already drafted in the worktree: (1)
  commit the u16 widening of `BitRef.bit` plus the
  `hpio_features_with_wide_bit_offsets_resolve` regression test
  (currently uncommitted in the worker's worktree); (2) get the
  contract change signed off, see FLAG below; (3) run `cargo
  fmt` (see P9); (4) request a critic re-run. Do not merge this
  branch into fm/ultra-bitstream before the re-run passes.

### FLAG: the u16 widening changes INTERFACES.md (needs orchestrator plus critic-interfaces)

The uncommitted fix changes `BitRef.bit` from u8 to u16. The
worker's own comment on it says it "deviates from
INTERFACES.md (`bit: u8`) and needs orchestrator /
critic-interfaces sign-off." The technical claim checks out:
prjuray bit tokens are a bit address within the frame, and a
93-word UltraScale+ frame holds 2976 bit positions, so u8
cannot hold the general case and u16 is the right type. But
the contract change is real and ripples: every `BitRef`
consumer must be re-checked, and the merged segbits.rs will
no longer be byte-identical to int-pip's copy (see the second
FLAG). The orchestrator rules the contract change and updates
INTERFACES.md; I rule only that the widening is technically
correct for the vendored data.

### FLAG: merge sequencing with int-pip, revised (needs orchestrator)

Session 2 recorded the two segbits.rs files as byte-identical.
That is still true for the committed tip (P4 below). Once the
u16 fix commits, they diverge: the merged file must be the u16
one, and int-pip's `tests/int_pip.rs` helper `sorted_bits`
returns `Vec<(u32, u8)>`, which will not compile against a u16
bit field. The orchestrator sequences it: segbits-vendor lands
the u16 fix, the merged segbits.rs is the u16 version, int-pip
rebases and fixes `sorted_bits` (plus any other u8 assumption),
and the int-pip tests re-run. Do not merge either branch until
the u16 version is the single shared file.

### PASS (verified by critic, each item re-checked independently)

- P1 vendoring byte-identical, SHA-256 verified. I recomputed
  every hash in PROVENANCE.md against the vendored files and
  the upstream clone (~/workspace/prjuray-db-ref, affbc5e):
  all 10 `.db` / `.origin_info.db` files match the recorded
  SHA-256 (data lines only, 4-line `#` header excluded) and
  their data bytes are byte-identical to upstream; all 14
  JSONs match the recorded SHA-256 and are byte-identical to
  upstream. Line counts match PROVENANCE.md exactly:
  3778 / 1560 / 812 / 828 / 104 for both the `.db` and the
  `.origin_info.db` of each tile type. Total 7082 segbits
  features as claimed.
- P2 license verified from the source, not from the worker's
  word. `git rev-parse HEAD` in ~/workspace/prjuray-db-ref is
  affbc5e555ebae16475f32e8fb2d6565d4204f3f; COPYING opens with
  "Creative Commons Legal Code / CC0 1.0 Universal".
  PROVENANCE.md records source repo, commit, and license.
  Every one of the 10 `.db` files carries the 4-line `#`
  header naming source, commit, and license (first line
  checked on all 10). JSONs carry no inline header, which is
  correct (JSON has no comment syntax); their provenance is
  recorded in PROVENANCE.md, which the brief's header rule
  allows via the CTO ruling (LICENSES note). BitFiltrator
  pointers in PROVENANCE.md name the MIT source; no
  BitFiltrator JSON was copied here, as specified.
- P3 INT PIP audit part 1, independently reproduced. I ran
  the INT_PIP_AUDIT.md Python reproduction verbatim against
  the upstream clone and again against the vendored copies;
  both give the same output: tile PIPs 3774, covered 1:1
  3770, missing exactly the 4 bidirectional base features
  (BYPASS_E14/E8/W14/W8), extra exactly the 8 `.FWD` / `.REV`
  features. 3770 + 8 = 3778, the DB size. Coverage 3774 of
  3774 as claimed. Part 2 is correctly recorded as pending:
  the PinPlanning probe has not been run, and the audit does
  not claim Vivado-level completeness.
- P4 loader port is verbatim. `diff` of the committed
  `src/segbits.rs` against int-pip commit 6bb30b6's
  `src/segbits.rs`: 0 diff lines. The `lib.rs` diff is 2
  lines (`pub mod segbits` plus the re-export). No duplicated
  loader.
- P5 tests, re-run by critic on the committed tip. Detached
  worktree at c8509e6, command `cargo test -j2 -p
  helion-ultrascale` (nohup setsid, log
  /tmp/critic-segbits-test.log): 17 unit plus 1 int_pip_audit
  plus 3 segbits plus 2 smoke = 23 passed, 0 failed.
- P6 audit test does not duplicate the int-pip test. The
  session-2 FLAG item is answered: `tests/int_pip_audit.rs`
  is one test, `int_pip_audit_db_internal`, enumeration-based
  over tile_type_INT.json; it contains no EXPECTED_PATH, no
  BFS, no routed-design code. The int-pip test
  (`blinky_int_tile_route_matches_db_bit_exact`,
  `full_db_every_line_parses_and_resolves`) is a different
  test of a different property.
- P7 scope and legal line. `data/` holds exactly the 26
  files the branch claims (2 docs, 5 `.db`, 5
  `.origin_info.db`, 14 JSONs), nothing else. No RapidWright
  `.dat` and no FPGA Interchange DB in the branch: the only
  `git ls-tree` match for `.dat` / `rapidwright` /
  `interchange` is the pre-existing base file
  `crates/helion-proj/src/netlist_interchange.rs`, already
  cleared by the interfaces critic. The loader never reads
  `/tmp`: the only `/tmp` mentions in the committed loader
  and tests are doc comments stating it never does.
  `cargo clippy -j2 -p helion-ultrascale --all-targets` in
  the detached worktree emits exactly 1 warning, the
  pre-existing useless `vec!` in tests/smoke_place.rs:43
  (F2, fixed only on fm/ultra-bitgen-device); zero new
  warnings from this branch.
- P8 hygiene: no em dashes in any file on the branch
  (`git diff aee77e3..c8509e6 --name-only`, every file
  scanned, zero hits).
- P9 fmt, minor, not a defect in the verdict: `cargo fmt -p
  helion-ultrascale -- --check` shows 4 diffs. Two are the
  worker's own new file, tests/segbits.rs:43 and :53
  (assert formatting); run `cargo fmt` when the u16 fix
  commits. The other two are pre-existing base churn
  (src/lib.rs:18 fabric re-export line,
  tests/smoke_place.rs:67, same class as the session-2
  note); no action on this branch.

### G1: NOT closed by this verdict

The license and vendoring substance is verified (P1, P2), so
G1 is closed in substance for the vendored bytes. Formal G1
closure needs the vendored data to be consumable through the
loader, and 3 of the 5 segbits DBs are not loadable by the
committed loader (F1). G1 closes on the re-review after the
u16 fix is committed and the HPIO/HDIO whole-load tests pass.

Tier 2 (Vivado bitstream) is not covered by this verdict and
stays blocked on the license server, unchanged since Oct 22.

## 2026-11-07 critic-correctness, session 3 addendum: re-review of 98e282a (u16 fix)

The worker committed the F1 repair while the session-3 review
was running: new tip 98e282a, "segbits-vendor: widen BitRef.bit
u8 to u16 so vendored HPIO/HDIO load", 2 files
(src/segbits.rs, tests/segbits.rs), worktree clean. I
re-reviewed the new tip in a fresh detached worktree at
exactly 98e282a (probe files removed afterwards; the worker's
worktree never touched).

### PASS: F1 fixed in execution, not just in code

- The committed diff is the widening the FAIL described:
  `BitRef.bit` is u16 (segbits.rs:42), `set`/`zero` take u16,
  the parser rejects only above `u16::MAX` (:276). The
  tests/segbits.rs NOTE is gone; the
  `hpio_features_with_wide_bit_offsets_resolve` regression
  test is present (:48).
- Execution proof, probe test in the detached worktree:
  `LOADED segbits_hpio_right.db features=1560`,
  `LOADED segbits_hdio_top_right.db features=812`,
  `LOADED segbits_hdio_bot_right.db features=828`,
  `LOADED segbits_cmt_right.db features=104`,
  `LOADED segbits_int.db features=3778`. All 5 vendored
  segbits DBs load whole through the committed loader. The
  F1 defect is gone.
- Tests re-run by critic on 98e282a, command `cargo test -j2
  -p helion-ultrascale` (nohup setsid, log
  /tmp/critic-segbits-98-test.log): 19 unit plus 1
  int_pip_audit plus 4 segbits plus 2 smoke = 26 passed, 0
  failed. (Unit 17 to 19 and segbits 3 to 4 are the widening
  regression tests.)
- Clippy re-run: `cargo clippy -j2 -p helion-ultrascale
  --all-targets` emits exactly 1 warning, the pre-existing
  useless `vec!` in tests/smoke_place.rs:43 (F2); zero new
  warnings.
- Minor, still open: `cargo fmt -p helion-ultrascale
  -- --check` shows diffs in the worker's own files,
  src/segbits.rs:390 and tests/segbits.rs:38, :50, :59.
  The worker did not run `cargo fmt` when committing the
  fix. Cosmetic only; does not block the verdict. The
  lib.rs:18 and smoke_place.rs:67 diffs are pre-existing base
  churn, no action.

### G1: CLOSED (formal)

License verified from COPYING at affbc5e (P2 of the main
review), all vendored bytes verified against upstream (P1),
the INT PIP audit reproduced (P3), and now every vendored
segbits DB loads whole through the committed loader. The
prjuray-db ground truth is vendored, verified, and
consumable. G1 closes.

### FLAG (still open, unchanged): the INTERFACES.md contract

98e282a does not touch INTERFACES.md, which still specifies
`bit: u8`. The committed code is now u16. The orchestrator
plus critic-interfaces must rule the contract change and
update INTERFACES.md before this branch merges; the code is
correct, the contract is stale. The merge-sequencing FLAG
stands as well: the merged segbits.rs must be the u16
version, int-pip's `sorted_bits` (`Vec<(u32, u8)>`) must be
fixed on rebase, and the int-pip tests must re-run. The
segbits-vendor deliverable itself is verified; the block goes
green when the contract ruling lands and the merge order is
set, not before.

## 2026-11-07 critic-correctness, session 4: lut-ff-bram deliverable (0e81099)

Scope: branch fm/ultra-bitgen-lut, tip 0e81099 (worktree
~/workspace/helion-ultra-bitgen-lut, base aee77e3). Four commits:
1adf250 (F1 Bel::name() injectivity), dfd08d0 (F2 clippy vec!),
0b83568 (vendored BitFiltrator LUT/FF/BRAM tables), 0e81099
(bitloc.rs emission plus tests/lut_ff_bram.rs). Files changed:
crates/helion-ultrascale/src/bel.rs, src/bitloc.rs (861 lines),
src/lib.rs, tests/lut_ff_bram.rs (576 lines),
tests/smoke_place.rs, Cargo.toml (serde_json), Cargo.lock,
data/README.md, data/generate_tables.py,
data/ultrascale_plus_lut_ff_bram.json. Worktree clean at
0e81099. This verdict covers the emission code and tables only.
No diff harness exists yet, so no block goes green on this
verdict; LUT_INIT, FF_INIT, BRAM_CONTENT, BRAM_PARITY still need
a zero diff plus a harness re-run.

### PASS: LUT/FF/BRAM emission and tables verified

- P1 tests, re-run by critic. Command:
  `cargo test -j2 -p helion-ultrascale` in
  ~/workspace/helion-ultra-bitgen-lut (via nohup setsid, log
  /tmp/critic-lut-test.log). Result: 11 unit plus 20 lut_ff_bram
  plus 2 smoke = 33 passed, 0 failed, exit 0. Matches the
  worker's claimed breakdown exactly. My first run failed to
  start (`setsid: failed to execute cargo: No such file or
  directory`, cargo is at ~/.cargo/bin, not in the setsid PATH);
  the re-run with the full path is the result above. Worktree
  clean after my run (`git status --porcelain` empty).
- P2 clippy and fmt, re-run by critic. Command:
  `cargo clippy -j2 -p helion-ultrascale --all-targets`, log
  /tmp/critic-lut-clippy.log. Result: 0 warnings. Command:
  `cargo fmt -p helion-ultrascale -- --check`. Result: exit 0,
  clean. The F2 fix (array literal in smoke_place.rs) is in
  place and verified.
- P3 F1, verified in code and by the test. bel.rs `name()` now
  returns F7AMUX..F7DMUX, F8AMUX, F8BMUX, F9MUX per UG574,
  RAMB18E2_L (index 0, lower) / RAMB18E2_U (index 1, upper) per
  Project U-Ray L/U naming, and None above each kind's valid
  index range. Test `bel_names_are_injective_within_a_tile`
  (bel.rs:246) enumerates every CLB BEL and asserts no duplicate
  name, plus explicit per-index assertions for the muxes and
  both RAMB18 halves; it passed in my re-run. The index 0 =
  lower half mapping is flagged in code as a model convention,
  not Vivado-verified.
- P4 vendored tables, verified verbatim by critic. Source
  bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json SHA-256
  is c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c,
  exactly the value in data/README.md; LICENSE verified MIT,
  Copyright (c) 2022 EPFL VLSC. Independent Python comparison
  of the vendored JSON against the source: all ten sections
  verbatim equal (LutLoc and RegLoc for CLEL_L, CLEL_R, CLEM,
  CLEM_R; BramMemLoc and BramMemParityLoc), top keys identical.
  generate_tables.py copies the four tables with no values
  changed, gates on the recorded SHA-256, and asserts the
  counts (60 Y x 8 LUTs x 64 bits, 60 Y x 16 FFs, 24 Y x
  16384, 24 Y x 2048).
- P5 hand-checked bits, spot-checked by critic against the
  source JSON (not the vendored copy, not the code). LutLoc
  CLEL_L Y0 A6LUT bit 0 = (minor 11, frame_ofst 15), bit 1 =
  (10, 15), bit 4 = (11, 14), bit 63 = (8, 0); the test file
  encodes the same four pairs. BramMemLoc Y0 bit 0 = (0, 0),
  bit 16383 = (255, 107); the test encodes bit 16383 the same.
  RegLoc Y0 AFF = (12, 2), HFF2 = (12, 46) per the worker's
  report; the test encodes AFF (12, 2). Paper V-C cross-check:
  SLICE_X136Y247/A6LUT bit 0 = (11, 351) at Y_ofst 7 (247 mod
  60), confirmed in the source JSON. Structural finding
  confirmed: Y29 A6LUT bit 0 frame 1407, Y30 bit 0 frame 1551,
  the extra 96 jump at Y_ofst 30. LutLoc and RegLoc are
  byte-identical across all four CLB tile types in the source.
- P6 G4, worker's REJECTION verified by critic. I extracted
  section V-B from bitfiltrator-ref/fpl22-bitfiltrator-paper.pdf
  with pdftotext and read it line-level. The paper says
  "Discovering the minor and frame offsets of registers and
  LUTRAMs is simple as the tools consider these BELs to be
  user-state bits. We again use the logic location file to
  determine their format in a column quickly," and then
  "However, determining the minors and frame offsets of
  standard LUTs is more complicated as these BELs are not
  user-state bits, and no logic location file is available."
  It never states LUTRAM shares the LUT INIT bits. The arch
  JSON has no key containing "lutram" (checked by full key
  walk: only LutLoc, RegLoc, BramMemLoc, BramMemParityLoc).
  `lut_placed_bits` with `LutMode::Lutram` returns
  `BitLocError::LutramBlocked` before touching any table, and
  test `lutram_blocked_fail_loud` asserts exactly that. Base
  G4 is closed: the "same bits" claim is rejected, LUTRAM
  emission stays blocked until a LUTRAM table or Vivado
  logic-location data lands.
- P7 G3, verified in code. `BramTileUse` is one enum with
  variants `Ramb36` or `Ramb18 { lower, upper }`; a tile can
  never be configured as both at the type level. Capacity (720
  total) stays with the device-file worker, as ruled.
- P8 interfaces and scope. FASM spellings match the
  INTERFACES.md contract: `SLICE_XxYy.A6LUT.INIT[63:0] =
  64'h...`, `AFF.INIT = 1'b0/1`, `RAMB18_XxYy.INIT[16383:0]`,
  `RAMB36_XxYy.INIT[32767:0]`, MSB-first, bit 0 = LSB. No
  absolute FARs are computed here: (minor, frame_ofst) is
  column-relative and the code says so. FF config beyond INIT
  is not emitted (no table exists), correctly out of scope.
  The worker claimed no diff-harness status and none is
  reported. No file outside crates/helion-ultrascale plus
  Cargo.lock/Cargo.toml was touched. Cargo.lock change is
  only the helion-ultrascale package entry adding serde_json.
- P9 hygiene. No em dashes in bitloc.rs, lut_ff_bram.rs,
  bel.rs, smoke_place.rs, data/README.md,
  data/generate_tables.py, or the whole of COORD.md (grep
  count 0). The INITP[2047:0] / INITP[4095:0] spellings and
  the RAMB36 INIT word split (INIT words 0..63 lower,
  64..127 upper) are stated in code and COORD.md as following
  the day-1 / U-Ray pattern with the diff harness as the
  verification path, not as Vivado-verified facts. That is
  honest and correctly flagged, not hidden.

Tier 2 (Vivado bitstream) is not covered by this verdict and stays
blocked on the license server, unchanged since Oct 22.

### FLAG: RAMB18 half naming diverges between branches (needs CTO ruling, not a defect in this branch)

Two F1 fixes exist on two branches with different RAMB18 half
names. This branch (1adf250) uses RAMB18E2_L / RAMB18E2_U per
Project U-Ray naming. fm/ultra-bitgen-device (aece1c6, per COORD.md
~17:05 entry) uses RAMB18E2_A / RAMB18E2_B, documented there as a
Helion-internal disambiguator. Both are injective and both are
documented, so either spelling satisfies F1 on its own branch,
but only one spelling can land: the FASM handoff keys on BEL
names, and bitloc.rs `BramHalf::suffix` hard-codes "L" / "U"
while the device-file branch hard-codes "A" / "B". The CTO must
pick one spelling before merge. Both branches also fix F2
(smoke_place.rs array literal) and both modify bel.rs and
lib.rs, so a merge of the two branches will conflict on bel.rs,
smoke_place.rs, and lib.rs regardless. The orchestrator
sequences the merge; the CTO rules the spelling.

### FLAG (minor, orchestrator): data/ provenance is split across branches

This branch adds data/README.md plus generate_tables.py for its
four tables, while fm/ultra-bitgen-segbits adds data/PROVENANCE.md
plus INT_PIP_AUDIT.md for the segbits DBs. Both provenance docs
are correct on their own, but after both merges data/ will have
two provenance records that do not reference each other.
Consolidate or cross-reference at merge time; do not let the
second merge silently overwrite or ignore the first doc.

## 2026-11-07 critic-interfaces, session 2: bitgen-core deviations (0f3aa2d)

Scope: commit 0f3aa2d on fm/ultra-bitgen-core (worktree
~/workspace/helion-ultra-bitgen-core, base aee77e3) against merged
tip fm/ultra-bitstream a09df12. Files: crates/helion-ultrascale/
src/bitgen.rs (1677 lines), tests/bitgen_roundtrip.rs (248 lines),
src/lib.rs. This is an interfaces, scope, and provenance ruling
only. The emitter's tests, CRC vectors, and byte-level round-trip
evidence are the correctness critic's session, not re-run here.
Every claim below was checked by reading the cited files and the
on-disk DBs, not by taking the worker's report on trust.

### Item 1: BitRef.bit u16, not u8: PASS, consistent, no further action

INTERFACES.md already states u16 with the orchestrator's Ruling 1
on segbits-vendor 98e282a. bitgen.rs:413 uses u16. The merged tip
segbits.rs:31 still reads u8 because the segbits branch (98e282a)
has not merged yet; the merge queue already carries the corrected
check (the diff vs merged segbits.rs is exactly the u16 widening,
nothing else). No further action.

### Item 2: BitRef.frame is a frame offset relative to the tile base FAR: PASS, contract update required

Verified, not assumed. The DB is keyed by tile type only, so the
frame number cannot be absolute: one line (e.g.
`INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 64_00 ...`) applies to
every INT tile, while absolute FARs differ per tile. The frame
ranges are tile-sized offsets, computed from the on-disk DBs:
CLEL 0..15, INT 0..75, bit 0..47 in both files. BitFiltrator
bit_locator.py confirms the same structure from the other side:
it returns a FAR built from the `minor` plus a separate
`frame_ofst`, and frame.py `ConfigFrame.bit` defines bit N as
word N/32, bit N%32, so `frame_ofst` is the bit index within the
frame and `minor` is the FAR minor address. The u32 sum
`base_far + frame` (bitgen.rs:747-753, checked_add) carries
through minor, column, and row in FAR increment order, and a
tile's frames are consecutive minors within one column, so the
sum lands on the tile's frames.

Ruling: update INTERFACES.md to state that `BitRef.frame` is a
frame offset relative to the tile base FAR (prjxray segbits
convention), and absolute FAR = `tile_base_far + frame` as a u32
sum. The segbits-vendor loader already produces BitRef lists in
this relative form (the DB numbers are relative); no loader
change. The lut-ff-bram bitloc.rs output is (minor, frame_ofst)
pairs, not BitRefs, so the integration step must convert them:
`frame = minor - base_minor`, `bit = frame_ofst`, where
`base_minor` is the minor field of `tile_base_far`. That
conversion requires `tile_base_far` to be defined as the FAR of
the tile's frame offset 0; that definition belongs in the
BitgenDevice implementation (see item 4 FLAG). Note: the vendor
report's citation of prjuray `utils/util.py: parse_tagbit` could
not be re-verified from the on-disk clone (no util.py exists in
prjuray-db-ref, checked with find). This ruling rests on the
keying argument and the frame ranges above, not on that citation.

### Item 3: SegbitsDb gains features(tile_type): PASS, approve the addition

`parse_bitstream` must invert the mapping, which needs
enumeration; `lookup` alone cannot enumerate (bitgen.rs:1215-1229
iterates `segbits.features(tile_type)` then `lookup`). The only
source of the feature list is the DB itself, so passing a
separate list would split the contract across two sources. The
addition is additive: no existing implementor breaks except
FileSegbitsDb, which must add it, and StubSegbitsDb already
implements it (bitgen.rs:1428). Return type `&[String]` matches
`tile_types()`. Merge requirement: FileSegbitsDb stores
per-tile-type sorted feature lists built at load, returns an
empty slice for an unknown tile type, and ships a test asserting
`features()` enumerates exactly the features `lookup()` resolves
(no missing, no extras). Approve the addition; INTERFACES.md
gains the method in the SegbitsDb block.

### Item 4: emit_bitstream takes &dyn BitgenDevice, not &dyn UltraScaleDevice: PASS, approve the trait

`UltraScaleDevice` is a struct (merged tip device.rs:56; day-1
INTERFACES.md line 20: `pub struct UltraScaleDevice { pub part:
String, /* grid, tiles */ }`), and `&dyn` on a struct does not
compile, so the INTERFACES.md signature as written cannot be
implemented. The alternative (turn UltraScaleDevice into a trait)
would refactor the day-1 device model used by pack, place, and
the router. `BitgenDevice` (bitgen.rs:451-470: part, idcode,
tile_type, tile_base_far, tile_names, config_order_fars) is
minimal and targeted, and it does not duplicate
`UltraScaleFabric` (fabric.rs:54: bels_at, wires, timing, no FAR
mapping methods). Approve the trait as the contract; INTERFACES.md
takes the `&dyn BitgenDevice` signature.

FLAG (integration, not a defect in bitgen-core): no real
implementation exists yet. DeviceFile holds the raw data
(tile_type_site_type_pairs, slr config order, idcode, counts) but
has no `BitgenDevice` impl and no `tile_base_far` method
(checked device_file.rs: no matches). The only implementation is
the provisional StubDevice in bitgen.rs tests. The emitter cannot
run on a real device file until the device-file worker (or
integration) implements `BitgenDevice` for `DeviceFile`, with
`tile_base_far` defined as the FAR of the tile's frame offset 0
(see item 2). This blocks FRAMING green on a real device, not the
bitgen-core merge.

### Item 5: FF INIT spelling: FAIL, both spellings non-canonical; FLAG polarity unverified, FF INIT blocked

What the DB says (proven, not assumed). The prjuray DB encodes FF
INIT as V0/V1 feature pairs for all 16 FFs in both tile types:
`CLEL_L.AFF.INIT.V0 12_02` and `CLEL_L.AFF.INIT.V1 !12_02`
(segbits_clel_l.db, same lines in segbits_clem.db; SRVAL the same
way). A grep for `AFF.INIT` in segbits_clel_l.db returns only
`.V0`/`.V1` lines. Neither the lut-ff-bram valued spelling
(`SLICE_XxYy.AFF.INIT = 1'b0/1`, bitloc.rs:670) nor the bitgen
parse-back bare spelling (`AFF.INIT` present means set, absent
means clear; bitgen.rs:1250-1275 renders bare when the bit reads
1 and drops the line when it reads 0) appears in the DB
vocabulary. The day-1 report's example (`AFF.INIT = 1'b0`,
report.md:170) and INTERFACES.md's example (bare `AFF.INIT`)
are not DB lines; they are invented examples and do not
establish the vocabulary.

The V0/V1 suffix does not uniformly encode the attribute value.
Counts from segbits_clel_l.db: V0-set 32, V0-zero-only 14, V1-set
14, V1-zero-only 32. For INIT and SRVAL, V0 sets the bit and V1
clears it; for CEUSED, CLKINV, and SRINV, V0 clears it and V1
sets it. So which of V0/V1 corresponds to INIT=1 is not
established by any on-disk source: the `014-cle-spec` fuzzing
spec is not in the ref clone (no spec files; only CONTRIBUTING,
COPYING, Info.md, Makefile, README.md, zynqusp), the paper
states only that INIT was set during fuzzing (no polarity),
demo.py locates the INIT bit without reading its value, and no
US+ reference bitstream exists on disk to test against (no .bit
files in bitfiltrator-ref, checked with find). A web search for
the V0/V1 semantics returned nothing usable and is not evidence.

Ruling: the canonical FASM spelling for FF INIT is the DB
vocabulary `.V0`/`.V1`, exactly one of the two features per FF,
selected by value. Both current spellings must change to it once
the value mapping lands. The value-to-feature mapping itself is
FLAG and BLOCKED: it needs a polarity experiment, either a
Vivado US+ bitstream containing an FF with known INIT=1 (tier-2
data, license-blocked) or a BitFiltrator demo.py run reading the
bit value at the located FAR and frame_ofst from a bitstream
with known FF INIT values. Until the mapping lands, FF INIT bits
must not be emitted on the assumption bit=value (SPEC hard rule:
never emit guessed bits). FF INIT stays blocked on this, not on
location data. The correctness critic session 4 PASS on the lut
emission covers locations only; this polarity gap is new, not a
contradiction of that verdict.

Latent defect in parse-back, found while checking this item
(bitgen.rs:1250-1266). For a feature whose bit list is all `!`
bits (n_set = 0), `present` is vacuously true (`all_set_bits_one`
over an empty set, bitgen.rs:1327), and the must-be-zero check
then ERRORS (MustBeZeroViolated) when the bit reads 1, instead
of treating the feature as absent. With CLEL segbits loaded,
parse-back would error on any bitstream containing an FF whose
INIT bit reads 1, and would emit `.V1` for every FF whose bit
reads 0. The same applies to every V0/V1 feature with zero set
bits (CEUSED.V0, CLKINV.V0, SRINV.V0). This is latent today
(CLEL DBs are not vendored or loaded yet) but must be fixed as
part of the V0/V1 reconciliation: presence must fold the
zero-bit test in (present = all set bits 1 AND all zero bits 0),
with the error path reserved for genuinely inconsistent input.
No code change in this session; the fix belongs to the
integration step, with a correctness-critic re-check.

### Item 6: Duplicate SegbitsDb/BitRef: PASS on the plan, with conditions; merge may proceed on it

Both definitions confirmed on disk. Merged tip segbits.rs:29-35
defines `BitRef { frame: u32, bit: u8, must_be_zero: bool }` with
constructors `set`/`zero` and a Display impl, and segbits.rs:67-72
defines the trait with `lookup` plus `tile_types` only.
bitgen.rs:411-416 defines `BitRef { frame: u32, bit: u16,
must_be_zero: bool }` with constructors `set`/`clear` and no
Display, and bitgen.rs:437-446 defines the trait with `lookup`,
`tile_types`, plus `features` (bitgen.rs:445). Correction to the
worker's hazard note: both definitions carry the same
`must_be_zero: bool` field, so there is no polarity difference
to unify; the real differences are the bit width (u16 wins, item
1), the constructor name (`zero` vs `clear`), the Display impl
(loader only), and `features()` (bitgen only).

The reconciliation plan is sound and the merge may proceed on
it, as an explicit reconciliation commit, not a blind merge:
keep the single definition in segbits.rs (the loader's home),
with u16 and `features()` added; delete the duplicate
definitions from bitgen.rs and rewire bitgen.rs to consume the
shared types; drop bitgen's lib.rs re-export of `BitRef` and
`SegbitsDb` (bitgen lib.rs:20-23) in favor of the merged tip's
segbits re-export (merged lib.rs:35); keep the loader's Display
impl; pick one constructor spelling (`set`/`zero` or
`set`/`clear`) and rename the other side's call sites. The
reconciliation commit must include the `features()` test from
item 3 and a test that FileSegbitsDb preserves DB file order
exactly, since the bitgen contract (bitgen.rs:439-440) requires
INIT lists ordered MSB-first and the loader is the only place
that order is established. Do not land both definitions: two
`BitRef` types in one crate would split the emitter and the
loader onto different types and nothing would unify.

### Required INTERFACES.md updates (orchestrator applies same-day)

1. SegbitsDb block: `bit` is u16 (already there); add that
`frame` is a frame offset relative to the tile base FAR, absolute
FAR = `tile_base_far + frame` as a u32 sum; add
`fn features(&self, tile_type: &str) -> &[String];` to the trait.
2. Bitgen API block: `emit_bitstream` and `parse_bitstream` take
`&dyn BitgenDevice`, not `&dyn UltraScaleDevice`, with the
`BitgenDevice` trait spelled out (part, idcode, tile_type,
tile_base_far, tile_names, config_order_fars).
3. FASM block: FF INIT canonical spelling is `.V0`/`.V1` per the
DB vocabulary, pending the polarity mapping (FLAG, item 5); the
valued (`= 1'b0/1`) and bare (`AFF.INIT`) spellings are
non-canonical and must not be emitted. The `AFF.INIT` example
in the FASM section must be corrected to the `.V0`/`.V1` form
once the mapping lands; until then it stays flagged, not
silently kept as an example.


## 2026-11-07 critic-correctness, session 5: diff-harness deliverable

Scope: harness at ~/workspace/ultra-bitgen-crew/harness/ (outside the
repo; worktree ~/workspace/helion-ultra-bitgen-diff clean at aee77e3,
zero repo edits). First run runs/20261107T220702Z/ (worker), plus my
own fresh re-run runs/20261107T221057Z/.

### PASS: diff harness verified (scoreboard, greedy queue, fail-loud)

- P1 tests, re-run by critic. Command: `python3 -m unittest discover
  -s tests` in the harness dir. Result: 18 tests, OK, 5.0s. Matches
  the worker's claim exactly.
- P2 full run, re-run by critic. Command: `python3 diff.py`, exit 0,
  40 records to runs/20261107T221057Z/diff.jsonl, tier tier1, emitter
  None. All 40 blocked, all diff_bits null, every record carries seed
  20261107, placement hash, tier, block, design, part, total_bits, and
  a missing field naming the exact gap. Placement hashes identical to
  the worker's four: blinky xcku3p
  b65979c112456c7049770e0fd1573939148c901606eef9f7a0b8a0f74fddacfb,
  blinky xcvu3p
  791ce94587b0ab2074fe211a91731584846b9a71cae189cee0d8a19ff4e9dffb,
  counter xcku3p
  1bf0ffe68ca860b2a3b7c05b56e79fc5e7f8e7949af49bb7504734a68ba012ea,
  counter xcvu3p
  74417af567417cdac387ade31bd081c6ea632f09556f5235a1a6ff7d6bd3e23c.
  Manifest cross_check: 4 LUTs x 64 bits, 0 mismatches; 4 FFs match
  True.
- P3 hand verification, re-run by critic. Command: `python3
  tests/hand_verify_lut.py`. Result: HAND VERIFICATION PASSED, hand
  diff_bits 3 = harness diff_bits 3. I read the script: it computes
  the 64 blinky A6LUT bits from the raw JSONs with json only (no
  harness imports for the computation), asserts col_major 5 and tile
  CLEL_R by hand, and applies the UG570 FAR layout by hand.
- P4 fake-emitter green/red, covered by the 18 re-run tests
  (test_fake_emitter_green_and_red): green LUT/FF/INT/FRAMING, IO
  blocked tile-relative; 3 flipped LUT bits gives LUT_INIT red
  diff_bits 3, FF stays green; corrupt parse gives FRAMING red;
  extra bit in an unused block gives DSP48E2 red diff_bits 1.
  Tamper test asserts exit 3 plus REJECTED; missing --reference-bit
  asserts exit 2; CLI full run asserts 40 blocked records, manifest
  tier1, 4 combos. I read the test code; the assertions match the
  claims.
- P5 interfaces: every JSONL record carries the INTERFACES.md
  contract fields (block, design, part, diff_bits, total_bits,
  status, seed, placement_hash, tier) plus coord_system and
  missing/detail where applicable. Blocked records carry diff_bits
  null, never zero. Greedy queues verified in my re-run output:
  blinky/xcku3p LUT_INIT 64, IO_CONFIG 55, INT_PIP 11, FF_INIT 1;
  counter/xcku3p LUT_INIT 256, IO_CONFIG 55, INT_PIP 17, FF_INIT 4;
  xcvu3p IO_CONFIG 55 first with LUT/FF/INT total 0 blocked on the
  missing device file. Matches the worker's queues exactly.
- P6 scope and fail-loud: every blocked record names its missing
  piece. Tier-2 slot verified by my own run: `python3 diff.py
  --design blinky --part xcku3p --reference-bit /tmp/dummy-ref.bit
  --out /tmp/tier2check`. Result: tier tier1+tier2, 10 records,
  manifest tier2 entry status blocked with missing "tier-2 diff
  needs the emitter to parse a Vivado bitstream (bitgen-core
  parse_bitstream on a Vivado .bit); no emitter is configured, so no
  tier-2 comparison ran". No tier-2 comparison faked. No emitter is
  ever invented: find_emitter checks --emitter-cmd then
  HELION_BITGEN_EMITTER only.
- P7 the two fixed bugs, verified in code. fake_emitter.py inserts
  the harness dir (not tests/) on sys.path, so `reference` resolves
  from the harness. The bits protocol prints `FAR OFFSET VALUE
  BLOCK` lines and diff_engine compares per block through
  emitted_bitset_for; regression tests cover both (FAKE_EXTRA_BIT_BLOCK
  asserts the unused-block red path).
- P8 provenance: prjuray-db pin
  affbc5e555ebae16475f32e8fb2d6565d4204f3f confirmed by git rev-parse
  in harness/reference/prjuray-db; COPYING opens "Creative Commons
  Legal Code / CC0 1.0 Universal". BitFiltrator pin
  79b9bd66ecae31354936143180fa578dc5732baf confirmed by git rev-parse
  in bitfiltrator-ref; harness/reference/LICENSE is the MIT License,
  Copyright (c) 2022 EPFL VLSC. ULTRASCALE_PLUS.json SHA-256
  c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c
  matches the bitfiltrator-ref source exactly; xcku3p.json
  07e6a7d3d586bf9ae3ad6c5d0959b669c5332af36f473ecd44e169c47512cfbe
  matches too. segbits_int.db 3778 lines, hpio 1560, cmt 104. No
  .dat, RapidWright, or Interchange files anywhere under harness/
  (find returned nothing).
- P9 cross-source agreement, spot-checked independently by critic
  (not the harness): prjuray CLEL_L.ALUT.INIT[0] is `11_15`,
  matching BitFiltrator LutLoc pair [0] (minor 11, offset 15);
  CLEL_L.AFF.INIT.V0 is `12_02` and V1 is `!12_02`, V0 sets and V1
  clears at the RegLoc position (12, 2); BRAM RAMB18E2_L.INIT_00[0..4]
  are 00_00/00_12/00_24/00_36/00_60, matching BramMemLoc Y0
  frame_ofst [0,12,24,36,60]; xczu3eg tilegrid INT_X11Y60 baseaddr
  0x00042300 (76 frames) vs CLEL_R_X11Y60 0x00042400 (16 frames),
  adjacent columns, confirming INT col = CLB col - 1. Raw
  segbits_int.db has no `#` comment lines, so parse_segbits_db needs
  no comment skipping for the pinned files.
- P10 hygiene: grep for em dash (U+2014) and en dash (U+2013) over
  harness *.py/*.md/*.json/*.fasm (excluding __pycache__, runs/,
  reference/) returns exit 1, zero matches. Repo worktree clean
  after my runs (git status empty, fm/ultra-bitgen-diff at aee77e3);
  my runs wrote only harness/runs/20261107T221057Z/ plus /tmp scratch.

Tier 2 stays blocked on the license server, unchanged since Oct 22.
No block goes green on this verdict: the first run is 40/40 blocked
(no emitter), which is the correct result, not a failure.

Observation (not a defect): the harness run lists INT_PIP blocked
(total 11/17) even though INT_PIP is green at tier 1 from critic
session 2. That is correct behavior, not a regression: the harness
has compared nothing yet (no emitter), and a blocked record must not
look like a pass. The harness will re-verify INT_PIP once the
emitter lands.

### FLAG: greedy queue fallback needs one line in INTERFACES.md (orchestrator, not a defect in this branch)

INTERFACES.md says only "The harness prints the blocks ordered by
diff_bits descending". The implementation orders by effective size
(diff_bits when the comparison ran, else total_bits, the size of the
work still standing), ties red before blocked before green, then
block name. That fallback is documented in the harness README
("Greedy queue") and in diff_engine.py, and is exercised by
test_greedy_order_largest_first (blocked total 64 outranks red diff
3). The fallback matches the greedy intent (largest work first: a
blocked block with 64 unconstrained bits outranks a red block with a
3-bit diff), so the implementation is consistent with the contract's
intent. But the contract of record does not state the null-diff_bits
rule. Add one line to INTERFACES.md recording it, so the contract
matches the implementation. No code change.

### FLAG: garbage emitter parse output crashes the run (fix before the first real emitter lands)

Confirmed by critic experiment, not assumed. An emitter whose parse
subcommand returns invalid FASM kills diff.py with a traceback and
no records at all. Commands: a shell emitter whose parse prints
"this is not fasm at all", run as `python3 diff.py --design blinky
--part xcku3p --emitter-cmd /tmp/critic5_badparse.sh --out
/tmp/critic5_badparse_run2`. Result: exit code 1, traceback ending
`fasm.FasmError: emitter parse output:1: no tile.feature separator
in line 'this is not fasm at all'`, from roundtrip.py
`parsed_canon = canonical_fasm(parsed_text, "emitter parse
output")`. The run dir contains only blinky-xcku3p/emit.bit and
roundtrip.bit; no diff.jsonl, no manifest.json. Root cause:
diff_engine.py diff_design calls roundtrip_mod.run_roundtrip
unwrapped, and roundtrip.py run_roundtrip calls emitter.emit,
emitter.parse, and canonical_fasm with no try/except, while the
first emit in diff_design IS wrapped (emit_error path). A broken
emitter therefore loses every record, including the blocked records
for combos that never ran. Fix: wrap the roundtrip body so
emit/parse/canonicalization failures return a FRAMING blocked
record (or red, "emitter parse output is not valid FASM") with the
error named, never a traceback. This cannot trigger today (no real
emitter exists; the fake emitter is well-formed), so it does not
block this PASS, but it must be fixed before the first real emitter
run.

## 2026-11-07 critic-correctness, session 6: bitgen-core deliverable (0f3aa2d)

Scope: commit 0f3aa2d on fm/ultra-bitgen-core (worktree
~/workspace/helion-ultra-bitgen-core, base aee77e3). Files:
crates/helion-ultrascale/src/bitgen.rs (1677 lines),
tests/bitgen_roundtrip.rs (248 lines), src/lib.rs. Every claim
below was checked by my own runs and reads, not by taking the
worker's report on trust. Worktree left clean after my runs
(git status empty).

### Verdict: FAIL, two concrete defects, both small and local

D1 (parse_bitstream, all-! valueless features) and D2
(emit_bitstream ending omits START and DESYNC), below. Fix both,
re-run tests, re-review. The core (FAR sequencing, CRC, round
trip on stub data, fail-loud paths, scope) is verified PASS, so
the re-review should be quick. Do not merge 0f3aa2d until D1 and
D2 are fixed. The SegbitsDb/BitRef reconciliation (interfaces
session 2 item 6) is the merge-time commit; the D1/D2 fixes ride
with it or just before it.

### PASS items, verified by my own runs

P1 Tests re-run: `cargo test -j2 -p helion-ultrascale` in the
worktree, 21 unit + 5 round-trip + 2 smoke = 28 passed, 0
failed, matching the worker's claim exactly (log
/tmp/critic-bitgen-test.log).

P2 CRC-32C, independently reproduced with my own Python
implementation of icap_crc from the prjxray description
(reflected poly 0x82F63B78, 37 bits per (addr, data)):
check 1, real Vivado vector 0x06b1285e reproduced from FDRI 123
zero words + FAR 1 from state 0, PASS; check 2, first-frame CRC
0x034886f2 reproduced from claimed pre-WCFG state 0x01e2bcae
feeding WCFG + FAR 0 + FDRI (exact payload parsed from
bitstream.py line 508, 0x10 at words 8, 18, 28, 38, 48, 58, 71,
81, 91, 101, 111, 121) + FAR 0, PASS, and an independent GF(2)
solve confirmed 0x01e2bcae is the unique pre-WCFG state
(residual 0); check 3, round-trip CRCs F2 0x9ac4dc29, F1
0xaa75db0b, F3 0x66924d7d all reproduced by independent Python
simulation of the full emit sequence, PASS; hand-computed words
reproduced (F2 word3 0x00000010; F1 words 0x00000001 /
0x80000000 / 0x00000001; F3 words 0x00000001 / 0x80000000).
The algorithm matches prjxray crc.h verbatim, read in both
local copies (prjxray-ref/tools/bits2rbt/crc.h and
prjxray-ref/lib/include/prjxray/xilinx/xc7series/crc.h).

P3 Scope G2: bitgen.rs never references the device.rs grid
(grep found only doc comments saying NOT to use it); the
emitter consumes only the BitgenDevice trait. Preamble
registers Vivado writes (TIMER, WBSTAR, POST_CRC, COR0, COR1,
SWITCH, MASK, CTL0, CTL1) appear only in the reg constants
module and doc comments; the emit path writes only CMD(RCRC),
IDCODE, CMD(WCFG), FAR, FDRI, CRC, CMD(RCRC), CMD(GRESTORE),
CMD(DGHIGH_LFRM). Confirmed not emitted, no guessed values.

P4 Frame-relative BitRef.frame: verified as the prjxray
convention, agreeing with interfaces session 2 item 2. The DB
is keyed by tile type only (one entry per tile type), frame
range 0..75 for segbits_int.db (verified by my own parse), and
the same INT tile type occurs at many tile positions with
different absolute FARs, so the frame number can only be an
offset from the tile base FAR. The merged segbits.rs loader
passes DB frame numbers straight through as BitRef.frame (no
transformation), so it already satisfies the relative form for
prjuray lines. BitFiltrator bit_locator.py confirms FARs are
built per tile from (row_major, col_major, minor) with
frame_ofst as a bit offset within the frame; frame.py confirms
bit N is word N/32, bit N%32, LSB-first. The worker's
conversion note (offset = far(minor) minus base FAR) applies
to BitFiltrator (minor, frame_ofst) pairs from bitloc.rs, not
to prjuray DB lines.

P5 Fail-loud is real: unknown feature/tile, duplicate FASM
line, value too wide, must-be-zero violation, CRC mismatch,
IDCODE mismatch all produce errors, covered by the re-run
tests and confirmed by reading the error paths.

P6 Hygiene: zero em dashes in bitgen.rs, bitgen_roundtrip.rs,
lib.rs (grep count 0). Clippy re-run: exactly 1 warning, the
pre-existing smoke_place vec! (F2); zero new warnings. fmt
--check: only 2 diffs, both pre-existing and untouched by
this commit (lib.rs:22 fabric re-export line,
tests/smoke_place.rs:67, same as session 2 found). Cargo.lock
diff is only the helion-ultrascale package entry. lib.rs diff
adds the bitgen module plus re-exports only.

P7 Padding frames and trailing NOOPs: NUM_END_OF_ROW_PADDING_
FRAMES = 2 verified in bitfiltrator-ref bitstream_spec.py:103;
the emitter does not emit padding frames explicitly, it emits
whatever the device config order lists, so that is a
device-file-side concern, not a defect here. Trailing NOOPs =
400 verified against the observed Vivado file
(zedboard_blinky.bit: 400 trailing NOOP words, DESYNC data
0x0000000D present).

### FAIL D1: parse_bitstream errors on real DB data for all-! valueless features

bitgen.rs:1250-1253, :1262, helper :1327. For a valueless
feature, `present` is `all_set_bits_one(...)` over the set
bits only. For a feature whose bit list is all `!` bits
(n_set = 0), that is vacuously true, and the must-be-zero
check then ERRORS (MustBeZeroViolated, :1262) when any of
its `!` bits reads 1, instead of treating the feature as
absent.

Proven against the real, vendored segbits_int.db (3778
features, 145 all-!), not the stub DB. The BOUNCE_E_0_FT1
mux has 7 features. `INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1`
sets 64_00. `INT.PIP.BOUNCE_E_0_FT1.VCC_WIRE` sets nothing
and requires !64_00 (among others). When a design routes
BOUNCE to INODE (the crew's own golden test design does
exactly this), parse_bitstream computes present=true for
VCC_WIRE (vacuous), finds 64_00 set, and returns
MustBeZeroViolated for the whole bitstream. So
parse_bitstream(emit_bitstream(f)) FAILS against the real
DB, breaking the INTERFACES.md exact-round-trip contract.
The stub tests pass only because the stub DB has no all-!
feature, so this defect is invisible to the worker's tests.

Second failure mode, same cause: for an untouched mux
(all-zero frames) the VCC_WIRE feature is present=true and
is emitted, so parse-back emits a VCC_WIRE line for every
INT tile even when the input FASM has none. Spurious lines
plus the error mode together break round-trip exactness.

Scale: all-! features exist in every vendored DB, not just
INT: INT 145/3778, CLEL_L 59/768, CLEM 85/829, HPIO
52/1560, HDIO_TOP 90/812, HDIO_BOT 96/828, CMT 56/104,
BRAM 57/270. Corroborated independently by interfaces
session 2 item 5, which found the same defect on the CLEL
side (FF INIT bit 1 errors; .V1 emitted for every FF bit 0)
for CEUSED.V0, CLKINV.V0, SRINV.V0.

Fix direction: presence for valueless features must fold
the zero-bit test in (present = all set bits 1 AND all zero
bits 0); a zero bit reading 1 means the feature is absent
(another selection holds), with the error path reserved for
genuinely inconsistent input. The fix must also render
.V0/.V1 in parse-back, not bare AFF.INIT (see FLAG F-D).
Add a test using the real segbits_int.db BOUNCE mux (or a
stub DB containing an all-! VCC_WIRE feature) proving
parse(emit(f)) == f for the routed case and the untouched
case.

### FAIL D2: emit_bitstream ending omits START and DESYNC

bitgen.rs:926-933. The ending emits RCRC, GRESTORE,
DGHIGH_LFRM, then 400 NOOPs. My packet walk of the observed
Vivado file in this workspace (zedboard_blinky.bit) shows
the real ending: RCRC, GRESTORE, DGHIGH_LFRM, about 100
NOOP words, START (CMD 5), FAR 0x03be0000, MASK 0x501,
CTL0 0x501, RCRC, DESYNC (CMD 13), 400 trailing NOOPs.
Both missing commands are documented UG570 table 9-22 codes
already defined in the worker's own cmd module (START=5 at
bitgen.rs:129, DESYNC=13 at :136) and require no guessed
values. DESYNC is the worker's own parse_frames end marker
(:1169), and BitFiltrator bitstream.py:265 defines a
bitstream boundary as a DESYNC packet, so the emitted stream
is unterminated by the worker's own convention. Without
START the configured device never runs the startup sequence
(DONE stays low). The header doc claim that the ending
mirrors the observed Vivado file (bitgen.rs:28-29, :867) is
inaccurate for these two commands.

Fix: emit START after DGHIGH_LFRM and DESYNC before the
trailing NOOPs; re-run tests. NOT flagged: the FAR/MASK/CTL0
writes after START in the observed file (7-series values
0x03be0000 / 0x501) are correctly omitted, US+ values
unknown. Exact NOOP counts between commands are tier-2
detail, not flagged.

### FLAG F-A: CRC_EXCLUDED_REGS provenance unverified, no observable effect

bitgen.rs:152, `CRC_EXCLUDED_REGS: [15, 18, 20, 21, 22]`,
comment cites "SIM_CONFIGE2 model, prjxray
bitstream_crc.py". I could not verify that citation from
any on-disk source: both local prjxray crc.h files contain
the algorithm only, no exclusion list, and bits2rbt
configuration_packets.cc GetFpgaConfigurationCRC feeds
EVERY non-NOP write packet in the head into the CRC with no
exclusions. No web search surfaced the bitstream_crc.py
source. No observable effect on this deliverable: the
emitter never writes those registers, real bitstreams never
write reserved registers, and all CRC checks in P2 were
verified independently. Needs a source citation
(SIM_CONFIGE2.vhd) or removal before tier-2 parsing is
trusted. Not a FAIL.

### FLAG F-B: base-FAR definition unproven until the device file lands

Agrees with interfaces session 2 item 4 FLAG. The u32 sum
base_far + frame carries through minor/column/row in FAR
increment order (interfaces item 2), but tile_base_far as
"FAR of the tile's frame offset 0" has no implementation yet
(no BitgenDevice impl for DeviceFile; only StubDevice).
Integration dependency, not a defect in this commit.

### FLAG F-C: INTERFACES.md deviations, ruled by interfaces session 2

u16 PASS; frame-relative PASS with contract update;
features() PASS approved with a merge requirement (test
that it enumerates exactly what lookup() resolves);
BitgenDevice trait PASS approved; duplicate SegbitsDb/BitRef
reconciliation plan PASS with conditions (single definition
in segbits.rs, u16 plus features(), drop bitgen's duplicate
and its lib.rs re-export). Orchestrator applies the
INTERFACES.md updates same-day per that session. No
further action from this session.

### FLAG F-D: FF INIT spelling, incorporated by reference from interfaces session 2 item 5

That session ruled FAIL on both current spellings (lut
valued `= 1'b0/1` and bitgen bare `AFF.INIT`); canonical is
the DB vocabulary `.V0`/`.V1`, exactly one of the two per
FF, and the value-to-feature mapping is BLOCKED pending a
polarity experiment (Vivado US+ bitstream with known INIT,
or a BitFiltrator demo.py run reading the bit value). Until
the mapping lands, FF INIT bits must not be emitted on the
assumption bit=value. The D1 fix must render .V0/.V1 in
parse-back.

### Ruling: int-pip src/dst direction, FASM is sink-first

The FASM convention IS sink-first (dst first). Evidence,
verified by my own reads: tile_type_INT.json pip key
`INT.INODE_E_1_FT1->>BOUNCE_E_0_FT1` with src_wire
INODE_E_1_FT1, dst_wire BOUNCE_E_0_FT1; the segbits DB
feature for that pip is
`INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1`, i.e.
`PIP.<dst_wire>.<src_wire>`; INT_PIP_AUDIT.md verified
empirically that dst.src matches 3770/3774 (plus 4
bidirectional via .FWD/.REV pairs) while naive src.dst
matches 0/3774. int_pip.rs `feature()` builds
`PIP.{src}.{dst}` and its doc says "a mux selection from
wire src to wire dst", but the worker's test fills
src=BOUNCE (physical dst) and dst=INODE (physical src). So
the emitted feature strings are correct and the INT_PIP
tier-1 green stands, but the `IntPip` field names are
inverted relative to physical direction and their own doc.
If the router-to-PIP translator ever fills src/dst in
physical direction, feature() would emit
`PIP.INODE.BOUNCE`, which is not in the DB, and emission
fails loud with UnknownFeature (fails safe, not silent).
This is a FLAG (rename or re-document the fields before the
translator is written), not a FAIL on emitted bits.

## 2026-11-07 critic-correctness, session 7: device-file deliverable (33a4b8f)

Scope: branch fm/ultra-bitgen-device, worktree
~/workspace/helion-ultra-bitgen-device, tip 33a4b8f (base a09df12).
Six commits: 91e5522 (G3 BRAM capacity), f378e75 (device-file loader),
ac22d8d (Cargo.lock sync), 5dcfe11 (fmt), 61012ff (demo.py
reconstruction validation on xcku3p), 33a4b8f (provisional xcvu3p.json).
Every claim below was checked by my own runs and reads, not by taking
the worker's report on trust. All probe runs were in a detached scratch
worktree (removed afterwards); the worker worktree is left clean at
33a4b8f (`git status --porcelain` empty).

### Verdict: PASS, with three FLAGs (none blocks the merge)

### PASS items, verified by my own runs

- **P1 Rebase is clean.** `git log --oneline a09df12..HEAD` shows exactly
  the six commits above; `git merge-base --is-ancestor a09df12 HEAD`
  passes. The duplicate F1 commit aece1c6 and duplicate F2 commit
  ad4da84 are gone from the log. `git diff a09df12 HEAD --
  crates/helion-ultrascale/src/bel.rs
  crates/helion-ultrascale/tests/smoke_place.rs` is empty: bel.rs and
  smoke_place.rs are exactly the merged tip. `git diff --stat
  a09df12 HEAD` is 18 files, all device-file lane (device_file.rs new,
  data files, scripts, tests, device.rs +50 which is only the G3 comment
  plus test, lib.rs +5 module wiring). No file outside the lane touched.
- **P2 Tests, clippy, fmt, all re-run by me at exactly 33a4b8f.**
  Command (detached scratch worktree, full cargo path, `-j2`):
  `~/.cargo/bin/cargo test -j2 -p helion-ultrascale`. Result:
  passed 63, failed 0, matching the worker's claim exactly (33 unit +
  3 demo + 3 xcvu3p + 2 int_pip + 20 lut_ff_bram + 2 smoke).
  `cargo clippy -j2 -p helion-ultrascale --all-targets`: exit 0,
  0 warnings. `cargo fmt -p helion-ultrascale -- --check`: exit 0,
  clean. Note: my first scratch run failed on disk space (`/tmp` is a
  512M tmpfs at 99%, the scratch target dir hit "No space left on
  device"); I moved the scratch worktree to the home disk and re-ran
  from a clean checkout. The failure was environmental, not a code
  failure, and the re-run is the one reported here.
- **P3 L/U naming and the NUM_CLB_PER_COLUMN fix.** bel.rs carries
  RAMB18E2_L (index 0) / RAMB18E2_U (index 1) only; no A/B string
  appears anywhere in bel.rs. `NUM_CLB_PER_COLUMN` is defined in both
  bitloc.rs:264 and device_file.rs:865 (both 60), and lib.rs re-exports
  only the bitloc copy (lines 23-26); the device_file re-export block
  (lines 29-32) omits it. device_file.rs uses its own copy internally
  (lines 947-954). The duplicate definition is ugly but unambiguous:
  each module resolves its own constant, and the crate compiles and
  tests clean.
- **P4 Cargo.lock sync (ac22d8d) and fmt (5dcfe11) are what they claim.**
  `git diff a09df12 HEAD -- Cargo.lock` is exactly one line: adding
  "serde" to the helion-ultrascale dependencies (serde_json was already
  there from the lut merge). The 5dcfe11 diff is pure reflows (line
  wraps, one import-order swap in lib.rs); I read the hunks, no tokens
  changed.
- **P5 Demo.py validation re-run by me, byte-identical.**
  Command: `python3 make_synth_xcku3p_bit.py` in the scratch worktree.
  Output reproduced the worker's verbatim: BELs 3, distinct FARs 261,
  `synth_xcku3p.bit` 100369 bytes, self-check 1 PASS (261 FARs, all
  marker bits set, byte offsets exact), self-check 2 PASS (demo.py
  exited 0, 18503 output lines, all 18,497 located bits match),
  ALL CHECKS PASSED. SHA-256 of all four regenerated artifacts
  (synth_xcku3p.bit 98e932b2..., synth_xcku3p_expected.json
  18726125..., demo_xcku3p_lut_ff.log fdca2b66...,
  demo_xcku3p_bram_summary.txt be9795b7...) equals the committed
  files exactly: the generator is deterministic and the committed
  artifacts are exactly what it produces.
- **P6 Loader hand-checks confirmed independently against the reference
  BitLocator** (bitfiltrator-ref/src, not the Rust code):
  SLICE_X23Y45/B6LUT bit 0 = 00003007 / 2271, bit 63 = 00003004 / 2256;
  SLICE_X23Y45/BFF = 0000300c / 2260; SLICE_X0Y0/AFF = 0000050c / 2;
  RAMB18_X0Y0 first mem FAR 01000000 / 0, 256 distinct mem FARs, 256
  distinct parity FARs. All match the Rust unit-test assertions, which
  passed in my run. Edge BELs resolve through the reference locator:
  SLICE_X112Y239/A6LUT (last column, last row) = 000ce20b / 2943,
  SLICE_X0Y239/HFF2 = 000c050c / 2974, RAMB18_X9Y23 mem first =
  01000900 / 2868, 16384 mem bits.
- **P7 The last-of-row limit is real.** I ran the reference
  `FrameAddressIncrementer.is_last_far_of_row` over the 261 distinct
  FARs from synth_xcku3p_expected.json myself: 0 last-of-row FARs.
  FAR 00003007 decodes as block CLB_IO_CLK, row 0, col 48, minor 7,
  matching the demo.py log line. The doc's recorded limit is accurate.
- **P8 Provisional xcvu3p.json: generator reproduces the file
  byte-identical.** I re-ran `python3 scripts/gen_xcvu3p.py` in the
  scratch worktree: output "slices: 49260 (header 49260)", "frames:
  73035 (UG570 Table 1: 71820; overshoot 1215)", "provisional notes:
  18", "self-checks PASS", and `cmp` against the committed file
  passed. The 18 `helion_provisional` notes are present (notes[0..17]),
  each with field, source, and value. Header counts confirmed:
  394,080 LUTs, 788,160 FFs, 720 RAMB36, 2280 DSP, 49,260 slices,
  1 SLR, part xcvu3p-ffvc1517-1-e, IDCODE 0x14B39093 (both the
  top-level value and slrs.SLR0.idcode). Structure confirmed: 7 rows,
  118 + 117 x 5 + 118 = 821 CLB columns, 821 x 60 = 49,260 slices;
  10 BRAM content columns and 19 DSP columns per row.
- **P9 Frame total recomputed independently from the file tables.**
  My own Python sum over the file (per row: sum of
  num_minors_per_std_colMajor plus sum of
  num_minors_per_bram_content_colMajor) gives xcvu3p 73,035 and
  xcku3p 41,460, matching the file claims exactly. The Rust
  `total_frames()` (device_file.rs:503) computes the same way from the
  tables, so the test's 73,035 assertion is a real cross-check, not a
  golden copied from the generator. The 1,215 overshoot against UG570
  Table 1 (71,820) is flagged in three places and never tuned away:
  the file note (total_frames source string), the generator header
  (lines 58-69), and data/README.md (lines 138-139, with the three
  hypotheses: row count wrong, per-row std table over-counts IO
  columns, or UG570 counts differently). The README also records that
  the 71,664 anticipation in the first draft is superseded.
- **P10 Sweep counts check out arithmetically.** xcku3p has 452 CLB
  columns; 452 x 31,680 (60 slices x 528 LUT/FF bits) = 14,319,360,
  exactly the worker's CLB sweep count. 40 BRAM columns x 24 sites x
  18,432 bits = 17,694,720, exactly the BRAM count. The structural
  guarantee behind the sweep (every FAR maps to a real tile) is
  enforced by `validate_structure()`, which passed in my run. I did
  not re-enumerate all 32M locations; the counts plus the structural
  check plus the edge-BEL spot checks in P6 are the evidence.
- **P11 Table-vs-header gaps are flagged, with correct numbers.**
  The file notes record: BRAM tables imply 840 sites vs header 720,
  DSP tables imply 3,192 sites vs header 2280. I verified from the
  template: xcku3p has 4 rows x 10 BRAM cols x 12 sites = 480 (header
  360) and 4 x 19 x 24 = 1824 DSP sites (header 1368); xcvu3p scales
  the same construction to 7 rows (840 and 3,192). The numbers are
  correct; the gap is a property of the copied template construction,
  flagged provisional, not hidden. The xcku3p README section records
  the same class (header LUT/FF 162,720 / 325,440 are 3/4 of the 8/16
  arch geometry over 27,120 slices) and the 41,460 vs 41,476 frame
  gap (16 frames).
- **P12 Hygiene.** Zero em dashes in any file changed in this branch
  (checked by byte scan over `git diff --name-only a09df12 HEAD`).
  No `/tmp` reads in device_file.rs. No RapidWright .dat or FPGA
  Interchange DBs ingested. The loader reads only paths its callers
  pass.

### FLAGs

- **F1: the 2-zero-padding path for last-of-row FARs is untested.**
  Verified 0 of 261 FARs is last-of-row (P7), so the generator branch
  that appends 2 zero padding frames after a last-of-row FAR has never
  executed. The doc records this honestly. Unblock: add one BEL whose
  FAR is last-of-row to the synthetic bitstream (the reference
  incrementer can identify one) and re-run. This does not block the
  merge: the generator is a validation tool, and the padding rule
  comes from the reference parser (`NUM_END_OF_ROW_PADDING_FRAMES =
  2`), but the path must be exercised before the generator is trusted
  for full-device bitstreams.
- **F2: make_synth_xcku3p_bit.py hardcodes the reference clone path**
  (`Path.home() / "workspace" / "helion-ultra-bitstream" /
  "bitfiltrator-ref" / "src"`, line 54). It fails loud
  (ModuleNotFoundError) if the clone is absent, never silently, but
  the script only runs from this workspace layout. Minor; a
  command-line `--ref-src` would fix it. gen_xcvu3p.py does not
  reference the clone (it builds from the vendored xcku3p.json).
- **F3 (carried, CTO): the slice-count discrepancy stays open.**
  This file uses 49,260 slices (394,080 LUTs / 8, XMP103) and flags
  the 49,250 vs 49,260 discrepancy in the num_slices note and in
  data/README.md; device.rs still says 49,250. My arithmetic confirms
  49,260 x 8 = 394,080 and 821 x 60 = 49,260, so the file is
  internally consistent; which count is ground truth is a CTO ruling,
  not a defect in this branch. The frame-total overshoot (P9) and the
  BRAM/DSP table-vs-header gaps (P11) likewise stay provisional until
  Vivado confirmation lands (license server refused since Oct 22).

### Merge readiness

This branch is ready to merge once the orchestrator sequences it. No
defect found in any of the six commits. The merge must take this
branch's bel.rs / smoke_place.rs (identical to the merged tip, P1) and
its data/ additions; the only known merge interaction is the
segbits-vendor branch, which also adds files under data/ (different
filenames; PROVENANCE.md consolidation is that branch's rebase item,
already delivered per COORD.md). The provisional xcvu3p.json must
never be presented as confirmed: it is flagged in the file, the
generator, and the README, and tier-2 confirmation stays blocked on
the Vivado license server.

## 2026-11-07 critic-correctness, session 9: lut-ff-bram session 2 deliverable (c0b3438)

Scope: branch fm/ultra-bitgen-lutff, worktree
~/workspace/helion-ultra-bitgen-lutff, tip c0b3438 (one commit on
c1bef40). Files: crates/helion-ultrascale/src/bitloc.rs,
crates/helion-ultrascale/tests/lut_ff_bram.rs (2 files, +359/-12).
Every claim below was checked by my own runs and reads, not by taking
the worker's report on trust. All probe runs were in a detached scratch
worktree at c0b3438 (removed afterwards); the worker worktree is left
clean at c0b3438.

### Verdict: PASS, no defects; one FLAG (cross-branch dependency, not a defect here)

The FF INIT/SRVAL emission is correct, the polarity chain is verified
from the fuzzer source by my own reads, the derived SRVAL delta rule
is verified against all four CLB DBs for all 16 FFs, the 128-line
fixture is byte-faithful to prjuray-db affbc5e, and the suite re-runs
56 passed, 0 failed with 0 clippy warnings. FF INIT is unblocked as a
question; it goes green only when this branch merges and the diff
harness runs against the bitgen-core emitter (see FLAG F1).

### P1: polarity chain re-verified from the fuzzer source

- `~/workspace/prjuray-tools-ref` (f4pga/prjuray, HEAD c550b03):
  `tools/dump_features.tcl:897-902` reads the cell INIT property
  (`set init_prop [get_property -quiet INIT $cell]`) and names the
  feature `"$bel_name.INIT.V[string range $init_prop end end]"`, so
  the V suffix IS the last character of the INIT attribute value:
  INIT 1'b0 gives V0, INIT 1'b1 gives V1.
  `utils/spec/flipflops.py:100` instantiates `.INIT(%s)` with a random
  choice of `"1'b0"` / `"1'b1"`, so both values are fuzzed.
  `fuzzers/014-cle-spec/Makefile:68, :81`: `design.features` is dumped
  from the post-route DCP by dump_features.tcl and correlated with
  `design_bits` by `create_segdata_from_features.py`, so the DB
  feature names are exactly the dump_features.tcl names.
- SRVAL side: dump_features.tcl:845-883 sets `srval 1` for FDPE /
  FDSE / LDPE and `srval 0` for FDCE / FDRE / LDCE, and :902 emits
  `"$bel_name.SRVAL.V$srval"`. In Xilinx semantics FDPE/FDSE/LDPE
  carry SRVAL 1 (preset/set) and FDCE/FDRE/LDCE carry SRVAL 0
  (clear/reset), so the V suffix names the SRVAL attribute value too.
- DB lines (vendored reference clone, checked this session):
  `CLEL_L.AFF.INIT.V0 12_02`, `V1 !12_02`;
  `CLEL_L.AFF.SRVAL.V0 12_01`, `V1 !12_01`. V0 sets the bit, V1
  clears it, for INIT and SRVAL alike. The mapping is therefore
  bit = NOT(INIT) and bit = NOT(SRVAL): INIT 0 selects .V0 (bit set),
  INIT 1 selects .V1 (bit clear). Interfaces session 2 item 5 and
  correctness session 6 FLAG F-D are answered; the polarity FLAG is
  closed. Note this also confirms the interfaces session 2 observation
  that V0/V1 does not uniformly encode the value across attributes
  (for CEUSED/CLKINV/SRINV the sense is reversed); the ruling here is
  for INIT/SRVAL only, which is what this branch emits.

### P2: emission spelling and placed-bit polarity

- `ff_init_fasm` (bitloc.rs) emits `SLICE_X{x}Y{y}.{name}.INIT.V0`
  (INIT 0) / `.INIT.V1` (INIT 1), V selected by value. `ff_placed_bit`
  records `value: !init` (bit = NOT(INIT)). The session-1 inverted
  values are corrected, and the corrected test
  `ff_init_zero_and_one` asserts INIT 0 sets (12, 2) and INIT 1
  clears it.
- `ff_srval_fasm` / `ff_srval_placed_bit` follow the same rule for
  SRVAL (.SRVAL.V0/.V1, value = !srval).
- The valued spelling (`AFF.INIT = 1'b0/1`) is gone from emission:
  the only remaining mentions in bitloc.rs are two doc comments
  stating it is non-canonical (bitloc.rs:51, :682). No other caller
  of these functions exists in src/ yet; integration wires them in.
- Scope is clean: the commit touches only bitloc.rs and the test
  file; LUT and BRAM emission are unchanged.

### P3: SRVAL delta rule verified against all four CLB DBs

The vendored BitFiltrator JSON carries no SRVAL table (RegLoc is
INIT only), so `ff_srval_location` derives SRVAL from INIT by a
within-slice delta: -1 for A/C/E/G letters, +1 for B/D/F/H letters.
The code expresses this as `(ff_index / 2) % 2 == 0 -> -1 else +1`,
where the letter index is `ff_index / 2` (0=AFF, 1=AFF2, 2=BFF, ...,
15=HFF2), which is exactly the claimed rule. I verified it
independently with a script over all four DB files
(segbits_clel_l.db, segbits_clel_r.db, segbits_clem.db,
segbits_clem_r.db) for all 16 FFs: V0 sets / V1 clears for INIT and
SRVAL, SRVAL bit = INIT bit + delta with the same frame, zero
problems in all four files. Spot values: BFF INIT 4 / SRVAL 5,
DFF 10/11, FFF 28/29, HFF 34/35 (+1); AFF 2/1, CFF 8/7, EFF 26/25,
GFF 32/31 (-1). The Y30 pin holds from the vendored table:
CLEL_L RegLoc Y_ofst 30 AFF frame_ofst 1538 (Y29->Y30 jump 144 = 48
per slice + 96 clock-region extra; normal step 48), so AFF SRVAL at
Y30 is 1537, as the test pins. Out-of-range results fail loud
(`BitLocError::TableCorrupt`) instead of wrapping; the guard is
defensive (current table range 1..47 at Y0, 1537..2931 at Y59, never
out of range) and the fail-loud behavior is the right shape.

### P4: 128-line fixture is byte-faithful to prjuray-db affbc5e

I extracted `FF_SEGBITS_FIXTURE` from tests/lut_ff_bram.rs and
compared it line by line against segbits_clel_l.db and
segbits_clem.db in the reference clone (HEAD
affbc5e555ebae16475f32e8fb2d6565d4204f3f): 128 lines, zero lines
missing from the DB, zero DB lines of this class omitted from the
fixture (64 CLEL_L + 64 CLEM). The fixture is a verbatim copy of
CC0 data with source and commit cited in its comment; the CLEL DBs
are not vendored in the repo, so the excerpt is the test's copy.
The test `ff_init_srval_resolve_through_segbits` loads it via
`FileSegbitsDb::from_text` (128 features) and, for all 16 FFs in
CLEL_L and CLEM, proves: emitted .V0 resolves to
`BitRef::set(12, frame_ofst)` and .V1 to `BitRef::zero(12,
frame_ofst)` for INIT and SRVAL, placed bits match the same
polarity, and the table frame_ofst equals the DB bit (the lookup
expectation is built from the table location, so equality is the
cross-check). The RegLoc table is therefore checked against the DB
for all 16 FFs, not asserted from itself.

### P5: tests, clippy, fmt re-run by me

Detached scratch worktree at c0b3438 (removed afterwards):
`~/.cargo/bin/cargo test -j2 -p helion-ultrascale` exit 0:
25 unit + 2 int_pip + 1 int_pip_audit + 22 lut_ff_bram + 4 segbits
+ 2 smoke = 56 passed, 0 failed, matching the worker exactly.
`cargo clippy -j2 -p helion-ultrascale --all-targets` exit 0, 0
warnings. `cargo fmt -p helion-ultrascale -- --check` exit 0,
clean. Zero em dashes and zero en dashes in both changed files
(checked by Python scan).

### FLAG F1: FF INIT green depends on the harness canonicalization, not just this merge

The worker's consistency note is stale: it says the bitgen-core D1
parse-back "renders .V0 for a set INIT bit and drops .V1 as
unobservable". Commit 88070a5 on fm/ultra-bitgen-core added the
exact-complement rule, so parse-back now renders .V1 when the bit
reads 0. I read the rule (bitgen.rs:1242-1312): an all-`!` feature
renders iff its zero signature is in the set of set-bit signatures
of same-tile-type features listing no `!` bits; for
AFF.INIT.V1 the zero signature [(12, 2)] matches AFF.INIT.V0's set
signature [(12, 2)], so it renders, while VCC_WIRE (six zero bits,
no single matching feature) is still dropped. The two sides agree
on the observable boundary: emit INIT 0 -> .V0 -> bit set ->
parse-back renders .V0; emit INIT 1 -> .V1 -> bit clear ->
parse-back renders .V1. So parse(emit(f)) == f holds for FF INIT
across the two branches. But the end-to-end round trip is still to
be run after both branches merge, and the CTO's complete-FASM
ruling (unspecified FF INIT defaults to V0, harness canonicalizes)
is being implemented by diff-harness session 3 and the integration
worker; FF INIT goes green only on that harness run, not on this
review alone.

### Base note (not a defect)

The branch is based on c1bef40 while fm/ultra-bitstream is now at
9289fed (device-file merge). The rebase comes at merge time and is
not part of this review. The device-file merge changed no file this
branch touches except Cargo.toml / data/README.md (auto-merge
territory), so no conflict is expected beyond the routine rebase.

## 2026-11-07 critic-correctness, session 8: bitgen-core re-review (0f3aa2d..88070a5)

Scope: branch fm/ultra-bitgen-core, worktree
~/workspace/helion-ultra-bitgen-core, tip 88070a5. Four session-2
commits on 0f3aa2d: 9e32ed6 (D1 fix), 049542c (D2 fix), dd4910f
(F-A removal), 88070a5 (polarity complement rule). Files:
crates/helion-ultrascale/src/bitgen.rs (+143/-30 over the range),
tests/bitgen_roundtrip.rs (+261/-0). Every claim below was checked
by my own runs and reads, not by taking the worker's report on
trust. All probe runs were in a detached scratch worktree
(~/workspace/.critic-s8, removed afterwards); the worker worktree
is left clean at 88070a5 (`git status --porcelain` empty).

### Verdict: PASS, no defects. D1, D2, F-A, and the polarity rule all verified.

The session-6 FAILs are fixed. Merge readiness: ready to merge
once the orchestrator sequences the SegbitsDb/BitRef reconciliation
commit (interfaces session 2 item 6, still the merge-time item and
not part of this branch). The branch still carries its own
SegbitsDb/BitRef definitions duplicating the merged segbits.rs;
that is the known reconciliation item, not a new defect.

### PASS items, verified by my own runs

- **P1 Tests, clippy, fmt, all re-run by me at exactly 88070a5.**
  Command (detached scratch worktree, full cargo path, `-j2`):
  `~/.cargo/bin/cargo test -j2 -p helion-ultrascale`. Result:
  passed 32, failed 0, matching the worker's claim exactly (21
  unit + 9 round-trip + 2 smoke). `cargo clippy -j2 -p
  helion-ultrascale --all-targets`: exit 0, exactly 1 warning,
  the pre-existing smoke_place vec! (F2); zero new warnings.
  `cargo fmt -p helion-ultrascale -- --check`: only the 2
  pre-existing diffs (lib.rs:22, tests/smoke_place.rs:67), both
  untouched by this branch. Zero em dashes and zero en dashes in
  bitgen.rs and bitgen_roundtrip.rs (byte scan).
- **P2 D1 verified against the REAL vendored DB, not the stub.**
  My own probe (tests/critic_s8_probe.rs in the scratch worktree
  only, deleted with it) parses the real
  `~/workspace/prjuray-db-ref/zynqusp/segbits_int.db` (3778
  features) and `segbits_clel_l.db` (768 features) with a parser
  written for this probe, builds a StubSegbitsDb from them, and
  checks emit/parse. First, the worker's stub DB is faithful: I
  read the real BOUNCE lines myself and they match the stub line
  by line (`INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 64_00 !66_00
  !68_00 !72_00 !72_01 !74_00`, plus the other five selections
  and VCC_WIRE). Then the probe: INT-only device, routed BOUNCE
  round-trips exactly; INT-only device, empty FASM round-trips
  to `""` (no spurious VCC_WIRE line); two-tile device, routed
  BOUNCE plus AFF.INIT.V0 plus BFF.INIT.V1 parses back to
  exactly the 58-line FASM I computed independently in Python
  from the same DB files (1 INT line, 57 CLEL_L lines, no
  VCC_WIRE, no bare AFF.INIT). All 4 probe tests pass at
  88070a5. Pre-fix control: I checked out dd4910f in the scratch
  worktree and re-ran the probe: 3 passed, 1 failed, the 58-line
  test returning only the 2 lines with set bits
  (INODE_E_1_FT1, AFF.INIT.V0). That pins the D1 fix (9e32ed6)
  as load-bearing for the INT cases and the complement rule
  (88070a5) as load-bearing for the polarity rendering, exactly
  as the worker claimed.
- **P3 D1 present rule reads correctly and matches the probe.**
  bitgen.rs:1290-1315: valueless presence is all set bits 1 AND
  all zero bits 0 AND (n_set > 0 OR exact complement). A `!` bit
  reading 1 means absent, not an error; the MustBeZeroViolated
  path is reserved for valued features, where presence is
  any-set. The complement rule is narrow: an all-`!` feature
  renders only when its zero signature exactly equals the
  set-bit signature of another feature of the same tile type
  that lists no `!` bits. My Python enumeration of the CLEL_L
  DB confirms the boundary cases behave: CARRY8.CI and
  CI_TOP (V0 `!10_18 !14_01`, V1 `10_18 !14_01`, sharing one
  `!` bit) have no complement and do not render, while the
  MODE.FF/LATCH and SYNC.ASYNC/SYNC pairs (not .V0/.V1, but
  exact complements) do render. VCC_WIRE has no complement (no
  single feature sets exactly its six bits) and is dropped.
- **P4 D2 verified by an independent packet walk, not the test
  assertions.** The probe wrote the emitted bitstream to
  /tmp/critic-s8-probe.bit; my own Python packet walk (type-1
  header decode, no reuse of the Rust parse code) finds 1033
  words with ending: RCRC (CMD 7) at word 620, NOOP 622-623,
  GRESTORE (CMD 10) at 624, NOOP 626, DGHIGH_LFRM (CMD 3) at
  627, START (CMD 5) at 629, DESYNC (CMD 13) at 631, then
  exactly 400 trailing NOOPs (633..1032). The stream parses
  (parse_bitstream returned the expected FASM in the probe), so
  parse_frames accepts the new ending and DESYNC terminates it
  by the module's own convention. The worker's D2 test asserts
  the same ending words directly and passes in my run. The two
  documented omissions stand and are not flagged: the ~100
  NOOPs Vivado emits between DGHIGH_LFRM and START are not
  reproduced (tier-2 detail, stated in the code comment), and
  the FAR/MASK/CTL0 writes after START are not emitted
  (7-series values 0x03be0000 / 0x501 are not UltraScale+
  values), exactly as session 6 ruled.
- **P5 F-A removal is correct and complete.** `git diff
  0f3aa2d 88070a5` shows `CRC_EXCLUDED_REGS` and its
  `contains` check deleted (dd4910f, bitgen.rs only, 12
  insertions / 12 deletions); grep finds no live reference
  left, only the module-doc sentence recording why it was
  removed. The CRC vectors session 6 reproduced independently
  (F2 0x9ac4dc29, F1 0xaa75db0b, F3 0x66924d7d) are still
  asserted by `bitstream_has_expected_length_sync_headers_and_
  crcs` and pass in my run, so no test outcome changed. The
  emitter never writes registers 15/18/20/21/22, so the
  removal has no observable effect on emitted streams; it
  changes only the CRC treatment of foreign bitstreams that
  write those registers, which is the intended bits2rbt
  semantic (feed every non-NOP write).
- **P6 Polarity chain re-verified by me from the source, not
  the researcher's report.** prjuray clone
  ~/workspace/prjuray-tools-ref at c550b03:
  tools/dump_features.tcl:897-902 reads `set init_prop
  [get_property -quiet INIT $cell]` then names
  `"$bel_name.INIT.V[string range $init_prop end end]"`, so
  the V suffix IS the INIT attribute value (INIT 1'b0 gives
  V0, 1'b1 gives V1); utils/spec/flipflops.py:100 instantiates
  `.INIT(1'b0)` or `.INIT(1'b1)`; fuzzers/014-cle-spec/Makefile
  dumps design.features from the post-route DCP via
  dump_features.tcl and design_bits via bit2fasm, which is the
  correlation step tying feature names to observed bits; the
  srval assignment (1 for FDPE/FDSE/LDPE, 0 for FDCE/FDRE/LDCE)
  matches dump_features.tcl:848-883. Vendored DB lines read by
  me: `CLEL_L.AFF.INIT.V0 12_02`, `V1 !12_02`,
  `CLEL_L.BFF.INIT.V0 12_04`, `V1 !12_04`,
  `CLEL_L.AFF.SRVAL.V0 12_01`, `V1 !12_01`, and
  `CLEL_L.ABCDFF.CEUSED.V0 !14_07`, `V1 14_07` (V0 clears,
  V1 sets, confirming V names the value, not the bit
  polarity). bit = NOT(INIT) and bit = NOT(SRVAL) follow:
  V0 sets the bit and is selected by value 0; V1 clears the
  bit and is selected by value 1. The new polarity test
  (`ff_init_polarity_renders_v0_for_bit_one_and_v1_for_bit_
  zero`) passes in my run, and my 58-line probe confirms the
  same rendering against real DB data (bit 1 renders .V0,
  bit 0 renders .V1, no bare AFF.INIT). Interfaces session 2
  item 5 and session-6 FLAG F-D are answered and closed.
- **P7 Scope.** `git diff --stat 0f3aa2d 88070a5` is 2 files
  only (bitgen.rs, bitgen_roundtrip.rs), 374 insertions, 30
  deletions. No file outside the bitgen lane touched. No
  RapidWright or Interchange ingestion. No guessed values:
  START/DESYNC are documented UG570 table 9-22 codes already
  defined in the worker's cmd module.

### FLAGs (none blocks the merge)

- **F1 (carried, harness/integration): complete FASM is now
  required for exact round-trip on real device data.** The
  complement rule renders exactly one line per binary
  attribute per tile in the device (my probe: a 3-line FASM
  parses back to 58 lines on the real CLEL_L DB). The module
  doc states this consequence honestly. The CTO's canonical-
  FASM ruling (complete FASM, unspecified FF INIT defaults
  to V0) is the answer; the harness round-trip check and the
  integration FASM generator must apply it. Not a defect in
  this branch.
- **F2 (minor, doc): stale polarity comment in the test
  file.** tests/bitgen_roundtrip.rs:38 still says the
  value-to-feature polarity mapping "is still BLOCKED on a
  polarity experiment". The polarity is resolved (P6) and
  the test itself pins .V0/.V1 with the resolved meaning.
  One-line doc fix; do not let it ride as a code change
  without the merge-time reconciliation commit.
- **F3 (carried from session 6): base-FAR definition
  unproven until a real BitgenDevice impl lands.** No
  change: only StubDevice exists. Integration dependency,
  not a defect in this commit.

## 2026-11-07 critic-correctness, session 11: bitgen-core reconciliation re-review (9289fed..5e26453)

Scope: branch fm/ultra-bitgen-core, worktree
~/workspace/helion-ultra-bitgen-core, tip 5e26453. Rebase of the 5
session-1/2 commits onto 9289fed (ec5d198, 4713fef, 3447b9b,
a22b471, ace0994) plus the reconciliation commit 5e26453
(interfaces session 2 item 6). Files in the reconciliation
commit: crates/helion-ultrascale/src/bitgen.rs,
src/lib.rs, src/segbits.rs, tests/bitgen_roundtrip.rs (4 files,
+143/-95). Every claim below was checked by my own runs and
reads, not by taking the worker's report on trust. All probe
runs were in a detached scratch worktree (~/workspace/.critic-s11,
removed afterwards); the worker worktree is left clean at
5e26453 (`git status --porcelain` empty).

### Verdict: PASS, no defects. Ready to merge.

The interfaces item 6 reconciliation is complete and correct.
Merge readiness: the orchestrator may merge fm/ultra-bitgen-core
into fm/ultra-bitstream (tip 7f80b54). `git merge-tree
--write-tree 7f80b54 5e26453` exits 0 with a conflict-free tree
(a34a9250...). Expected post-merge test count is 94, confirmed
by test-name sets (see P8).

### PASS items, verified by my own runs

- **P1 Tests, clippy, fmt, all re-run by me at exactly 5e26453.**
  Command (detached scratch worktree, full cargo path, `-j2`):
  `~/.cargo/bin/cargo test -j2 -p helion-ultrascale`. Result:
  passed 92, failed 0 (48 unit + 9 round-trip + 3 demo + 3
  xcvu3p + 2 int_pip + 1 audit + 20 lut_ff_bram + 4 segbits +
  2 smoke), matching the worker's claim exactly.
  `cargo clippy -j2 -p helion-ultrascale --all-targets`:
  exit 0, 0 warnings. `cargo fmt -p helion-ultrascale
  -- --check`: exit 0, clean. Zero em dashes and zero en
  dashes in all four changed files (byte scan).
- **P2 Rebase is clean and faithful.** `git log 88070a5..5e26453`
  shows the 5 replayed commits plus 5e26453. The replayed
  content is byte-identical to the critic-passed originals:
  `git diff 88070a5 ace0994 --
  crates/helion-ultrascale/src/bitgen.rs
  crates/helion-ultrascale/tests/bitgen_roundtrip.rs` is empty,
  and neither file appears in the `88070a5 ace0994 --stat`
  list. The two rebase conflicts resolve as the union of both
  sides: Cargo.lock at 5e26453 carries the base entry
  (`helion-ultrascale` with `serde`/`serde_json` deps, which
  supersedes the no-deps entry), and lib.rs at ace0994 adds
  `pub mod bitgen;` plus the bitgen re-export block in
  alphabetical module order, keeping both sides' modules.
  `git diff 9289fed 5e26453 --stat` is 4 files, all additions
  (bitgen.rs +1743, lib.rs +5, segbits.rs +82,
  bitgen_roundtrip.rs +492): the rebase brought in only the
  bitgen lane, nothing else.
- **P3 Single definition, verified by grep, not by the commit
  message.** Exactly one `pub struct BitRef` in the crate
  (src/segbits.rs:40) and one `pub trait SegbitsDb`
  (src/segbits.rs:78). No `struct BitRef` or `trait SegbitsDb`
  remains in src/bitgen.rs. bitgen.rs:100 consumes the shared
  types via `use crate::segbits::{BitRef, SegbitsDb};`, and
  src/int_pip.rs:27 does the same. `StubSegbitsDb`
  (bitgen.rs:1456) implements the shared trait, with
  `features()` at :1494.
- **P4 `features()` implemented per the interface merge
  requirement.** `FileSegbitsDb` builds per-tile-type sorted
  feature lists at load (segbits.rs:165, :222-231) and
  returns an empty slice for unknown types (:262-266).
  The new test `features_enumerates_exactly_what_lookup_
  resolves` proves the enumeration against the vendored DB:
  I counted the vendored segbits_int.db myself with an
  independent script (3778 non-comment lines, single tile
  type INT, 3778 unique features), so the test's
  `features("INT").len() == 3778 == feature_count()` is a
  genuine check, not a number copied from the implementation.
  Total vendored features 104 + 828 + 812 + 1560 + 3778 =
  7082, matching the `all_vendored_dbs_load` assertion.
- **P5 File-order preservation is real and the fixture is
  faithful.** `lookup_preserves_db_file_order` pins
  out-of-order bits (12_02 12_00 12_01 returned in exactly
  that order) and the BYPASS line. I read the real DB line
  myself: `INT.PIP.INODE_E_1_FT1.BYPASS_E4 !57_00 !59_00
  !61_00 65_01 !67_01 !69_01`, which matches the FIXTURE
  and the test's expected refs exactly, including the `!`
  tokens. The MSB-first INIT contract survives the deleted
  trait doc: it is stated in bitgen.rs module docs (:54,
  :57, :751-752, :1453) and the roundtrip test header (:27),
  and pinned by this test.
- **P6 Constructor spelling applied consistently.**
  `set`/`zero` picked, `clear` dropped. At ace0994,
  `BitRef::clear` occurs 42 times (1 in bitgen.rs, 41 in
  bitgen_roundtrip.rs); at 5e26453, zero `BitRef::clear`
  occurrences remain anywhere in src/ or tests/, and the
  42 sites are `BitRef::zero` (the other `zero` counts at
  5e26453 are pre-existing loader-side uses). The reason is
  recorded where it belongs: the bitgen.rs comment at the
  deletion site, the commit message, and the COORD entry.
- **P7 FRAME_BITS collision fix is correct.** lib.rs drops
  `BitRef`, `SegbitsDb`, and `FRAME_BITS` from the bitgen
  re-export in favor of the segbits/bitloc re-exports; the
  crate root keeps bitloc's `FRAME_BITS: u32`. I grepped
  for root-name consumers (`crate::FRAME_BITS`,
  `helion_ultrascale::FRAME_BITS`, `use .*FRAME_BITS`):
  none outside bitgen.rs/bitloc.rs. bitgen.rs keeps its own
  module-level `pub const FRAME_BITS: usize` (:122), used
  internally at :721/:724 only. The intermediate E0252
  failure the worker reports is consistent with this fix
  (the double re-export) and is gone in the final state.
- **P8 Post-merge count 94 confirmed by test-name sets, not
  by adding the two totals.** `cargo test -- --list` in the
  base worktree (7f80b54): 72 names. Same in the scratch
  worktree (5e26453): 92 names. Union: 94 names.
  Intersection 70. Base-only: the 2 lut_ff_bram tests from
  the lut-ff merge (`ff_init_srval_resolve_through_segbits`,
  `ff_srval_fasm_and_location`). Branch-only: 22 (11
  bitgen unit + 9 round-trip + 2 segbits unit, including
  the two new tests). No name collisions, no losses: the
  merge takes the base's tests/lut_ff_bram.rs (22) and the
  branch's bitgen files, which is exactly the union.
- **P9 F2 stale comment fixed.** tests/bitgen_roundtrip.rs:38
  now states the resolved polarity (bit = NOT(INIT), bit =
  NOT(SRVAL); V0 selected by value 0 and sets the bit, V1 by
  value 1 and clears it). The roundtrip-file diff is only
  the comment fix, the import change (`use
  helion_ultrascale::BitRef;`, BitRef dropped from the
  bitgen import), and the 41 renames: 11 non-rename changed
  lines, all accounted for.
- **P10 Scope.** The reconciliation commit touches only the
  4 files named above. No file outside the bitgen lane
  touched. No RapidWright or Interchange ingestion. No
  guessed values. No semantic change to emission or
  parsing: the reconciliation is a type move plus a rename,
  and the full suite (including the session-8 D1/D2
  round-trip tests) passes unchanged at 92.

### FLAGs (none blocks the merge)

- **F1 (carried, orchestrator sequencing): the integration
  branch's bitgen.rs must not overwrite this tip.** The
  integration worker's branch (fm/ultra-bitgen-integ)
  carries bitgen.rs at 0f3aa2d plus its own weaker D1 fix
  (7365f2e), without the complement rule, D2, or F-A
  removal. The merge-tree check above covers only
  fm/ultra-bitstream (7f80b54) as the target; when the
  integration branch merges, the orchestrator must take
  this branch's bitgen.rs (5e26453) and the reconciled
  SegbitsDb, not the integration branch's copies. Not a
  defect in this branch.
- **F2 (carried from session 8): base-FAR definition
  unproven until a real BitgenDevice impl lands.** No
  change: only StubDevice exists. Integration dependency,
  not a defect in this commit.
- **F3 (carried from session 8): complete FASM required
  for exact round-trip on real device data.** The
  canonical-FASM ruling (complete FASM, unspecified FF
  INIT defaults to V0) is being implemented by
  diff-harness session 4 and the integration worker.
  Not a defect in this branch.


## 2026-11-07 critic-correctness, session 10: integration branch review (3e6d17f)

Scope: branch fm/ultra-bitgen-integ, worktree
~/workspace/helion-ultra-bitgen-integ, tip 3e6d17f, 8 commits on
base aee77e3. Diff stat vs base: 26 files, 6906 insertions, 34
deletions. Every claim below was checked by my own runs and reads,
not by taking the worker's report on trust. All probe runs were in
detached scratch worktrees (~/workspace/.critic-s10 at 3e6d17f and
~/workspace/.critic-s10-base at aee77e3 for the fmt baseline, both
removed afterwards); the worker worktree is left clean at 3e6d17f
(`git status --porcelain` empty).

### Verdict: PASS on correctness and scope, with three must-fix hygiene items before the orchestrator merges

No correctness defect found in this branch's code. All seven
brief items verify on my own runs except the hygiene half of item
7: the worker's "clippy 0 warnings" and "fmt clean" claims are
both incorrect (FLAGs F1, F2 below). The branch is NOT directly
mergeable as-is: it must not overwrite the bitgen-core tip
88070a5 bitgen.rs, and the SegbitsDb/BitRef reconciliation
(bitgen-core session 3) is the merge-time item. The
orchestrator's merge sequence (take 88070a5 plus the reconciled
segbits plus this branch's non-bitgen files) is confirmed correct
and sufficient for the known divergences.

### PASS items, verified by my own runs

- **P1 Tests: 77 passed, 0 failed, re-run by me at exactly
  3e6d17f.** `cargo test -j2 -p helion-ultrascale` in the scratch
  worktree: 63 unit plus 5 bitgen_roundtrip plus 2 int_pip plus 2
  corpus_legality plus 2 smoke plus 3 sta_sanity plus 0 doc =
  77 passed, 0 failed. All 8 fasm_gen tests pass by name:
  unspecified_ff_init_emits_v0_line,
  specified_ff_init_one_emits_v1_line,
  mixed_specified_values_pick_v_per_attribute,
  placed_site_is_used_not_packed_name,
  ff_bel_names_cover_all_sixteen, multiple_ffs_emit_in_bel_order,
  pack_place_then_generate_emits_v0_for_unspecified_ff,
  placed_index_out_of_range_fails_loud.
- **P2 Staging port is faithful.** Diffed each ported file
  against its day-1 staging source by hand. pack.rs differs
  from fm/ultra-packplace staging by exactly the three
  documented corrections: header corrected to one slice per CLB
  (the staging header said two, which is the 7-series layout);
  per-LUT slice-kind counting removed in two places;
  per-slice counting added at the end. place.rs and legality.rs
  are byte-identical to staging. router.rs vs the
  router-scaffold lib.rs: only the WireDb trait moved to
  fabric.rs and the `gen` field renamed to `generation`
  (the `visited_gen` array name is unchanged). mock.rs: import
  change plus the wire_info None impl plus its test. sta.rs is
  byte-identical to staging; timing_db.rs is identical plus
  the `impl crate::fabric::TimingDb` adapter (has_data true).
  corpus_legality.rs: import paths plus a workspace_example
  path fallback replacing a hardcoded /home/hatch path.
  sta_sanity.rs: import paths only.
- **P3 Gate 2 reproduced exactly by my own runs.** Built
  helion-cli from the scratch worktree and ran all four
  `helion impl` commands (blinky/counter x xcku3p/xcvu3p,
  seed 0). All four pass SYNTH, PACK, PLACE, LEGALITY, FF
  FASM, and preliminary STA, then fail loud at ROUTE with
  exit 1 and no .bit written (verified: no fresh .bit file
  anywhere in the scratch worktree). Exact lines, identical
  for both parts: blinky `luts=1 ffs=1`, `pack slices=1
  slicel=1 slicem=0`, `place slices=1 seed=0`, `legality ok
  violations=0`, `fasm ff_lines=2`, `timing WNS_PS=9650
  TNS_PS=0 endpoints=1 r2r_ps=270`; counter `luts=4 ffs=4`,
  `pack slices=1`, `fasm ff_lines=8`, `timing WNS_PS=9650
  TNS_PS=0 endpoints=4 r2r_ps=270`. WNS_PS=9650 matches the
  day-1 gold value. The ROUTE error text is exact:
  "ULTRA ROUTE: WireDb is empty (no routing graph;
  routing-graph worker owns it). Clock-net routing (phase 2)
  and IOB placement are also pending; see COORD.md gate."
  cmd_impl prints it to stderr and exits 1; run_impl writes
  no output file on any path before the ROUTE gate.
- **P4 WireDb seam matches the CTO ruling (option A)
  exactly.** fabric.rs: `WireInfo { name, tile }`,
  `fn wire_info(&self, wire: WireId) ->
  Option<WireInfo<'_>>` on the WireDb trait; EmptyWireDb and
  the mock return None; the NamedTestDb test returns Some.
  The two test DB lines are verbatim in prjuray-db-ref
  zynqusp/segbits_int.db, checked by me:
  `INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 64_00 !66_00 !68_00
  !72_00 !72_01 !74_00` and line 1063
  `INT.PIP.INODE_E_1_FT1.BYPASS_E4 !57_00 !59_00 !61_00 65_01
  !67_01 !69_01` (the test comment cites only `!57_00`,
  an abbreviation; the test uses only the wire names, so
  the abbreviation is harmless). The INTERFACES.md WireDb
  section states the same shape and the same Some/None
  contract.
- **P5 IntPip sink-first rename: emitted strings
  byte-identical.** feature() builds `PIP.{dst}.{src}`;
  the tests now construct with the physical source first
  (`IntPip::new("INT_X12Y45", "INODE_E_1_FT1",
  "BOUNCE_E_0_FT1")`) and assert the same emitted string
  as the merged branch (`PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1`).
  Full diff vs the merged fm/ultra-bitstream int_pip.rs is
  only the rename (module docs, feature() order, test
  constructions, MissingPip Display) plus the u8-vs-u16 bit
  width in ZeroBitSet and verify_zero_bits (flag (b) below).
  The MissingPip error text follows the same sink-first
  order. INT_PIP tier-1 green stands.
- **P6 Slice counts match the CTO ruling, in code.**
  `Xcvu3pSpec::CLB_SLICE_COUNT = 49_260`,
  `Xcku3pSpec::CLB_SLICE_COUNT = 20_340`, pinned by tests
  that pass in my run (xcvu3p 394,080 LUTs, 788,160 FFs,
  49,260 CARRY8, one per slice; xcku3p 162,720 LUTs,
  325,440 FFs). The device file's num_slices 27,120 is
  documented as a file-header anomaly and is not used.
- **P7 Complete-FASM generation is correct.**
  fasm_gen.rs emits one INIT line plus one SRVAL line per
  used FF BEL, canonical `.V0`/`.V1` spelling, V selected
  by value, unspecified defaults to V0, in BEL order, using
  the placed site as the FASM tile. The tile spelling
  matches the harness golden (`LOGIC_TILE = "SLICE_X0Y0"`
  in gen_golden.py). The ff_fasm_name table is identical to
  the merged bitloc::ff_fasm_name (0 AFF through 15 HFF2,
  out-of-range errors). Polarity follows the resolved rule:
  V names the value, bit = NOT(value). The 8 fasm_gen tests
  pass (P1).
- **P8 bitgen.rs divergence is exactly as the worker
  declared.** This branch carries 0f3aa2d plus its own
  weaker D1 variant (present = all_set_bits_one AND
  all_zero_bits_zero for valueless features; no n_set > 0
  refinement, so an all-`!` feature with all zero bits
  reading 0 is still called present, e.g. VCC_WIRE on an
  untouched mux). START/DESYNC constants are defined but
  not emitted in the ending; CRC_EXCLUDED_REGS is still
  present. Grep counts, branch vs bitgen-core tip 88070a5:
  START/DESYNC 2 definition-only hits vs 13, complement
  0 code hits vs 14, CRC_EXCLUDED_REGS present vs 0. The
  worker's claim that its 7365f2e D1 test expectations are
  compatible with the complement rule holds: the test uses
  the V0/V1 complement pair (n_set = 1), on which both
  rules agree.
- **P9 Two SegbitsDb/BitRef definitions coexist, exactly
  as the worker declared.** lib.rs root re-exports
  bitgen's BitRef/SegbitsDb; segbits items are module-path
  only. This branch's segbits.rs is the 6bb30b6 port:
  `pub bit: u8`, parser bound u8::MAX, no features(), no
  u16 regression test. The merged branch has u16 plus
  features(). The branch is internally self-consistent at
  u8 (its tests pass), but it cannot land alongside the
  merged u16 segbits.rs without the reconciliation commit.

### Rulings on the worker's flags

- **(a) CONFIRMED: the orchestrator must take 88070a5
  bitgen.rs at merge; this branch's bitgen.rs must not
  overwrite it.** Evidence is P8. The 88070a5 tip carries
  the stronger D1 (n_set > 0 refinement), the complement
  rule, D2 (START/DESYNC emitted), and the F-A removal.
  This branch's bitgen.rs regresses all four.
- **(b) CONFIRMED: the SegbitsDb/BitRef reconciliation is
  the merge-time item, owned by bitgen-core session 3.**
  Evidence is P9. The reconciliation commit is a single
  definition in segbits.rs (u16 plus features()), the
  duplicate deleted from bitgen.rs, the lib.rs re-export
  fixed, one constructor spelling picked, plus the
  features() test and a file-order preservation test
  (interfaces session 2 item 6).
- **(c) fasm_gen ff_fasm_name duplication: ACCEPTABLE ONLY
  with the dedup applied in the merge commit.** The table
  is byte-identical to bitloc::ff_fasm_name now on the
  merged branch (7f80b54, P7); fasm_gen can call bitloc
  directly with a map_err to String, a 3-line change. The
  reconcile note names the merge as the moment; a second
  copy must not survive the merge, because this is
  load-bearing naming data and two copies drift silently.
  If the orchestrator cannot do the dedup in the merge
  commit, the branch goes back to the worker for it.

### FLAGs

- **F1 (must-fix before merge, hygiene): the worker's
  "clippy 0 warnings" is incorrect.** `cargo clippy -p
  helion-ultrascale -j2` (plain, no --all-targets) exits 0
  but reports 7 warnings in the helion-ultrascale lib: 6x
  clippy::for_kv_map (pack.rs:149, 152, 157;
  legality.rs:161, 164, 169) and 1x
  clippy::too_many_arguments (router.rs:163, astar, 8
  args). All 7 are inherited from day-1 staging/scaffold
  text, none in the worker's new code (fasm_gen, the fabric
  seam, ultra.rs, main.rs are warning-clean in the build),
  but they are new warnings on this branch under the
  standing zero-new-warnings bar. Fix: `.values()` for the
  6 loops; `#[allow]` or a parameter split for astar. Note:
  `cargo clippy --all-targets` additionally fails with
  exit 101 on a pre-existing helion-sv logic-bug error
  (crates/helion-sv/src/lib.rs:15059, `step_down` boolean
  expression; zero files in this branch's diff are under
  helion-sv), so --all-targets cannot pass on this branch
  or on base; that error is not this worker's to fix here.
- **F2 (must-fix before merge, hygiene): the worker's
  "fmt clean" is incorrect.** `cargo fmt -p
  helion-ultrascale -- --check` exits 1 with 36 hunks in
  12 files, including this branch's new files (fasm_gen.rs
  5, grid.rs 2, corpus_legality.rs 4) and ported files
  (pack.rs 3, place.rs 1, legality.rs 6, router.rs 5,
  router/tests.rs 3, lib.rs 2, device.rs 2, sta_sanity.rs
  1, smoke_place.rs 1). Baseline check on base aee77e3 in
  a second scratch worktree: fmt fails with only 2 hunks
  (lib.rs:17, smoke_place.rs:67), and only smoke_place.rs:67
  overlaps, so 35 hunks are introduced by this branch.
  Cosmetic only (line-width reflows), no behavior change.
  Fix: run `cargo fmt` in the merge commit or send back.
- **F3 (scope, not a defect): xcku3p STA uses the xcvu3p
  preliminary DB.** sta_on_placed takes no part argument
  and always calls preliminary_xcvu3p(), so the xcku3p
  WNS_PS=9650 carries no xcku3p-specific delay data. The
  flow labels STA preliminary, so this is honest, but the
  orchestrator must not treat the xcku3p STA number as
  part-specific evidence. Fix when an xcku3p timing DB
  exists.
- **F4 (minor, doc): two stale comments in delivered
  code.** fasm_gen.rs header says "The lut worker's
  emission must change separately" (stale; the lut-ff fix
  already landed, merged 7f80b54, session 9 PASS).
  grid.rs header says tile names are provisional "until
  the device-file worker lands real tile names" (stale;
  the device file has landed, merged 9289fed, session 7
  PASS; the adapter still uses provisional names, which is
  correct until wired to the device file, but the "until"
  clause is now wrong). One-line fixes each, at merge.

### Stale worker statements (noted, not defects)

- "lut worker's ff_init_fasm is still bare/inverted":
  stale; merged as 7f80b54, critic session 9 PASS.
- "the xcvu3p device file" still blocked: stale; merged
  as 9289fed, critic session 7 PASS.
- "No critic has verified the researcher's chain yet":
  stale; critic sessions 8 and 9 both re-verified it from
  source. The worker's COORD.md entries already flag these
  as stale; recorded here so the merge record is
  unambiguous.

### Merge readiness

Ready for the orchestrator's merge sequence, NOT for a
blind merge of tip 3e6d17f. Merge order: (1) the bitgen-core
reconciliation commit lands (single SegbitsDb/BitRef, u16
plus features()); (2) take bitgen-core tip 88070a5
bitgen.rs plus bitgen_roundtrip.rs, never this branch's
bitgen.rs; (3) take this branch's non-bitgen files (the
pack/place/router/timing port, the fabric seam, device
counts, the int-pip rename, fasm_gen, ultra.rs, main.rs
--seed); (4) in the merge commit, apply the ff_fasm_name
dedup (ruling (c)), `cargo fmt` (F2), the 7 clippy fixes
(F1), and the two stale doc lines (F4); (5) re-run `cargo
test -j2 -p helion-ultrascale` (expect 77 plus whatever the
reconciliation adds) and the four gate-2 commands before
declaring the merge done. FF INIT goes green only on the
harness run against the emitter, not on this merge.

### Commands run (all by me, in detached scratch worktrees, since removed)

- `git worktree add --detach ~/workspace/.critic-s10 3e6d17f`
  (plus ~/workspace/.critic-s10-base at aee77e3 for the fmt
  baseline)
- `cargo test -j2 -p helion-ultrascale` in the scratch
  worktree: 63 + 5 + 2 + 2 + 2 + 3 + 0 = 77 passed,
  0 failed (log /tmp/critic-s10/test.log, removed with the
  worktree)
- `cargo clippy -p helion-ultrascale -j2`: exit 0, 7
  warnings in the helion-ultrascale lib
- `cargo clippy -p helion-ultrascale -j2 --all-targets`:
  exit 101, helion-sv lib.rs:15059 logic-bug error
  (pre-existing, untouched by this branch)
- `cargo fmt -p helion-ultrascale -- --check`: exit 1, 36
  hunks in 12 files; same command on base aee77e3: exit 1,
  2 hunks
- `cargo build -p helion-cli -j2`: finished, binary
  target/debug/helion (the helion-cli bin's 2 warnings are
  in helion-sv, pre-existing)
- Four gate-2 runs, all exit 1 at ROUTE, no .bit written:
  `./target/debug/helion impl examples/blinky.sv --part
  xcku3p --seed 0`, same with xcvu3p, same for
  examples/counter.sv on both parts
- Diffs, all by hand: staging pack.rs / place.rs /
  legality.rs vs the port; router-scaffold lib.rs, mock.rs,
  tests.rs vs router.rs, router/mock.rs, router/tests.rs;
  timing-impl sta.rs, timing_db.rs vs the port; merged
  fm/ultra-bitstream int_pip.rs and segbits.rs vs this
  branch; merged bitloc.rs ff_fasm_name vs fasm_gen.rs
  ff_fasm_name
- Greps: START/DESYNC, complement, CRC_EXCLUDED_REGS,
  n_set, all_zero_bits_zero in bitgen.rs on both branches;
  slice counts in device.rs; wire_info in fabric.rs and
  router/mock.rs
- DB line checks: `grep` in
  ~/workspace/prjuray-db-ref/zynqusp/segbits_int.db for
  the two NamedTestDb lines (BOUNCE line; INODE_E_1_FT1.
  BYPASS_E4 at line 1063)

## 2026-11-07 critic-correctness, session 12: bitgen-device deliverable (uncommitted, base 2c8c9e3)

Scope: branch fm/ultra-bitgen-bitgendev, worktree
~/workspace/helion-ultra-bitgen-bitgendev, base fm/ultra-bitstream
tip 2c8c9e3. The work is UNCOMMITTED: modified
crates/helion-ultrascale/src/device_file.rs (+23/-1: two
OnceLock caches, new `row_majors` accessor, FromStr init) and
src/lib.rs (+1: `pub mod bitgen_device`), plus new
src/bitgen_device.rs (impl) and tests/bitgen_device.rs (13
tests). Every claim below was checked by my own runs and reads,
not by taking the worker's report on trust. Probes ran in a
detached scratch copy on the home disk
(~/workspace/.critic-s12, removed afterwards); the worker
worktree was left exactly as found (same 4 files, uncommitted).
Process note: my first scratch build failed on /tmp (512M tmpfs
exhausted, "No space left on device" compiling
sv-parser-syntaxtree); the scratch moved to the home disk, same
lesson as critic session 7.

### Verdict: PASS on correctness and scope, with two must-fix test items before the orchestrator commits/merges

The `BitgenDevice` impl is correct and complete, the FAR
resolution is grounded in the device file's own tables, the
library-level FRAMING proof is real, and the full suite is green.
The two must-fix items are both in the test file, not in the
impl: M1, the `part()` test does not pin the trait method it
claims to pin (its comment is factually wrong); M2, one
misleading SLICE comment. Neither changes any emitted bit, but
the orchestrator must not commit the test comment as written.

### PASS items, verified by my own runs and reads

- **P1 Tests re-run by me at the exact uncommitted tree.**
  Command (scratch copy, full cargo path, `-j2`):
  `~/.cargo/bin/cargo test -j2 -p helion-ultrascale`.
  Result: **142 passed, 0 failed** across 14 suites, of which 1
  is my own probe test (see M1), so the real tree is **141
  passed, 0 failed**: 77 unit + 13 bitgen_device + 9 round-trip
  + 2 int_pip + 1 audit + 3 demo + 3 xcvu3p + 2 corpus_legality
  + 22 lut_ff_bram + 4 segbits + 2 smoke + 3 sta_sanity + 0
  doc. That is exactly 128 baseline + 13 new, as claimed.
- **P2 Trait impl complete.** `impl BitgenDevice for
  DeviceFile` in src/bitgen_device.rs covers all six trait
  methods (bitgen.rs trait at :460-470): part, idcode,
  tile_type, tile_base_far, tile_names, config_order_fars.
  `part()` returns the short device name, `idcode()` returns
  the single SLR IDCODE when all SLRs agree (None on
  disagreement), unknown tiles return None everywhere (fail
  loud, never invented).
- **P3 SLICE base FARs grounded, cross-checked two ways.**
  Hand-computed from xcku3p.json row 0: CLB column 23 is major
  48, so SLICE_X23Y45 base FAR is (0, 48, 0) = 0x0000_3000;
  CLB column 0 is major 5, type CLEL_R, so SLICE_X0Y0 base is
  0x0000_0500. Both match the test assertions. The DB-frame
  cross-check holds against the vendored DB lines I read
  directly in prjuray-db-ref/zynqusp/segbits_clel_l.db
  (affbc5e): AFF.INIT.V0 frame 12, BFF.INIT.V0 frame 12,
  ALUT.INIT[0] frame 11, INIT[1] frame 10, INIT[2] frame 9,
  so base + DB frame == BitLocator FAR for the same BELs, and
  the tile base minor is 0. The same cross-check passes on
  the provisional xcvu3p file (base + 12 == locator AFF FAR;
  X0Y419 resolves, X0Y420 does not).
- **P4 INT provisional rule validated, not assumed.**
  Hand-computed from xcku3p.json: CLB column 0 is major 5, so
  the rule gives INT major 4, which carries 76 minors
  (num_minors_per_std_colMajor[4] = 76), exactly the INT
  segbits frame span (frames 0..75), so INT_X0Y0 resolves to
  base FAR 0x400. INT_X2Y0 resolves (CLB major 8, INT major 7,
  minors[7] = 76, base 0x700). INT_X1Y0 does NOT resolve: the
  rule points at major 5, which carries only 16 minors
  (minors[5] = 16), so tile_type and tile_base_far return
  None. INT_X0Y3 resolves to 0x000C_0400, INT_X0Y4 does not
  (only rows 0..3 exist). All match the test assertions.
- **P5 BRAM and fail-loud tiles.** RAMB18_X0Y0 resolves to
  Far(BramContent, 0, 0, 0) = 0x0100_0000, matching the
  locator's first mem FAR. RAMB36, HPIO, HDIO, CMT, DSP, and
  malformed names all return None for tile_type and
  tile_base_far. BRAM parity needs no separate enumeration:
  the file's bram_content and bram_content_parity tables are
  identical (verified in the JSON), and the bram_reg majors
  index into the std columns (validated against n_std in
  device_file.rs:590), so the reg bits live in standard
  frames the config order already enumerates.
- **P6 tile_names and config_order counts recomputed from
  the JSON, not taken from the tests.** xcku3p: 27,120 SLICE
  (113 cols x 60 x 4 rows) + 208 INT (52 valid rule columns
  per row x 4 rows, recomputed per row from the minors table)
  + 960 RAMB18 (10 cols x 24 x 4 rows) = 28,288, sorted and
  deduplicated. xcvu3p: 49,260 SLICE (821 cols x 60) + 364
  INT (52 x 7) + 1,680 RAMB18 (10 x 24 x 7) = 51,304.
  Config order: xcku3p 41,460 and xcvu3p 73,035, both
  ascending, unique, and equal to `total_frames()`; I summed
  the tables myself (std minors + BRAM content minors) and
  got the same two numbers. First FAR 0x0000_0000; last FARs
  0x010C_09FF (xcku3p: block 1, row 3, col 9, minor 255) and
  0x0118_09FF (xcvu3p: block 1, row 6, col 9, minor 255),
  both hand-decoded from the FAR layout.
- **P7 The invented device.rs grid (critic G2) is not
  consumed.** bitgen_device.rs imports only
  `crate::bitgen::BitgenDevice` and `crate::device_file::{...}`;
  grep finds no `device::`, `UltraScaleDevice`, or
  `UltraScaleFabric` reference in bitgen.rs or
  bitgen_device.rs. All FAR mapping goes through the device
  file's own accessors (clb_col_major, clb_tile_type,
  bram_content_col_major, num_minors_per_std_col_major).
- **P8 FRAMING proof at the library level is real.** The
  `emit_parse_roundtrip_on_real_device` test passes, and I
  verified its two set bits against the vendored DB lines I
  read directly in segbits_int.db (3778 lines): BOUNCE
  `64_00` at base 0x400 gives FAR 0x440 bit 0, GCLK `41_03`
  gives FAR 0x429 bit 3 (word 0 = 8). Parse-back equals the
  input FASM exactly, and re-emitting the parse-back is
  byte-identical. The zero-checked FARs account for the 10
  frames in `int_pip_frames_land_at_expected_fars`: BOUNCE
  zero FARs 0x442, 0x444, 0x448, 0x44A (frame 72 listed twice,
  bits 0 and 1, one FAR) plus GCLK zero FARs 0x425, 0x427,
  0x42B, 0x42F, all asserted all-zero.
- **P9 The emit_frames zero-frame correction is sound and
  pinned.** Read in bitgen.rs:737: `frames.entry(far)
  .or_insert([0u32; FRAME_WORDS])` runs for every BitRef,
  including `must_be_zero` ones, so zero-checked FARs are
  materialized as all-zero frames in the returned map. This
  is pre-existing `emit_frames` behavior, not a worker
  change, and the test pins it (10 entries, all non-set
  frames all-zero). The bitstream writer writes them as zero
  frames, identical to untouched frames, so the bitstream is
  unaffected.
- **P10 fmt clean, clippy status verified.** `cargo fmt -p
  helion-ultrascale -- --check`: exit 0, clean. Plain
  `cargo clippy -j2 -p helion-ultrascale --all-targets`:
  exit 101, exactly 1 error, `crates/helion-sv/src/lib.rs:
  15059:21` ("this boolean expression contains a logic bug",
  `clippy::overly_complex_bool_expr`, denied by default) in
  the `step_down` expression, plus 426 pre-existing helion-sv
  warnings. The worker's diff touches zero files under
  helion-sv (`git diff --name-only` shows only
  device_file.rs and lib.rs in helion-ultrascale, plus the
  two new files), so the block is pre-existing and out of
  scope, as critic session 10 already recorded. Scoped
  re-run with `-- -A clippy::overly_complex_bool_expr`:
  exit 0; the only helion-ultrascale warning is the
  pre-existing `type_complexity` at
  tests/corpus_legality.rs:52 (a file this worker never
  touched); zero warnings in any file this worker touched.
  helion-ir 47 and helion-sv 426 warnings are pre-existing
  in dependencies.

### Must-fix before commit/merge

- **M1 (test evidence, the `part()` claim is wrong as
  stated).** The test `xcku3p_part_and_idcode` asserts
  `device.part() == "xcku3p-ffva676-1-e"` with the comment
  "Trait `part` is the full part string (DeviceFile::part),
  the value a bitstream header names. The short device name
  is `device()`. The stub's short part was a stub
  simplification." That comment is factually wrong, and the
  test does not pin the trait method. I proved it with a
  probe test in the scratch copy (deleted afterwards):
  `device.part()` calls the INHERENT `DeviceFile::part()`
  (inherent methods win method resolution even with the
  trait in scope), while `<DeviceFile as
  BitgenDevice>::part(&device)` returns something else.
  Probe output: inherent `DeviceFile::part()` =
  `xcku3p-ffva676-1-e`; trait `<DeviceFile as
  BitgenDevice>::part()` = `xcku3p`; `DeviceFile::device()`
  = `xcku3p`. The trait impl itself is CORRECT: it returns
  the short name, matching the trait doc ("Part name, e.g.
  'xcku3p'"), its own doc comment, and the stub convention
  (every StubDevice construction in bitgen_roundtrip.rs uses
  "xcku3p"). The trait method's only consumer is the
  UnknownIdcode error message (bitgen.rs:858), so no emitted
  bit is affected. Fix: in `xcku3p_part_and_idcode` and
  `xcvu3p_part_and_idcode_and_provisional`, assert the trait
  method by qualification
  (`<DeviceFile as BitgenDevice>::part(&device) ==
  "xcku3p"` / `"xcvu3p"`), keep the inherent assertion as a
  separate line, and correct the comment to state the trait
  returns the short device name. Re-run the bitgen_device
  tests after the edit.
- **M2 (test comment).** In
  `slice_base_far_matches_bitlocator_xcku3p`, the comment
  says "SLICE_X23Y45: row 0, CLB column 0 is major 5 per the
  file". The asserted base 0x0000_3000 is correct, but the
  column is 23, not 0: CLB column 23 is major 48 (48 << 8 =
  0x3000), while column 0 is major 5 (that is the SLICE_X0Y0
  case on the next lines). Fix the comment to "CLB column 23
  is major 48".

### Carried, not fixed (recorded, not hidden)

- Critic session 7 FLAG F1: the last-of-row FAR
  2-zero-padding path is still untested (0 of 261 FARs
  exercised). This impl does not change frame emission, so
  it does not fix F1; the generator must not be trusted for
  full-device bitstreams until F1 is exercised.
- The xcvu3p frame-total gap: 73,035 computed vs UG570
  Table 1's 71,820 (overshoot 1,215), flagged provisional in
  the file and in the test, never tuned away. This impl
  enumerates the file's own frames.
- Plain `cargo clippy --all-targets` stays blocked on the
  pre-existing helion-sv lib.rs:15059 error above; that fix
  belongs to the helion-sv lane (the 25 pre-existing suite
  failures disposition), not this worker.
- `row_majors()` returns HashMap key order (unsorted); harmless
  here because both consumers sort at the end (tile names,
  config FARs), but a future consumer that iterates rows in
  order should sort first.

### Merge readiness

Ready for the orchestrator to commit and merge into
fm/ultra-bitstream once M1 and M2 are applied and the
bitgen_device tests re-run green (13 tests, 0 failed). No
re-review needed for those two test-only edits beyond the
re-run, provided the diff stays confined to the test file
comments and the qualified assertions. This closes the
interfaces session 2 item 4 FLAG: a real BitgenDevice impl
now exists for the real device file, and FRAMING green on a
real device is unblocked at the library level. The remaining
gate for FRAMING green in the harness is the emitter
executable wiring.

## Session 13: correctness critic review of the emitter branch (fm/ultra-bitgen-emitter, f692cd5)

Scope: `crates/helion-ultrascale/src/bin/helion-bitgen.rs` (859 lines: emit / parse / bits), the vendored CLB segbits DBs plus `.origin_info.db` files, `PROVENANCE.md` updates, `src/segbits.rs` (+254 lines, `synthesize_lut_init`), and `tests/fixtures/blinky.xcku3p.fasm`. Base fm/ultra-bitstream 2c8c9e3. Branch tip f692cd5, worktree clean, `git show --stat` confirms 12 files, 7578 insertions, 2 deletions. Reviewer ran every check below in the emitter worktree or the harness directory on 2026-11-08; nothing in this entry is taken from the worker's report on trust.

### Verdict: PASS, no defects. Ready to merge, subject to the merge-time wiring note in FLAG F4.

### 1. Harness contract: PASS

Read `~/workspace/ultra-bitgen-crew/harness/emitter.py` lines 11-28 (the protocol) and the binary's `parse_args`. They agree exactly:

- `<emitter> emit --fasm FILE --part PART --out FILE.bit`, exit 0 on success.
- `<emitter> parse --bit FILE.bit --part PART`, FASM on stdout, exit 0.
- `<emitter> bits --bit FILE.bit --part PART`, one line per set bit: `FAR_HEX OFFSET VALUE BLOCK` (e.g. `00000509 0 1 LUT_INIT`, `00000440 0 1 INT_PIP`).

Verified live, not by reading alone: emit, parse, and bits were all run against both harness goldens (see items 4 and 5). The parse-back form is consumed by the harness canonicalizer; the critic ran the harness's own `canonical_roundtrip_fasm` plus `scope_fasm_to_feature_keys` against the binary's parse output for both designs (item 5) and it normalizes cleanly (Verilog `64'h...` hex, `.V0`/`.V1` spellings, per-bit `ALUT.INIT[i]` lines). All four negative paths fail loud with exit 1 and a named reason: missing `--part` (`--part PART is required` plus usage), unknown subcommand (`unknown subcommand "frobnicate"`), unknown flag (`unknown flag "--bogus"`), bare `AFF.INIT` (`emit failed: tile SLICE_X0Y0: unknown feature AFF.INIT`).

### 2. Data paths: PASS

`--data-dir` override works; a nonexistent dir fails loud: `helion-bitgen: cannot load vendored segbits DBs from /tmp/does-not-exist: cannot read segbits file: /tmp/does-not-exist/segbits_int.db: No such file or directory (os error 2)`, exit 1. The `CARGO_MANIFEST_DIR` default works invoked from the harness directory, not just the repo root: `cd ~/workspace/ultra-bitgen-crew/harness && helion-bitgen emit --fasm golden/blinky.xcku3p.fasm --part xcku3p --out /tmp/critic13-blinky.bit` exited 0 and wrote an 8532-byte bitstream. Stdin emit is byte-identical to `--fasm` emit (`cmp` clean).

### 3. load_device(part) seam: PASS

`--part xcvu3p` fails loud, exit 1: `helion-bitgen: part xcvu3p has no emitter device yet: only xcku3p is wired; other parts need the real DeviceFile BitgenDevice implementation`. Only `xcku3p` resolves, to a StubDevice. No silent fallback exists in the code path (read) or in behavior (probed). The stub's base FARs were verified independently against the vendored device file, not against the binary's comments: `data/xcku3p.json` gives `slrs.SLR0.rowMajors["0"].clb_colMajors["0"] = 5` and `clb_tileTypes["0"] = CLEL_R`, so SLICE_X0Y0 base FAR 0x500 is the file's own value; INT column major = 5 - 1 = 4, base FAR 0x400, matching the stub; IDCODE 0x04a63093 matches the file's `slrs.SLR0.idcode`. The HPIO base FAR 0x000 is admitted provisional in the binary comments; see FLAG F1.

### 4. Round-trip proof: PASS, first real-emitter harness run reproduced

The critic ran the harness fresh with the emitter binary:

```
cd ~/workspace/ultra-bitgen-crew/harness
HELION_BITGEN_EMITTER=~/workspace/helion-ultra-bitgen-emitter/target/debug/helion-bitgen python3 diff.py --design all --part xcku3p --out runs/critic13
```

Exit 0, 20 records written. `diff runs/emitter-session1/diff.jsonl runs/critic13/diff.jsonl` is empty: the critic's run is byte-identical to the worker's `runs/emitter-session1/diff.jsonl`. Placement hashes match the baseline (blinky `b65979c112456c7049770e0fd1573939148c901606eef9f7a0b8a0f74fddacfb`, counter `1bf0ffe68ca860b2a3b7c05b56e79fc5e7f8e7949af49bb7504734a68ba012ea`). Status by design, all via `bits` (`emitted_via=bits`, not parse-mediated):

- blinky: LUT_INIT 0/64 green, FF_INIT 0/1 green, INT_PIP 0/11 green.
- counter: LUT_INIT 0/256 green, FF_INIT 0/4 green, INT_PIP 0/17 green.
- FRAMING red 1/3 on both designs (see item 5). IO_CONFIG blocked on both (tilegrid, pre-existing). BRAM_CONTENT, BRAM_PARITY, DSP48E2, URAM288, CLK blocked (not exercised by the v1 goldens).

The `bits` output was spot-checked against the DB by hand: BOUNCE at FAR 0x440 bit 0 and GCLK at FAR 0x429 bit 3 match the bitgen-device FRAMING proof; AFF.INIT.V0 at FAR 0x50c bit 2 is exactly the DB line `CLEL_R.AFF.INIT.V0 12_02` (base 0x500 + frame 12, bit 2); LUT 0x5555... renders 32 set bits at FARs 0x509 and 0x50b.

### 5. FRAMING red claim: CONFIRMED, golden-side only, not an emitter defect

The worker's claim was verified three independent ways:

(a) DB structure. `grep` on the vendored `segbits_clel_r.db`: `CLEL_R.AFF.SRVAL.V0 12_01` and `CLEL_R.AFF.SRVAL.V1 !12_01`, the same shape as the INIT pair (`V0 12_02`, `V1 !12_02`). Bit = NOT(value) holds for SRVAL exactly as for INIT, per the researcher session 1 polarity resolution. The goldens contain zero SRVAL lines (`grep -c SRVAL` returns 0 for both goldens).

(b) Harness canonical comparison. Running the harness's own `canonical_roundtrip_fasm` plus feature scoping on the binary's parse-back output gives, for blinky, exactly one line only in the golden (`SLICE_X0Y0.AFF.SRVAL.V0`, the harness default completion for the missing line) and exactly one line only in the parse-back (`SLICE_X0Y0.AFF.SRVAL.V1`). For counter the same, times four FFs (AFF, BFF, CFF, DFF). No other line differs on either design. The emitter leaves the SRVAL bit clear because the input FASM specifies no SRVAL feature; the parse-back faithfully renders the clear bit as `.V1` under the exact-complement rule (V1's zero set {12_01} equals V0's set-bit set {12_01}).

(c) Raw-frame proof, independent of the binary's own cross-check. A from-scratch packet walker over the emitted bitstreams (sync at byte 48, FAR register 0x01, FDRI 0x02): with `SLICE_X0Y0.AFF.SRVAL = 0` added to the blinky golden, frame 0x50c word0 = 0x6, bits 1 and 2 both set (SRVAL.V0 12_01 and INIT.V0 12_02), and parse-back renders `AFF.SRVAL.V0` with `bits` reporting `0000050c 1 1 FF_INIT` and `0000050c 2 1 FF_INIT`. Without the SRVAL line (the golden as shipped), frame 0x50c word0 = 0x4, bit 1 clear, bit 2 set. The emitter sets exactly the bits the input FASM specifies and nothing else. The fix belongs to the goldens (add `SLICE_X0Y0.AFF.SRVAL = 0` etc.), not to the emitter, which must not invent defaults. FRAMING red stands until the goldens are completed.

### 6. Exact counts: PASS, worker's numbers reproduced exactly

`cargo test -j2 -p helion-ultrascale` in the emitter worktree (log `/tmp/critic13-test.log`, exit 0). Per-suite `test result` lines: lib 82, bin helion-bitgen 7, bitgen_roundtrip 9, corpus_legality 2, device_file_demo 3, device_file_xcvu3p 3, int_pip 2, int_pip_audit 1, lut_ff_bram 22, segbits 4, smoke (2), sta (3), doc-tests 0. Total: 140 passed, 0 failed. The worker's claim (140 passed, 0 failed) is exact. The 23 `warning` lines in the test log are all from other crates (helion-ir, helion-sv); none mentions helion-ultrascale or helion-bitgen.

Scoped clippy, run by the critic: `cargo clippy -j2 -p helion-ultrascale --lib --bins` exits 0, 0 warnings. `cargo fmt -p helion-ultrascale --check` exits 0, clean. Full-workspace `cargo clippy --all-targets` was not re-run in this session; the pre-existing helion-sv lib.rs:15059 error recorded by sessions 10 and 12 is outside this branch's files and unchanged by it.

### 7. Provenance: PASS, all hashes verified

All nine SHA-256 values in `PROVENANCE.md` were recomputed from the vendored files (4-line `#` header excluded) and MATCH: segbits_int, segbits_hpio_right, segbits_hdio_top_right, segbits_hdio_bot_right, segbits_cmt_right, plus the four new CLB DBs (clel_l, clel_r, clem, clem_r). The four CLB DBs are byte-identical to `~/workspace/prjuray-db-ref/zynqusp/segbits_*.db` (commit affbc5e) modulo the 4-line vendoring header (`diff` shows only the added header lines). Line counts: the five original DBs sum to 7082 data lines (3778 + 1560 + 812 + 828 + 104); the four CLB DBs sum to 3194 (768 + 768 + 829 + 829); 7082 + 3194 = 10276, matching the PROVENANCE.md total. All eight `.origin_info.db` files are present with matching line counts (772/772/833/833 for the CLB files, header included). The fixture `tests/fixtures/blinky.xcku3p.fasm` body is byte-identical to the harness golden (5-line provenance header on top).

### FLAGs (none blocks the merge)

- F1 (carried, provisional data): the stub's HPIO base FAR 0x000 is invented, admitted in the binary comments. The harness blocks IO_CONFIG before comparing, so no greenness depends on it, but the `bits` IO_CONFIG absolute FARs (0x00000002, 0x00000003) must never be consumed as ground truth. Resolves when the xcku3p tilegrid lands and the stub is replaced by the real DeviceFile impl.
- F2 (carried from the worker, needs a CTO ruling): `bits` fails loud on a CARRY8 feature that sets bits, because CARRY8 has no block in the harness registry. Unexercised by the v1 goldens: `CARRY_TYPE.SINGLE_CY8` is `!12_45` (all-!, complement-rendered, sets no bits), as are the `DI*.A*` lines. A CARRY-using design (e.g. `CARRY_TYPE.DUAL_CY4 12_45`, `DI0.DI0 12_03`) will hit this. The ruling needed is which block CARRY8 belongs to before such a design runs through `bits`.
- F3 (harness-side gap, not exercised): `harness/fasm.py` classify raises FasmError on unclassifiable features, so the parse-mediated fallback path (used only if `bits` is absent) would fail on the parse-back's extra lines (e.g. the ABCDFF/EFGHFF control lines and the per-bit LUT lines). Every record in this review's run used `bits` (`emitted_via=bits`), so the gap is latent, not live. It becomes live only if a future emitter drops `bits`.
- F4 (merge-time wiring, orchestrator action): this branch is based on 2c8c9e3; the base has since moved to 1ad06df, which carries the real DeviceFile BitgenDevice impl (session 12). At merge, wire `load_device(part)` to the real impl and delete the stub table, per the worker's own note and the CTO direction. The emitter's tests were written against the trait plus StubDevice, so the merge must re-run the full suite (140 tests) plus the harness run after the wiring. Do not extend the stub table to new parts instead.
- F5 (minor observation, no action required for this merge): parse-back renders both the grouped `A6LUT.INIT[63:0] = 64'h5555...` line and the per-bit `ALUT.INIT[i] = 1'h1` lines for the same set bits, because the DB carries both the synthesized grouped entry and the per-bit entries. Harmless under the harness's feature-scoped comparison (verified in item 5: no extra lines survive scoping). A future consumer that diffs raw parse-back text would see duplication; that consumer, if it ever exists, needs a minimal/canonical parse decision from the CTO.

### Merge readiness

Ready for the orchestrator to merge into fm/ultra-bitstream once F4 is executed at merge time (wire `load_device` to the real DeviceFile impl, delete the stub, re-run the 140 tests and the harness run). No re-review is needed for the merge itself, provided the diff stays confined to the rebase plus the F4 wiring. After the merge, the next greedy step is the golden-side SRVAL completion (item 5), which is the only thing standing between the current state and FRAMING green on xcku3p for blinky and counter.

## Session 14: correctness critic review of the HpioProvisionalFallback bridge (merged tip fa58d5c)

Scope: ONLY the bridge in `crates/helion-ultrascale/src/bin/helion-bitgen.rs`
(struct `HpioProvisionalFallback` plus `load_device`, about lines 201-282)
at merged tip fa58d5c, branch fm/ultra-bitstream, worktree
~/workspace/helion-ultra-bitstream. The rest of the emitter was passed by
session 13 and is not re-reviewed here. Every check below was run by the
reviewer on 2026-11-08; nothing is taken from the orchestrator session 12
report on trust. Scratch reproductions ran in a detached scratch worktree
at /tmp/critic14-scratch (commit 9290ce6, since removed); the base worktree
and committed code were never modified.

### Verdict: PASS, no defects.

### 1. Load-bearing claim: REPRODUCED, bridge is required

The real impl returns `None` for HPIO tiles. Read, not assumed:
`crates/helion-ultrascale/src/bitgen_device.rs` `resolve_tile` handles only
SLICE, INT, and RAMB18 tiles and falls through to `None` for anything else
(lines 175-208), and the module docs state HPIO, HDIO, CMT, DSP are NOT
resolved because no column mapping exists in the device file (lines 47-55).
`tile_type` and `tile_base_far` both return `resolve_tile(...).map(...)`,
so both are `None` for `HPIO_RIGHT_X0Y0`.

Literal deletion reproduced in scratch (variant A: `load_device` returns
the raw `DeviceFile`, bridge unused, one-line edit, log
/tmp/critic14-buildA.log, build finished in 38.90s):

```
cd /tmp/critic14-scratch
./target/debug/helion-bitgen emit --fasm ~/workspace/ultra-bitgen-crew/harness/golden/blinky.xcku3p.fasm --part xcku3p --out /tmp/critic14-variantA.bit
helion-bitgen: emit failed: unknown tile HPIO_RIGHT_X0Y0
exit:1, no bitstream written
```

Same for the counter golden (`unknown tile HPIO_RIGHT_X0Y0`, exit 1).
The v1 goldens each carry 2 `HPIO_RIGHT_X0Y0` lines
(`grep -c HPIO_RIGHT` returns 2 for both goldens), so both fail.

Regression consequence confirmed in the harness code, not by assertion:
`harness/diff_engine.py` lines 264-268, on a non-None `emit_error` every
block record gets `status blocked` with
`missing = "emitter failed to emit: ..."`. Literal bridge deletion therefore
regresses every green block (LUT_INIT, FF_INIT, INT_PIP) to blocked, which
the SPEC greedy rule forbids. The bridge is load-bearing, as claimed.

### 2. Delegation: PASS, everything except the one tile goes to the inner DeviceFile

Read at the merged tip, `helion-bitgen.rs` lines 221-275. `part`,
`idcode`, and `config_order_fars` delegate with trait-qualified calls
(`<DeviceFile as BitgenDevice>::part(&self.inner)` etc.), which also
avoids the critic session 12 M1 inherent-method trap: the trait method,
not the inherent `DeviceFile::part`, is what the emitter sees.
`tile_type` and `tile_base_far` special-case exactly one string,
`HPIO_RIGHT_X0Y0`, returning `Some("HPIO_RIGHT")` and `Some(0x000)`,
and delegate everything else. `tile_names` returns the stored list,
built in `load_device` as the real list plus the bridged tile, sorted
and deduped. No other delegation path exists in the struct.

### 3. Invented 0x000 pinned by a test and flagged provisional: PASS

The value is pinned by `real_device_fars_match_device_file`
(`helion-bitgen.rs:706-711`):

```
assert_eq!(device.tile_type("HPIO_RIGHT_X0Y0"), Some("HPIO_RIGHT"));
assert_eq!(device.tile_base_far("HPIO_RIGHT_X0Y0"), Some(0x000));
```

with the comment "invented, pinned here so any change is deliberate, and
must never be consumed as ground truth", plus a fail-loud pin that a tile
the file does not cover at all still returns `None`
(`CMT_RIGHT_X0Y0`, lines 713-715). The same test pins the real values it
sits beside (SLICE_X0Y0 0x500, INT_X0Y0 0x400, IDCODE 0x04A6_3093).

Flagged provisional in two doc blocks, not hidden: the module header
(lines 32-38) says the bridge "must never be consumed as ground truth"
and "dies the day the xcku3p tilegrid lands"; the `load_device` doc
(lines 207-216) says "admitted invented (no public HPIO column major
exists)" and "dies when the xcku3p tilegrid lands". The COORD.md
orchestrator session 12 entry records the same.

No greenness depends on the invented value. The merged binary was run
live against the blinky golden at the merged tip: emit wrote a
16,585,732-byte bitstream (exactly the orchestrator's count), and `bits`
reports the HPIO bits at FARs 0x00000002 and 0x00000003 labeled
IO_CONFIG. The orchestrator12 harness run
(`harness/runs/orchestrator12/diff.jsonl`, read directly) shows
IO_CONFIG blocked on both designs with the tilegrid missing reason,
while LUT_INIT, FF_INIT, INT_PIP stay green and FRAMING stays red 1.
The invented FARs are never compared as truth.

The pinning test was re-run by the reviewer at the merged tip:
`cargo test -j2 -p helion-ultrascale --bin helion-bitgen`
(log /tmp/critic14-bintests.log): 7 passed, 0 failed, finished in
264.81s, including `real_device_fars_match_device_file ... ok`.

### 4. tile_names includes the bridged tile: PASS, failure mode reproduced

`load_device` pushes `"HPIO_RIGHT_X0Y0"` into the name list
(line 272), and the test asserts
`device.tile_names().contains(&"HPIO_RIGHT_X0Y0".to_string())`
(line 717). The UnexplainedBits failure mode is real and was reproduced
in scratch (variant B: bridge restored, only the `names.push` line
removed; incremental rebuild 2.78s, log /tmp/critic14-buildB.log).
Emit succeeds (16,585,732 bytes), then parse fails:

```
./target/debug/helion-bitgen parse --bit /tmp/critic14-variantB.bit --part xcku3p
parse exit:1
helion-bitgen: parse failed: FAR 0x00000002 has set bits no feature explains: [73, 74, 109, 123, 124, 125, 126, 127, 129, 130, 132, 133, 134, 201]
```

That bit set is byte-identical to the orchestrator's first-F4-attempt log
and to the reviewer's own base-binary `bits` output for FAR 0x00000002
(14 bits, same list). The merged binary's parse-back renders both HPIO
lines (`HPIO_RIGHT_X0Y0.IOB_X0Y0.IOSTANDARD_IN.LVCMOS12` and
`...IOB_X0Y1.IOSTANDARD_OUT.LVCMOS12_IDRIVE_I2_SLEW_SLEW_SLOW`), so the
bridged tile is visited by parse only because `tile_names` includes it.

### 5. No other tile bridged, no other behavior changed: PASS

`grep -n HPIO_RIGHT crates/helion-ultrascale/src/bin/helion-bitgen.rs`
shows the string only as `HPIO_RIGHT_X0Y0`: the two bridge checks
(lines 240, 247), the `names.push` (line 272), the pinning test
(lines 710-711, 717), and one `classify_feature` test (line 804).
No other tile name is special-cased anywhere in the bridge.

Merge integrity, checked by the reviewer:
`git diff fa58d5c 9290ce6` is empty (merge tree byte-identical to
9290ce6). Commit 9290ce6 touches exactly one file,
`crates/helion-ultrascale/src/bin/helion-bitgen.rs`, 119 insertions,
68 deletions (`git diff c95da1a 9290ce6 --stat` shows the same single
file, so the F4 wiring is the only change over the rebased emitter).
Merge fa58d5c has exactly two parents, 1ad06df and 9290ce6
(`git rev-list --parents -n 1 fa58d5c`), and `git diff 1ad06df fa58d5c
--stat` is 12 files, 7629 insertions, 2 deletions, matching the
orchestrator's merge stat. The StubDevice table is fully deleted from
the binary: `grep -c StubDevice` on `helion-bitgen.rs` returns 0.

### Carried observations (not defects, no ruling needed)

- The bridge lives in the binary only. The library-level `DeviceFile`
  `BitgenDevice` impl still returns `None` for HPIO tiles, so a future
  library consumer (for example the BITGEN stage in `ultra.rs`) that
  meets HPIO lines fails loud instead of using the bridge. That is the
  documented shape, the bridge dies with the tilegrid, and the harness
  only drives the binary, so there is no live gap.
- The invented FARs 0x00000002 and 0x00000003 appear in `bits` output
  labeled IO_CONFIG. Any future consumer that reads `bits` output as
  ground truth would inherit the invented value. The harness blocks
  IO_CONFIG before comparing, so no current greenness depends on it.

### Commands run (all by me, in the base worktree unless noted)

- `git log --oneline -3`, `git status --short`, `git branch --show-current`
  in ~/workspace/helion-ultra-bitstream: tip fa58d5c, branch
  fm/ultra-bitstream, clean except untracked bitfiltrator-ref/.
- `git diff fa58d5c 9290ce6`: empty. `git diff c95da1a 9290ce6 --stat`:
  1 file. `git rev-list --parents -n 1 fa58d5c`: parents 1ad06df, 9290ce6.
- Base binary at the merged tip: emit blinky golden, 16,585,732 bytes;
  `bits` shows HPIO bits at FAR 0x00000002 (14 bits) and 0x00000003
  (13 bits); parse-back renders both HPIO lines.
- Scratch variant A (bridge deleted): emit fails on both goldens,
  `unknown tile HPIO_RIGHT_X0Y0`, exit 1, no bitstream written.
- Scratch variant B (bridge kept, tile dropped from `tile_names`):
  emit succeeds, parse fails exit 1 with the UnexplainedBits set above.
- `cargo test -j2 -p helion-ultrascale --bin helion-bitgen` at the
  merged tip: 7 passed, 0 failed, 264.81s (log /tmp/critic14-bintests.log).
- Scratch worktree /tmp/critic14-scratch removed after the reproductions;
  base worktree verified clean afterward.

## Session 15: correctness critic review of the SRVAL queue item (merged tip fa58d5c)

Scope: ONLY the SRVAL item at merged tip fa58d5c, branch
fm/ultra-bitstream, worktree ~/workspace/helion-ultra-bitstream:
harness golden completion (worker 1) plus the harness resolver wiring
(worker 2), orchestrator session 13, COORD.md:2896 and COORD.md:2971.
The emitter was passed by session 13 and the HPIO bridge by session
14; neither is re-reviewed here. Every check below was run by the
reviewer on 2026-11-08; nothing is taken from the orchestrator session
13 report on trust. Scratch script at /tmp/critic15-delta-check.py
(independent DB check, kept out of the project tree).

### Verdict: PASS, no defects. FRAMING green and the FF_INIT total change are formal.

### 1. Golden changes: PASS, correct at the bit level, nothing else changed

`gen_golden.py` build_fasm (line 156) now emits
`SLICE_X0Y0.<BEL>.SRVAL = 0` immediately after each FF's INIT line.

- Counts from the on-disk goldens: `grep -c SRVAL` gives blinky 1
  (AFF), counter 4 (AFF, BFF, CFF, DFF). The xcvu3p goldens carry the
  same counts (blinky 1, counter 4).
- sha256 matches the orchestrator's claim exactly:
  `sha256sum golden/blinky.xcku3p.fasm` =
  378bd098a02dc04abab27282cbfa966068ef8a73bb3cb4ce448ec5fd3008d7ee,
  `golden/counter.xcku3p.fasm` =
  9bcedfbd7b4e97c8659151e662f8c81e95668a505433a34714bd9ed509437ec2.
- No other golden line changed. Current blinky golden with the SRVAL
  line removed is byte-identical to the pre-SRVAL golden preserved as
  the repo fixture
  `crates/helion-ultrascale/tests/fixtures/blinky.xcku3p.fasm`
  (diff empty after stripping the fixture's 5-line provenance header).
  Counter minus its 4 SRVAL lines shows the old structure (4 LUTs, 4
  FF INITs, 3 PIPs, 2 HPIO lines) with no other edits.
- Placement hashes recomputed with the harness's own
  `gen_golden.placement_hash` match the claimed new hashes exactly:
  blinky 7bf851696e269c847fb967137a3d4e6c741fdcb6b4cfdb97dec3f8b966dbf7f1,
  counter bbb402552d5ccae18b341112bb2f838c88c1d8c8891d7507fa74878f13986a3e.
  Stripping the SRVAL lines recovers the old baseline hashes
  (blinky b65979c112456c7049770e0fd1573939148c901606eef9f7a0b8a0f74fddacfb,
  counter 1bf0ffe68ca860b2a3b7c05b56e79fc5e7f8e7949af49bb7504734a68ba012ea),
  proving the SRVAL lines are the only change to the placement-hash
  input. Seed unchanged (20261107).
- Regression test `test_every_ff_has_exactly_one_srval_zero`
  (tests/test_harness.py:85) is real: it regenerates each golden from
  build_fasm and asserts the on-disk file matches, exactly one
  `.SRVAL = 0` per FF, each immediately after its INIT line, for both
  designs and both parts. It passed in the suite re-run below.

### 2. Resolver changes: PASS, delta rule verified against the raw DB by an independent script

`reference.py` resolve_srval (lines 210-250) derives the SRVAL
position from the INIT RegLoc entry (same frame/minor, bit delta -1
for A/C/E/G, +1 for B/D/F/H, bit = NOT(value)) and cross_check_srval
(lines 438-475) checks it against the prjuray .SRVAL.V0/.V1 lines.
`diff_engine.py` (lines 81-86) now calls `ref.resolve_srval` and
returns FF_INIT / absolute, with a fail-loud value check (0 or 1).
`fasm.py` classify already maps SRVAL to FF_INIT (line 165).

Independent verification, not reusing harness code
(/tmp/critic15-delta-check.py, reads the BitFiltrator arch JSON RegLoc
and the four vendored prjuray CLB DBs directly, exit 0):

- RegLoc minor is 12 for all 4 tile types x all 60 Y offsets x all 16
  FFs (AFF..HFF plus AFF2..HFF2): 0 non-12 minors.
- 128 FF positions checked (4 tile types x 16 FFs x INIT/SRVAL),
  0 mismatches: INIT.V0 equals the RegLoc position; SRVAL.V0 equals
  (same minor, INIT offset +/- delta); V1 is the exact complement at
  the same position with value 0. Polarity bit = NOT(value) holds for
  SRVAL exactly as for INIT.
- Golden FF spot values on CLEL_R, read from the DB: AFF INIT.V0
  12_02, SRVAL.V0 12_01 (delta -1); BFF 12_04 / 12_05 (+1); CFF 12_08
  / 12_07 (-1); DFF 12_10 / 12_11 (+1).
- SRVAL derived offsets stay in-frame (0..2975) for all Y offsets and
  never collide with an INIT offset: 0 out of frame, 0 collisions.
- The stale wording is gone: `grep -rn "no golden uses them"` over
  the harness returns nothing (exit 1).
- Regression test `test_srval_resolves_to_prjuray_db_positions`
  (tests/test_harness.py:311) is real: it reads the .SRVAL.V0
  position from the DB (not hand-typed), asserts resolve_srval returns
  exactly that position with bit = NOT(value) for both values, asserts
  cross_check_srval true for all 16 FFs, and resolves every golden
  SRVAL line through resolve_feature with no MissingReference. The
  far_base in the test (0x50c) is independently derived: block 0, row
  0, col major 5, minor 12.

### 3. The claimed green: PASS, re-run by the reviewer, byte-identical output

Binary provenance first: `cargo build -j2 -p helion-ultrascale --bin
helion-bitgen` at merged tip fa58d5c finished in 1.17s with no
recompilation, so `target/debug/helion-bitgen` (16,417,128 bytes,
Nov 8 14:58) is the merged-tip binary. Base worktree tip fa58d5c,
branch fm/ultra-bitstream, clean except untracked bitfiltrator-ref/.

Harness test suite, re-run by the reviewer:

```
cd ~/workspace/ultra-bitgen-crew/harness
python3 -m unittest discover -s tests
Ran 36 tests in 9.207s
OK
```

The worker's claim (36 tests OK) is exact.

Harness run, re-run by the reviewer with the merged-tip binary:

```
cd ~/workspace/ultra-bitgen-crew/harness
HELION_BITGEN_EMITTER=/home/hatch/workspace/helion-ultra-bitstream/target/debug/helion-bitgen python3 diff.py --design all --part xcku3p --out runs/critic15
exit 0, wrote 20 records to runs/critic15/diff.jsonl (521.6s)
```

`cmp runs/critic15/diff.jsonl runs/srval-complete/diff.jsonl` is
empty: the reviewer's run is byte-identical to the claimed
runs/srval-complete. Read directly from runs/critic15/diff.jsonl:

- blinky: LUT_INIT 0/64 green, FF_INIT 0/2 green, INT_PIP 0/11
  green, FRAMING 0/3 green.
- counter: LUT_INIT 0/256 green, FF_INIT 0/8 green, INT_PIP 0/17
  green, FRAMING 0/3 green.
- IO_CONFIG blocked on both (total 55, tilegrid missing, same reason
  as before). BRAM_CONTENT, BRAM_PARITY, DSP48E2, URAM288, CLK
  blocked on both, not exercised by the v1 goldens.
- Manifest runs/critic15/manifest.json records the new placement
  hashes, seed 20261107, tier tier1, and the cross-checks (ff-AFF..
  ff-DFF match true; lut checked 64, mismatches empty).

Direct bit check, independent of the harness diff: the reviewer
emitted the blinky golden with the merged-tip binary
(/tmp/critic15-blinky.bit, 16,585,732 bytes) and ran `bits` on it.
FAR 0x50c reports exactly `0000050c 1 1 FF_INIT` and
`0000050c 2 1 FF_INIT`: bit 1 is SRVAL.V0 (12_01), bit 2 is INIT.V0
(12_02), matching critic 13's predicted raw-frame proof (frame 0x50c
word0 = 0x6). The SRVAL bit is set in the emitted bitstream. The
same `bits` output confirms LUT 0x509 bits set, INT GCLK 0x429 bit 3,
and INT BOUNCE 0x440 bit 0.

### 4. FF_INIT total change and no-regression: PASS

Semantic comparison of runs/srval-complete/diff.jsonl against
runs/orchestrator12/diff.jsonl (placement_hash stripped, since the
golden change legitimately changes it): exactly 4 records differ,
and only in the expected fields:

- blinky FF_INIT total_bits 1 to 2 (status stays green, diff 0).
- counter FF_INIT total_bits 4 to 8 (status stays green, diff 0).
- blinky FRAMING red to green, diff_bits 1 to 0, detail now
  "round-trip green: 3 passed, 0 failed, 0 skipped".
- counter FRAMING red to green, diff_bits 1 to 0, same detail.

The totals are correct accounting: INIT plus SRVAL bits both counted
(blinky 1 + 1 = 2, counter 4 + 4 = 8). No other block changed
status, diff_bits, or total_bits. Against runs/srval-golden, exactly
the 2 FF_INIT lines differ (blocked to green, totals 1 to 2 and 4 to
8), confirming the resolver wiring closed the interim gap with no
other movement.

The two updated test assertions are legitimate arithmetic, not
goldens copied from the implementation: blinky FF_INIT total 2 = 1
INIT + 1 SRVAL (test_harness.py:422-423, with the counting rule in
the comment), and the partial-counter case total 7 = 3 specified
INITs (DFF.INIT dropped) + all 4 SRVALs (test_harness.py:524-527).

### 5. Scope: PASS, harness-side only, no emitter, crate, or base change

Repo tree at tip fa58d5c is clean (`git status --short` shows only
untracked bitfiltrator-ref/), so no crate or emitter file changed
with this item. The item touches only harness files (gen_golden.py,
reference.py, diff_engine.py, tests/test_harness.py, golden/*.fasm),
which live outside the repo. No CARRY8 registry work, no IO_CONFIG
tilegrid work, no change to the emitter binary. No em dashes in any
edited file (0 in gen_golden.py, reference.py, diff_engine.py,
tests/test_harness.py, and all four golden FASM files, checked by
character count).

### Carried observations (not defects, no ruling needed)

- `helion-bitgen bits` is slow on full-device bitstreams: the single
  `bits` run on the 16,585,732-byte blinky bitstream took 111s, and
  the reviewer's full harness run took 521.6s, dominated by `bits`
  invocations. This is a performance note only; every `bits` output
  was correct and the harness run completed. A faster `bits` (or a
  frame-indexed parse) would shorten the greedy loop, but nothing
  here depends on it.
- The earlier packet-walk attempt by this reviewer failed on a wrong
  header-field assumption and is not recorded as evidence; the
  `bits`-based bit check above replaces it. No conclusion in this
  entry rests on a hand-rolled packet walker.

### Block status after this review

LUT_INIT, FF_INIT, INT_PIP, FRAMING are green tier 1 on xcku3p for
blinky and counter against the merged emitter on the real device
file, critic re-run. IO_CONFIG remains the standing largest blocked
item (55 bits on both designs, tilegrid). BRAM_CONTENT, BRAM_PARITY,
DSP48E2, URAM288, CLK remain blocked, not exercised by the v1
goldens. Tier 2 (real Vivado bitstream) is untouched by this item.

## Session 16: correctness critic review of the INT column-offset fix (merged tip 060b230, commit a63f49c)

Scope: ONLY the tile-type-dependent INT column-offset fix in commit
a63f49c (merge tip 060b230 on fm/ultra-bitstream) plus the harness-side
change in harness/reference.py (int_col_offset_from_clb). The FRAMING
green, the HPIO bridge, the tilegrid question, and every other block
were NOT re-reviewed here.

Verdict: PASS, no defects. The INT fix is formal.

### What was claimed

INT tiles sit at CLB column major -1 ONLY for CLEL_R tiles; for
CLEL_L, CLEM, and CLEM_R tiles the INT tile is at column major +1.
The old -1-only rule was wrong for 61 of 113 CLB columns per row.
The fix: bitgen_device.rs int_col_major gains a +1 branch with the
same 76-minor validation, -1 restricted to CLEL_R; harness
reference.py gains int_col_offset_from_clb; INT_X1Y0 and INT_X7Y0 now
resolve on the +1 branch; tile_names counts updated (xcku3p INT 208
to 452, xcvu3p INT 364 to 791); tail columns 113..117 fail loud.

### Independent reproduction 1: xcku3p minors table (my own script, raw JSON)

I wrote /tmp/critic16-minors-check.py from scratch, reading
crates/helion-ultrascale/data/xcku3p.json and xcvu3p.json directly
(no Rust code, no harness code). For every CLB column in every FAR
row it reads the tile type and column major, then checks whether the
left (major - 1) and right (major + 1) neighbours carry at least 76
minors.

xcku3p (4 rows, 113 columns per row): left-only 208, right-only 244,
both 0, neither 0. By type: CLEL_R left 208; CLEL_L right 36; CLEM
right 184; CLEM_R right 24. Per row: 52 left, all CLEL_R; 61 right,
all other CLB types. Rule-resolving columns across all rows: 452.
First-5 entries match the researcher report exactly: (0, CLEL_R,
major 5, left 76, right 16), (1, CLEM, major 6, left 16, right 76),
(2, CLEL_R, major 8, left 76, right 6), (3, CLEL_R, major 11, left
76, right 16), (4, CLEM, major 12, left 16, right 76). The
orchestrator's 52/61 split is reproduced exactly, including the
zero overlap and zero neither.

xcvu3p (7 rows, 5x117 + 2x118 columns): left 364, right 427,
neither 30, both 0. By type: CLEL_R left 364; CLEL_L right 63; CLEM
right 322; CLEM_R right 42 plus neither 30. Rule-resolving: 791.
The 30 neither are exactly the tail columns 113..117 (4 columns x
5 rows of 117 plus 5 columns x 2 rows of 118). Spot value: column
113 is CLEM_R, major 230, left neighbour 8 minors, right neighbour
16 minors, matching the test comment verbatim.

### Independent reproduction 2: xczu3eg tilegrid (my own script, prjuray-db-ref)

I wrote /tmp/critic16-tilegrid-check.py from scratch, reading
~/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/tilegrid.json
directly. For every CLB tile it looks up the tile types at grid
positions (grid_x - 1, grid_y) and (grid_x + 1, grid_y).

Result over all 8,820 CLB tiles: CLEL_R INT on the left, 4,500
tiles, 0 on the right; CLEL_L INT on the right, 720 tiles, 0 on the
left; CLEM INT on the right, 2,520 tiles; CLEM_R INT on the right,
1,080 tiles. Every CLB tile has exactly one INT neighbour, on the
claimed side. The researcher's Q5.2 counts (CLEL_R L 4,500; CLEL_L
R 720; CLEM R 2,520; CLEM_R R 1,080) are reproduced exactly. Two
independent public sources (xczu3eg tilegrid geometry, xcku3p
minors table) agree on the split, and my counts come from my own
scripts, not from the orchestrator's or the researcher's output.

### Code review (read, not trusted)

- int_col_major (bitgen_device.rs:162-185): branches on
  device.clb_tile_type, CLEL_R takes checked_sub(1), every other
  type takes checked_add(1), then the same 76-minor validation
  (INT_FRAME_SPAN = 76, line 104) and None on failure. The +1
  branch cannot silently resolve a wrong column: a neighbour under
  76 minors still fails loud. The else branch covers CLEL_L, CLEM,
  and CLEM_R; there is no fifth CLB tile type in either device
  file (verified by the minors-table script: exactly four types
  appear). The doc comment above the function states the corrected
  rule, cites both sources with their counts, and keeps the
  provisional flag plus the statement that the +1 side has never
  been exercised by a golden.
- Harness reference.py: int_col_offset_from_clb (line 56) returns
  -1 for CLEL_R, +1 otherwise, with a provisional comment noting
  the +1 side is unexercised by goldens. resolve_int_pip (line
  333) applies it to the (col_major, tile_type) pair returned by
  _clb_column (lines 157-167), which reads both values from the
  device file row, not from a hardcoded type. No stale reference
  to INT_COL_OFFSET_FROM_CLB remains anywhere in the harness
  (grep, 0 hits).
- Merge diff fa58d5c..060b230 touches exactly 2 files:
  crates/helion-ultrascale/src/bitgen_device.rs (+53/-22 in the
  file's hunks, 71 insertions / 37 deletions total with the test
  file) and crates/helion-ultrascale/tests/bitgen_device.rs. The
  merge tree is identical to a63f49c (git diff a63f49c 060b230
  empty). The HPIO bridge is untouched: git diff fa58d5c..060b230
  on crates/helion-ultrascale/src/bin/helion-bitgen.rs is empty.
  Zero em dashes in the commit (character count 0).
- Test diff is confined to the INT resolution test and the two
  count tests. No assertion was weakened except the intentional
  INT_X1Y0 flip from None to Some, which is the behaviour change
  itself and is pinned by the new value assertions (tile_type
  INT, base FAR 0x700). New positive assertions added: INT_X7Y0
  resolves to 0x1300, INT_X1Y0 present in tile_names,
  INT_X112Y3 present, xcvu3p INT_X113Y0 absent. The claimed test
  values were verified against the raw device file by my own
  reads: xcku3p column 1 is CLEM major 6 with right neighbour 76
  minors (FAR 0x700); column 7 is CLEL_L major 18 with right
  neighbour 76 minors (FAR 0x1300); xcvu3p column 113 is CLEM_R
  major 230 with neighbours 8 and 16 minors.
- The two new harness tests are real and pin the +1 branch end to
  end: test_int_col_offset_is_tile_type_dependent asserts -1 for
  CLEL_R and +1 for the other three types; test_int_pip_resolves_
  on_clem_column resolves the BOUNCE PIP on CLB column 1 and
  asserts the bits land at column major 7, not 5 (the old -1
  result), with set bit (0x700, frame 64) = 1 and zero bit (0x700,
  frame 66) = 0.

### Test runs (run by me at merged tip 060b230)

- cargo test -j2 -p helion-ultrascale: 153 passed, 0 failed, 14
  suites (82 + 7 + 13 + 9 + 2 + 3 + 3 + 2 + 1 + 22 + 4 + 2 + 3 +
  0), log /tmp/critic16-cargo-test.log. First attempt failed to
  start because cargo is not on the default PATH in a fresh shell
  (cargo lives at ~/.cargo/bin); re-ran with the PATH fixed, so
  the 153 count is from a real run, not a stale log.
- Harness suite: python3 -m unittest discover -s tests, Ran 38
  tests, OK (was 36, +2 new tests).
- Harness run comparison: cmp runs/tilegrid-int-rule/diff.jsonl
  runs/srval-complete/diff.jsonl, byte-identical, 0 differing
  records. No block regressed: LUT_INIT, FF_INIT, INT_PIP,
  FRAMING still green on both designs.
- cargo fmt --check -p helion-ultrascale: exit 0, clean.
- cargo clippy -j2 -p helion-ultrascale: exit 0, 0 warnings.
  Plain cargo clippy --all-targets still fails on the pre-existing
  helion-sv lib.rs:15059 error (overly_complex_bool_expr); zero
  files in this diff are under helion-sv, so that failure is
  carried, not introduced.

### Carried observations (not defects, no ruling needed)

- The +1 side remains absolute-provisional in the strict sense:
  no xcku3p tilegrid exists, and no golden exercises a +1 INT
  tile (every golden INT line is INT_X0Y0 on CLEL_R column 0).
  The rule is now grounded in two independent public sources
  rather than one example, but it is still a rule, not a tilegrid.
  The tilegrid stays blocked pending the Q6 Vivado probe; nothing
  in this review changes that.
- INT_X2Y0 and INT_X1Y0 now resolve to the same INT column
  (major 7, base FAR 0x700). That is correct per the interleave
  pattern (one INT column serves the CLB column on each side),
  and both the Rust and harness sides agree on it.

### Block status after this review

LUT_INIT, FF_INIT, INT_PIP, FRAMING remain green tier 1 on xcku3p
for blinky and counter, critic re-run, with INT placement now
correct for all 113 CLB columns per row instead of 52. IO_CONFIG
remains blocked (55 bits, tilegrid). BRAM_CONTENT, BRAM_PARITY,
DSP48E2, URAM288, CLK remain blocked, not exercised by the v1
goldens. Tier 2 (real Vivado bitstream) is untouched by this item.
