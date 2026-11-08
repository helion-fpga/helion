# Coordination Log

Owner: orchestrator agent. Append-only. Newest at the bottom.

## Crew roster

| Role | Agent | Worktree | Branch | Status |
|---|---|---|---|---|
| CTO / first mate | main | ~/workspace/helion-ice40 | master | active |
| orchestrator | 947547eb-c874-4698-a824-8b0a131d242d | ~/workspace/ultra-bitgen-crew | - | done (session 1 complete, duplicate-spawn resolved) |
| orchestrator (session 2) | f9b99fe4-23dc-47bc-8944-e108d0d59ba4 | ~/workspace/ultra-bitgen-crew | - | done (session 2 complete ~16:55 EST) |
| orchestrator (session 3) | a9e8f0b0-44d2-493a-8665-0d538217474d | ~/workspace/ultra-bitgen-crew | - | done (session 3 complete ~17:15 EST, INT_PIP merged, u16 ruling recorded) |
| orchestrator (session 4) | 48332feb-d9de-4395-8a2f-d70331467426 | ~/workspace/ultra-bitgen-crew | - | done (session 4 complete ~17:10 EST, lut merged as a09df12) |
| orchestrator (session 5) | ca15335b-ba87-44f3-826c-bb7a0429a70d | ~/workspace/ultra-bitgen-crew | - | done (session 5 complete ~17:40 EST, contract updates plus CTO rulings, no merges) |
| orchestrator (session 6) | f8e788f3-425e-4698-8f6d-d9bb44e72de5 | ~/workspace/ultra-bitgen-crew | - | done (session 6 complete ~17:40 EST, segbits merged as c1bef40) |
| orchestrator (session 7) | edaca706-7096-4533-8195-48c70adb07f2 | ~/workspace/ultra-bitgen-crew | - | done (session 7 complete ~17:45 EST, device-file merged as 9289fed) |
| orchestrator (session 8) | 8099cc8f-a1d6-4bc6-bd0f-c7d809dd775b | ~/workspace/ultra-bitgen-crew | - | done (session 8 complete ~17:55 EST, lut-ff merged as 7f80b54) |
| orchestrator (session 9) | 24fa1469-ef59-496d-87cf-7156ff96864a | ~/workspace/ultra-bitgen-crew | - | done (session 9 complete ~18:10 EST, bitgen-core merged as 004d7cf) |
| orchestrator (session 10) | 6491b103-6f9c-421f-b8af-f32fce6078d4 | ~/workspace/ultra-bitgen-crew | - | done (session 10 complete ~18:35 EST, integration merged as 2c8c9e3) |
| critic-correctness | e13ddb77-2643-4993-aa9a-f61e3c840d7e | - | - | done (baseline review complete 2026-11-07, verdicts in CRITIQUE.md; returns when deliverables land) |
| critic-interfaces | e64f7358-7e26-46c8-bfac-9807c9391156 | - | - | done (baseline critique complete 2026-11-07 ~16:50 EST, verdicts in CRITIQUE.md) |
| bitgen-core | c4fe04c0-d773-4cc3-a5c4-fdd6a5639f25 | ~/workspace/helion-ultra-bitgen-core | fm/ultra-bitgen-core | done (delivered 5e26453, critic session 11 PASS, merged to fm/ultra-bitstream 004d7cf) |
| device-file | 2a82ddaf-badd-449c-86ff-efd3f44c7b72 | ~/workspace/helion-ultra-bitgen-device | fm/ultra-bitgen-device | done (delivered 33a4b8f, critic session 7 PASS, merged to fm/ultra-bitstream 9289fed) |
| segbits-vendor | d3668f3b-e6f5-40b7-a4fd-ce595ab53585 | ~/workspace/helion-ultra-bitgen-segbits | fm/ultra-bitgen-segbits | done (delivered 98e282a, critic session 3 PASS, rebased to c1bef40, merged to fm/ultra-bitstream c1bef40) |
| lut-ff-bram | 3b3ded28-0720-4c10-9219-219597ca8979 | ~/workspace/helion-ultra-bitgen-lut | fm/ultra-bitgen-lut | done (delivered 0e81099, critic session 4 PASS, merged to fm/ultra-bitstream a09df12) |
| lut-ff-bram (session 2) | 023c2381-f473-4abf-a9dd-6ecb1b64d213 | ~/workspace/helion-ultra-bitgen-lutff | fm/ultra-bitgen-lutff | done (delivered c0b3438, critic session 9 PASS, rebased to 7f80b54, merged to fm/ultra-bitstream 7f80b54) |
| int-pip | e387430b-9624-4345-af15-8a5cc4c8808f | ~/workspace/helion-ultra-bitgen-int | fm/ultra-bitgen-int | done (delivered 6bb30b6, critic PASS tier 1, merged to fm/ultra-bitstream) |
| diff-harness | 175ea4d8-99ec-4fce-b49c-36219ff6ace0 | ~/workspace/helion-ultra-bitgen-diff | fm/ultra-bitgen-diff | active |
| integration | f290fb7a-9c0d-4b0b-9c0a-01fcd68d95a9 | ~/workspace/helion-ultra-bitgen-integ | fm/ultra-bitgen-integ | done (delivered 3e6d17f, critic session 10 PASS with hygiene items, rebased onto 004d7cf, merged to fm/ultra-bitstream 2c8c9e3) |

## Decisions

- 2026-11-07 (CTO): branch is fm/ultra-bitstream, tip aee77e3 (day-1 scout
  report.md plus helion-ultrascale crate merged from fm/ultra-device
  7a4146f). All bitgen work lands on fm/ultra-bitgen-* branches cut from
  aee77e3, integrated back to fm/ultra-bitstream by the orchestrator.
- 2026-11-07 (CTO): identical means two tiers. Tier 1 (today, no Vivado):
  zero diff against the public segbits DBs for the same FASM design.
  Tier 2 (needs Vivado): zero diff against a Vivado bitstream for the same
  design. Tier 2 is blocked on the license server, unchanged since Oct 22.
- 2026-11-07 (CTO): control part is xcku3p first (BitFiltrator ships a
  full device file plus demo.py reconstruction), then xcvu3p. Never debug
  pipeline and device file at the same time.
- 2026-11-07 (CTO): greedy loop is locked. Diff harness orders blocks by
  diff size descending; fix largest first; re-run full diff after each fix;
  a fix that regresses a green block is reverted on the spot.
- 2026-11-07 (CTO ruling, critic-interfaces FLAG on prjuray-db CC0
  verification pending): no spec change needed. The segbits-vendor brief
  already requires verifying the CC0 1.0 license file in the fresh clone
  and recording source plus license in each vendored file header or a
  LICENSES note before any segbits file lands under
  crates/helion-ultrascale/data/. The critic re-checks at vendoring time;
  nothing is vendored until then. FLAG stays open in CRITIQUE.md until
  segbits-vendor verifies the license; the ruling is recorded here so the
  FLAG is not re-raised as a spec question.
- 2026-11-07 (CTO rulings, critic-correctness base review, F1 and F2
  verified by the CTO in the code): F1 Bel::name() not injective
  (bel.rs:139) plus F2 stale zero-clippy claim (smoke_place.rs:43) plus
  G2 device.rs grid is invented, not ground truth, plus G3 BRAM
  accounting (720 RAMB36E2 total, RAMB18 halves are alternate views,
  never both) are routed to the device-file worker, ordered F1, F2, G2,
  G3. G2 is also routed to bitgen-core: it must not consume the
  device.rs grid for FAR mapping. G3 accounting and G4 (LUTRAM V-B
  lines must be verified before any LUTRAM emission; blocked until then)
  are routed to lut-ff-bram. G1 (prjuray-db unverifiable until re-clone)
  needs no ruling: no INT_PIP verdict until segbits-vendor vendors.
  G5 and G6 carried, no action.

## Blockers

- Vivado license server 27000@license2.forest.usf.edu connection refused
  since Oct 22, 2026. Blocks tier-2 diff and xcvu3p device-file confirmation.
  Saksham follows up with the lab admin. Carried from the day-1 crew.
- PinPlanning probe (license-free) not yet run on the cluster. It unblocks
  the INT PIP completeness audit. Carried from the day-1 crew.
- /tmp/prjuray-db clone is gone (deployments wipe /tmp). segbits-vendor
  re-clones it and vendors the needed DBs into the repo.

## Build schedule

Updated 2026-11-07 ~16:52 EST: 7 workers plus critic-correctness active.
Rule stands: cargo check anytime, full release builds need orchestrator
clearance (machine OOMs on parallel release builds; -j1/-j2 max). The
orchestrator granted the CTO-spawned workers the same clearance in their
spawn briefs: cargo check freely; full builds only -j1/-j2 max via nohup
setsid with logs to files; never clean target dirs.

## 2026-11-07 ~16:50 EST: orchestrator started, workers spawned

Orchestrator agent 947547eb-c874-4698-a824-8b0a131d242d. Read all briefs,
SPEC.md, INTERFACES.md, COORD.md, CRITIQUE.md. Verified base state with
`git worktree list` and `git log` in ~/workspace/helion-ultra-bitstream:
branch fm/ultra-bitstream at aee77e3, "merge fm/ultra-device:
helion-ultrascale crate as bitgen base". All seven worker worktrees exist
at aee77e3 (helion-ultra-bitgen-core/device/diff/int/integ/lut/segbits).

Spawned 7 workers in parallel (each got its brief path, worktree, branch,
base aee77e3, and standing rules). These 7 were duplicates of the
CTO-spawned workers and were closed ~16:51 EST; see the duplicate-spawn
entry below. Build clearance granted to all workers: cargo check freely;
full builds only -j1/-j2 max via nohup setsid with logs to files; never
clean target dirs. Workers report by final message to their parent; the
orchestrator appends to COORD.md (workers do not edit COORD.md directly,
to avoid concurrent-edit conflicts).

Correction to the earlier "critics held" note in this entry: both critics
were in fact spawned by the CTO, and critic-interfaces already completed
its baseline critique. The hold decision is void.

Greedy queue status: no diff harness output yet, so no queue exists. The
diff-harness worker builds the scoreboard first; until it lands, the
foundation workers (segbits-vendor, device-file, bitgen-core) build
against INTERFACES.md with stubs where a dependency is not ready, per
their briefs.

## 2026-11-07 ~16:51 EST: duplicate-spawn incident, resolved by orchestrator

The CTO spawned all 10 crew agents directly (created 1794088223) in
parallel with the orchestrator task. The orchestrator, not yet seeing the
CTO's spawns, spawned 7 workers of its own (created 1794088284, about 61
seconds later) for the same 7 roles in the same 7 worktrees. Evidence:
`subagent.list` showed 7 CTO-spawned workers (parent 4b6f28e0) and 7
orchestrator-spawned workers (parent 947547eb) all status running, plus
critic-interfaces e64f7358 already done with its baseline critique in
CRITIQUE.md.

Resolution: the CTO's set is canonical (the CTO's roster update names
those ids). The orchestrator closed all 7 of its duplicate workers
(5803ac2b segbits-vendor, 6ba93042 device-file, 4c8a125a bitgen-core,
49842246 lut-ff-bram, 6bb4a51c int-pip, 2ba74463 diff-harness, 54d77e46
integration); all 7 returned shutdown. The duplicates ran about 2 minutes
in read-briefs phase only. Verified after the close with `git rev-parse`
plus `git status --porcelain` in all 7 worktrees: every worktree still at
aee77e3 with zero changed files. No work lost, no branch conflict. The
base worktree shows pre-existing `M Cargo.lock` and untracked
`bitfiltrator-ref/` (the reference clone per SPEC.md); neither came from
the duplicates, which worked only in their own worktrees.

Roster correction: critic-interfaces e64f7358 is recorded as done, not
active. It completed its baseline critique at ~16:50 EST (PASS on no
interface drift, PASS on provenance baseline, PASS clearing
netlist_interchange.rs, FLAG on prjuray-db CC0 verification pending).
Marking a completed agent active would be false; the CTO's "mark each
active" instruction is taken to mean "no longer spawning". Critic
correctness e13ddb77 is running; its baseline verdict is pending.

Lesson for the crew: one spawner per crew. The orchestrator owns the
roster from here on; the CTO routes spawn requests through the
orchestrator instead of spawning workers directly.

## 2026-11-07 ~16:55 EST: orchestrator session 2 started

Orchestrator session 2 agent f9b99fe4-23dc-47bc-8944-e108d0d59ba4. Took
over from session 1 (947547eb, complete). Read SPEC.md, INTERFACES.md,
COORD.md, CRITIQUE.md. No workers spawned: all 7 worker roles are
already filled by the CTO-spawned agents (bitgen-core c4fe04c0,
device-file 2a82ddaf, segbits-vendor d3668f3b, lut-ff-bram 3b3ded28,
int-pip e387430b, diff-harness 175ea4d8, integration f290fb7a). Both
critics completed baseline passes (critic-interfaces e64f7358 done,
critic-correctness e13ddb77 done); they work in sessions and return when
deliverables land.

Base worktree verification: `git status --porcelain` in
~/workspace/helion-ultra-bitstream returns only `?? bitfiltrator-ref/`
(untracked reference clone, per SPEC.md). No modified Cargo.lock: the
session-1 pre-existing `M Cargo.lock` is gone, the correctness critic
restored it after its cargo test run (CRITIQUE.md G6) and the restore
held. No restore was needed and none was done. Branch fm/ultra-bitstream
at aee77e3, confirmed with `git rev-parse --abbrev-ref HEAD` plus
`git rev-parse --short HEAD`.

Spot-checked the routed findings in the base code: bel.rs:139 fallback arm
`_ => Some(self.kind.unisim().to_string())` (F1); smoke_place.rs:43
`let placements = vec![...]` (F2); device.rs build_xcvu3p 144x400 grid
(G2); device.rs Bram tile creates one Ramb36e2 plus two Ramb18e2 BELs
(G3). CTO rulings recorded in Decisions above.

Greedy queue state: no diff harness output yet. Verified the diff
worktree ~/workspace/helion-ultra-bitgen-diff is clean at aee77e3 with
zero commits past base. No vendored segbits data exists yet
(`crates/helion-ultrascale/data/` absent) and /tmp/prjuray-db is still
gone. No queue exists until diff-harness lands the scoreboard.

Build schedule confirmed, unchanged: cargo check freely; full builds only
-j1/-j2 max via nohup setsid with logs to files; never clean target
dirs. No release build in flight.

## 2026-11-07 ~17:00 EST: int-pip DONE (commit 6bb30b6 on fm/ultra-bitgen-int)

Worker e387430b. Branch fm/ultra-bitgen-int, base aee77e3. Delivered in
crates/helion-ultrascale: `src/segbits.rs` (SegbitsDb trait per
INTERFACES.md plus strict FASM loader), `src/int_pip.rs` (PIP emission),
`tests/int_pip.rs` (routed-design verification). Evidence, all with exact
commands:

- `cargo test -j2 -p helion-ultrascale`: 26 passed, 0 failed (22 unit, 2
  integration, 2 pre-existing smoke). `cargo fmt -p helion-ultrascale`
  clean. Clippy: 0 new warnings (1 pre-existing `vec!` warning in
  tests/smoke_place.rs:43, untouched, not this block).
- BOUNCE hand-check: `INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1` resolves to
  set 64_00 and zero 66_00, 68_00, 72_00, 72_01, 74_00, exactly as in
  the DB line.
- Full DB: all 3778 lines of zynqusp/segbits_int.db (prjuray-db commit
  affbc5e, CC0 1.0, COPYING verified) parse and resolve, and the loader's
  bits agree with an independent second parser on every line.
- Routed design: blinky net routed across INT_X12Y45 on an 11-hop path
  (BOUNCE_E_0_FT1 to NN12_END7) built only from real DB edges; every
  emitted bit verified by independent DB lookup; 11 set bits, 48 zero
  bits; every `!` bit asserted 0 against the all-zero base frameset;
  0 missing PIPs.
- Missing-PIP list for the verified blinky route: empty. The mechanism is
  exact: `resolve_pips` partitions into emitted/missing with tile, src,
  dst, design; `emit_pips` fails loud with the full list. Negative test
  proves a bogus PIP lands on the list and stops emission.

Corrections found while working (all in code and tests, not assumed):

- Segbits bit index is NOT 0..32. Observed range in the DB is frame 0..75,
  bit 0..47. The parser accepts the full u8 range and lets the DB be the
  authority.
- The day-1 router scaffold's mock WireDb cannot name wires, so it cannot
  name PIPs. The routed test builds its named-wire graph from real DB
  edges instead. Open interface item for integration: WireDb needs a
  wire-ID-to-wire-name seam before production routing can feed this
  emitter.

Dependency note: segbits-vendor has not landed yet (worktree still at
aee77e3, no data/ dir). This worker used the surviving reference clone
at ~/workspace/prjuray-db-ref (commit affbc5e, CC0 verified) for
verification only and vendored nothing. The integration test locates the
DB via HELION_SEGBITS_INT_DB, then the reference clone, then
crates/helion-ultrascale/data/segbits_int.db, and fails loud naming all
three paths if none exists. Once segbits-vendor lands the vendored copy,
the test resolves through the repo path. The `FileSegbitsDb` loader here
is the shared implementation segbits-vendor should reuse, not duplicate.

Block status: INT_PIP tier-1 emission is implemented and verified
bit-exact against the public DB. Critic re-run still needed before the
block goes green. Tier-2 (Vivado bitstream) still blocked on the license
server, unchanged.

## 2026-11-07 ~17:05 EST: device-file worker, critic items F1/F2/G3 done

Branch fm/ultra-bitgen-device, worktree ~/workspace/helion-ultra-bitgen-device.
Three CTO rulings from the correctness critic base review are done, one fix
per commit, all verified with `cargo test -p helion-ultrascale -j2` and
`cargo clippy -p helion-ultrascale -j2 --all-targets` clean:

- F1 (aece1c6): Bel::name() injective per tile. F7Mux 0..3 are now
  F7AMUX/F7BMUX/F7CMUX/F7DMUX, F8Mux 0..1 F8AMUX/F8BMUX, F9Mux 0 F9MUX
  (UG574 site names); RAMB18 halves are RAMB18E2_A/RAMB18E2_B
  (Helion-internal disambiguator, documented in code, not a Vivado site
  name); every kind returns None above its valid index range. Tests:
  per-index wide-mux names, RAMB18 half names, out-of-range None, and
  name uniqueness across all BELs in a CLB tile and a BRAM tile.
- F2 (ad4da84): smoke_place.rs vec! with a fixed two-element literal
  replaced by an array literal; clippy --all-targets now zero warnings.
- G3 (8e73f0c): BRAM capacity accounting. One tile is one RAMB36E2 OR
  two RAMB18E2 halves, never both; the halves are alternate views, not
  extra capacity. Test asserts 720 BRAM tiles, 720 RAMB36E2, 1440
  RAMB18E2 halves (exactly 2x), plus a per-tile one-plus-two check.
- G2 (ruling, heeded): the device.rs 144x400 grid stays a logical
  model. The xcvu3p.json work below is built from the BitFiltrator
  xcku3p schema plus XMP103 counts, never from that grid.

Brief work in progress: BitFiltrator-schema device-file loader (xcku3p
control first), demo.py reconstruction validation, provisional
xcvu3p.json. Evidence so far, all from public sources, no Vivado:
AMD UG570 Table 1 gives VU3P 71,820 configuration frames (93 words
each), Table 2 gives VU3P IDCODE X4B39093 (production revision 1 or
later, so 0x14B39093); distributor listings of AMD datasheet values
give 49,260 CLBs for XCVU3P, which conflicts with the 49,250 slices
in device.rs (394,000 / 8). The 49,260 figure is the one used for the
provisional file (49,260 x 8 = 394,080 LUTs, XMP103 "394K"), and the
49,250 vs 49,260 discrepancy is flagged for the CTO, not resolved
here. Frame-count cross-check on the control: xcku3p.json sums to
41,460 frames vs UG570 KU3P 41,476, a 16-frame gap under
investigation, recorded here, not hidden. Vivado confirmation of
xcvu3p.json stays blocked on the license server since Oct 22.

## 2026-11-07 ~17:10 EST: integration worker, staging port + WireDb naming-seam proposal

Worker f290fb7a. Branch fm/ultra-bitgen-integ, base aee77e3.

Porting day-1 staging into `crates/helion-ultrascale` (in progress):
pack.rs, place.rs, legality.rs (fm/ultra-packplace c9e96c4), router.rs
plus mock (fm/ultra-router 3d49b98), timing_db.rs, sta.rs
(fm/ultra-timing 7ff0007). `cargo check -j1 -p helion-ultrascale` green
after one port fix (see below). Crate tests running at time of writing;
results to follow in the next entry.

Corrections filed (crate wins over staging, per brief):
1. pack.rs staging header said "Two slices per CLB" (7-series layout).
   Corrected to one slice per CLB (UG574). Code already named slices
   one-per-CLB; comment only.
2. pack.rs `slicel_count`/`slicem_count` incremented once per LUT, not
   once per slice. Now computed from packed slices at the end of
   `pack_ultra`.
3. router.rs `AstarScratch.gen` renamed to `generation`: `gen` is a
   reserved keyword in edition 2024 and the crate would not compile.
   No behavior change.
4. `fabric::WireDb` extended to the full day-1 contract (fanout, wire_xy,
   wire_for_bel_pin; `WireId`, `UltraSite` moved into fabric.rs).
   `EmptyWireDb` returns empty/None. This is the contract the router
   scaffold already implemented, not a new contract.

xcku3p provisional model added (`UltraScaleDevice::xcku3p`,
`Xcku3pSpec`): 20,340 slices, 1,368 DSP, 360 RAMB36, 48 URAM288, from
XMP103 v2.8 / DS890, cross-checked exactly against the BitFiltrator
xcku3p device file (num_luts 162,720 = 20,340 x 8; num_regs 325,440 =
20,340 x 16; num_dsps 1,368). Known discrepancy for the device-file
worker: the device file reports num_slices 27,120, one clock row
(6,780 slices) more than AMD's 20,340; the model uses AMD's count.
Same open question as the device-file worker's 49,250 vs 49,260 flag
for xcvu3p: this worker did NOT change the xcvu3p model (still 49,250
slices); the CTO rules on both counts together.

NOT merged: int-pip deliverable 6bb30b6 (fm/ultra-bitgen-int) is a
separate branch. Its lib.rs re-export changes overlap this branch's
lib.rs rewrite; the orchestrator reconciles at merge time. No worker
branches are merged into fm/ultra-bitgen-integ without orchestrator
order.

### PROPOSAL (needs orchestrator + CTO ruling): WireDb wire-naming seam

Open interface item from the int-pip deliverable (commit 6bb30b6,
fm/ultra-bitgen-int; recorded in the ~17:00 entry above). Status:
PROPOSED, not implemented. No code in this branch invents the seam.

Problem, as established by the int-pip worker:
- The router emits `UltraRouted { routes: Vec<Vec<WireId>> }`. Wire IDs
  carry no names and no tile.
- The int-pip emitter consumes `IntPip { tile, src, dst }` (wire NAME
  strings, e.g. `INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1`) and
  resolves each against `SegbitsDb`. A wire-ID path alone cannot name a
  PIP, so production routing cannot feed int-pip emission today.
- The day-1 router scaffold's mock WireDb cannot name wires at all (its
  wires are grid positions). The int-pip test therefore built its
  named-wire graph from real DB edges instead, which is the same input
  the production router will produce once the seam exists.
- The production WireDb (routing-graph worker) will be built from real
  DB edges, so its wires ARE named in principle; the current
  `fabric::WireDb` trait simply has no query to ask for the name.

Proposed contract change (recommendation, option A):
- Add to `fabric::WireDb` one query returning the wire's name and its
  Vivado tile name:
  ```rust
  pub struct WireInfo<'a> { pub name: &'a str, pub tile: &'a str }
  fn wire_info(&self, wire: WireId) -> Option<WireInfo<'_>>;
  ```
  Both fields are needed: the segbits lookup uses tile type `INT` plus
  feature `PIP.<src>.<dst>`, while the FASM line needs the full tile
  name (`INT_X12Y45`). One query keeps name and tile in the same node
  table as the fanout data (one source of truth, no parallel table to
  keep in sync).
- Implementations: the routing-graph WireDb returns `Some` from its
  DB-derived node table; `EmptyWireDb` and the mock return `None`.
  The router-to-PIP translator (integration-owned, to be written once
  the seam lands) maps each consecutive wire pair in a route to an
  `IntPip` via `wire_info` and fails loud on any unnamed wire. Mock-
  routed designs can never feed int-pip emission; that is expected,
  the mock is algorithm scaffolding only.
- Alternatives considered: (B) a separate `WireNames` trait implemented
  alongside `WireDb` (works, but splits one node table across two
  traits); (C) baking names into `UltraRouted.routes` (rejected: changes
  the router core and the day-1 contract more invasively).
- Files the ruling touches: `crates/helion-ultrascale/src/fabric.rs`
  (trait), `crates/helion-ultrascale/src/router/mock.rs` (None impl),
  `ultra-bitgen-crew/INTERFACES.md` (WireDb section), and the routing-
  graph worker's WireDb impl when it lands.
- Until ruled and implemented, the gate records INT_PIP as BLOCKED on
  this seam (naming), not on PIP data: the segbits data itself is
  delivered (6bb30b6, 3,778 lines verified) and the emitter is delivered.

## 2026-11-07 ~22:05 EST: lut-ff-bram worker DONE (branch fm/ultra-bitgen-lut)

Worktree ~/workspace/helion-ultra-bitgen-lut, base aee77e3. Four commits,
worktree clean. LUT INIT, FF INIT/config (INIT only, see gap), and BRAM
content plus parity emission are implemented from the BitFiltrator tables,
with per-block tests and hand-checked bits.

### Commits

- 1adf250 fix(ultrascale): distinct Bel names for indexed muxes and RAMB18
  halves (critic F1, base review). F7AMUX..F7DMUX, F8AMUX, F8BMUX, F9MUX
  per UG574; RAMB18E2_L (index 0, lower) / RAMB18E2_U (index 1, upper)
  per Project U-Ray L/U naming. Test asserts name injectivity in a tile.
- dfd08d0 fix(ultrascale): clippy useless vec! in smoke_place test
  (critic F2, base review).
- 0b83568 feat(ultrascale): vendor BitFiltrator LUT/FF/BRAM tables.
  `crates/helion-ultrascale/data/ultrascale_plus_lut_ff_bram.json`
  (4,580,694 bytes), extracted verbatim by `generate_tables.py`, plus
  `README.md` provenance. Source SHA-256
  c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c,
  MIT License, Copyright (c) 2022 EPFL VLSC. Verbatim equality with the
  source checked after extraction.
- 0e81099 feat(ultrascale): LUT/FF/BRAM bit locations and FASM emission.
  New `crates/helion-ultrascale/src/bitloc.rs`, 20 tests in
  `tests/lut_ff_bram.rs`, serde_json dependency (lock update is the
  helion-ultrascale package entry only).

### What was delivered

- `lut_locations` / `lut_placed_bits`: 64 (minor, frame_ofst) pairs per
  LUT, list index = INIT bit, y_ofst = slice_y mod 60. XOR pattern
  64'h6996966996696996 tested (32 set bits, round trip back to the input).
- `ff_location` / `ff_placed_bit`: one pair per FF, AFF=AQ through
  HFF2=HQ2, tested with INIT 0 and INIT 1 (same location, value differs).
- `bram_mem_locations` / `bram_parity_locations` plus single-bit and
  placed-bits variants: 16384 content bits (INIT word * 256 + bit) and
  2048 parity bits (INITP word * 256 + bit) per RAMB18 site,
  y_ofst = bram_y mod 24. `bram36_placed_bits` splits a RAMB36 into
  lower/upper halves.
- FASM emission in the crew contract spelling: `SLICE_XxYy.A6LUT.INIT[63:0]
  = 64'h...`, `SLICE_XxYy.AFF.INIT = 1'b0/1`,
  `RAMB18_XxYy.INIT[16383:0] = 16384'h...` plus `INITP[2047:0]`,
  `RAMB36_XxYy.INIT[32767:0] = 32768'h...` plus `INITP[4095:0]`,
  MSB-first, bit 0 = LSB.
- Load-time table validation (counts, minor <= 255, frame_ofst < 2976,
  distinct pairs per LUT/FF). Unknown tile types, indices, and BRAM
  indexes fail loud; no bit is ever guessed.
- Absolute frame addresses are NOT computed here: (minor, frame_ofst) is
  column-relative and needs the device file (row major, column major,
  SLR). That is the device-file worker's input to bitgen-core.

### Evidence

Command: `cargo test -p helion-ultrascale -j2` in the worktree.
Result: 11 unit + 20 new + 2 smoke pass, 0 fail.
Command: `cargo clippy -p helion-ultrascale -j2 --all-targets`.
Result: 0 warnings. Command: `cargo fmt -p helion-ultrascale -- --check`.
Result: clean.

### Hand-checked bits (read by hand from the source JSON, not from code)

- LutLoc, CLEL_L Y0 A6LUT: bit 0 = (minor 11, frame_ofst 15), bit 1 =
  (10, 15), bit 4 = (11, 14), bit 63 = (8, 0). H6LUT bit 0 = (11, 47).
- LutLoc, paper V-C cross-check: SLICE_X136Y247/A6LUT bit 0 = (11, 351),
  matching the paper's worked example at Y_ofst 7.
- RegLoc, CLEL_L Y0: AFF = (12, 2), AFF2 = (12, 14), BFF = (12, 4),
  EFF2 = (12, 38), HFF2 = (12, 46).
- BramMemLoc, Y0: bit 0 = (0, 0), bit 1 = (0, 12), bit 255 = (3, 107),
  bit 256 = (4, 0), bit 16383 = (255, 107).
- BramMemParityLoc, Y0: bit 0 = (0, 48), bit 255 = (31, 59),
  bit 2047 = (255, 59).
- Structural finding: frame offsets are NOT a 48-per-Y formula; they jump
  by an extra 96 at Y_ofst 30 (LUT Y29 bit 0 = 1407, Y30 bit 0 = 1551).
  Tables are vendored in full for that reason.

### CTO rulings applied

- G3 (BRAM accounting): consumed, not computed here. `BramTileUse`
  encodes one tile as a RAMB36 OR two RAMB18 halves at the type level;
  a tile can never be configured as both. Capacity (720 total) stays
  with the device-file worker. The Bel index 0 = lower half mapping is
  a model convention, flagged in code, not Vivado-verified.
- G4 (LUTRAM "same bits"): VERIFIED AGAINST THE PAPER, CLAIM REJECTED.
  Read `bitfiltrator-ref/fpl22-bitfiltrator-paper.pdf` section V-B
  (pdftotext). It says LUTRAM/register formats were discovered from the
  logic location file because they are user-state bits, while standard
  LUTs were discovered separately by fuzzing ("However, determining the
  minors and frame offsets of standard LUTs is more complicated as these
  BELs are not user-state bits, and no logic location file is available").
  It never states LUTRAM shares the LUT INIT bits. The arch JSON also
  contains no LUTRAM table. LUTRAM emission is therefore BLOCKED:
  `lut_placed_bits` with `LutMode::Lutram` returns `LutramBlocked` and
  emits nothing (tested). Unblock needs a LUTRAM table or Vivado
  logic-location data.

### Gaps and follow-ups (not hidden)

- Diff-harness block status for LUT_INIT, FF_INIT, BRAM_CONTENT,
  BRAM_PARITY on blinky and counter: NOT AVAILABLE. No diff harness
  exists yet, so no harness status can be reported. The `PlacedBit`
  output of this module (block, minor, frame_ofst, value) is the exact
  input the harness will compare against reference bitstreams.
- FF "config" beyond INIT (clock/data inversion, SRVAL, etc.) has no
  BitFiltrator table and is not emitted here. Only the INIT bit is
  covered; other FF attributes need their encoding from segbits-vendor
  or fail loud in bitgen-core.
- BRAM `INITP[2047:0]` / `INITP[4095:0]` range spellings follow the
  `INIT[...]` pattern from the day-1 scout report; bitgen-core must
  confirm them against the vendored prjuray segbits DB and fail loud if
  absent. The (minor, frame_ofst) resolution does not depend on them.
- RAMB36 INIT word split (words 0..63 lower half, 64..127 upper half)
  follows the U-Ray L/U mapping; the diff harness can verify it
  independently from the per-half locations.
- segbits-vendor may consolidate `data/` later; this worker vendored
  only the four tables it consumes, with full provenance, so work did
  not block on that lane.

## 2026-11-07 ~17:05 EST: orchestrator session 3 started, merge queue set

Orchestrator session 3 agent a9e8f0b0-44d2-493a-8665-0d538217474d. Took
over from session 2 (f9b99fe4, complete). Read SPEC.md, INTERFACES.md,
COORD.md, CRITIQUE.md. No workers spawned. Roster updated: session 2
done, session 3 active, int-pip done (6bb30b6, critic PASS tier 1),
lut-ff-bram done (0e81099, critic session 4 review running).

Routing limitation, recorded plainly: the integration worker
(f290fb7a) is CTO-spawned, so it is outside this orchestrator's send
scope (both `direct` and `descendants` return "outside scope"). The
orchestrator cannot direct it by message. The CTO must route merge
directions to it, or the orchestrator performs the merge directly in
the base worktree. For this first merge the orchestrator performs it
directly (fast-forward of a critic-passed commit, no code authored);
see the merge entry below.

Merge queue (locked order, one branch at a time, re-verify after each):
1. fm/ultra-bitgen-int 6bb30b6 (INT_PIP, critic PASS tier 1). Merging
   now, first.
2. fm/ultra-bitgen-segbits (886102a vendored DBs, c8509e6 loader port).
   NOT until its critic review (session 3) passes. After it passes,
   the segbits-vendor worker rebases onto the new fm/ultra-bitstream
   tip and the orchestrator confirms the shared src/segbits.rs stays
   byte-identical and tests/int_pip_audit.rs does not duplicate
   tests/int_pip.rs, then merges it.
3. fm/ultra-bitgen-lut 0e81099 (LUT_INIT, FF_INIT, BRAM_CONTENT,
   BRAM_PARITY emission plus tables). NOT until critic session 4
   passes. It adds crates/helion-ultrascale/data/
   ultrascale_plus_lut_ff_bram.json plus README, so the merge must
   check for data/ consolidation conflicts with the segbits-vendor
   branch (both add files under data/, different filenames, but the
   README/provenance notes must not clash). Order after segbits-vendor
   so the data/ layout is settled by the vendor lane first.

Registry change (CTO ruling 2026-11-07, G4 REJECTED, verified by the
CTO reading the lut-ff-bram report): the paper section V-B does not
state LUTRAM shares LUT INIT bits and the arch JSON has no LUTRAM
table, so LUTRAM emission stays BLOCKED as its own item
(LutramBlocked, fails loud), not covered by LUT_INIT. Recorded in
INTERFACES.md block registry as LUTRAM, BLOCKED, with the unblock
condition (LUTRAM table or Vivado logic-location data). The
diff-harness consumes this: LUTRAM is a blocked block in the
scoreboard, separate from LUT_INIT.

WireDb naming seam: still PROPOSED, not ruled. It gates the
router-to-PIP translator work, not the INT_PIP tier-1 emission
verdict (critic PASS stands: emission is bit-exact against the DB).
The seam needs a CTO ruling before implementation; the orchestrator
will route it with the next session.

## 2026-11-07 ~17:15 EST: segbits-vendor worker, delivered 98e282a

Branch fm/ultra-bitgen-segbits, base aee77e3, worktree clean. Three
commits, one fix per commit:

- 886102a vendor prjuray-db segbits DBs (CC0 1.0) under data/
- c8509e6 port FileSegbitsDb + SegbitsDb trait from fm/ultra-bitgen-int
- 98e282a widen BitRef.bit u8 to u16 so vendored HPIO/HDIO load

### License verification (CTO ruling on the FLAG, now closable)

Verified from the on-disk clone myself, before vendoring anything:
`~/workspace/prjuray-db-ref`, commit
`affbc5e555ebae16475f32e8fb2d6565d4204f3f` (2020-07-21). Its COPYING
file opens "Creative Commons Legal Code / CC0 1.0 Universal" and is
the full CC0 1.0 legal code. BitFiltrator MIT LICENSE verified in
`~/workspace/helion-ultra-bitstream/bitfiltrator-ref/` (Copyright (c)
2022 EPFL VLSC), clone commit
`79b9bd66ecae31354936143180fa578dc5732baf`. No RapidWright .dat and no
FPGA Interchange DBs were ingested. Provenance per file (source,
commit, license, SHA-256) is in
`crates/helion-ultrascale/data/PROVENANCE.md`; each `.db` file carries
a 4-line header with the same, and data lines were verified
byte-identical to upstream by SHA-256. The critic-interfaces FLAG
stays open until the critic re-checks at vendoring time; this report
is the evidence for that re-check.

### What was vendored

`crates/helion-ultrascale/data/`: segbits_int.db (3778),
segbits_hpio_right.db (1560), segbits_hdio_top_right.db (812),
segbits_hdio_bot_right.db (828), segbits_cmt_right.db (104), each plus
its `.origin_info.db` provenance file; tile_type_INT.json (INT PIP
audit source); 13 site-type JSONs (SLICEL, SLICEM, DSP48E2, BUFGCE,
BUFGCTRL, RAMB181, RAMBFIFO36, HPIOB_M/S, HDIOLOGIC_M/S, MMCM, PLL).
The 4.5 MB BitFiltrator ULTRASCALE_PLUS.json was NOT copied; its
pointers are recorded in PROVENANCE.md and the file stays with the
device-file worker.

### Loader: reused, not rewritten (CTO direction)

`src/segbits.rs` is ported byte-identical from commit 6bb30b6 on
fm/ultra-bitgen-int (`git checkout fm/ultra-bitgen-int -- <file>`,
verified identical by diff), keeping its unit tests verbatim. Only
`FileSegbitsDb` / `SegbitsDb` / `BitRef` / `SegbitsLoadError` were
taken; `int_pip.rs` and `tests/int_pip.rs` were NOT ported (they
belong to the int-pip worker). lib.rs exports the segbits items only.
The loader reads paths its callers pass; it never reads /tmp. Tests
load the vendored files via CARGO_MANIFEST_DIR paths. This also
unblocks the int-pip test's repo-path fallback
(`crates/helion-ultrascale/data/segbits_int.db`) once this branch
lands.

### Loader change: BitRef.bit u8 to u16 (one commit, 98e282a)

The ported loader was verified against segbits_int.db only (observed
bits 0..47) and rejects any bit above 255. The vendored set proves
that cannot hold: HDIO_TOP_RIGHT max bit 1390, HDIO_BOT_RIGHT 1310,
HPIO_RIGHT 1359. The second token number is a bit address within the
frame (prjuray `utils/util.py: parse_tagbit` returns `(isset,
(frame, bitaddr))`, `addr_bit2word` splits it into word/bit), and a
93-word UltraScale+ frame holds 2976 bit positions, so u16 is the
correct width. Negative control run before the change: with the u8
loader, the HPIO load fails at line 241 (`!02_259`, "bit 259 outside
0..256"). Regression tests: `bit_width_above_u8_parses_exactly`
(verbatim HDIO line, bit 1310), `all_vendored_dbs_load` (all 5 files,
7082 features), plus an HPIO integration test. The malformed test
case `64_256` was updated to `64_65536` (256 is now valid).

NEEDS-CTO / critic-interfaces: this deviates from INTERFACES.md
(`BitRef.bit: u8`). The interface needs updating to u16, or the
deviation needs a ruling.

MERGE NOTE for the orchestrator: the merge queue says to confirm the
shared `src/segbits.rs` "stays byte-identical" after my rebase onto
the merged fm/ultra-bitstream tip. It will NOT be byte-identical to
6bb30b6: it differs by exactly the u16 widening in 98e282a (verified:
`git diff 6bb30b6 HEAD -- crates/helion-ultrascale/src/segbits.rs`
shows only the u16 change plus its two regression tests). That
difference is required for the vendored HPIO/HDIO DBs to load, so the
rebase check should expect it, not treat it as drift.

### INT PIP completeness audit (part 1 done, part 2 pending)

Recorded in `crates/helion-ultrascale/data/INT_PIP_AUDIT.md` and
enforced by `tests/int_pip_audit.rs`. Enumeration source:
tile_type_INT.json, 3774 PIPs. Mapping verified empirically: a tile
pip maps to segbits feature `INT.PIP.<dst_wire>.<src_wire>` (the naive
src.dst mapping matches 0 of 3774). Results: 3770 directional PIPs
match 1:1, 4 bidirectional PIPs are covered via `.FWD` / `.REV`
pairs (8 features), 0 tile PIPs missing, 0 segbits features
unexplained. Coverage 3774 of 3774 (100 percent). Part 2 (probe
comparison vs Vivado `get_pips`) is PENDING: the PinPlanning probe
has still not been run on the cluster.

### Flag for the critic (not mine to fix)

`int_pip.rs` on fm/ultra-bitgen-int builds its feature as
`PIP.{src}.{dst}` with `IntPip.src = BOUNCE_E_0_FT1`,
`dst = INODE_E_1_FT1`, but the tile JSON's pip for that DB feature is
`INT.INODE_E_1_FT1->>BOUNCE_E_0_FT1` (src_wire INODE_E_1_FT1,
dst_wire BOUNCE_E_0_FT1). Their `src`/`dst` fields are the reverse of
the tile DB's physical direction, while the feature string itself
matches the DB. If the router ever fills `src`/`dst` in physical
direction, `feature()` would miss the DB. The critic should confirm
whether the FASM convention is sink-first (then only the field names
are misleading) or whether int-pip has the direction backwards. The
loader is direction-agnostic (exact string lookup), so this does not
affect this worker's deliverables.

### Evidence (commands and outputs)

- `cargo test -p helion-ultrascale -j1 --offline`: 19 unit + 1 audit
  + 4 integration + 2 smoke, all pass.
- `cargo clippy -p helion-ultrascale -j1 --offline --all-targets`:
  zero new warnings (one pre-existing `vec!` warning in smoke_place,
  untouched).
- Negative control for the u16 change: u8 loader fails the HPIO load
  at line 241 (`!02_259` outside 0..256); u16 loader passes.
- No code was pushed anywhere (no merge to master, nothing pushed to
  GitHub, per crew rules). Awaiting critic session 3, then rebase onto
  the merged fm/ultra-bitstream tip per the merge queue.

## 2026-11-07 ~17:10 EST: INT_PIP merged, FIRST GREEN BLOCK (tier 1)

Merge performed by the orchestrator directly in the base worktree
(see the routing limitation in the session-3 entry above: the
integration worker is outside the orchestrator's send scope, so the
CTO routes merge directions to it, or the orchestrator merges
directly; no code was authored by the orchestrator).

Pre-merge verification, all with exact commands in
~/workspace/helion-ultra-bitstream:
- `git status --porcelain`: only `?? bitfiltrator-ref/` (untracked
  reference clone, per SPEC.md).
- `git rev-parse --abbrev-ref HEAD` plus `--short HEAD`: fm/ultra-
  bitstream at aee77e3.
- `git merge-base --is-ancestor aee77e3 6bb30b6`: yes; `git log
  --oneline aee77e3..6bb30b6`: exactly one commit, 6bb30b6.
- `git merge --ff-only fm/ultra-bitgen-int`: fast-forward
  aee77e3..6bb30b6, 5 files changed, 1040 insertions, 1 deletion
  (src/segbits.rs, src/int_pip.rs, tests/int_pip.rs new; lib.rs
  wiring; Cargo.lock helion-ultrascale entry only).
- Post-merge verification on the new tip: `cargo test -j2 -p
  helion-ultrascale` (full path ~/.cargo/bin/cargo, via nohup
  setsid, log /tmp/orch-merge-int-test.log). Result: 22 unit plus 2
  int_pip integration plus 2 smoke = 26 passed, 0 failed, matching
  the critic's re-run exactly. First attempt failed only because
  cargo is not on the default PATH in this shell (`setsid: failed
  to execute cargo`); retried with the full path, no code change.
- Post-test `git status --porcelain`: only `?? bitfiltrator-ref/`;
  no Cargo.lock churn this run. Tip confirmed 6bb30b6.

INT_PIP is GREEN at tier 1, the first green block. Zero diff against
the public segbits DB (3,778 lines verified, 11-hop blinky route with
11 set bits and 48 zero bits, 0 missing PIPs) PLUS critic re-run
(session 2, CRITIQUE.md, PASS with independent re-verification).
Tier 2 stays blocked on the Vivado license server, unchanged since
Oct 22.

Next in the queue: fm/ultra-bitgen-segbits, only after its critic
review (session 3) passes, then a rebase onto this tip with the
byte-identical segbits.rs check and the int_pip_audit.rs duplication
check. Then fm/ultra-bitgen-lut 0e81099, only after critic session 4
passes, with the data/ consolidation check.

## 2026-11-07 ~17:15 EST: orchestrator rulings on the segbits-vendor delivery (98e282a)

The segbits-vendor worker delivered 98e282a and appended its own
COORD.md entry plus roster line directly (see the ~17:15 entry
above). That breaks the session-1 rule that workers do not edit
COORD.md directly. It landed cleanly this time (no conflict with
the orchestrator's concurrent entries), and the content is
accurate, so no repair is needed. Rule restated: workers report by
final message; only the orchestrator edits COORD.md. The worker's
entry stands as written.

### Ruling 1: BitRef.bit u8 to u16, APPROVED (interface change)

98e282a widens BitRef.bit from u8 to u16. The evidence is decisive:
the u8 loader fails the vendored HPIO load at line 241 (`!02_259`,
"bit 259 outside 0..256"), while the vendored set carries bits up
to 1390 (HDIO_TOP_RIGHT), 1359 (HPIO_RIGHT), 1310
(HDIO_BOT_RIGHT). A 93-word frame holds 2976 bit positions, so u16
is the correct width and u8 was factually wrong for the vendored
data. Regression tests cover it (`bit_width_above_u8_parses_exactly`,
`all_vendored_dbs_load`, HPIO integration test). INTERFACES.md is
updated the same day, per its own rule: BitRef.bit is now u16, with
the rationale recorded there. The critic session 3 review of the
vendor branch still runs and must verify the change before any
merge; this ruling is on the contract, not a substitute for that
review.

### Ruling 2: merge-queue rebase check CORRECTED (not byte-identical)

The session-3 queue entry said to confirm the shared
src/segbits.rs "stays byte-identical" after the vendor rebase. That
check is wrong as written and is corrected here. The vendor branch
differs from 6bb30b6 by exactly the u16 widening in 98e282a (worker
verified: `git diff 6bb30b6 HEAD --
crates/helion-ultrascale/src/segbits.rs` shows only the u16 change
plus its two regression tests). After the rebase, the check is: the
diff between the rebased segbits.rs and the merged tip's segbits.rs
is exactly the u16 widening and nothing else. Any other difference
is drift and blocks the merge. The tests/int_pip_audit.rs
duplication check stands unchanged.

### Open item for the correctness critic: int-pip src/dst direction

Flagged by the segbits-vendor worker, not yet ruled. `int_pip.rs`
builds its feature as `PIP.{src}.{dst}` with src BOUNCE_E_0_FT1
and dst INODE_E_1_FT1, but the tile JSON's pip for that DB feature
is `INT.INODE_E_1_FT1->>BOUNCE_E_0_FT1` (src INODE_E_1_FT1, dst
BOUNCE_E_0_FT1). The feature string itself matches the DB, so the
emitted bits are unaffected and the INT_PIP tier-1 green stands.
The question is whether the FASM convention is sink-first (only
the field names are misleading) or int-pip has the direction
backwards. If the router ever fills src/dst in physical direction,
`feature()` would miss the DB. The correctness critic is asked to
confirm which it is in its next session; until then the IntPip
src/dst naming is a known API hazard, not a defect in the emitted
bits. No code changes on this item without a critic verdict plus a
CTO ruling.

### Queue status, unchanged otherwise

fm/ultra-bitgen-segbits (tip 98e282a) still NOT merged: critic
session 3 review is still running. After it passes, the vendor
worker rebases onto the new fm/ultra-bitstream tip (6bb30b6) under
the corrected check above, then merges. fm/ultra-bitgen-lut 0e81099
still waits on critic session 4, with the data/ consolidation
check. INT PIP completeness audit part 1 (3774 of 3774, 100 percent)
is recorded in data/INT_PIP_AUDIT.md; part 2 (Vivado get_pips
comparison) stays pending on the PinPlanning probe, unchanged.

## 2026-11-07 ~17:10 EST: orchestrator session 4 started, CTO ruling recorded, lut-ff-bram merged

Orchestrator session 4 agent 48332feb-d9de-4395-8a2f-d70331467426.
Took over from session 3 (a9e8f0b0, complete). Read SPEC.md,
INTERFACES.md, COORD.md, CRITIQUE.md. No workers spawned. Roster
updated: session 3 done, session 4 active, lut-ff-bram merged,
int-pip merged.

### CTO ruling: RAMB18 half naming is RAMB18E2_L / RAMB18E2_U

Critic session 4 FLAG, ruled by the CTO 2026-11-07. The L/U
spelling lands; RAMB18E2_A / RAMB18E2_B does not. Evidence: A/B
is admitted by its own author (COORD.md ~17:05 entry) to be an
arbitrary Helion-internal disambiguator with no source; L/U
matches the Xilinx-native lower/upper convention (pin suffixes
_L/_U in UG573/UG574, and prjuray site_type_RAMB181 carries only
_U pins, the upper half). L/U is also the spelling bitloc.rs
hard-codes (BramHalf::suffix) and the one the critic verified in
session 4. No code change needed on the lut branch: it already
uses L/U.

### LUT/FF/BRAM merged into fm/ultra-bitstream (a09df12)

Merge performed by the orchestrator directly in the base worktree
(same routing limitation as session 3: the integration worker is
CTO-spawned and outside the orchestrator's send scope; no code
was authored by the orchestrator, only a merge with mechanical
conflict resolution).

Pre-merge verification, all with exact commands in
~/workspace/helion-ultra-bitstream:
- `git status --porcelain`: only `?? bitfiltrator-ref/` (untracked
  reference clone, per SPEC.md).
- `git rev-parse --abbrev-ref HEAD` plus `--short HEAD`: fm/ultra-
  bitstream at 6bb30b6.
- `git log --oneline 6bb30b6..0e81099`: exactly the four lut
  commits (1adf250, dfd08d0, 0b83568, 0e81099).
- `git merge-base --is-ancestor 6bb30b6 0e81099`: exit 1, not a
  fast-forward; a merge commit was needed.
- `git merge --no-ff fm/ultra-bitgen-lut`: two conflicts, both
  mechanical, resolved as the union of both sides. src/lib.rs kept
  the int-pip/segbits re-exports from 6bb30b6 plus the bitloc
  module wiring from 0e81099 (both module lists already present).
  Cargo.lock kept the serde_json dependency from 0e81099 (HEAD
  side had no dependencies entry for helion-ultrascale). No
  conflict markers remain (grep count 0 on both files). All other
  files merged cleanly (bel.rs, bitloc.rs new, data/ files new,
  tests/lut_ff_bram.rs new, smoke_place.rs, Cargo.toml).
- Merge commit a09df12, message "merge fm/ultra-bitgen-lut:
  LUT/FF/BRAM bit locations and FASM emission (0e81099)".

Post-merge verification on the new tip: `cargo test -j2 -p
helion-ultrascale` (full path ~/.cargo/bin/cargo, via nohup
setsid, log /tmp/orch-merge-lut-test.log). Result: 23 unit plus 2
int_pip integration plus 20 lut_ff_bram plus 2 smoke = 47 passed,
0 failed. That is the exact union of the two critic-verified
suites: the lut worktree's 33 (11 unit + 20 lut + 2 smoke) plus
the int-pip worktree's 14 additional tests (12 unit + 2 int_pip).
Post-test `git status --porcelain`: only `?? bitfiltrator-ref/`;
no Cargo.lock churn this run. Tip confirmed a09df12.

Block status: LUT_INIT, FF_INIT, BRAM_CONTENT, BRAM_PARITY stay
UN-GREEN. The critic session 4 verdict covers the emission code
and tables only; no diff harness exists yet, so no zero-diff
check has run. The merge does not change any green block:
INT_PIP is unaffected (its 2 integration tests still pass on the
merged tip).

### Direction for the device-file worker: rebase onto a09df12, adopt L/U

Recorded here because the device-file worker (2a82ddaf) is
CTO-spawned and outside the orchestrator's send scope; the CTO
routes this direction to it. On rebase of fm/ultra-bitgen-device
onto the new fm/ultra-bitstream tip a09df12: adopt
RAMB18E2_L / RAMB18E2_U in bel.rs, dropping the duplicate F1
commit aece1c6 (A/B spelling) by taking the merged bel.rs; drop
the duplicate F2 commit ad4da84 by taking the merged
smoke_place.rs (array literal already in place); keep the G3
capacity commit 8e73f0c and all device-file work (xcvu3p.json,
loader). Expected conflicts on rebase: bel.rs, smoke_place.rs,
lib.rs (same three files the critic session 4 FLAG named).
Resolution rule: merged tip wins for the F1/F2 parts, the
device-file worker's own work wins everywhere else.

### Direction for the segbits-vendor worker: provenance consolidation on rebase

Recorded here for the same routing reason (vendor worker d3668f3b
is CTO-spawned). Data/ provenance: PROVENANCE.md is the single
provenance index. During the vendor rebase onto the new tip, add
the lut tables' source plus SHA-256 line to PROVENANCE.md (source
BitFiltrator ULTRASCALE_PLUS.json, SHA-256
c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c,
MIT, Copyright (c) 2022 EPFL VLSC). Keep the per-table README.md
for extraction details; cross-reference both ways (README points
to PROVENANCE.md, PROVENANCE.md points to README). Do not let the
rebase silently overwrite or ignore either doc.

### Queue status

1. fm/ultra-bitgen-int 6bb30b6: MERGED, INT_PIP green tier 1.
2. fm/ultra-bitgen-lut 0e81099: MERGED as a09df12. Emission code
   and tables critic-passed; blocks un-green until the diff
   harness runs.
3. fm/ultra-bitgen-segbits, tip 98e282a: NOT merged, critic
   session 3 review still running. After it passes, the vendor
   worker rebases onto the new tip (a09df12) under the corrected
   u16 check (diff vs merged segbits.rs is exactly the u16
   widening, nothing else), adds the lut-tables provenance line,
   and the orchestrator confirms tests/int_pip_audit.rs does not
   duplicate tests/int_pip.rs, then merges.
4. fm/ultra-bitgen-device: NOT merged, rebase pending on the
   direction above (adopt L/U, drop duplicate F1/F2). Its
   xcvu3p.json work continues on its branch.
5. WireDb naming seam: still PROPOSED, not ruled. It gates the
   router-to-PIP translator work, not any merged verdict. Needs a
   CTO ruling before implementation.
6. Open item for the correctness critic: int-pip src/dst
   direction (known API hazard, not a defect in emitted bits).
   Awaits the critic's next session; no code changes on it
   without a critic verdict plus a CTO ruling.

Routing note: the direct send to the device-file worker
(2a82ddaf) failed on both scopes ("outside scope 'direct'" and
"outside scope 'descendants'"), the same limitation session 3
recorded for the integration worker. The rebase direction above
is recorded in COORD.md for the CTO to route. The same applies
to the segbits-vendor provenance direction: the vendor worker
(d3668f3b) is also CTO-spawned, so its rebase instructions go
through the CTO as well.

## 2026-11-07 ~17:25 EST: integration worker, crate tests green (35/35)

Command: `cargo test -j1 -p helion-ultrascale` (log
/tmp/ultra-logs/test-port1.log). Results:
- 28 unit tests passed (pack, place, legality, router incl. 7 PathFinder
  tests, device incl. new xcku3p counts test, fabric).
- 2 corpus legality tests passed (blinky, counter, real helion-sv synth
  through pack/place/legality on the fake grid).
- 2 smoke_place tests passed (pre-existing).
- 3 STA sanity tests passed (counter 810ps/WNS 9110, 4-LUT chain 1530ps,
  UNKNOWN-delay fail-loud).
- 0 failed. `cargo check -j1 -p helion-ultrascale` green.

CLI flow (`helion impl --part xcku3p/xcvu3p [--seed N]`) is written
(crates/helion-cli/src/ultra.rs, dispatch in cmd_impl) and `cargo check
-j1 -p helion-cli` is running; gate run to follow.

## 2026-11-07 ~22:10 EST: diff-harness worker, harness delivered

Harness at ~/workspace/ultra-bitgen-crew/harness/ (outside the repo;
worktree ~/workspace/helion-ultra-bitgen-diff on fm/ultra-bitgen-diff
is clean, no repo edits). One command runs the full diff:

Command: `python3 diff.py` (from the harness dir)
Final run: `runs/20261107T220702Z/`, exit 0. Cross-check PASS
(BitFiltrator vs prjuray, SLICE_X0Y0: 4 LUTs x 64 bits, 0 mismatches;
4 FFs, V0/V1 complement). 40 JSONL records (10 blocks x 4 combos),
ALL blocked, all diff_bits null, seed 20261107 on every record.
Manifest tier tier1, emitter null.

Greedy queues (largest first, this IS the work queue):
- blinky/xcku3p: LUT_INIT 64, IO_CONFIG 55, INT_PIP 11, FF_INIT 1, rest 0
- counter/xcku3p: LUT_INIT 256, IO_CONFIG 55, INT_PIP 17, FF_INIT 4, rest 0
- blinky/xcvu3p and counter/xcvu3p: IO_CONFIG 55 first; LUT/FF/INT total 0,
  blocked "xcvu3p device file not yet generated (device-file worker)"

Placement hashes (bind placement record + FASM text, sha256):
- blinky xcku3p b65979c112456c7049770e0fd1573939148c901606eef9f7a0b8a0f74fddacfb
- blinky xcvu3p 791ce94587b0ab2074fe211a91731584846b9a71cae189cee0d8a19ff4e9dffb
- counter xcku3p 1bf0ffe68ca860b2a3b7c05b56e79fc5e7f8e7949af49bb7504734a68ba012ea
- counter xcvu3p 74417af567417cdac387ade31bd081c6ea632f09556f5235a1a6ff7d6bd3e23c

Verification: `python3 -m unittest discover -s tests` = 18/18 OK;
`python3 tests/hand_verify_lut.py` = HAND VERIFICATION PASSED (all 64
blinky A6LUT bits match by independent hand computation; hand diff_bits
3 = harness diff_bits 3). Tamper rejection exits 3 (REJECTED). Tier-2
slot checked: --reference-bit records tier1+tier2 with tier2 blocked
(no emitter to parse a Vivado bitstream), no comparison faked.
Docs: harness README.md, SOURCES.md, VERIFICATION.md.

Blocked items named by the harness, in the order they unblock work:
1. bitgen-core emitter (emit_bitstream/parse_bitstream) - unblocks
   LUT_INIT, FF_INIT, INT_PIP, FRAMING comparisons on xcku3p.
2. device-file xcvu3p device file - unblocks LUT/FF/INT on xcvu3p.
3. xcku3p tilegrid (Vivado probe / prjuray-db) - unblocks absolute
   IO_CONFIG comparison (currently tile-relative) and confirms the
   provisional INT col = CLB col - 1 rule.
4. Vivado fuzzing (license blocked since Oct 22) - only path to CLK
   (BUFGCE/MMCM), DSP48E2, URAM288 encodings.
5. A BRAM golden design - BRAM_CONTENT/BRAM_PARITY resolve but are
   not exercised by blinky/counter.

Two harness bugs found and fixed during testing (in the tests, not
hidden): fake emitter path import, and per-block diff comparing the
whole-design bitset (fixed via block-labelled `bits` protocol lines
`FAR OFFSET VALUE BLOCK`). See harness VERIFICATION.md.

## 2026-11-07 ~17:20 EST: device-file worker, loader DONE (3d23b2e); CTO L/U ruling recorded

Loader deliverable DONE and committed on fm/ultra-bitgen-device as
3d23b2e (on top of 8e73f0c G3). `cargo test -p helion-ultrascale -j2`:
24 unit + 2 integration pass. `cargo clippy -p helion-ultrascale -j2
--all-targets`: zero warnings (7 warnings fixed in this session:
two `from_str` converted to `FromStr` impls, type aliases for the
nested arch maps and the BRAM resolution tuple).

What landed: `crates/helion-ultrascale/src/device_file.rs` (DeviceFile,
ArchSummary, BitLocator, Far, ProvisionalNote), serde/serde_json deps
in Cargo.toml, vendored `data/xcku3p.json` and
`data/ULTRASCALE_PLUS.json` (MIT, BitFiltrator) plus `data/README.md`.
9 new loader tests, all hand-checked against BitFiltrator
bit_locator.py (SLICE_X23Y45/B6LUT first FAR 00003007, BFF FAR
0000300c offset 2260, SLICE_X0Y0/AFF FAR 0000050c offset 2,
RAMB18_X0Y0 16,384 mem bits over 256 distinct FARs from 01000000).
Sweeps: every CLB FAR (14,319,360 pairs) and every BRAM FAR
(17,694,720 pairs) of xcku3p maps to a real tile inside the file.
Cargo.lock restored after verification per the G6 convention (diff
was only the helion-ultrascale package entry).

Two open questions recorded in data/README.md, not hidden: (1) file
total 41,460 frames vs UG570 41,476 for KU3P, gap 16 = 4 per FAR row,
systematic across every BitFiltrator device file checked; (2) header
LUT/FF counts are 3/4 of the 8-LUT / 16-FF arch geometry (162,720 /
325,440 over 27,120 slices), reported as Vivado part-property file
truth, never derived from geometry; the loader does not assert
num_luts == num_slices * 8.

CTO ruling (critic session 4 FLAG), recorded for the rebase: RAMB18
half naming is RAMB18E2_L / RAMB18E2_U, NOT my RAMB18E2_A /
RAMB18E2_B from aece1c6 (F1). L/U matches the Xilinx-native
lower/upper convention and what bitloc.rs hard-codes. When the
orchestrator orders the rebase onto the new fm/ultra-bitstream tip
(after the lut branch merges), I will adopt L/U in bel.rs, drop my
duplicate F1/F2 commits (aece1c6, ad4da84) by taking the merged
bel.rs and smoke_place.rs, and keep the G3 capacity commit (8e73f0c)
and the loader commit (3d23b2e). Verified this session: the G3 doc
comment in device.rs uses no A/B names (says "two RAMB18E2 halves"
generically), so it survives the rename untouched. The rebase has NOT
been ordered yet: fm/ultra-bitgen-lut 0e81099 still waits on critic
session 4, and fm/ultra-bitstream tip is still 6bb30b6 (int-pip).

Still to do on this brief: (2) demo.py reconstruction validation on
xcku3p (synthetic minimal bitstream; no real xcku3p bitstream exists
in the workspace), (3) scripts/gen_xcvu3p.py plus provisional
xcvu3p.json with helion_provisional flags. Vivado confirmation stays
a named blocker (license server refused since Oct 22).

## 2026-11-07 ~22:15 EST: diff-harness session 2, critic FLAG 2 fixed

Critic session 5 PASSed the harness but filed FLAG 2: garbage
emitter parse output crashed the run. Fixed in
`~/workspace/ultra-bitgen-crew/harness/roundtrip.py` (harness is
outside the repo; worktree ~/workspace/helion-ultra-bitgen-diff
clean, no repo edits).

Reproduced first, exactly as the critic reported it: a shell
emitter whose emit writes a dummy .bit and whose parse prints "this
is not fasm at all", run as `python3 diff.py --design blinky --part
xcku3p --emitter-cmd /tmp/critic5_badparse.sh --out
/tmp/critic5_badparse_run2`, exited 1 with a traceback ending
`fasm.FasmError: emitter parse output:1: no tile.feature separator
in line 'this is not fasm at all'`, and the run dir held only
`emit.bit` and `roundtrip.bit` (no diff.jsonl, no manifest).

Fix: `run_roundtrip` never raises. Golden FASM malformed goes
blocked (the golden is at fault, not the emitter); emitter
subprocess failure goes blocked, mirroring the emit_error path in
`diff_engine.diff_design`; emitter parse output that is not valid
FASM goes FRAMING red with one failed check
(`roundtrip-parse(emit(f))==f`) and the error named in the detail,
the same as a corrupt parse (valid FASM, wrong value), which is
already a red; anything else goes blocked with the exception named.
After the fix the same repro command exits 0, writes all 10 records
plus manifest, and the FRAMING record is red, diff_bits 1, detail
"emitter parse output is not valid FASM: emitter parse output:1: no
tile.feature separator in line 'this is not fasm at all'".

Regression tests: fake emitter supports `FAKE_PARSE_GARBAGE=1`;
`test_garbage_parse_output_is_framing_red_not_crash` (diff_design
level, FRAMING red with the error named, LUT/FF/INT still green
through the bits path) and
`test_garbage_parse_emitter_full_run_writes_all_records` (CLI level,
exit 0, no traceback, 40 records, FRAMING red on all 4 combos,
manifest with 4 combos). Suite now 20 tests, all OK in 4.9s.
Default run re-checked after the fix: `runs/20261107T221652Z/`,
exit 0, 40 records, all blocked. Fix recorded in
`harness/VERIFICATION.md` (FLAG 2 section). FLAG 1 (one line in
INTERFACES.md recording the null-diff_bits greedy-order rule) is
not this session's: it belongs to the orchestrator, no code change.

## 2026-11-07 ~22:35 UTC - device-file session 2 DONE (worker report, appended by the worker per CTO direction)

Branch fm/ultra-bitgen-device, tip 33a4b8f. Rebase onto a09df12 plus
both remaining brief items are done, verified, and committed. No
merge requested; the orchestrator sequences it.

Rebase (a09df12 + 91e5522 + f378e75, verified at the time):
- Duplicate F1 commit aece1c6 dropped by `git rebase --skip`
  (bel.rs conflict); duplicate F2 commit ad4da84 auto-dropped
  ("patch contents already upstream"). Merged bel.rs and
  smoke_place.rs adopted, so RAMB18 halves are RAMB18E2_L /
  RAMB18E2_U per the CTO ruling, not A/B.
- G3 commit kept as 91e5522; loader commit kept as f378e75.
- One rebase-only hazard fixed in the resolution: bitloc.rs:264
  and device_file.rs:866 both define NUM_CLB_PER_COLUMN, so the
  lib.rs re-export drops the device_file copy (bitloc's stays).
- Post-rebase: `cargo test -j2 -p helion-ultrascale` 57 passed, 0
  failed; `cargo clippy -j2 --all-targets` 0 warnings.
- Two follow-up commits: ac22d8d syncs Cargo.lock with the serde
  dependency Cargo.toml declares (the lock predated it; cargo
  regenerated the entry on every build), and 5dcfe11 applies
  `cargo fmt` to device_file.rs and lib.rs (formatting only, token
  streams verified identical apart from the import order rustfmt
  enforces; session 1 had never run fmt on device_file.rs).

Demo.py reconstruction validation on xcku3p (61012ff):
- `scripts/make_synth_xcku3p_bit.py` builds a synthetic minimal
  bitstream from the reference BitLocator for three BELs
  (SLICE_X23Y45/B6LUT, SLICE_X23Y45/BFF, RAMB18_X0Y0; 261 distinct
  FARs) with a marker bit at every located frame offset. Two bugs
  found and fixed during the run: the header omitted field 7 (tag
  0x65 + data length), so the reference parser read the first dummy
  byte as the field-7 tag; and the demo.py output parser assumed
  the wrong frame-address parenthetical format (it is the FAR
  __str__, not a second hex address).
- Result: self-check 1 PASS (reference re-parse: 261 FARs, all
  marker bits set, byte offsets exact); self-check 2 PASS (demo.py
  output matches the reference locator and the built byte offsets
  on all 18,497 located bits). Committed: the generator, the .bit
  (100,369 bytes), the golden JSON, the LUT/FF log, the BRAM
  summary, `scripts/demo_validation.md` (command, verbatim output,
  what it proves and does not prove), and
  `tests/device_file_demo.rs` (3 tests: LUT pair-for-pair, FF,
  BRAM by distinct-FAR sets plus first-8 pairs plus FNV-1a
  digests of the full ordered lists).
- Limit recorded in the doc: no last-of-row FAR was exercised
  (verified: 0 of the 261 FARs is last-of-row), so the generator's
  2-zero-padding path for last-of-row FARs is untested.

Provisional xcvu3p.json (33a4b8f):
- `scripts/gen_xcvu3p.py` builds `data/xcvu3p.json` from the
  xcku3p template plus XMP103 counts: 1 SLR, 7 FAR rows (five of
  117 CLB columns, two of 118, rows 0 and 6; 821 columns x 60 =
  49,260 slices), header counts 394,080 LUTs / 788,160 FFs / 720
  RAMB36 / 2280 DSP, IDCODE 0x14B39093 (UG570 Table 2, production
  rev 1+), part xcvu3p-ffvc1517-1-e. 18 helion_provisional notes,
  one per derived value with its source. `tests/device_file_xcvu3p.rs`
  (3 tests) pins the header counts, the frame total, and BEL
  resolution in all 7 rows including the last row and last column.
- Frame total computed 73,035 OVERSHOOTS UG570 Table 1 (71,820) by
  1,215. Recorded in the file notes, the generator header, and
  data/README.md, never tuned away. This supersedes the 71,664
  anticipation in the first draft of the README section, which no
  rows x (std + BRAM) construction with these tables reproduces.
  The 7-row count is the unique small row count giving an exact
  49,260-slice split; Vivado confirmation must resolve whether the
  row count, the per-row std table, or the UG570 counting is off.
- The README xcvu3p section is updated to the computed total with
  the overshoot note. The 49,250 vs 49,260 CLB discrepancy stays
  open for the CTO (this file uses 49,260 per XMP103).

Final verification on tip 33a4b8f: `cargo test -j2
-p helion-ultrascale` 63 passed, 0 failed (33 unit + 3 demo + 3
xcvu3p + 2 int_pip + 20 lut_ff_bram + 2 smoke); `cargo clippy -j2
--all-targets` 0 warnings; `cargo fmt --check` clean. Worktree
clean. Nothing pushed; no merge to master. The diff harness can
now run xcvu3p combos against this provisional file; those numbers
are provisional until Vivado confirmation lands (license server
still refused since Oct 22).

## 2026-11-07 ~17:33 EST: segbits-vendor session 2 DONE (rebase onto a09df12, u16 widening, provenance, 014-cle-spec check)

Branch fm/ultra-bitgen-segbits, tip c1bef40, worktree clean, nothing
pushed, no merge to master. Five commits on top of a09df12: 0da9b7e
(vendor data), 619928a (port loader), 9ea177c (u16 widening),
6e7e073 (int-pip u8 to u16), c1bef40 (provenance consolidation).

- Rebase onto a09df12 verified: `git diff a09df12 HEAD --
  src/segbits.rs` is exactly the u16 widening plus its two regression
  tests plus the doc fix and one fmt reflow. No drift.
- Step 2 (6e7e073): the u8 bit field that breaks against the u16
  BitRef is `IntPipError::ZeroBitSet.bit` in src/int_pip.rs:119,
  widened to u16, plus the verify_zero_bits reader closure
  `&dyn Fn(u32, u8)` to `Fn(u32, u16)`, and the int_pip test
  fixtures to u16 throughout. (The critic called it sorted_bits;
  the actual field is ZeroBitSet.bit.) Also fixes the stale
  segbits.rs doc comment (INTERFACES.md states bit u16 per
  orchestrator ruling 2026-11-07 on 98e282a) and normalizes
  formatting in src/segbits.rs and tests/segbits.rs (formatting
  only, no logic change; the pre-existing fmt drift in those two
  files is now gone and the tree is `cargo fmt --check` clean).
- Step 3 (c1bef40): PROVENANCE.md now records the vendored LUT/FF/
  BRAM tables (source BitFiltrator ULTRASCALE_PLUS.json, SHA-256
  c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c,
  MIT, Copyright (c) 2022 EPFL VLSC; source hash recomputed 2026-11-07
  during this session) and fixes the stale sentence claiming the arch
  JSON is not copied here. README.md in data/ cross-references
  PROVENANCE.md both ways; README.md keeps the per-table extraction
  details.
- Step 4, 014-cle-spec: the fuzzer EXISTS upstream in f4pga/prjuray
  at fuzzers/014-cle-spec (tree sha
  4416b5297af81de598f4d65f837e46c19630604b; files Makefile, README.md,
  dump_features.tcl, generate.sh, generate.tcl,
  generate_features.sh, top.py, top.sh). It is NOT in prjuray-db:
  upstream master root listing (GitHub API, 2026-11-07 22:24 UTC)
  holds only .gitignore, CONTRIBUTING.md, COPYING, Info.md, Makefile,
  README.md, zynqusp/, and the local clone at
  ~/workspace/prjuray-db-ref (affbc5e) has no fuzzers/ or specs/
  directory. The name survives only as an origin label in the
  vendored .origin_info.db files (for example
  `CLEL_L.ABCDFF.CEUSED.V0 origin:014-cle-spec !14_07`). The
  fuzzer's top.py drives specs from utils.spec including flipflops,
  so the FF INIT value mapping lives in the prjuray tools repo, not
  the DB repo. The exact INIT value to V0/V1 mapping in
  utils/spec.py could NOT be read this session: the GitHub API
  denied the utils directory listing (HTTP 403, likely rate limit)
  and the request was not retried through another endpoint. FF INIT
  polarity stays open. License note: prjuray is Apache-2.0
  (SPDX headers on the fuzzer files), which is outside the crew's
  CC0/MIT-only vendoring line; reading it is fine, vendoring any of
  it would need a CTO ruling. Nothing vendored on this item.
- Step 5 verification, run after the final edits and before
  committing (commands: `~/.cargo/bin/cargo test -p
  helion-ultrascale -j2`, `~/.cargo/bin/cargo clippy -p
  helion-ultrascale -j2 --all-targets`, `~/.cargo/bin/cargo fmt -p
  helion-ultrascale -- --check`, all via nohup setsid, -j2 max, no
  target cleaning): cargo test 54 passed, 0 failed (25 unit + 2
  int_pip + 1 int_pip_audit + 20 lut_ff_bram + 4 segbits + 2
  smoke_place); clippy 0 warnings; fmt clean. All 5 vendored DBs
  load whole (int 3778, hpio_right 1560, hdio_top_right 812,
  hdio_bot_right 828, cmt_right 104; 7082 features total, asserted
  by all_vendored_dbs_load). INT PIP audit still passes
  (int_pip_audit_db_internal, 3774 of 3774).
- Worktree AGENTS.md read this session: generic Helion IDE notes
  only (egui scrolling, IDE chrome, toasts, macOS signing); nothing
  bitstream-specific, nothing affecting this work.
- Ready for orchestrator merge sequencing. Merge note: this
  branch's src/segbits.rs IS the u16 version; the merge must take
  it, not the merged tip's u8 version. The segbits-vendor /
  bitgen-core SegbitsDb reconciliation (single definition in
  segbits.rs with u16 plus features()) is still a separate
  integration item, not done here.

## 2026-11-07 ~17:40 EST: orchestrator session 5, contract updates plus CTO rulings recorded (no merges)

No worker branch merged this session. Segbits rebase is done
(c1bef40, recorded by the worker below), bitgen-core D1/D2 fixes
are running (session 2), device-file tip 33a4b8f is under critic
review (session 7). This session applied contract updates and
recorded rulings only. Worktree ~/workspace/helion-ultra-bitstream
at a09df12, clean except the two crew doc files.

INTERFACES.md updates applied (all already ruled, same-day rule):
- SegbitsDb block: added `fn features(&self, tile_type: &str) ->
  &[String];` (interfaces session 2 item 3, CRITIQUE.md:712,
  approved; bitgen.rs:455 already implements it) plus the merge
  requirement (FileSegbitsDb stores per-tile-type sorted feature
  lists built at load, empty slice for unknown tile type, test
  asserting features() enumerates exactly what lookup()
  resolves). Added the frame-relative statement: `BitRef.frame`
  is a frame offset relative to the tile base FAR (prjxray
  convention), absolute FAR = `tile_base_far + frame` as a u32
  sum, `tile_base_far` the FAR of the tile's frame offset 0;
  loader already produces relative numbers; bitloc.rs (minor,
  frame_ofst) pairs convert as `frame = minor - base_minor`,
  `bit = frame_ofst` at integration (item 2, CRITIQUE.md:677).
- Bitgen API block: `emit_bitstream` and `parse_bitstream` now
  take `&dyn BitgenDevice`, not `&dyn UltraScaleDevice`, with the
  trait spelled out (part, idcode, tile_type, tile_base_far,
  tile_names, config_order_fars, verified against
  bitgen.rs:460-470 in the bitgen-core worktree). UltraScaleDevice
  is a struct (day-1 INTERFACES.md:20), so `&dyn` on it cannot
  compile (item 4, CRITIQUE.md:729). FLAG recorded: no real
  BitgenDevice impl exists yet (DeviceFile has no impl and no
  tile_base_far; only StubDevice in bitgen.rs tests). This blocks
  FRAMING green on a real device, not the bitgen-core merge.
- FASM block: the `AFF.INIT` example corrected to
  `AFF.INIT.V0` / `AFF.INIT.V1`. FF INIT canonical spelling is
  the DB vocabulary `.V0`/`.V1`, exactly one feature per FF
  selected by value; the valued (`= 1'b0/1`) and bare (`AFF.INIT`)
  spellings are non-canonical and must not be emitted (item 5,
  CRITIQUE.md:756). FLAG recorded: the value-to-feature mapping
  (which of V0/V1 is INIT=1) is unverified by any on-disk source;
  FF INIT stays BLOCKED on a polarity experiment. V0/V1 does not
  uniformly encode the value (INIT/SRVAL: V0 sets, V1 clears;
  CEUSED/CLKINV/SRINV: reverse).
- Diff report block: FLAG 1 ordering line added (harness critic
  session 5, CRITIQUE.md:992): order by effective size,
  diff_bits when the comparison ran, else total_bits; ties red
  before blocked before green, then block name. No code change.

Contradiction check: SPEC.md states no UltraScaleDevice,
AFF.INIT, or u8-bit contract (grep, zero hits); the day-1
INTERFACES.md:20 struct definition agrees with the new Bitgen
API text. INTERFACES.md has zero em/en dashes (grep count 0).

WireDb seam ruling sign-off (CTO, 2026-11-07, option A):
`fn wire_info(&self, wire: WireId) -> Option<WireInfo<'_>>` on
fabric::WireDb; WireInfo { name, tile }; routing-graph WireDb
returns Some from its DB-derived node table; EmptyWireDb and the
mock return None; the translator maps each consecutive wire pair
to an IntPip via wire_info and fails loud on unnamed wires.
This supersedes the stale "still PROPOSED, not ruled" lines at
COORD.md:531 and :885. The seam gates the router-to-PIP
translator as its own integration item; it does NOT
retroactively block the INT_PIP tier-1 green (that green is for
emission, zero diff plus critic re-run, and it stands). The
INTERFACES.md WireDb section already carries this ruling and
the status note; this entry is the COORD.md sign-off.

CTO slice-count rulings (2026-11-07):
- xcvu3p adopts 49,260 slices (394,080 LUTs, 788,160 FFs).
  device.rs 49,250 is a rounding artifact of the "394K" figure
  (49,250 x 8 = 394,000) and must be corrected; correction
  routed to the integration worker. The provisional xcvu3p.json
  already uses 49,260 per XMP103.
- xcku3p adopts AMD's 20,340 slices (162,720 LUTs, 325,440
  FFs). The device file's num_slices 27,120 contradicts its own
  num_luts 162,720 (27,120 x 8 = 216,960) and is recorded as a
  file-header anomaly, not ground truth. The model already uses
  AMD's count.
- The provisional xcvu3p.json frame-total gap (73,035 computed
  vs UG570 Table 1 71,820, overshoot 1,215) stays open and
  flagged provisional, to be resolved by Vivado confirmation;
  do not tune the file.

Bitgen-core critic session 6 verdict (CRITIQUE.md:1022):
FAIL on 0f3aa2d, two concrete defects, both small and local.
D1: parse_bitstream errors on real DB data for all-! valueless
features (bitgen.rs:1250-1253, :1262, helper :1327). Proven
against vendored segbits_int.db (145 all-! of 3778): the
BOUNCE_E_0_FT1 VCC_WIRE feature (sets nothing, requires
!64_00) computes present=true vacuously when a design routes
BOUNCE to INODE, and MustBeZeroViolated fails the whole
bitstream. Fix: present = all set bits 1 AND all zero bits 0;
a zero bit reading 1 means absent; render .V0/.V1 in
parse-back; add a test with the real BOUNCE mux proving
parse(emit(f)) == f for routed and untouched cases. D2:
emit_bitstream ending omits START (CMD 5) and DESYNC (CMD
13); without START the device never runs the startup sequence
(DONE stays low). Fix: emit START after DGHIGH_LFRM and
DESYNC before the trailing NOOPs; do NOT add the FAR/MASK/
CTL0 writes after START (7-series values correctly omitted,
US+ values unknown). FLAG F-A: CRC_EXCLUDED_REGS [15, 18, 20,
21, 22] provenance unverified (no observable effect; cite or
remove before tier-2 parsing is trusted). FLAG F-B: base-FAR
definition unproven until the device file lands (integration
dependency). FLAG F-D: FF INIT spelling per interfaces item 5.
Session 2 is fixing D1, D2, and F-A; the critic re-reviews
before merge. Do not merge 0f3aa2d until then. The
SegbitsDb/BitRef reconciliation (interfaces item 6: single
definition in segbits.rs with u16 plus features(), drop
bitgen's duplicate and its lib.rs re-export, one constructor
spelling, features() test plus file-order preservation test)
is the merge-time commit; the D1/D2 fixes ride with it or just
before it.
Ruling from session 6 (CRITIQUE.md:1244): int-pip src/dst
direction, FASM is sink-first. DB feature is PIP.<dst>.<src>
(dst.src matches 3770/3774 in the audit, src.dst 0/3774).
int_pip.rs feature() builds PIP.{src}.{dst} with field names
inverted relative to physical direction, but the emitted
feature strings are correct, so the INT_PIP tier-1 green
stands. FLAG: rename or re-document the fields before the
router-to-PIP translator is written (routed to the
integration worker by the CTO); if the translator ever filled
src/dst in physical direction, feature() would emit
PIP.INODE.BOUNCE, not in the DB, and emission fails loud with
UnknownFeature (fails safe, not silent).

Merge sequencing after this session:
- Segbits c1bef40 (rebase onto a09df12, u16 widening, ZeroBitSet
  widening, provenance consolidation, 014-cle-spec check) is
  critic-passed at 98e282a and rebase-verified by its worker
  (54 tests pass, clippy 0, fmt clean); ready for orchestrator
  merge sequencing. Note: the 014-cle-spec fuzzer EXISTS
  upstream in f4pga/prjuray at fuzzers/014-cle-spec, but is NOT
  in prjuray-db; the FF INIT value mapping lives in the
  prjuray tools repo (utils/spec.py), which the session could
  not read (GitHub API 403 on the utils listing). FF INIT
  polarity stays open. prjuray is Apache-2.0, outside the
  crew's CC0/MIT-only vendoring line; reading is fine,
  vendoring needs a CTO ruling.
- Bitgen-core merge waits on D1/D2 fixes plus critic re-review
  plus the reconciliation commit.
- Device-file 33a4b8f (rebase, L/U adopted, demo.py validation
  with the 2-zero-padding limit, provisional xcvu3p.json, 63
  tests pass, clippy 0) is under critic review (session 7);
  merge sequencing follows its verdict.

## 2026-11-07 ~17:40 EST: orchestrator session 6, segbits-vendor MERGED (c1bef40)

Orchestrator session 6 agent f8e788f3-425e-4698-8f6d-d9bb44e72de5.
Single task: merge fm/ultra-bitgen-segbits (tip c1bef40) into
fm/ultra-bitstream. No other branch merged this session.

Pre-merge verification, all with exact commands in
~/workspace/helion-ultra-bitstream:
- `git rev-parse --abbrev-ref HEAD` plus `--short HEAD`:
  fm/ultra-bitstream at a09df12.
- `git status --porcelain`: only `?? bitfiltrator-ref/`
  (untracked reference clone, per SPEC.md).
- `git rev-parse --short fm/ultra-bitgen-segbits`: c1bef40.
  `git log --oneline a09df12..fm/ultra-bitgen-segbits`: exactly
  the five rebased commits (0da9b7e vendor data, 619928a port
  loader, 9ea177c u16 widening, 6e7e073 int-pip u8 to u16,
  c1bef40 provenance consolidation).
- `git merge-base --is-ancestor a09df12 fm/ultra-bitgen-segbits`:
  yes, fast-forwardable.
- `git diff a09df12 fm/ultra-bitgen-segbits --
  crates/helion-ultrascale/src/segbits.rs`: exactly the u16
  widening (BitRef.bit u8 to u16, set/zero signatures, parser
  bound u16::MAX, doc comments, malformed-test case 64_256 to
  64_65536, two regression tests bit_width_above_u8_parses_exactly
  and all_vendored_dbs_load, one fmt reflow). No other logic
  change. This is the corrected rebase check from orchestrator
  session 3 ruling 2, and it passes.
- `git diff --stat a09df12 fm/ultra-bitgen-segbits`: 33 files,
  49696 insertions, 19 deletions. int_pip.rs 4 lines changed
  (IntPipError::ZeroBitSet.bit u8 to u16, verify_zero_bits reader
  closure Fn(u32, u8) to Fn(u32, u16)); tests/int_pip.rs 12
  lines (fixtures to u16, including the sorted_bits helper the
  critic session 3 FLAG named, now Vec<(u32, u16)>);
  tests/int_pip_audit.rs 90 lines new (single enumeration-based
  test, no duplication of tests/int_pip.rs, per critic session
  3 P6); tests/segbits.rs 69 lines; vendored data files (5
  segbits DBs plus .origin_info.db, tile_type_INT.json, 13
  site-type JSONs, PROVENANCE.md, INT_PIP_AUDIT.md, README.md).
  PROVENANCE.md carries the lut tables line (source BitFiltrator
  ULTRASCALE_PLUS.json, SHA-256 c6320fe7322d918fc3995f9122f62b0f
  4ab7b282b517bf8231d0c63ac70b436c, MIT, Copyright (c) 2022 EPFL
  VLSC), per the session-4 provenance direction.
- Critic gates: session 3 PASS at 98e282a (CRITIQUE.md:445
  addendum, F1 fixed in execution, all 5 DBs load whole, G1
  formally CLOSED). Its INTERFACES.md FLAG is closed:
  INTERFACES.md:78 states `bit: u16` (orchestrator session 5
  applied the contract update). Its merge-sequencing FLAG is
  closed: the merged segbits.rs is the u16 version, and
  int-pip's u8 bits are widened on this branch.

Merge: `git merge --ff-only fm/ultra-bitgen-segbits`, fast-forward
a09df12..c1bef40, 33 files changed, 49696 insertions, 19
deletions. Tip confirmed c1bef40. Post-merge `git status
--porcelain`: only `?? bitfiltrator-ref/`.

Post-merge verification on the new tip (full path
~/.cargo/bin/cargo, via nohup setsid, -j2 max, logs
/tmp/orch-merge-segbits-test.log and
/tmp/orch-merge-segbits-clippy.log):
- `cargo test -j2 -p helion-ultrascale`: 54 passed, 0 failed
  (25 unit + 2 int_pip + 1 int_pip_audit + 20 lut_ff_bram + 4
  segbits + 2 smoke_place), matching the worker's rebase
  verification exactly. Named tests confirmed in the log:
  segbits::tests::all_vendored_dbs_load ok (all 5 DBs load
  whole, 7082 features), int_pip_audit_db_internal ok (3774
  of 3774), segbits::tests::bit_width_above_u8_parses_exactly
  ok.
- `cargo clippy -j2 -p helion-ultrascale --all-targets`: exit 0,
  0 warnings. The pre-existing vec! warning is gone: it was
  fixed on the lut branch by dfd08d0, already in the merged
  tip.
- `cargo fmt -p helion-ultrascale -- --check`: clean.

Block status: the prjuray-db ground truth is now vendored,
verified, and consumable on fm/ultra-bitstream. No block goes
green from this merge alone: segbits-vendor is a data plus
loader lane, and the diff harness has not yet run a real
emitter comparison. INT_PIP stays green tier 1 (its 2
integration tests still pass on the merged tip). LUT_INIT,
FF_INIT, BRAM_CONTENT, BRAM_PARITY stay un-green until the
diff harness runs against the bitgen-core emitter.

Queue status after this merge:
1. fm/ultra-bitgen-int 6bb30b6: MERGED, INT_PIP green tier 1.
2. fm/ultra-bitgen-lut 0e81099: MERGED as a09df12.
3. fm/ultra-bitgen-segbits c1bef40: MERGED (this session).
4. fm/ultra-bitgen-core: NOT merged, D1/D2 fixes in progress
   (session 2), critic re-review required, then the
   SegbitsDb/BitRef reconciliation is the merge-time commit.
5. fm/ultra-bitgen-device 33a4b8f: NOT merged, critic session 7
   review running; merge sequencing follows its verdict.
6. Integration worker: active (WireDb seam option A approved,
   IntPip field rename routed, slice-count correction routed);
   its deliverable needs a critic review when it lands.
7. FF INIT polarity: open, researcher session reading
   f4pga/prjuray utils/spec.py via a direct clone (the
   014-cle-spec fuzzer exists upstream; the mapping lives in
   the tools repo, not prjuray-db). FF INIT stays BLOCKED
   until the mapping lands.

## 2026-11-07 ~17:36 EST: bitgen-core session 2 DONE (critic D1/D2 plus FLAG F-A, all fixed and committed)

Branch fm/ultra-bitgen-core, tip dd4910f (three commits on
0f3aa2d), worktree clean, nothing pushed, no merge to master.
Ready for the correctness critic re-review; merge waits on that
plus the SegbitsDb/BitRef reconciliation commit.

D1 (9e32ed6, parse-back for all-! valueless features):
- Present for a valueless feature is now n_set > 0 AND all set
  bits 1 AND all zero bits 0, plus a new `all_zero_bits_zero`
  helper. A `!` bit reading 1 means absent, not an error; the
  error path is reserved for genuinely inconsistent input (it
  now guards valued features only).
- The n_set > 0 clause is a deliberate refinement of the
  critic's literal formula (all set 1 AND all zero 0). The bare
  formula is vacuously true for an all-! feature, so it would
  still emit a VCC_WIRE line for every untouched INT mux and a
  .V1 line for every FF: the second failure mode D1 names. An
  all-! feature sets no bits, so it is indistinguishable from
  absent and is dropped on parse-back, the same boundary as the
  documented all-zero-value limit. The reasoning is recorded in
  the code comment, the commit message, and this entry for the
  re-review to judge.
- Parse-back renders the DB vocabulary .V0/.V1, never bare
  AFF.INIT: a set INIT bit renders as AFF.INIT.V0; a clear bit
  renders nothing (.V1 is all-!, hence unobservable). The V0/V1
  polarity mapping is still BLOCKED, so no value meaning is
  claimed; the module pins only the bit pattern.
- Tests: the real BOUNCE_E_0_FT1 mux from segbits_int.db (7
  features, lines verified verbatim against the on-disk clone)
  proves parse(emit(f)) == f for the routed case and the
  untouched case, plus an explicit-VCC case pinning the drop
  boundary. Stub AFF.INIT renamed to AFF.INIT.V0 with V1 added.
  Pre-fix evidence (tests written first, run on 0f3aa2d): 3
  passed, 5 failed; routed case errored MustBeZeroViolated for
  VCC_WIRE at far 832 bit 0, untouched case emitted a spurious
  VCC_WIRE line, round-trip errored for AFF.INIT.V1. After the
  fix all pass.

D2 (049542c, START and DESYNC in the ending):
- The ending now emits START (CMD 5) after DGHIGH_LFRM and
  DESYNC (CMD 13) before the trailing NOOPs. Both codes are
  UG570 table 9-22 and were already defined in the module's cmd
  table; no values were guessed. The FAR/MASK/CTL0 writes Vivado
  emits after START are NOT added (US+ values unknown). The
  ~100 NOOP words Vivado emits between DGHIGH_LFRM and START are
  not reproduced; their exact count is tier-2 detail, per the
  critic. Docs updated (module doc, emit_bitstream doc, TRAILING
  _NOOPS comment) so the ending sequence is stated correctly
  everywhere.
- Test: hand-computed stream grows 729 to 733 words (2932
  bytes); ending offsets recomputed (START 329/330, DESYNC
  331/332, trailing NOOPs 333..732). Frame CRCs unchanged.

F-A (dd4910f, CRC exclusion list):
- REMOVED, not re-cited. No bitstream_crc.py and no SIM_CONFIGE2
  reference exists anywhere on disk (grep over prjxray-ref and
  bitfiltrator-ref, zero hits), both local prjxray crc.h copies
  contain the algorithm only, no exclusion list, and the verified
  source says the opposite: bits2rbt GetFpgaConfigurationCRC
  feeds every non-NOP write packet into the CRC with no
  exclusions. The emitter never writes those registers, so
  removal changes no test outcome.

Verification on tip dd4910f (commands via nohup setsid, -j2 max,
no target cleaning): `cargo test -p helion-ultrascale -j2` 31
passed, 0 failed (21 unit + 8 round-trip + 2 smoke);
`cargo clippy -p helion-ultrascale -j2 --all-targets` exit 0
with exactly 1 warning, the pre-existing F2 vec! in
smoke_place.rs:43 (fixed on fm/ultra-bitgen-device, not this
branch); `cargo fmt -p helion-ultrascale -- --check` shows no
diffs in the touched files, only the 2 pre-existing diffs in
lib.rs and smoke_place.rs, deliberately untouched.

## 2026-11-07 ~17:45 EST: researcher session 1, FF INIT polarity mapping RESOLVED

FF INIT is UNBLOCKED. The value-to-feature mapping is verified from the
fuzzer source, not guessed. Reference clone: ~/workspace/prjuray-tools-ref,
f4pga/prjuray at c550b03a26b4c4a9c4453353bd642a21f710b3ec (shallow, read-only).
Nothing vendored or copied into any repo worktree (prjuray is Apache-2.0,
outside the CC0/MIT-only line; only facts cited here).

The chain, all verified by direct reads:
- utils/spec/flipflops.py instantiates FFs with .INIT(1'b0) or .INIT(1'b1)
  (line with `print("          .INIT(%s)" % np.random.choice(["1'b0", "1'b1"]))`).
- tools/dump_features.tcl:897-902 names the feature from the cell property:
  `set init_prop [get_property -quiet INIT $cell]` then
  `"$bel_name.INIT.V[string range $init_prop end end]"`, and
  `"$bel_name.SRVAL.V$srval"` where srval is 1 for FDPE/FDSE/LDPE and 0
  for FDCE/FDRE/LDCE. The V suffix IS the attribute value: INIT 1'b0
  gives V0, INIT 1'b1 gives V1; SRVAL 0 gives V0, SRVAL 1 gives V1.
  (remove_verilog_notation at :60-67 strips the 1'b prefix elsewhere,
  same convention.)
- fuzzers/014-cle-spec/Makefile:68, :81: design.features is dumped from
  the post-route DCP by dump_features.tcl and correlated with design_bits
  by create_segdata_from_features.py, so the DB feature names are exactly
  the dump_features.tcl names.
- Vendored DB lines (checked this session in
  ~/workspace/prjuray-db-ref/zynqusp/segbits_clel_l.db and segbits_clem.db):
  `CLEL_L.AFF.INIT.V0 12_02`, `CLEL_L.AFF.INIT.V1 !12_02`;
  `CLEL_L.AFF.SRVAL.V0 12_01`, `CLEL_L.AFF.SRVAL.V1 !12_01`;
  same pattern for BFF, CFF, HFF2 and in CLEM. CEUSED confirms the V
  suffix names the value, not the bit polarity:
  `ABCDFF.CEUSED.V0 !14_07`, `CEUSED.V1 14_07` (V0 clears, V1 sets).

The mapping:
- INIT value 0 -> V0 -> bit SET (1). INIT value 1 -> V1 -> bit CLEAR (0).
  The configuration bit is the INVERSE of the INIT value: bit = NOT(INIT).
- SRVAL value 0 -> V0 -> bit SET. SRVAL value 1 -> V1 -> bit CLEAR.
  bit = NOT(SRVAL).

Consequences (for the CTO, not decided here):
- The lut worker's emission (bit = value) is INVERTED and must change:
  canonical spelling is .V0/.V1 with V selected by value, and the
  segbits resolution handles the bit polarity (V0 sets, V1 clears).
- The bitgen parse-back (bare AFF.INIT present when the bit reads 1)
  is also inverted: bit 1 means INIT 0 (render .V0), bit 0 means INIT 1
  (render .V1).
- Interfaces session 2 item 5 and correctness session 6 FLAG F-D are
  answered: the polarity is no longer a FLAG. FF INIT stays blocked
  only on the spelling change landing in lut-ff-bram and bitgen, plus
  critic re-review.

## 2026-11-07 ~17:41 EST: bitgen-core session 2 follow-up, FF INIT polarity applied to the D1 fix (commit 88070a5)

Branch fm/ultra-bitgen-core, tip 88070a5 (one commit on dd4910f),
worktree clean, nothing pushed, no merge to master. The CTO ruling
(verified by the CTO against the prjuray clone at c550b03) is now
applied in parse-back; the mapping and its source are recorded here
and in the bitgen.rs module doc.

Mapping: INIT value 0 selects .V0, which SETS the bit; INIT value 1
selects .V1, which CLEARS the bit. bit = NOT(INIT). SRVAL is the
same: bit = NOT(SRVAL). Source chain, re-verified this session by
direct reads: tools/dump_features.tcl:897-902 names the feature from
the cell INIT property (`set init_prop [get_property -quiet INIT
$cell]` then `"$bel_name.INIT.V[string range $init_prop end end]"`),
so the V suffix IS the attribute value; utils/spec/flipflops.py:100
instantiates .INIT(1'b0) or .INIT(1'b1); vendored DB lines
CLEL_L.AFF.INIT.V0 12_02, V1 !12_02 and BFF.INIT.V0 12_04,
V1 !12_04 confirm V0 sets and V1 clears (SRVAL the same way,
V0 12_01 / V1 !12_01). This answers interfaces session 2 item 5
and correctness session 6 FLAG F-D, as the researcher entry above
records.

Change: parse-back renders .V0 for a bit reading 1 (INIT 0) and .V1
for a bit reading 0 (INIT 1); the bare AFF.INIT spelling is never
emitted. No inversion is applied in the parse path: the DB's V0/V1
features already encode the polarity, so rendering the matching
feature renders the right value.

This refines the D1 present rule (9e32ed6), which dropped every
all-! feature: an all-! feature now renders when it is the exact
complement of another feature of the same tile type (its zero set
equals that feature's set-bit set, and that feature lists no `!`
bits), implemented as a per-tile-type cached signature set
(complement_signatures plus zero_signature in bitgen.rs). VCC_WIRE
still has no complement (no single feature sets exactly its six
bits with no `!` bits) and is still dropped, so the D1 untouched-mux
and explicit-VCC tests still pass. The D1 commit message's claim
that .V1 is unobservable is superseded by this commit; the module
doc now states the complement rule and the resolved polarity.

Tests: new ff_init_polarity_renders_v0_for_bit_one_and_v1_for_bit_zero
uses the real vendored bit positions (AFF 12_02, BFF 12_04) and
proves emit(INIT 0) plus emit(INIT 1) parse back to .V0/.V1 with
no bare AFF.INIT line. The all-zero-valued-feature test now expects
AFF.INIT.V1 in the output: the A6LUT line is still dropped, while
the FF INIT bit reading 0 renders V1 through the complement rule.
Pre-fix check (new assertions run against dd4910f): both fail, 7
passed 2 failed (polarity test drops BFF.INIT.V1; all-zero test
returns empty instead of AFF.INIT.V1).

Consequence for the orchestrator and the harness, stated in the
module doc: parse-back renders exactly one .V0/.V1 line per FF in
the device for DBs with the pair structure, so round-trip exactness
on real device data requires complete FASM (every FF's INIT
specified); a partial FASM gains .V1 lines for unmentioned FFs. The
harness round-trip check (parse(emit(f))==f) should account for
this. The lut worker's emission (bit = value) is inverted per the
researcher entry and must change separately; this commit touches
only the bitgen parse-back side.

Verification on tip 88070a5 (commands via nohup setsid, -j2 max,
no target cleaning): `cargo test -p helion-ultrascale -j2` 32
passed, 0 failed (21 unit + 9 round-trip + 2 smoke);
`cargo clippy -p helion-ultrascale -j2 --all-targets` exit 0 with
exactly 1 warning, the pre-existing F2 vec! in smoke_place.rs:43;
`cargo fmt -p helion-ultrascale -- --check` clean on the touched
files (3 fmt diffs in the new code fixed; only the 2 pre-existing
diffs in lib.rs and smoke_place.rs remain, deliberately untouched).

## 2026-11-07 ~17:45 EST: orchestrator session 6 addendum, FF INIT polarity correction to the merge entry

The session-6 merge entry above (item 7, queue status) says FF INIT
polarity is open and the researcher is still reading utils/spec.py.
That was true when the entry was written; the researcher session 1
entry directly above this one resolved it minutes later, while the
merge verification was running. Correcting the record here rather
than editing the merge entry (COORD.md is append-only).

The researcher reports the mapping RESOLVED from the fuzzer source:
INIT value 0 -> V0 -> bit SET (1); INIT value 1 -> V1 -> bit CLEAR
(0). bit = NOT(INIT). SRVAL the same: bit = NOT(SRVAL). The
orchestrator spot-verified the two load-bearing links itself:
tools/dump_features.tcl:897-902 in ~/workspace/prjuray-tools-ref
(f4pga/prjuray at c550b03) names the feature from the cell property
(`$bel_name.INIT.V[string range $init_prop end end]`, so the V
suffix is the attribute value), and the vendored DB lines confirm
the pattern (`CLEL_L.AFF.INIT.V0 12_02`, `V1 !12_02`;
`CEUSED.V0 !14_07`, `V1 14_07`, so V names the value, not the bit
polarity). utils/spec/flipflops.py:100 instantiates .INIT(1'b0)
or (1'b1).

Consequences, all for the CTO to route (not decided here): the lut
worker's FF emission (bit = value) is INVERTED and must change to
canonical .V0/.V1 spelling with V selected by value; the bitgen
parse-back is also inverted (bit 1 means INIT 0, render .V0; bit 0
means INIT 1, render .V1). The D1 fix in bitgen-core session 2 must
render .V0/.V1 with this polarity, not bit=value. FF INIT is
unblocked as a question, but stays blocked on the spelling change
landing in lut-ff-bram and bitgen plus critic re-review. No
critic has verified the researcher's chain yet; the next critic
review of the spelling-change work should re-verify it.

## 2026-11-07 ~17:45 EST: orchestrator session 7, device-file MERGED (9289fed)

Orchestrator session 7 agent edaca706-7096-4533-8195-48c70adb07f2.
Single task: merge fm/ultra-bitgen-device (tip 33a4b8f) into
fm/ultra-bitstream. No other branch merged this session.

Pre-merge verification, all with exact commands in
~/workspace/helion-ultra-bitstream:
- `git rev-parse --abbrev-ref HEAD` plus `--short HEAD`:
  fm/ultra-bitstream at c1bef40.
- `git status --porcelain`: only `?? bitfiltrator-ref/`
  (untracked reference clone, per SPEC.md).
- `git rev-parse --short fm/ultra-bitgen-device`: 33a4b8f.
  `git log --oneline c1bef40..fm/ultra-bitgen-device`: exactly
  the six rebased commits (91e5522 G3, f378e75 loader, ac22d8d
  Cargo.lock sync, 5dcfe11 fmt, 61012ff demo validation, 33a4b8f
  provisional xcvu3p.json).
- `git merge-base c1bef40 fm/ultra-bitgen-device`: a09df12, so a
  merge commit was needed, not a fast-forward.
- Overlap check before merging: only 2 files changed on both
  sides relative to base a09df12 (crates/helion-ultrascale/
  Cargo.toml and data/README.md). All other files are
  one-sided: the segbits side (segbits.rs, int_pip.rs, tests,
  vendored DBs, site JSONs, PROVENANCE.md, INT_PIP_AUDIT.md)
  takes the c1bef40 version; the device side (device_file.rs,
  device.rs, lib.rs, scripts, xcku3p.json, ULTRASCALE_PLUS.json,
  xcvu3p.json, device tests, Cargo.lock) takes the branch
  version. Critic session 7 P1 already confirmed bel.rs and
  smoke_place.rs are identical to the merged tip, so no
  conflict was expected there.

Merge: `git merge --no-ff fm/ultra-bitgen-device`, clean, zero
conflicts. Both overlapping files auto-merged by the ort
strategy (Cargo.toml: serde line from the device side;
README.md: segbits cross-references plus the device-file
xcvu3p section). No conflict markers remain (grep count 0).
Merge commit 9289fed, message "merge fm/ultra-bitgen-device:
device-file loader, demo.py validation, provisional xcvu3p.json
(33a4b8f)". 18 files changed, 14294 insertions, 6 deletions.
Post-merge `git status --porcelain`: only `?? bitfiltrator-ref/`.

Post-merge verification on the new tip (full path
~/.cargo/bin/cargo, via nohup setsid, -j2 max, logs
/tmp/orch-merge-device-test.log and
/tmp/orch-merge-device-clippy.log):
- `cargo test -j2 -p helion-ultrascale`: 70 passed, 0 failed
  (35 unit + 3 demo + 3 xcvu3p + 2 int_pip + 1 int_pip_audit +
  20 lut_ff_bram + 4 segbits + 2 smoke_place). That is the exact
  union of the two critic-verified suites: the c1bef40 tip's 54
  plus the device branch's 16 additions (10 unit loader tests +
  3 demo + 3 xcvu3p). Named tests confirmed in the log:
  segbits::tests::all_vendored_dbs_load ok (all 5 DBs load
  whole, 7082 features), int_pip_audit_db_internal ok (3774
  of 3774), device_file loader hand-checks ok (B6LUT/BFF/AFF/
  BRAM vs BitFiltrator), xcvu3p tests ok (all rows, frame
  total, provisional header counts).
- `cargo clippy -j2 -p helion-ultrascale --all-targets`: exit 0,
  0 warnings.
- `cargo fmt -p helion-ultrascale -- --check`: exit 0, clean.

Merged-tree spot checks (read from the merged tip, not assumed):
- src/segbits.rs:42 `pub bit: u16`: the u16 version from the
  segbits merge survived; the device branch's older u8 copy
  did not regress it.
- data/xcvu3p.json: 18 helion_provisional notes, all with
  field/source/value (note 0 device, note 17 total_frames 73035
  with the UG570 71820 overshoot note); part
  xcvu3p-ffvc1517-1-e, 1 SLR, 49,260 slices, 394,080 LUTs,
  788,160 FFs, 720 BRAM, 2280 DSP, IDCODE 0x14B39093 in
  slrs.SLR0.idcode.
- data/README.md carries both sides: the PROVENANCE.md
  cross-reference (segbits lane) and the xcvu3p section with
  the frame-total overshoot note (device lane).

Critic session 7 FLAGs, disposition:
- F1 (2-zero-padding path for last-of-row FARs untested, 0 of
  261 FARs): carried forward as an open item. It does not
  block this merge (the generator is a validation tool), but
  the path must be exercised before the generator is trusted
  for full-device bitstreams.
- F2 (make_synth_xcku3p_bit.py hardcodes the reference clone
  path): carried forward, minor; a `--ref-src` flag would fix
  it.
- F3 (49,250 vs 49,260 slice discrepancy): already resolved
  by the CTO ruling recorded in orchestrator session 5
  (xcvu3p adopts 49,260 slices, 394,080 LUTs, 788,160 FFs;
  device.rs 49,250 is a "394K" rounding artifact). The merged
  file's 49,260 is correct per that ruling; the critic's
  "stays open" is stale. The device.rs correction itself is
  routed to the integration worker and is not part of this
  merge.

Block status: no block goes green from this merge alone:
device-file is a data plus loader lane. INT_PIP stays green
tier 1 (its 2 integration tests still pass on the merged
tip). LUT_INIT, FF_INIT, BRAM_CONTENT, BRAM_PARITY stay
un-green until the diff harness runs against the bitgen-core
emitter. What this merge unblocks: the harness can now run
xcvu3p combos against the provisional xcvu3p.json (previously
blocked on the missing device file); those numbers stay
provisional until Vivado confirmation lands (license server
still refused since Oct 22).

Queue status after this merge:
1. fm/ultra-bitgen-int 6bb30b6: MERGED, INT_PIP green tier 1.
2. fm/ultra-bitgen-lut 0e81099: MERGED as a09df12.
3. fm/ultra-bitgen-segbits c1bef40: MERGED.
4. fm/ultra-bitgen-device 33a4b8f: MERGED (this session,
   9289fed).
5. fm/ultra-bitgen-core: NOT merged, D1/D2 fixes done in
   session 2 (tip dd4910f), critic re-review required, then
   the SegbitsDb/BitRef reconciliation is the merge-time
   commit.
6. lut-ff session 2: running, FF emission spelling change to
   canonical .V0/.V1 with the resolved polarity (bit =
   NOT(INIT)); critic re-review must re-verify the
   researcher's polarity chain.
7. Integration worker: active (WireDb seam option A approved,
   IntPip field rename routed, slice-count correction routed);
   its deliverable needs a critic review when it lands.

## 2026-11-07 ~17:55 EST: lut-ff-bram session 2 DONE (FF INIT/SRVAL canonical .V0/.V1 spelling, resolved inverse polarity)

Branch fm/ultra-bitgen-lutff, worktree ~/workspace/helion-ultra-bitgen-lutff,
tip c0b3438 (one commit on c1bef40), worktree clean, nothing pushed, no
merge to master. Ready for critic re-review; the critic must re-verify
the researcher's polarity chain itself (see below).

What changed (crates/helion-ultrascale/src/bitloc.rs only, plus tests):
- `ff_init_fasm` now emits the canonical spelling: `SLICE_XxYy.{AFF}.INIT.V0`
  (INIT 0) or `.INIT.V1` (INIT 1). The valued spelling (`AFF.INIT = 1'b0/1`)
  is removed entirely; the only remaining mentions are doc comments stating
  it is non-canonical. Module docs updated with the resolved polarity and
  its sources.
- `ff_placed_bit` now records the physical bit: value = !init (bit =
  NOT(INIT)). The session-1 value (bit = value) was inverted; the
  session-1 test asserted the inverted values and is corrected here.
- New SRVAL emission, same spelling rule: `ff_srval_fasm` emits
  `.SRVAL.V0` (SRVAL 0) / `.SRVAL.V1` (SRVAL 1); `ff_srval_placed_bit`
  records value = !srval; `ff_srval_location` derives the SRVAL location
  from the INIT location. LUT INIT and BRAM emission unchanged.

SRVAL location rule (derived, not a table read; flag for the critic):
the vendored BitFiltrator JSON carries no SRVAL table (RegLoc is the INIT
location only), so the SRVAL bit is the INIT bit plus a within-slice
delta: -1 for A/C/E/G letters, +1 for B/D/F/H letters. A uniform -1
rule would be wrong for half the FFs (BFF INIT 4, SRVAL 5; DFF 10/11;
FFF 28/29; HFF 34/35). The delta is verified against prjuray-db
`segbits_clel_l.db`, `segbits_clel_r.db`, `segbits_clem.db`, and
`segbits_clem_r.db` for all 16 FFs (identical in all four files), and the
test checks it against verbatim DB lines. The delta is a within-slice
offset, so it holds at every slice Y; the test pins it across the +96
clock-region jump (AFF SRVAL at Y30 is 1537, INIT 1538). Out-of-range
results fail loud (TableCorrupt) instead of wrapping.

Tests (tests/lut_ff_bram.rs, 22 tests, was 20):
- `ff_init_zero_and_one` corrected to the resolved polarity.
- `ff_fasm_lines` corrected to the .V0/.V1 spelling.
- `ff_srval_fasm_and_location`: SRVAL spelling, SRVAL locations for an
  A letter (AFF 12_01) and a B letter (BFF 12_05), placed-bit polarity,
  Y30 delta pin, out-of-range index error.
- `ff_init_srval_resolve_through_segbits`: 128-line verbatim fixture
  (all 16 FFs x INIT/SRVAL x V0/V1 in CLEL_L and CLEM, extracted by
  script from prjuray-db commit affbc5e, CC0 1.0; the CLEL DBs are not
  vendored in the repo, so this excerpt is the test's copy of the DB).
  For every FF in both tile types it proves, against the DB: the emitted
  .V0 feature resolves to setting the table-located bit, .V1 resolves to
  clearing it, for INIT and SRVAL alike, and the placed-bit values match
  the same polarity. It also cross-checks the whole RegLoc table against
  the DB (table frame_ofst equals the DB bit for all 16 FFs).

Verification (full cargo path, nohup setsid, -j2, no target cleaning):
`cargo test -p helion-ultrascale -j2` 56 passed, 0 failed (25 unit + 2
int_pip + 1 audit + 22 lut_ff_bram + 4 segbits + 2 smoke);
`cargo clippy -p helion-ultrascale -j2 --all-targets` 0 warnings;
`cargo fmt -p helion-ultrascale -- --check` clean. Zero em dashes in
either changed file.

Open for the critic: re-verify the polarity chain
(dump_features.tcl:897-902, flipflops.py:100, DB lines) and rule on the
derived SRVAL delta rule. Consistency note: the bitgen-core session 2
D1 parse-back renders .V0 for a set INIT bit and drops .V1 as
unobservable (all-!); with the resolved polarity that is exactly
INIT 0 -> .V0 and INIT 1 -> .V1 dropped, so the two sides agree and
parse(emit(f)) == f holds for FF INIT at the observable boundary.

## 2026-11-07 ~22:19 EST: integration worker, WireDb naming seam IMPLEMENTED (CTO ruling option A)

Proposal filed ~17:10 EST, CTO ruling received same evening: option A
APPROVED. Implemented in commit 469e805 on fm/ultra-bitgen-integ:
`fn wire_info(&self, wire: WireId) -> Option<WireInfo<'_>>` on
fabric::WireDb, `WireInfo { name, tile }` (wire name exactly as in the
segbits DB, Vivado tile name). EmptyWireDb and the mock return None;
the routing-graph WireDb returns Some from its DB-derived node table.
INTERFACES.md gained a WireDb section (crew dir, not versioned with
the code) in the same change; the orchestrator signs off the contract
update at merge time (ruling already signed off at COORD.md:1270,
superseding the stale PROPOSED lines at :531 and :885).

Tests: db_backed_wire_db_wire_info_names_wires (two verbatim
segbits_int.db lines, INT_X12Y45), empty_wire_db_wire_info_is_none,
mock_wire_info_is_none. `cargo test -j1 -p helion-ultrascale`: 38
passed, 0 failed.

Status clarification, per the CTO ruling: the seam does NOT
retroactively block the INT_PIP tier-1 green. That green was for
emission (zero diff against the public segbits DB plus critic
re-run) and it stands. The seam gates the router-to-PIP translator
as its own integration item, tracked separately. The translator
(maps each consecutive wire pair in a route to an IntPip via
wire_info, fails loud on any unnamed wire) is not written yet.

## 2026-11-07 ~22:29 EST: integration worker, bitgen parse-back fix (critic D1) plus FF INIT polarity record

Two CTO tasks arrived together; the second superseded the first
within the hour. Recorded in order.

FF INIT polarity: the CTO task as received said FF INIT was BLOCKED
on the V0/V1 polarity mapping (canonical spelling .V0/.V1, one
feature per FF selected by value, but which V means INIT 0 vs 1
unverified by any on-disk source; do not emit FF INIT bits assuming
bit=value; do not change the FF spelling in the translator yet).
That task is SUPERSEDED by researcher session 1 (COORD.md ~17:45):
the mapping is RESOLVED from the fuzzer source: INIT 0 -> V0 -> bit
SET (1); INIT 1 -> V1 -> bit CLEAR (0); bit = NOT(INIT); SRVAL the
same. Interfaces session 2 item 5 and correctness session 6 FLAG F-D
are answered. FF INIT stays blocked only on the spelling change
landing in lut-ff-bram and bitgen, plus critic re-review. No critic
has verified the researcher's chain yet.

Bitgen parse-back defect (critic D1, CRITIQUE.md session 6): for a
valueless feature whose bit list is all `!` bits, `present` was
vacuously true and the must-be-zero check errored when the bit read
1, instead of treating the feature as absent. Fixed in commit
7365f2e on fm/ultra-bitgen-integ (bitgen.rs ported verbatim from
fm/ultra-bitgen-core 0f3aa2d in 281e9f8 first): valueless presence
now requires all set bits 1 AND all `!` bits 0 (new
`all_zero_bits_zero` helper); the error path guards valued features
only. Regression test all_bang_feature_with_bit_set_parses_back_absent
(V0 12_02 / V1 !12_02 complements). Revert proof: with the old rule
the test fails with MustBeZeroViolated { feature: AFF.INIT.V1, far:
1292, bit: 2 }. The test's V0/V1 stub matches the resolved polarity
exactly (V0 sets, V1 clears). `cargo test -j1 -p
helion-ultrascale`: 55 passed, 0 failed at commit time.

Supersession, recorded plainly: bitgen-core session 2 fixed the same
defect at 9e32ed6 (adds an `n_set > 0` clause: an all-`!` feature is
indistinguishable from absent, so it is dropped) and 88070a5 (exact-
complement rule: an all-`!` feature renders when it is the exact
complement of another feature, so FF .V1 renders when the bit reads
0). My 7365f2e presence rule is the weaker variant (it would still
render a VCC_WIRE line for every untouched INT mux). An uncommitted
`n_set > 0` refinement of my fix was DISCARDED after 88070a5: it
would wrongly drop .V1. My branch's bitgen.rs (0f3aa2d + 7365f2e) does
NOT contain D2 (START/DESYNC, 049542c), F-A (CRC list removal,
dd4910f), the `n_set > 0` refinement, or the complement rendering;
the orchestrator must take the bitgen-core tip 88070a5 at merge, and
my branch's bitgen.rs must not overwrite it. My 7365f2e test
expectations are compatible with the complement rule (V1 emitted
parses back to V1). Correctness-critic re-check of the D1 fix is
requested; the critic re-reviews the bitgen-core tip before merge.
Correction to the tail note above: the claim that D1 "drops .V1 as
unobservable" is superseded by 88070a5; .V1 renders via the
complement rule.

## 2026-11-07 ~22:33 EST: integration worker, IntPip sink-first rename (critic session 6)

CTO ruling (CRITIQUE.md:1244, verified against tile_type_INT.json
and INT_PIP_AUDIT.md): FASM PIP naming is sink-first. The DB feature
is PIP.<dst>.<src> (dst.src matches 3770/3774, src.dst 0/3774).
int_pip.rs feature() built PIP.{src}.{dst} with the field names
inverted relative to physical direction (its test used src=BOUNCE,
the physical dst, and dst=INODE, the physical src). The emitted
feature strings were correct and INT_PIP tier-1 green stands.

Ported int_pip.rs plus segbits.rs plus tests/int_pip.rs verbatim
from fm/ultra-bitgen-int 6bb30b6, then renamed in commit f92d09b:
IntPip.src is now the physical source wire, IntPip.dst the physical
sink, feature() builds PIP.{dst}.{src}, MissingPip Display follows
the same order. Emitted strings are byte-identical: the unit-test
expectations and the integration-test FASM spot checks are unchanged,
and the integration test still verifies all 11 blinky hops bit-exact
against an independent parse (11 set bits, 48 zero bits) plus all
3778 DB lines. `cargo test -j1 -p helion-ultrascale`: 69 passed, 0
failed (55 unit incl. 5 int_pip plus 7 segbits, 5 bitgen_roundtrip,
2 int_pip, 2 corpus legality, 2 smoke, 3 STA sanity).

Notes for the merge: tests/int_pip.rs imports SegbitsDb/BitRef by
module path (helion_ultrascale::segbits::...) because the crate root
already re-exports bitgen's BitRef/SegbitsDb; the two-trait
reconciliation belongs to the orchestrator at merge time. The pulled
segbits.rs still has u8 BitRef.bit; the u16 widening rides with the
segbits-vendor rebase, not this commit. The integration test's BFS
walks DB edges in their written order and its DRIVER/SINK labels name
the walk start/end, not physical signal flow; the verified path is
unchanged.

## 2026-11-07 ~22:38 EST: integration worker, xcvu3p slice-count correction (CTO ruling)

Commit dd4b2c7: xcvu3p adopts 49,260 slices (394,080 LUTs, 788,160
FFs). device.rs's 49,250 was a rounding artifact of dividing XMP103's
rounded 394K figure. xcvu3p_clb_counts_match_amd_docs now pins LUTs
394,080 and FFs 788,160 exactly; xcvu3p_carry8_one_per_slice pins
49,260. xcku3p stays at AMD's 20,340 slices (162,720 LUTs, 325,440
FFs); the device file's num_slices 27,120 contradicts its own
num_luts and is a file-header anomaly per the ruling, not ground
truth (doc comment updated; the xcku3p counts test already pins
20,340). The provisional xcvu3p.json frame-total gap (73,035 computed
vs UG570 Table 1 71,820) is untouched and stays flagged provisional.
`cargo test -j1 -p helion-ultrascale`: 69 passed, 0 failed.

## 2026-11-07 ~22:48 EST: integration worker, complete-FASM generation for FF INIT/SRVAL (default V0)

CTO ruling 2026-11-07 (complete FASM): parse-back renders exactly
one .V0/.V1 line per FF (exact-complement rule, bitgen-core 88070a5),
so round-trip exactness requires COMPLETE FASM. Commit 3e6d17f on
fm/ultra-bitgen-integ adds crates/helion-ultrascale/src/fasm_gen.rs:
emit_ff_fasm walks placed slices and emits one INIT line plus one
SRVAL line per used FF BEL, canonical spelling
{SITE}.{FF}.INIT.V{0,1} / .SRVAL.V{0,1}, V selected by value.
Unspecified INIT/SRVAL defaults to V0 (Xilinx default 0). No bare
`AFF.INIT = 1'b` spelling: that is the lut worker's bitloc
ff_init_fasm, bit=value, INVERTED per the resolved polarity
(bit = NOT(INIT)); the lut worker's emission must change separately.
The FF BEL name table is duplicated from bitloc::ff_fasm_name with a
reconcile note (that file is not in this branch yet). Today helion-ir
Hff carries no INIT/SRVAL params, so every FF is unspecified; FfAttr
leaves the specified path open. ultra.rs runs the generator after
legality (empty attr map) and logs the line count; the lines feed the
BITGEN stage once ROUTE lands.

Tests: 8 new (unspecified emits V0, specified 1 emits V1, mixed,
placed-site-not-packed-name, all 16 BEL names, BEL order,
pack-place-then-generate end to end, bad index fails loud).
`cargo test -j1 -p helion-ultrascale`: 77 passed, 0 failed (63 unit
incl. 8 fasm_gen, 5 bitgen_roundtrip, 2 int_pip, 2 corpus legality,
2 smoke, 3 STA sanity).

## 2026-11-07 ~22:50 EST: integration worker, gate 2 (gate script + diff harness run)

Gate script (integration-owned): ~/workspace/ultra-bitgen-crew/gate.sh.
Runs the four `helion impl` commands (blinky/counter x xcku3p/xcvu3p,
seed 0) from the integration worktree, then `python3 diff.py` in the
harness dir, then prints the greedy queue from the newest run's
diff.jsonl. Impl logs go to /tmp/ultra-logs/gate-<design>-<part>.log.

Gate 2 run (binary rebuilt after 3e6d17f, log /tmp/ultra-logs/gate2.log):
all four impl commands pass SYNTH, PACK, PLACE, LEGALITY, FF FASM
(blinky ff_lines=2, counter ff_lines=8), and preliminary STA
(WNS_PS=9650), then fail loud at ROUTE (WireDb empty), exit 1, no
.bit file written. Diff harness run 20261107T225008Z: 40 records, ALL
blocked, diff_bits null throughout. Greedy queue (by total_bits, the
harness's ordering when nothing is measured): counter/xcku3p LUT_INIT
256, blinky/xcku3p LUT_INIT 64, IO_CONFIG 55 (all four combos),
INT_PIP 17/11 (xcku3p only), FF_INIT 4/1 (xcku3p only), rest 0.
xcvu3p LUT/FF/INT rows are 0, blocked on the xcvu3p device file not
yet generated.

Next unblockers for measured diffs: (1) an emitter command for the
harness (HELION_BITGEN_EMITTER or --emitter-cmd; the harness records
"bitgen-core emitter absent" although bitgen.rs exists in this
branch, because the harness invokes an executable, not the library);
(2) ROUTE (routing-graph worker); (3) the xcvu3p device file.

## 2026-11-07 ~17:55 EST: orchestrator session 8, lut-ff MERGED (7f80b54)

Orchestrator session 8 agent 8099cc8f-a1d6-4bc6-bd0f-c7d809dd775b.
Single task: rebase and merge fm/ultra-bitgen-lutff (tip c0b3438, one
commit on c1bef40) into fm/ultra-bitstream (tip 9289fed, device-file
merge). No other branch merged this session.

Pre-merge gates verified: base worktree clean except untracked
`bitfiltrator-ref/`; base tip 9289fed; lut-ff tip c0b3438, worktree
clean; correctness critic session 9 PASS, no defects, on c0b3438
(CRITIQUE.md:1442, verdict at :1453), re-verifying the polarity chain
from the fuzzer source and the derived SRVAL delta rule, and
confirming the 128-line fixture is byte-faithful to prjuray-db
affbc5e.

Rebase onto 9289fed in the lut-ff worktree
(~/workspace/helion-ultra-bitgen-lutff): the first `git rebase
9289fed` attempt failed transiently with "Your local changes to the
following files would be overwritten by merge" on bitloc.rs and
tests/lut_ff_bram.rs, although `git status --porcelain` was clean
before the attempt; the rebase state showed the pick rescheduled.
`git rebase --continue` then applied the pick cleanly with no
conflicts as 7f80b54. The rebased diff is byte-identical to the
critic-passed commit's diff: `git diff 9289fed 7f80b54` equals `git
diff c1bef40 c0b3438` (2 files, +359/-12: bitloc.rs, tests/
lut_ff_bram.rs), so the critic session 9 PASS on c0b3438's content
stands for the rebased commit. No code was authored by the
orchestrator; the rebase is mechanical.

Pre-merge verification on the rebased tip 7f80b54 (full cargo path,
nohup setsid, -j2, logs /tmp/orch-rebase-lutff-test.log and
/tmp/orch-rebase-lutff-clippy.log): `cargo test -j2 -p
helion-ultrascale` = 72 passed, 0 failed (35 unit + 3 demo + 3
xcvu3p + 2 int_pip + 1 int_pip_audit + 22 lut_ff_bram + 4 segbits +
2 smoke). `cargo clippy -j2 --all-targets`: 0 warnings. `cargo fmt
-p helion-ultrascale -- --check`: clean.

Merge: `git merge --ff-only fm/ultra-bitgen-lutff` in the base
worktree, fast-forward 9289fed..7f80b54, 2 files changed, 359
insertions, 12 deletions. New tip 7f80b54, message "feat(ultrascale):
canonical .V0/.V1 FF INIT/SRVAL emission with resolved inverse
polarity".

Post-merge verification on the base tip 7f80b54 (logs
/tmp/orch-merge-lutff-test.log and /tmp/orch-merge-lutff-clippy.log):
`cargo test -j2 -p helion-ultrascale` = 72 passed, 0 failed (same
breakdown as above). `cargo clippy -j2 --all-targets`: 0 warnings.
Post-test `git status --porcelain`: only `?? bitfiltrator-ref/`.
Tip confirmed 7f80b54.

Block status: no block goes green from this merge alone (FF emission
is a code plus tables lane). INT_PIP stays green tier 1. FF INIT
stays un-green until the diff harness runs against the bitgen-core
emitter: critic session 9 FLAG F1 records the end-to-end round trip
(parse(emit(f)) == f across both branches) is still to be run after
both branches merge, and the CTO's complete-FASM ruling
(unspecified FF INIT defaults to V0, harness canonicalizes) is being
implemented by diff-harness session 3 and the integration worker.
Correction to the lut-ff session 2 COORD entry's consistency note:
its claim that bitgen-core D1 parse-back "drops .V1 as unobservable"
is stale; commit 88070a5 added the exact-complement rule, so
parse-back renders .V1 when the bit reads 0, per critic session 9.
The two sides agree on the observable boundary.

Queue: int-pip, lut, segbits, device-file, lut-ff all merged.
Remaining: bitgen-core (D1/D2 fixes plus F-A removal plus polarity
rule at 88070a5, critic session 8 re-review running, then the
SegbitsDb/BitRef reconciliation is the merge-time commit),
integration worker (active, deliverable needs a critic review when
it lands).

## 2026-11-07 ~17:55 EST: diff-harness session 3, FF INIT completion in the round-trip check (CTO complete-FASM ruling)

Done, verified, recorded. The harness now compares the round trip in
complete FASM. Change in `harness/roundtrip.py`:
`canonical_roundtrip_fasm(text, design)` parses, normalizes
spellings, completes every FF in the design with exactly one INIT
line (a missing FF gets the default `.V0`, Xilinx default INIT is
0), sorts the lines, and serializes. Normalization: valued FF
INIT/SRVAL (`AFF.INIT = 0`, `1'b0`) becomes canonical `.V0`/`.V1`
(the V suffix names the value, bit = NOT(value) per the resolved
polarity); Verilog-style values (`64'h...` as the real emitter's
parse-back renders LUT INIT) become the golden's lowercase `0x`
spelling. Bare `AFF.INIT`, a valued `.V0`/`.V1` line, and an FF value
outside 0/1 raise FasmError (fail loud, never guess).
`run_roundtrip` takes the design name now (threaded from
`diff_design`) and compares both sides in this form. The design's FF
list comes from the golden design definition: new
`design_ff_locations(design)` in `gen_golden.py` (blinky: AFF;
counter: AFF, BFF, CFF, DFF; all in SLICE_X0Y0). The placement-hash
canonical form in `fasm.py` is untouched, so the golden hashes are
unchanged.

Semantics, stated honestly: a faithful emitter reproduces a partial
golden clean (both sides complete identically). The real emitter
renders `.V1` for an unmentioned FF (bit 0), so a partial golden
still fails against it, as a precise `.V0` vs `.V1` value mismatch:
the emitted bitstream genuinely encodes INIT=1 where the design
default is INIT=0. Only a complete golden can go green against the
real emitter. Completion is round-trip-only: the per-block diff
still sees the partial golden (a 3-of-4 FF counter golden reports
FF_INIT total_bits 3, not 4).

Tests: suite now 26, all OK (was 20; +6). New fake-emitter mode
`FAKE_PARSE_FF_FLIP=1` flips the AFF INIT value in parse output
only. `test_partial_ff_init_roundtrips_clean_after_completion`
(partial counter golden, echo fake emitter: FRAMING green, FF_INIT
green, total_bits 3);
`test_wrong_ff_init_value_still_fails_roundtrip` (blinky with the
flip: FRAMING red, diff_bits 1, LUT/FF/INT green through the bits
path);
`test_completes_missing_ff_init_with_v0` (partial counter
canonicalizes identically to the full golden, all four `.V0` lines);
`test_normalizes_valued_ff_init_and_verilog_lut_value` (`64'h` and
`1'b0` normalize, shuffled input gives the same form);
`test_real_emitter_parseback_spelling_matches_complete_golden`
(complete golden equals its `.V0`/`.V1` parse-back spelling;
partial golden against a `.V1`-rendering parse-back differs on the
value);
`test_rejects_bare_ff_init_and_unknown_design` (FasmError for bare
`AFF.INIT`, valued `.V0`, value 2, unknown design). Negative
control: plain `canonical_fasm` has no DFF.V0 line and keeps the
valued spelling, and a complete golden does not equal its parse-back
spelling under it; the new function fixes both.

Verification (exact commands, all from
`~/workspace/ultra-bitgen-crew/harness`): `python3 -m unittest
discover -s tests` gives `Ran 26 tests ... OK`. `python3 diff.py`
full run `runs/20261107T225008Z/` (exit 0), 40 records, all blocked,
all `diff_bits` null; placement hashes identical to the baseline run
and to VERIFICATION.md (blinky xcku3p b65979c1..., blinky xcvu3p
791ce945..., counter xcku3p 1bf0ffe6..., counter xcvu3p 74417af5...).
Recorded in `harness/VERIFICATION.md` (session 3 section). No repo
files touched (`git status` in `~/workspace/helion-ultra-bitgen-diff`
clean). No em dashes in any changed file.

Known limitation, flagged for the CTO (not hidden): parse-back
renders `.V0`/`.V1` for every FF in the device model's tile list,
not just the design's FFs. This session completes the design's FFs
only, per the CTO task. Until the comparison (or the emitter's parse
scope) is scoped to the design's tiles, a real-emitter run will fail
the FRAMING check on the device-wide lines. The per-block bits diff
still catches stray set bits anywhere. Likewise SRVAL is normalized
when specified but has no default completion here (INIT only, per
the task); parse-back renders SRVAL `.V0`/`.V1` per FF too, so the
same completion will be needed for SRVAL when the real emitter
lands. Critic session 9 FLAG F1 (end-to-end round trip after both
branches merge) still stands: this session implements the harness
side; the run itself awaits the bitgen-core merge plus the
integration worker's emitter.

## 2026-11-07 ~18:00 EST: bitgen-core session 3 DONE, SegbitsDb/BitRef reconciliation (interfaces item 6)

Branch fm/ultra-bitgen-core, tip 5e26453 (one commit on ace0994),
worktree clean, nothing pushed, no merge to master. Ready for critic
re-review.

Rebase first: all 5 session-1/2 commits replayed onto fm/ultra-bitstream
tip 9289fed (rebase commit hashes ec5d198, 4713fef, 3447b9b, a22b471,
ace0994). Two conflicts on the first commit, both resolved as the union
of both sides: Cargo.lock takes the base entry (helion-ultrascale with
serde/serde_json deps, which supersedes my no-deps entry); lib.rs takes
both module lists and both re-export blocks, alphabetical module order.

Reconciliation, one explicit commit (5e26453, 4 files, +143/-95):
- Single definition of BitRef and SegbitsDb in src/segbits.rs only.
  The duplicate struct + trait are deleted from src/bitgen.rs; bitgen.rs
  consumes the shared types via `use crate::segbits::{BitRef, SegbitsDb};`.
- `features(tile_type)` added to the trait and to FileSegbitsDb: per-tile-
  type sorted feature lists built at load, empty slice for an unknown
  tile type (StubSegbitsDb already did the same). Interface merge
  requirement (item 3) met.
- Constructor spelling picked: `set`/`zero`, the loader's spelling.
  `zero` names the `!` token's meaning (the bit must read 0) and matches
  the `must_be_zero` field; `clear` is dropped and all 42 call sites
  renamed (1 in bitgen.rs tests, 41 in bitgen_roundtrip.rs).
- lib.rs drops BitRef and SegbitsDb from the bitgen re-export in favor
  of the segbits re-export. Second collision found during the rebase
  verification and fixed in the same commit: FRAME_BITS was re-exported
  from both bitgen and bitloc (same value 2976, types usize vs u32);
  the bitgen re-export is dropped, the crate root keeps bitloc's u32.
  Nothing outside bitgen.rs/bitloc.rs used the root name.
- Critic session 8 FLAG F2 fixed: tests/bitgen_roundtrip.rs:38 now
  states the resolved polarity mapping (bit = NOT(INIT), bit =
  NOT(SRVAL), V0 selected by value 0 and sets the bit, V1 by value 1
  and clears it) instead of "still BLOCKED on a polarity experiment".
- Tests added in segbits.rs: `features_enumerates_exactly_what_lookup_
  resolves` (fixture sorted list, every listed feature resolves, empty
  for unknown types; vendored segbits_int.db 3778/3778, list length
  equals feature_count, so no missing and no extras) and
  `lookup_preserves_db_file_order` (bits deliberately out of numeric
  order return in exact file order; BYPASS line exact order including
  `!` tokens; the bitgen MSB-first INIT contract rests on this).

Verification (full cargo path, nohup setsid, -j2 max, no target
cleaning, final state after the fmt pass): `cargo test -p
helion-ultrascale -j2` 92 passed, 0 failed (48 unit + 9 round-trip +
3 demo + 3 xcvu3p + 2 int_pip + 1 audit + 20 lut_ff_bram + 4 segbits +
2 smoke); `cargo clippy -j2 --all-targets` exit 0, 0 warnings (the
pre-existing vec! warning was fixed on the lut branch by dfd08d0);
`cargo fmt -p helion-ultrascale -- --check` clean (one import-order
reflow applied). Zero em dashes and zero en dashes in all four
changed files (byte scan). One intermediate test run failed to compile
(E0252, the FRAME_BITS double re-export above); fixed in the same
commit before the passing runs, not hidden.

Open for the critic: re-review the reconciliation (single definition,
constructor rename, features() + file-order tests) and confirm the
branch is merge-ready; the orchestrator sequences the merge.

## 2026-11-07 ~18:05 EST: diff-harness session 4, tile-scoped round-trip comparison and SRVAL completion (CTO rulings on session 3 flags)

Done, verified, recorded. Both flags from the session 3 report are closed as ruled; one residual gap is counted and flagged for the next CTO ruling, not hidden.

1. Device-wide parse-back scope: `run_roundtrip` now computes the design's tile set from the golden FASM itself (SLICE_X0Y0, INT_X0Y0, HPIO_RIGHT_X0Y0 for the v1 goldens) and passes it to `canonical_roundtrip_fasm` for the parse-back side, which drops lines on tiles outside the set before completing. The golden side is canonicalized without the filter (its lines define the set). The per-block bits diff still catches stray set bits anywhere, so scoping the FRAMING comparison loses no coverage. New fake-emitter mode `FAKE_PARSE_DEVICE_WIDE=1` appends the lines a real parse-back renders for an untouched tile (every FF of SLICE_X9Y9, INIT.V1 and SRVAL.V1, bit 0 renders V1 through the exact-complement rule).
2. SRVAL completion: `canonical_roundtrip_fasm` now completes every FF in the design with exactly one SRVAL line as well as one INIT line; a missing SRVAL gets the default `.V0` (Xilinx default SRVAL is 0). Valued SRVAL spellings normalize to `.V0`/`.V1` like INIT. New fake-emitter mode `FAKE_PARSE_SRVAL_V1=1` appends `SLICE_X0Y0.AFF.SRVAL.V1` to the parse output.

Tests: suite now 30, all OK (was 26; +4). `test_completes_missing_ff_srval_with_v0` (counter golden gains one `.SRVAL.V0` per design FF, valued SRVAL normalizes, completion idempotent); `test_device_wide_parseback_lines_outside_design_tiles_pass` (blinky with device-wide lines: FRAMING green, FF/LUT/INT green); `test_wrong_value_on_design_tile_still_fails_with_device_wide_lines` (device-wide lines plus a flipped AFF INIT: FRAMING red, `diff_bits` 1, LUT/FF/INT green); `test_wrong_srval_value_still_fails_roundtrip` (parse renders AFF.SRVAL.V1 against a golden completed to .V0: FRAMING red, `diff_bits` 1, LUT/FF/INT green through the bits path). Full run re-checked: `python3 diff.py --out /tmp/session4-default-run` (exit 0), 40 records, all blocked, all `diff_bits` null; placement hashes identical to the baseline (blinky xcku3p b65979c1..., blinky xcvu3p 791ce945..., counter xcku3p 1bf0ffe6..., counter xcvu3p 74417af5...). The placement-hash canonical form in `fasm.py` is untouched, so the golden hashes are unchanged. No repo files touched (`git status` in `~/workspace/helion-ultra-bitgen-diff` clean).

Residual gap, counted from the real DB, for the next CTO ruling: tile scoping does not cover the lines parse-back renders for unused BELs and attribute pairs on the design's OWN tile. Counted from `segbits_clel_l.db` (768 features): 57 all-`!` features render on an untouched CLEL_L tile through the exact-complement rule (16 INIT.V1 + 16 SRVAL.V1, one per FF BEL, plus CEUSED 4, CLKINV 2, SRINV 2, SRUSED 4, MODE 2, SYNC 2, CARRY_TYPE 1, DI0-DI7 8). The completed golden carries only the design's FFs, so a real-emitter FRAMING run still needs either the comparison scoped to the design's features, or the golden completed tile-wide with attribute defaults. This session does not claim the real-emitter FRAMING run is green yet.

## 2026-11-07 ~18:10 EST: diff-harness session 5, feature-scoped FRAMING comparison (CTO ruling on session 4 flag)

Done, verified, recorded. The residual gap session 4 counted (57 complement lines on the design's own tile) is closed as ruled: scope the FRAMING comparison to the design's features.

Change in `harness/roundtrip.py` only (plus tests and the fake emitter): after canonicalization, both sides are filtered to the design's feature set, the golden's features plus the completed FF INIT/SRVAL defaults. New helpers: `_feature_key(feat)` keys a canonical feature by (tile, attribute), the `.V0`/`.V1` suffix stripped because the suffix IS the value; `roundtrip_feature_keys(text)` returns the key set of a canonical text; `scope_fasm_to_feature_keys(text, keys)` drops lines whose key is not in the set, sorts, serializes. `run_roundtrip` computes the golden's keys from the completed golden canonical form and scopes both sides through it; scoping the golden to its own keys is a no-op by construction, stated in the code and proven by test. The parse-back side is still tile-scoped first (session 4), then feature-scoped. The per-block bits diff still catches stray set bits anywhere, so the scoping loses no coverage.

The 57-line count was re-verified this session, not taken on trust: a script implementing the exact-complement rule over the real `segbits_clel_l.db` returns exactly 57 lines, matching the session-4 breakdown line for line (16 INIT.V1 + 16 SRVAL.V1 including FF2 BELs, plus CEUSED 4, CLKINV 2, SRINV 2, SRUSED 4, MODE 2, SYNC 2, CARRY_TYPE 1, DI0-DI7 8 as .AX/.BX/.CX/.DX/.EX/.FX/.GX/.HX).

Tests: suite now 34, all OK (was 30; +4). New fake-emitter mode `FAKE_PARSE_OWN_TILE_NOISE=1` appends own-tile noise (unused FF BELs EFF..HFF and AFF2..HFF2 INIT.V1/SRVAL.V1, ABCDFF/EFGHFF CEUSED/CLKINV/SRINV/SRUSED/MODE/SYNC, CARRY_TYPE.SINGLE_CY8, DI0.AX..DI7.HX, spellings verbatim from `segbits_clel_l.db`) plus session-4 device-wide lines. New tests: `test_feature_scoped_comparison_drops_own_tile_noise` (canonical level: noise scopes away to exactly the completed golden; golden self-scope is a no-op; a wrong AFF INIT value survives the scope via the shared (tile, AFF.INIT) key); `test_own_tile_noise_lines_outside_design_features_pass` (blinky and counter with noise: FRAMING green, FF/LUT/INT green); `test_wrong_value_on_design_feature_still_fails_with_own_tile_noise` (noise plus flipped AFF INIT: FRAMING red, diff_bits 1, LUT/FF/INT green); `test_stray_set_bit_outside_design_features_caught_by_bits_diff` (noise plus a DSP48E2 bit: FRAMING green, DSP48E2 red diff_bits 1, LUT/FF/INT green).

Full run re-checked: `python3 diff.py --out /tmp/session5-default-run` (exit 0), 40 records, all blocked, all diff_bits null; placement hashes verified full-hash against VERIFICATION.md, all four MATCH. No repo files touched (`git status` in ~/workspace/helion-ultra-bitgen-diff clean). Zero em dashes and zero en dashes in all changed files (byte scan).

What this session does NOT claim: the real-emitter FRAMING run is green yet. The feature scope closes the last known gap in the comparison itself; the run still awaits the bitgen-core merge plus the integration worker's emitter executable (critic session 9 FLAG F1).

## 2026-11-07 ~18:10 EST: orchestrator session 9, bitgen-core MERGED (004d7cf)

Orchestrator session 9 agent 24fa1469-ef59-496d-87cf-7156ff96864a.
Single task: merge fm/ultra-bitgen-core (tip 5e26453) into fm/ultra-bitstream.
No other branch merged this session.

Pre-merge gates, all verified by the orchestrator:
- Base at 7f80b54 (lut-ff merge), worktree clean except the pre-existing
  untracked bitfiltrator-ref/.
- Branch tip confirmed 5e26453.
- Correctness critic session 11 PASS, no defects (CRITIQUE.md:1773):
  rebase onto 9289fed faithful (replayed content byte-identical to the
  critic-passed originals), interfaces item 6 reconciliation complete
  (single SegbitsDb/BitRef in segbits.rs, features() plus file-order
  tests, set/zero constructor spelling, FRAME_BITS collision fixed,
  F2 stale comment fixed). Critic pre-check `git merge-tree --write-tree
  7f80b54 5e26453` exits 0 with a conflict-free tree; expected post-merge
  test count 94, confirmed by test-name sets.

Merge: `git merge --no-ff fm/ultra-bitgen-core`, merge commit 004d7cf,
"merge fm/ultra-bitgen-core: bitgen FRAMING block with SegbitsDb/BitRef
reconciliation (5e26453)". 4 files changed, 2322 insertions, 0 deletions:
src/bitgen.rs new (1743 lines), tests/bitgen_roundtrip.rs new (492 lines),
src/lib.rs +5, src/segbits.rs +82. No conflicts; the ort strategy merged
clean.

Post-merge verification (full cargo path, nohup setsid, -j2, logs
/tmp/orch-merge-bitgen-test.log and /tmp/orch-merge-bitgen-clippy.log):
`cargo test -j2 -p helion-ultrascale` = 94 passed, 0 failed (48 unit +
9 round-trip + 3 demo + 3 xcvu3p + 2 int_pip + 1 audit + 22 lut_ff_bram
+ 4 segbits + 2 smoke + 0 doc), exactly the critic's predicted count.
`cargo clippy -j2 -p helion-ultrascale --all-targets`: exit 0,
0 warnings. `cargo fmt -p helion-ultrascale -- --check`: clean.
Worktree clean except the pre-existing untracked bitfiltrator-ref/.

Block status: no block goes green from this merge alone (bitgen-core is
the emitter lane). INT_PIP stays green tier 1. LUT_INIT, FF_INIT,
BRAM_CONTENT, BRAM_PARITY stay un-green until the diff harness runs
against the bitgen-core emitter. What this merge unblocks: the emitter
now exists on fm/ultra-bitstream (emit_bitstream / parse_bitstream with
the reconciled shared SegbitsDb/BitRef, D1 all-! fix, D2 START/DESYNC
ending, F-A CRC list removal, FF INIT .V0/.V1 polarity complement rule),
so the first real-emitter harness run is now possible once the
integration worker's emitter executable lands (critic session 9 FLAG F1;
harness sessions 3-5 closed the round-trip comparison gaps: complete
FASM canonicalization, tile scoping, SRVAL completion, feature scoping).

Queue: int-pip, lut, segbits, device-file, lut-ff, bitgen-core all merged.
Remaining: integration (fm/ultra-bitgen-integ tip 3e6d17f, critic session
10 PASS on correctness and scope with three must-fix hygiene items before
merge: 7 clippy warnings, 36 fmt hunks, 2 stale doc lines; orchestrated
merge sequence per session 10, NOT a blind merge: reconciliation commit,
take 88070a5 bitgen.rs, take this branch's non-bitgen files, apply the
ff_fasm_name dedup plus fmt plus clippy fixes plus doc lines in the merge
commit, re-run the 77 tests and the four gate-2 commands).

## 2026-11-07 ~18:35 EST: orchestrator session 10, integration MERGED (2c8c9e3)

Orchestrator session 10 agent 6491b103-6f9c-421f-b8af-f32fce6078d4.
Single task: merge fm/ultra-bitgen-integ (tip 3e6d17f) into fm/ultra-bitstream.
NOT a blind merge: the critic session 10 sequence (CRITIQUE.md:1928) was
followed exactly, and every step is recorded here with its evidence.

Rebase (in ~/workspace/helion-ultra-bitgen-integ, onto 004d7cf):
- 95cf78d replayed as 21782ae; conflicts in Cargo.lock (took base, cargo
  regenerates), helion-ultrascale/Cargo.toml (union: serde + serde_json
  deps, dev-deps helion-ir + helion-sv + serde_json with the audit-test
  comment), device.rs (kept BOTH tests: base G3 BRAM-capacity test and
  the integration xcku3p-counts test, same insertion point), lib.rs
  (union of the module lists and the re-export blocks, reconciled
  BitRef/SegbitsDb from segbits only, no FRAME_BITS from bitgen).
- 469e805 and ab848e4 replayed cleanly (bb385e3, 29a2485).
- 281e9f8 (bitgen port): add/add on bitgen.rs and bitgen_roundtrip.rs,
  took the base (merged 88070a5-derived, reconciled) versions; its
  lib.rs change was only the old bitgen re-export, already present in
  reconciled form, so base was kept.
- 7365f2e (integration's weaker D1 fix) SKIPPED at rebase: it touches
  only bitgen.rs (66 insertions, 1 deletion) and is superseded by the
  base's stronger D1 (n_set > 0 refinement) plus the complement rule,
  D2 START/DESYNC, and the F-A removal. Nothing lost: the base's
  round-trip tests cover D1 against the real DB (critic session 8).
- f92d09b (int-pip port + rename): add/add on int_pip.rs, segbits.rs,
  tests/int_pip.rs, took the base versions; the rename was re-applied
  onto the base afterwards (see 374f911 below), so the commit itself
  dropped as empty.
- dd4b2c7 and 3e6d17f replayed cleanly (7735dc3, abb7389).

Merge-prep commits on the rebased branch:
- 374f911: IntPip sink-first rename re-applied onto the reconciled u16
  base (feature() builds PIP.{dst}.{src}, MissingPip Display sink-first,
  unit-test constructions swapped with the physical-direction comment,
  integration-test BFS swap with its comment; the u8 widenings from
  f92d09b dropped, u16 kept everywhere); ff_fasm_name dedup (ruling
  (c)): fasm_gen drops its 16-arm copy and calls bitloc::ff_fasm_name
  with a map_err to String (3 lines plus doc); F4 doc fixes (fasm_gen
  header no longer says the lut worker's emission must change separately,
  it landed as 7f80b54 session 9 PASS; grid.rs header no longer says
  tile names wait on the device-file worker, it landed as 9289fed
  session 7 PASS, adapter still unwired, which is correct until wired).
- f1f6134: Cargo.toml quoting fix (the rebase resolution had landed
  literal backslash escapes in the serde/serde_json version strings,
  which broke TOML parsing; found by the cargo fmt run, fixed to plain
  quotes); clippy F1 (6x for_kv_map become .values() loops; astar keeps
  its day-1 8-argument scaffold signature with a targeted
  #[allow(clippy::too_many_arguments)] and a comment, per the critic's
  permitted fix); fmt F2 (cargo fmt -p helion-ultrascale, reflows and
  import ordering only).
- f5ebcf7: duplicate `pub mod bel;` dropped (E0428 from the lib.rs
  union); clippy manual_flatten follow-up (the four Option-valued loops
  become .values().flatten(), semantics unchanged).

Verification on the rebased branch tip f5ebcf7 (full cargo path, nohup
setsid, -j2, logs /tmp/orch-integ-test3.log, /tmp/orch-integ-clippy3.log,
/tmp/orch-integ-cli-build.log, /tmp/orch-integ-gate2.log):
- `cargo test -j2 -p helion-ultrascale`: 128 passed, 0 failed (77 unit +
  9 round-trip + 2 int_pip + 3 demo + 3 xcvu3p + 2 corpus_legality +
  1 audit + 22 lut_ff_bram + 4 segbits + 2 smoke + 3 sta_sanity + 0 doc).
- `cargo clippy -j2 -p helion-ultrascale`: exit 0, 0 warnings (the 4
  manual_flatten warnings from the .values() fix were fixed in f5ebcf7;
  the plain-clippy log contains zero "warning" lines).
- `cargo clippy -j2 -p helion-ultrascale --all-targets`: exit 101,
  REPRODUCES the pre-existing helion-sv error at lib.rs:15059
  (`step_down` boolean expression), exactly as critic session 10
  recorded. Zero files in this branch's diff are under helion-sv; not
  fixed here, carried as a known pre-existing failure.
- `cargo fmt -p helion-ultrascale -- --check`: clean.
- Gate 2 reproduced exactly (helion-cli built from this branch,
  target/debug/helion; the bin's 2 warnings are unused imports
  `place_with` / `route_with` in main.rs:15,19, lines the branch did not
  touch and byte-identical on base 004d7cf, so pre-existing): all four
  `helion impl` commands (blinky/counter x xcku3p/xcvu3p, seed 0) pass
  SYNTH (blinky luts=1 ffs=1, counter luts=4 ffs=4), PACK (slices=1),
  PLACE (seed=0), LEGALITY (violations=0), FF FASM (blinky ff_lines=2,
  counter ff_lines=8), preliminary STA (WNS_PS=9650 TNS_PS=0 r2r_ps=270),
  then fail loud at ROUTE with the exact WireDb-empty error, exit 1,
  no .bit written (verified: no fresh .bit file in the worktree).

Merge: `git merge --no-ff fm/ultra-bitgen-integ`, merge commit 2c8c9e3,
23 files changed, 3870 insertions, 50 deletions. No conflicts (the
branch was rebased onto 004d7cf first).

Post-merge verification on base tip 2c8c9e3 (full cargo path, nohup
setsid, -j2, logs /tmp/orch-merge-integ-test.log,
/tmp/orch-merge-integ-clippy.log, /tmp/orch-merge-integ-fmt.log):
`cargo test -j2 -p helion-ultrascale` = 128 passed, 0 failed (same
breakdown as the branch tip); `cargo clippy -j2 -p helion-ultrascale`:
exit 0, 0 warnings; `cargo fmt --check`: clean. Worktree clean except
the pre-existing untracked bitfiltrator-ref/. Spot checks on the merged
tip: exactly one `pub struct BitRef` (segbits.rs), zero
CRC_EXCLUDED_REGS in bitgen.rs (F-A removal kept), 14 complement hits
in bitgen.rs, START/DESYNC emitted at bitgen.rs:925-926, feature()
sink-first at int_pip.rs:63, fasm_gen ff_fasm_name is the thin wrapper
calling bitloc::ff_fasm_name (fasm_gen.rs:48, bitloc.rs:538).

Carried, recorded not fixed (critic session 10 F3, scope): sta_on_placed
always uses preliminary_xcvu3p() for both parts, so the xcku3p
WNS_PS=9650 carries no xcku3p-specific delay data. The flow labels STA
preliminary, so this is honest, but the xcku3p STA number must not be
treated as part-specific evidence. Fix when an xcku3p timing DB exists.

Block status: no block goes green from this merge alone (integration is
the flow lane, not an emission lane). INT_PIP stays green tier 1.
LUT_INIT, FF_INIT, BRAM_CONTENT, BRAM_PARITY stay un-green until the
diff harness runs against the emitter. What this merge unblocks: the
`helion impl` CLI flow now exists on fm/ultra-bitstream (synth, pack,
place, legality, complete-FF FASM, preliminary STA, fail-loud ROUTE),
so the integration worker's emitter executable (critic session 9 FLAG
F1) is the last piece before the first real-emitter harness run.

Queue: int-pip, lut, segbits, device-file, lut-ff, bitgen-core,
integration all merged. All seven worker lanes are now on
fm/ultra-bitstream. Remaining before the first real-emitter harness
run: the emitter executable wiring (HELION_BITGEN_EMITTER or
--emitter-cmd for the harness, or the BITGEN stage in ultra.rs, which
currently fails loud at ROUTE before bitgen); the real BitgenDevice
impl for DeviceFile (interfaces session 2 item 4 FLAG, gates FRAMING
green on a real device); then the greedy block loop against the real
emitter, largest diff first.

## Emitter worker session 1: helion-bitgen executable delivered (2026-11-07 ~19:03 EST)

Worktree ~/workspace/helion-ultra-bitgen-emitter, branch fm/ultra-bitgen-emitter,
base fm/ultra-bitstream 2c8c9e3, uncommitted at delivery (commit to follow on
the branch; nothing pushed, no merge). Closes critic session 9 FLAG F1: the
harness now has an executable to invoke, not just the library.

What was built (thin CLI only, one place: crates/helion-ultrascale/src/bin/helion-bitgen.rs):
- `helion-bitgen emit --fasm FILE --part PART --out FILE.bit [--data-dir DIR]`
  (FASM from --fasm, or from stdin when --fasm is absent; stdin emit is
  byte-identical to --fasm emit on both goldens), `parse --bit FILE --part PART`
  (FASM on stdout), `bits --bit FILE --part PART` (one line per set bit:
  `FAR_HEX OFFSET VALUE BLOCK`, FAR 8 hex digits, OFFSET bit offset in the
  93-word frame, VALUE always 1, BLOCK from the harness registry).
- Loads the vendored segbits DBs via FileSegbitsDb (all 9 files, LUT INIT
  synthesized at load) and a device through one `load_device(part)` seam on
  the BitgenDevice trait. Today only xcku3p, as a StubDevice with the three
  golden tiles (SLICE_X0Y0 CLEL_R base FAR 0x500, INT_X0Y0 INT base 0x400,
  HPIO_RIGHT_X0Y0 HPIO_RIGHT base 0x000, IDCODE 0x04A63093, provenance in the
  binary comments). xcvu3p and any other part fail loud with the seam named.
  The real DeviceFile BitgenDevice impl drops into load_device when it lands;
  the stub table is then deleted, not extended.
- Data paths: --data-dir overrides, default is the crate data/ dir via
  CARGO_MANIFEST_DIR at compile time, so the harness invoking from its own
  working directory works (proven: diff.py ran it from the harness dir).
- FASM vocabulary gap closed in the binary, not the library: valued FF
  INIT/SRVAL lines (`AFF.INIT = 0`) are rewritten to the canonical DB
  spelling (`AFF.INIT.V0`, bit = NOT(value)); bare `AFF.INIT` and valued
  `.V0`/`.V1` lines fail loud. The LUT [63:0] form is closed in the library
  by synthesize_lut_init (5 new unit tests in segbits.rs).
- `bits` derives the bitset from the parse-back FASM and cross-checks it
  against the raw frames both ways (no set bit unattributed, no attributed
  bit reading 0), so a parse/emit shared bug cannot silently pass. Block
  labels mirror the harness registry (fasm.py classify) plus the per-bit
  `ALUT.INIT[i]` form and the general FF-BEL rule (ABCDFF/EFGHFF control
  attributes the registry does not list by name). Classification is lazy:
  complement lines (all-`!` refs, e.g. AFF.INIT.V1, CARRY8.DI0.AX) set no
  bits and need no label.
- New fixture: crates/helion-ultrascale/tests/fixtures/blinky.xcku3p.fasm,
  byte-identical copy of the harness golden plus a 5-line provenance header,
  so the binary tests do not depend on the harness directory.
- Vendored in this branch (untracked, to be committed with the binary): the
  four CLB segbits DBs (segbits_clel_l/r.db, segbits_clem/r.db, 768/768/829/829
  features) plus their .origin_info.db files; PROVENANCE.md updated
  (7082 -> 10276 total vendored features).

Verification (full cargo path, nohup setsid, -j2):
- `cargo test -j2 -p helion-ultrascale`: 140 passed, 0 failed across 13
  suites (lib 82, bin 7, plus 11 integration/doc suites; base was 128, this
  branch adds 7 bin tests + 5 segbits tests). Log /tmp/emitter-full-test3.log.
- `cargo clippy -j2 -p helion-ultrascale --lib --bins`: exit 0, 0 warnings.
  Full `cargo clippy -j2 -p helion-ultrascale --all-targets` exits 101 on ONE
  pre-existing deny in untouched helion-sv/src/lib.rs:15059
  (clippy::overly_complex_bool_expr, `step_down` expression) plus 426
  helion-sv warnings; helion-ultrascale itself is clean in that run (the
  crate is Checked, zero warnings). The helion-sv error is not mine to fix
  on this branch; the test targets need helion-sv as a dev-dep, so their
  clippy check is blocked by it. The 3 bin warnings the first clippy run
  found (unused test-only imports, unused `rhs`) are fixed.
- `cargo fmt -p helion-ultrascale -- --check`: clean (whole-workspace
  `cargo fmt --check` still shows pre-existing diffs in untouched crates
  such as helion-bd; this toolchain's rustfmt disagrees with the base
  formatting there, out of scope).

First real-emitter harness run (2026-11-07 ~19:00 EST,
HELION_BITGEN_EMITTER=<worktree>/target/debug/helion-bitgen,
`python3 diff.py --design all --part xcku3p --out runs/emitter-session1`,
20 records, tier1):
- blinky: LUT_INIT 0/64 green, INT_PIP 0/11 green, FF_INIT 0/1 green.
- counter: LUT_INIT 0/256 green, INT_PIP 0/17 green, FF_INIT 0/4 green.
- All per-block diffs came via `bits` (emitted_via=bits, not parse-mediated).
- FRAMING red on both, 1 failed of 3 checks (sync word and IDCODE pass).
  The single failing check is roundtrip-parse(emit(f))==f, and the exact
  mismatch is one class only: the golden has no SRVAL line for its FFs, the
  harness canonical completion assumes the Xilinx default SRVAL=0 (.V0),
  the emitter faithfully leaves the bit clear, which parses back as .V1
  (bit = NOT(SRVAL)). Blinky differs on AFF.SRVAL.V0 vs .V1; counter on
  AFF/BFF/CFF/DFF SRVAL, same class. This is the documented partial-golden
  case in roundtrip.py ("Only a complete golden can go green against the
  real emitter"); the fix is golden-side (add `SLICE_X0Y0.AFF.SRVAL = 0`
  etc. to the goldens), not emitter-side (the emitter must not invent
  defaults). Verified by direct canonical comparison, not by inspection.
- IO_CONFIG blocked on both designs (reference bits tile-relative; absolute
  FARs need the xcku3p tilegrid, prjuray-db ships xczu3eg tilegrids only):
  pre-existing harness limitation, not the emitter.
- BRAM_CONTENT, BRAM_PARITY, DSP48E2, URAM288, CLK blocked (not exercised
  by the v1 goldens), as before.

Carried, recorded not fixed (scope, needs a ruling or another lane):
- `bits` fails loud on a CARRY8 feature that sets bits: CARRY8 has no block
  in the harness registry, and the binary will not guess one. The v1
  goldens only render CARRY8 complement lines (no set bits), so this does
  not affect the current run; a CARRY-using design needs a CTO ruling on
  which block CARRY8 belongs to before it runs through `bits`.
- The binary's in-process round-trip test compares SET BITS, not raw frame
  maps: emit creates an (empty) frame entry for every frame any feature
  references, including value-0 .V1 features, so re-emitting the parse-back
  legitimately carries empty CLEL frames the first emit never touched
  (FF2 complement lines). Empty frames carry no configuration.
- StubDevice only; FRAMING green on a real device still gates on the real
  DeviceFile BitgenDevice impl (interfaces session 2 item 4 FLAG).

Queue: emitter executable is the last piece before the greedy block loop.
Next: critic review of this branch, orchestrator merge, then (a) complete
the goldens with SRVAL = 0 lines (golden-side, diff-harness lane) so
FRAMING can go green, (b) real BitgenDevice for DeviceFile, (c) greedy
loop, largest diff first (IO_CONFIG tilegrid is the standing largest
blocked item after FRAMING).

## 2026-11-07 ~19:05 EST: bitgen-device worker DONE (branch fm/ultra-bitgen-bitgendev)

Worktree ~/workspace/helion-ultra-bitgen-bitgendev, branch
fm/ultra-bitgen-bitgendev, base fm/ultra-bitstream tip 2c8c9e3.
Closes interfaces session 2 item 4 FLAG: a real BitgenDevice impl
for DeviceFile now exists (StubDevice is no longer the only impl).
Work NOT committed, NOT merged: left in the worktree for critic
re-review and orchestrator merge, per the brief.

### What was delivered

- `crates/helion-ultrascale/src/bitgen_device.rs` (new):
  `impl BitgenDevice for DeviceFile` plus the resolution helpers.
  `src/lib.rs` registers the module; `src/device_file.rs` gains
  `tile_names()` and `config_order()` as cached OnceLock vectors
  plus a `row_majors()` accessor (23 added lines).
- `crates/helion-ultrascale/tests/bitgen_device.rs` (new): 13
  integration tests.

### Resolution rules (all from the device file's own tables)

- SLICE: base FAR is `Far(ClbIoClk, row_major, clb_col_major, 0)`
  from the file's `row_majors`, `clb_col_major`, and `clb_row`
  tables. Cross-checked against the file's own BitLocator: base +
  DB frame == locator FAR (BFF.INIT.V0 frame 12, A6LUT bit 0
  frame 11, prjuray-db commit affbc5e). SLICE_X23Y45 base
  0x3000, SLICE_X0Y0 base 0x500, both minor 0.
- INT: provisional harness rule, CLB column major - 1, validated
  only if that column carries at least 76 minors (the INT frame
  span 0..75). INT_X0Y0 resolves to major 4, base FAR 0x400.
  INT_X1Y0 does NOT resolve: the rule points at major 5, which
  carries only 16 minors. 52 valid INT columns per row on both
  parts. This rule is provisional, not Vivado-verified.
- RAMB18: base FAR is `Far(BramContent, row_major,
  bram_content_col_major, 0)`; RAMB18_X0Y0 is 0x01000000.
  RAMB36, HPIO, HDIO, CMT, DSP have no column mapping in the file
  and fail loud (None), never invented.
- `tile_names()`: every resolving SLICE plus INT plus RAMB18,
  sorted, deduped. xcku3p: 27,120 + 208 + 960 = 28,288. xcvu3p:
  49,260 + 364 + 1,680 = 51,304.
- `config_order_fars()`: every real frame from the file's column
  tables, ascending FAR integer, count == `total_frames()`.
  xcku3p: 41,460, last FAR 0x010C09FF. xcvu3p: 73,035, last FAR
  0x011809FF.
- The invented device.rs grid (critic G2) is NOT consumed for FAR
  mapping anywhere in this impl.
- `part()` is the full part string (`xcku3p-ffva676-1-e`,
  `xcvu3p-ffvc1517-1-e`); the short device name is `device()`.
  The stub's short part was a stub simplification.

### Evidence (exact commands, full cargo path, nohup setsid, -j2)

- `cargo test -j2 -p helion-ultrascale`: 141 passed, 0 failed
  (128 baseline + 13 new). The 13 new tests include the
  FRAMING proof at the library level: `emit_bitstream` with the
  real DeviceFile plus the real vendored `segbits_int.db` for the
  two golden INT PIP lines parses back to exactly the input
  FASM, and re-emitting the parse-back is byte-identical. The
  full 41,460-frame config order is emitted (not touched-frames
  only). Set bits pinned: BOUNCE at (FAR 0x440, bit 0), GCLK at
  (FAR 0x429, bit 3).
- `cargo fmt --check -p helion-ultrascale`: clean (after one
  `cargo fmt` pass on the two new files; only the worker's files
  changed).
- `cargo clippy -j2 -p helion-ultrascale --all-targets`: BLOCKED
  on a pre-existing base-branch error, NOT this worker's code:
  `crates/helion-sv/src/lib.rs:15059`,
  `clippy::overly_complex_bool_expr` denied by default, in a
  file this worker never touched (worktree HEAD is 2c8c9e3,
  `git diff --name-only` shows only device_file.rs and lib.rs).
  Scoped re-run with `-- -A clippy::overly_complex_bool_expr`:
  exit 0, no errors, zero warnings in any file this worker
  touched; the only helion-ultrascale warning is pre-existing in
  tests/corpus_legality.rs:52 (type_complexity). helion-ir 47
  and helion-sv 426 warnings are pre-existing in dependencies.
  The plain clippy command needs the helion-sv error fixed
  elsewhere before it goes green; this worker did not touch it
  (out of scope, and it sits in the lane that owns the 25
  pre-existing helion-sv failures disposition).

### Observations for the orchestrator (not defects in this impl)

- `emit_frames` materializes the zero-checked FARs as all-zero
  frames in its map (10 entries for the 2-PIP FASM: 2 set, 8
  zero). The bitstream writer writes them as zero frames,
  identical to untouched frames, so the bitstream is unaffected;
  the map is just larger than the touched set. Pinned in the
  test with the reason stated. Performance note for a
  full-design emit, not a correctness issue.
- FRAMING green at the library level is now demonstrated on the
  real device file. The remaining gate for FRAMING green in the
  harness is the emitter executable wiring (HELION_BITGEN_EMITTER
  or --emitter-cmd, or the BITGEN stage in ultra.rs, which still
  fails loud at ROUTE before bitgen).

### Carried, not fixed (per the brief)

- Critic session 7 FLAG F1: the last-of-row FAR 2-zero-padding
  path is still untested (0 of 261 FARs exercised). This worker
  did not claim the generator is trusted for full-device
  bitstreams.
- xcvu3p frame-total gap: 73,035 computed vs UG570 Table 1
  71,820, flagged provisional in the file and in the test, never
  tuned away.

## 2026-11-07 ~19:25 EST: orchestrator session 11, bitgen-device MERGED (1ad06df)

Roster updated: bitgen-device worker done (committed 4fd7120, merged
this session); orchestrator session 11 done. Emitter worker (fm/ultra-bitgen-emitter) still running in parallel; its merge waits on its own critic re-review.

Gates: correctness critic session 12 PASS on correctness and scope
(CRITIQUE.md:2230) on the uncommitted tree, with two must-fix
test-only items M1/M2 and no re-review needed beyond the re-run,
provided the diff stayed confined to the test file. It did: the
worker's 4 files are unchanged except tests/bitgen_device.rs
(M1/M2 below).

M1/M2 fixes applied by the orchestrator in
~/workspace/helion-ultra-bitgen-bitgendev, test file only:
- M1: `xcku3p_part_and_idcode` and
  `xcvu3p_part_and_idcode_and_provisional` now assert the trait
  method by qualification
  (`<DeviceFile as BitgenDevice>::part(&device) == "xcku3p"` /
  `"xcvu3p"`), keep the inherent `device.part()` assertion as a
  separate line ("xcku3p-ffva676-1-e" / "xcvu3p-ffvc1517-1-e"),
  and the comment now states the trait returns the short device
  name per the trait doc, matching the stub convention. The old
  comment (trait part is the full part string) is gone.
- M2: the SLICE_X23Y45 comment now reads "CLB column 23 is major
  48 per the file" (48 << 8 = 0x3000, the asserted base, unchanged).

Verification before commit (worktree fm/ultra-bitgen-bitgendev,
full cargo path, nohup setsid, -j2):
- `cargo test -j2 -p helion-ultrascale --test bitgen_device`:
  13 passed, 0 failed (log /tmp/orch11-bitgendev-tests.log).
- `cargo test -j2 -p helion-ultrascale`: 141 passed, 0 failed
  (128 baseline + 13 new, exactly the critic's expected count;
  log /tmp/orch11-full-tests.log).
- `cargo fmt -p helion-ultrascale -- --check`: exit 0, clean.

Commit: 4fd7120 "bitgen-device: BitgenDevice impl for DeviceFile
(real device, closes interfaces item 4 FLAG)", 4 files, 678
insertions, 1 deletion (new src/bitgen_device.rs and
tests/bitgen_device.rs; modified src/device_file.rs and src/lib.rs).

Merge: `git merge --no-ff fm/ultra-bitgen-bitgendev` into
fm/ultra-bitstream, ort strategy, no conflicts. Merge commit
1ad06df. Base worktree clean except pre-existing untracked
bitfiltrator-ref/.

Post-merge verification (base worktree, tip 1ad06df):
- `cargo test -j2 -p helion-ultrascale`: 141 passed, 0 failed
  (log /tmp/orch11-postmerge-tests.log); the bitgen_device suite
  ran 13 passed, 0 failed, including the M1-fixed
  xcku3p_part_and_idcode and emit_parse_roundtrip_on_real_device.
- `cargo fmt -p helion-ultrascale -- --check`: exit 0, clean.

Clippy note: plain `cargo clippy --all-targets` stays blocked on
the pre-existing helion-sv lib.rs:15059 error (zero files in this
diff under helion-sv; out of scope, belongs to the helion-sv lane
that owns the 25 pre-existing helion-sv failures disposition).
Critic session 12 verified the scoped re-run
(`-- -A clippy::overly_complex_bool_expr`) exits 0 with zero
warnings in any file this worker touched.

Block status: no block goes green from this merge alone. INT_PIP
stays green tier 1. What this merge unblocks: the emitter can now
run on a real device file (real BitgenDevice for DeviceFile), so
FRAMING green on a real device is unblocked at the library level.
Remaining before the first real-emitter harness run: the emitter
executable wiring (HELION_BITGEN_EMITTER or --emitter-cmd, or the
BITGEN stage in ultra.rs, which still fails loud at ROUTE before
bitgen). Carried, not fixed: critic session 7 FLAG F1 (last-of-row
FAR 2-zero-padding path untested); xcvu3p frame-total gap 73,035
vs UG570 Table 1 71,820, flagged provisional, never tuned away.

## 2026-11-08 ~15:10 EST: orchestrator session 12, emitter MERGED (fa58d5c)

Branch fm/ultra-bitgen-emitter (tip f692cd5) merged into fm/ultra-bitstream.
Critic session 13 verdict was PASS, no defects, subject to FLAG F4 (merge-time
wiring). The merge is done; the base tip is fa58d5c.

### Rebase

`git rebase 1ad06df` in ~/workspace/helion-ultra-bitgen-emitter: f692cd5
replayed clean as c95da1a, no conflicts. The base moved since the branch was
cut only by the bitgen-device lane (4fd7120), which touches no emitter file.

### FLAG F4 wiring (commit 9290ce6, one file)

`load_device(part, data_dir)` in
crates/helion-ultrascale/src/bin/helion-bitgen.rs now loads the real device:
`DeviceFile::from_path(data_dir.join("xcku3p.json"))` for `xcku3p`, and
every other part fails loud. The StubDevice table is deleted from the
binary; no `StubDevice` reference remains in it (the library test stub in
bitgen.rs is untouched, it belongs to the library's own tests). SLICE/INT/
RAMB base FARs and the full 41,460-frame config order now come from the
device file. Tests updated in the same commit: `stub_base_fars_match_device_file`
renamed to `real_device_fars_match_device_file`; `unknown_part_fails_loud`
unchanged in expectation (`xcvu3p` still fails loud, see below).

DEVIATION FROM THE LITERAL F4, recorded not hidden: one provisional bridge
stays, and it had to. The real `DeviceFile` impl returns `None` for HPIO
tiles, but the v1 goldens carry `HPIO_RIGHT_X0Y0` lines, so literal
deletion fails emit on the golden with `UnknownTile` and regresses every
green block to blocked (`emitter failed to emit`), which the SPEC greedy
rule forbids (a fix that regresses a green block is reverted on the spot).
The bridge is exactly one tile, not a table: `HpioProvisionalFallback`
delegates everything to the inner `DeviceFile` except
`HPIO_RIGHT_X0Y0` -> tile type `HPIO_RIGHT`, base FAR 0x000, the stub's
provisional value (critic session 13 FLAG F1). The value is invented,
pinned by the renamed test, and must never be consumed as ground truth;
the harness blocks IO_CONFIG before comparing, so no greenness depends
on it. The bridge dies when the xcku3p tilegrid lands.

Evidence that the bridge is load-bearing, found during this merge and
fixed in the same commit: the first F4 attempt delegated `tile_names()`
straight to the inner `DeviceFile`, and the two golden bin tests failed
with `UnexplainedBits { far: 2, bits: [73, 74, 109, 123, 124, 125, 126,
127, 129, 130, 132, 133, 134, 201] }` (log /tmp/orch12-test.log):
`parse_bitstream` only visits tiles in `tile_names()`, and the bridged
HPIO tile was absent, so its emitted bits (FAR 0x00000002) had no
explaining feature. Fix: the wrapper's `tile_names()` is the real list
plus `HPIO_RIGHT_X0Y0`, pinned by a test assertion. After the fix the
bin suite went 7 passed, 0 failed (log /tmp/orch12-test-bin2.log).

`xcvu3p` stays unwired: `xcvu3p.json` exists on disk but is provisional
(computed frame total 73,035 vs UG570 Table 1's 71,820). Wiring it into
the emitter is a CTO decision for the device-file lane, not a merge-time
call. The error message says so.

### Merge

`git merge --no-ff fm/ultra-bitgen-emitter` in ~/workspace/helion-ultra-bitstream,
ort strategy, no conflicts. Merge commit fa58d5c. 12 files changed, 7629
insertions, 2 deletions. Merge tree verified byte-identical to 9290ce6
(`git diff fa58d5c 9290ce6` empty). No side's tests dropped: base 141 plus
emitter 12 (7 bin + 5 lib segbits) = 153. Base worktree clean except
pre-existing untracked bitfiltrator-ref/.

### Verification on the merged tip fa58d5c

- `cargo test -j2 -p helion-ultrascale`: 153 passed, 0 failed
  (log /tmp/orch12-test-merged.log). Per-suite: lib 82, bin helion-bitgen 7,
  bitgen_device 13, bitgen_roundtrip 9, corpus_legality 2, device_file_demo 3,
  device_file_xcvu3p 3, int_pip 2, int_pip_audit 1, lut_ff_bram 22,
  segbits 4, smoke 2, sta 3, doc-tests 0.
- `cargo clippy -j2 -p helion-ultrascale --lib --bins`: exit 0, 0 warnings
  (log /tmp/orch12-clippy-merged.log). Plain `cargo clippy --all-targets`
  stays blocked on the pre-existing helion-sv lib.rs:15059 error (zero
  files in this diff under helion-sv; helion-sv lane).
- `cargo fmt -p helion-ultrascale -- --check`: exit 0, clean.
- Harness run with the merged binary
  (`HELION_BITGEN_EMITTER=<base worktree>/target/debug/helion-bitgen
  python3 diff.py --design all --part xcku3p --out runs/orchestrator12`,
  log /tmp/orch12-harness.log, exit 0): `diff
  runs/emitter-session1/diff.jsonl runs/orchestrator12/diff.jsonl` is
  EMPTY, byte-identical. blinky: LUT_INIT 0/64 green, FF_INIT 0/1 green,
  INT_PIP 0/11 green. counter: LUT_INIT 0/256 green, FF_INIT 0/4 green,
  INT_PIP 0/17 green. FRAMING red diff=1 on both, the SRVAL class only
  (critic session 13 item 5 verified the session-1 FRAMING red is
  SRVAL-only; the output is byte-identical, so the cause is the same).
  IO_CONFIG blocked on both (tilegrid). BRAM_CONTENT, BRAM_PARITY,
  DSP48E2, URAM288, CLK blocked (not exercised by the v1 goldens).
  Placement hashes match the baseline.

### Behavior change to know about

Emitted bitstreams are now full-device: with the real config order,
`emit_bitstream` writes all 41,460 frames (untouched as zeros), so
emit.bit is 16,585,732 bytes per design, vs 8,532 bytes from the stub's
touched-frames-only stream. Per-block diffs are unchanged (only set bits
are compared), but parse and bits now walk the full device: the bin
suite took 168s, the harness run about 8 minutes. A future full-design
emit should watch this cost; emit_frames zero-frame materialization is
already a recorded performance note.

### Carried FLAGs (from critic session 13, none blocks this merge)

- F1 (provisional data): the HPIO bridge base FAR 0x000 is invented;
  its absolute FARs must never be consumed as truth. Resolves when the
  xcku3p tilegrid lands and the bridge is deleted.
- F2 (needs a CTO ruling): `bits` fails loud on a CARRY8 feature that
  sets bits; CARRY8 has no block in the harness registry. Do NOT guess
  which block it belongs to. Unexercised by the v1 goldens.
- F3 (harness-side, latent): harness fasm.py classify raises on
  unclassifiable features, so the parse-mediated fallback path would
  fail on parse-back extra lines. Every record in this run uses `bits`,
  so the gap is latent, not live. It becomes live only if a future
  emitter drops `bits`.
- F5 (minor, no action): parse-back renders both the grouped
  `A6LUT.INIT[63:0]` line and the per-bit `ALUT.INIT[i]` lines for the
  same set bits. Harmless under the harness feature-scoped comparison.

Carried from earlier sessions, not fixed here: critic session 7 FLAG F1
(last-of-row FAR 2-zero-padding path untested); xcvu3p frame-total gap
73,035 vs UG570 Table 1 71,820, flagged provisional, never tuned away;
plain clippy --all-targets blocked on helion-sv lib.rs:15059.

### Block status and next greedy step

LUT_INIT, FF_INIT, INT_PIP green tier 1 on xcku3p for blinky and counter,
now against the merged emitter on the real device file. FRAMING is the
standing largest red item (diff 1, SRVAL class only): the next greedy
step is the golden-side SRVAL completion (add `SLICE_X0Y0.AFF.SRVAL = 0`
etc. to the goldens, diff-harness lane), which is the only thing standing
between the current state and FRAMING green on xcku3p for blinky and
counter. After that, IO_CONFIG tilegrid is the standing largest blocked
item. The HPIO bridge above is a merge-time judgment, not critic-reviewed
in this exact shape; a critic re-review of just the bridge (session 14)
is recommended when the CTO next looks at this lane.

## CTO ruling 2026-11-08 ~10:08 EST: CARRY8 gets its own block

Critic session 13 FLAG F2, carried by orchestrator session 12: `bits` fails loud on a CARRY8 feature that sets bits because the harness registry has no CARRY8 block. Ruling: add CARRY8 as its own block in the harness registry (INTERFACES.md block registry and harness fasm.py classify), alongside LUT_INIT, FF_INIT, and the rest. Do not fold CARRY8 bits into LUT_INIT or any other block; CARRY8 is a distinct CLB primitive with its own config bits. The next worker that touches the registry implements it. Until then, `bits` keeps failing loud on set CARRY8 bits, never guessing.

## 2026-11-08 ~10:26 EST: orchestrator session 13, SRVAL golden completion (INTERIM: FRAMING green, FF_INIT blocked on a resolver gap)

Queue item: golden-side SRVAL completion, diff-harness lane. Base fm/ultra-bitstream tip fa58d5c, 153 tests, emitter merged.

### Worker 1 (diff-harness): goldens completed, FRAMING green, FF_INIT regressed

`gen_golden.py` build_fasm now emits `SLICE_X0Y0.<BEL>.SRVAL = 0` per FF
right after its INIT line. Regenerated goldens (harness dir is not a git
repo, no commit): blinky 1 SRVAL line (AFF), counter 4 (AFF, BFF, CFF,
DFF). New placement hashes: blinky xcku3p
7bf851696e269c847fb967137a3d4e6c741fdcb6b4cfdb97dec3f8b966dbf7f1, counter
xcku3p bbb402552d5ccae18b341112bb2f838c88c1d8c8891d7507fa74878f13986a3e
(was b65979c1... / 1bf0ffe6...). Golden sha256: blinky.xcku3p.fasm
378bd098a02dc04abab27282cbfa966068ef8a73bb3cb4ce448ec5fd3008d7ee,
counter.xcku3p.fasm
9bcedfbd7b4e97c8659151e662f8c81e95668a505433a34714bd9ed509437ec2.
Regression test added: TestFasm.test_every_ff_has_exactly_one_srval_zero
(passes); test_classify_golden_lines counts updated blinky 1 to 2,
counter 4 to 8 (fasm.py classify already maps SRVAL to FF_INIT).

Harness run runs/srval-golden (merged emitter binary, no rebuild, log
/tmp/srval-golden-run.log, about 12 min, 20 records, tier1):
FRAMING green 0 on BOTH designs ("round-trip green: 3 passed, 0 failed,
0 skipped"). LUT_INIT and INT_PIP still green. But FF_INIT regressed
green to blocked on both designs (diff null), with missing text "SRVAL
encoding not wired into the resolver yet: BitFiltrator RegLoc covers
INIT only; prjuray AFF.SRVAL SRVAL bits exist but no golden uses them".
Harness test suite after the change: 25 passed, 10 failed (baseline 34
passed, 0 failed); all 10 failures are TestDiffEngine with the same
single root cause. Worker stopped per its brief and did not paper over
it. No emitter, crate, or base-branch change was made. Scope compliance
confirmed.

### Root cause, verified by the orchestrator on disk

diff_engine.py resolve_feature, the `.SRVAL` branch (lines 81-86),
raises MissingReference for every SRVAL feature. The golden change is
correct at the bit level (FRAMING green proves the emitter sets the
SRVAL bits); the regression is a harness resolver gap, not a bit
regression, so the golden change is NOT reverted. The SRVAL bits exist
in the vendored prjuray DB (verified by grep on
reference/prjuray-db/zynqusp/segbits_clel_r.db): CLEL_R AFF.SRVAL.V0
12_01 / V1 !12_01, BFF 12_05, CFF 12_07, DFF 12_11. Polarity is bit =
NOT(value), same as INIT, per critic session 13 item 5(a). The SRVAL
delta rule (same frame as INIT, bit -1 for A/C/E/G, +1 for B/D/F/H)
was verified by critic session 9 against all four CLB DBs for all 16
FFs.

### Worker 2 (diff-harness follow-up): wire SRVAL into the resolver

Spawned 2026-11-08 ~10:26 EST. Task: add reference.py
resolve_srval using the delta rule, cross-checked against the prjuray
DB lines (must agree for every golden FF; any disagreement stops the
work), replace the MissingReference raise in diff_engine.py with a
resolve_srval call returning FF_INIT / absolute, fix the stale "no
golden uses them" wording, add a regression test, re-run the full
harness test suite (all 10 TestDiffEngine failures must pass again;
baseline 34 passed, 0 failed, now 35 tests), and re-run the harness
into runs/srval-complete. Verification bar for this item: FRAMING
green on both designs AND FF_INIT green on both designs, everything
else unchanged vs runs/orchestrator12. Expected FF_INIT total_bits 2
(blinky) / 8 (counter) once INIT plus SRVAL are both counted; to be
confirmed from the actual output, not assumed. Scope stays
harness-side: no emitter or crate changes, no CARRY8 registry work,
IO_CONFIG tilegrid not started.

### Block status (interim)

FRAMING is green on xcku3p for blinky and counter in runs/srval-golden,
pending the resolver fix and a critic re-run before it can be called
green per the SPEC greedy rule. FF_INIT is blocked in that run on the
resolver gap, not on a bit diff. The item is NOT complete until
runs/srval-complete shows both FRAMING and FF_INIT green with nothing
else changed.

## 2026-11-08 ~10:40 EST: orchestrator session 13, SRVAL item COMPLETE (harness level)

Worker 2 closed the resolver gap. `reference.py` gained
`resolve_srval` (INIT RegLoc entry, same frame, bit delta -1 for
A/C/E/G, +1 for B/D/F/H, bit = NOT(value)) plus `cross_check_srval`
mirroring `cross_check_ff`. `diff_engine.py` `.SRVAL` branch now calls
`ref.resolve_srval` and returns FF_INIT / absolute; the stale "no
golden uses them" wording is deleted (grep over the harness finds no
occurrence of "no golden uses them" or "not wired into the resolver").
Delta-rule vs prjuray-DB cross-check: AGREE, zero disagreements, all
128 DB lines (4 tile types x 16 FFs x V0/V1); RegLoc minor is constant
12 for all tile types x 16 FFs x all 60 Y offsets; derived SRVAL
offsets stay in-frame and never collide with INIT offsets. One note:
the first check script assumed a uniform 48-per-row stride and tripped
at Y=30 (AFF ofst 1394 at Y29 to 1538 at Y30); that is BitFiltrator's
own RegLoc layout, not a delta-rule failure, and the rerun without the
stride assumption passed clean.

Regression test added: test_srval_resolves_to_prjuray_db_positions
(TestReference), asserting resolve_srval returns the exact DB position
with bit = NOT(value) for both values, cross_check_srval true for all
16 FFs, and every golden SRVAL line resolving through resolve_feature
with no MissingReference. Two pre-existing assertions updated for the
now-correct counts: test_blocked_without_emitter_never_zero blinky
FF_INIT total_bits 1 to 2, and
test_partial_ff_init_roundtrips_clean_after_completion partial-counter
FF_INIT total_bits 3 to 7 (3 INITs plus all 4 SRVALs).

Harness test suite, re-run by the orchestrator: `python3 -m unittest
discover -s tests` gives Ran 36 tests in 9.236s, OK. All 10 prior
TestDiffEngine failures pass again, plus the new test.

Harness run runs/srval-complete (merged emitter binary, no rebuild,
log /tmp/srval-complete-run.log, 20 records, tier1), verified by the
orchestrator from runs/srval-complete/diff.jsonl:
- blinky: LUT_INIT 0/64 green, FF_INIT 0/2 green, INT_PIP 0/11 green,
  FRAMING 0/3 green.
- counter: LUT_INIT 0/256 green, FF_INIT 0/8 green, INT_PIP 0/17
  green, FRAMING 0/3 green.
- IO_CONFIG still blocked on both (tilegrid). BRAM_CONTENT,
  BRAM_PARITY, DSP48E2, URAM288, CLK still blocked, not exercised.
Diff vs runs/orchestrator12: exactly 4 lines differ (FF_INIT
total_bits 1 to 2 / 4 to 8, FRAMING red 1 to green 0, both designs).
Diff vs runs/srval-golden: exactly 2 lines differ (FF_INIT blocked to
green, both designs). Goldens untouched by worker 2 (sha256 match
worker 1: blinky 378bd098..., counter 9bcedfbd...). Base branch still
fa58d5c, clean except untracked bitfiltrator-ref/. No em dashes in any
edited file (grep count 0). No emitter, crate, or base-branch change
in this item; no CARRY8 registry work; IO_CONFIG tilegrid not started.

### Block status and next greedy step

LUT_INIT, FF_INIT, INT_PIP, FRAMING green tier 1 on xcku3p for blinky
and counter against the merged emitter on the real device file, at the
harness level. Per the SPEC greedy rule a block goes green only on
zero diff AND a critic re-run: FRAMING and the FF_INIT total change
still need critic session 14 (CTO-spawned) to re-run the check before
the green is formal. After that, IO_CONFIG tilegrid is the standing
largest blocked item (55 bits on both designs). Carried: CTO ruling
2026-11-08 ~10:08 EST, CARRY8 gets its own harness block, for the
next worker that touches the registry; critic session 7 FLAG F1
(last-of-row FAR 2-zero-padding path untested); xcvu3p frame-total gap
73,035 vs UG570 Table 1 71,820, flagged provisional, never tuned away;
plain clippy --all-targets blocked on helion-sv lib.rs:15059.

## 2026-11-08 ~10:27 EST: bitgen-100 driver run (hourly)

- Base verified fresh: fm/ultra-bitstream tip fa58d5c, clean except untracked bitfiltrator-ref/. Critic 13 PASS (emitter), critic 14 PASS (HPIO bridge only, CRITIQUE.md:2506). Queue items 1-3 closed; emitter merged by orchestrator session 12.
- Queue item 4 (golden-side SRVAL) IN FLIGHT, no spawn: orchestrator session 13 (0adb7d15) running, worker 2 wiring SRVAL into the harness resolver. runs/srval-golden verified from diff.jsonl: FRAMING green 0/3 both designs, LUT_INIT and INT_PIP green, FF_INIT blocked on the resolver gap. runs/srval-complete not yet present. FRAMING green is interim until the resolver fix plus a critic re-run.
- LICENSE SERVER: TCP connect to license2.forest.usf.edu:27000 succeeded twice this run (DNS 198.18.111.127), first time the port answers since Oct 22. No lmutil or Vivado on this VM, so a real license checkout is NOT verified. Tier-2 diff still cannot run (2 designs only, tier 1 not at 100). Flagged as tier-2 progress; next runs re-check.

## 2026-11-08 ~10:55 EST: orchestrator session 14, xcku3p tilegrid (IO_CONFIG)

Queue item: the xcku3p tilegrid, next greedy step after critic session 15
formalized FRAMING green and the FF_INIT total change at merged tip
fa58d5c. IO_CONFIG is the standing largest blocked item (55 bits on both
designs, tile-relative reference bits, absolute FARs need the tilegrid).
Base fm/ultra-bitstream tip fa58d5c, 153 tests, LUT_INIT / FF_INIT /
INT_PIP / FRAMING green tier 1 on xcku3p for blinky and counter.

Shape: researcher first (ground truth for the tilegrid), then a builder
worker (tilegrid plus IO_CONFIG wired to absolute FARs), then a harness
re-run showing IO_CONFIG green on both designs with no other block
regressed. Hard rules: no invented tile positions presented as ground
truth; every provisional value flagged provisional with its source; if
the tilegrid cannot be grounded, the item stays blocked with the exact
missing data named, never a silent skip or a guessed grid. The HPIO
bridge in the emitter binary (HPIO_RIGHT_X0Y0 to base FAR 0x000,
invented, critic session 14 PASS as load-bearing) must not be deleted
until the tilegrid actually resolves HPIO tiles; deleting it early
regresses every green block to blocked. Out of scope for this session:
BRAM golden design, random-design generator (later queue items the
scheduler drives next). CARRY8 registry work stays with the next worker
that touches the registry (CTO ruling 2026-11-08 ~10:08 EST).

### Researcher DONE 2026-11-08 ~11:05 EST: no xcku3p tilegrid exists in any public source

Report: ~/workspace/ultra-bitgen-crew/BRIEFS/tilegrid-research.md
(1,173 lines, 0 em dashes, every claim with exact command and pasted
output). Read-only work only.

Grounded findings:
- No xcku3p tilegrid exists in any public source. BitFiltrator has no
  tilegrid for any part; prjuray-db covers xczu3eg only; a
  workspace-wide search finds only 7-series and ECP5 tilegrids.
- xcku3p.json is a column-major index, not a tile map: CLB 113, BRAM
  10, DSP 19 column majors, 230-entry minors table. No tile names, no
  HPIO/HDIO/CMT columns. Vendored copy byte-identical to BitFiltrator
  (md5 e4e060cb73f243eccbe9dacf23072897).
- NEW FLAG: tileType_siteType_pairs lists HPIO_L, not HPIO_RIGHT.
  Every non-Zynq UltraScale+ device file in BitFiltrator shows HPIO_L;
  only Zynq xczu1/2/3eg/cg and xazu1/2/3eg show HPIO_RIGHT. The
  golden's tile type HPIO_RIGHT may not exist on xcku3p at all. There
  is no segbits_hpio_left.db anywhere, so if it does not exist,
  IO_CONFIG needs a new HPIO_L fuzzing campaign, not just a tilegrid.
- BitFiltrator demo.py / bit_locator.py cover LUT/FF/BRAM only; arch
  file has 5 keys (BRAM, CLEL_L/R, CLEM/R); zero HPIO/HDIO mentions.
- xczu3eg tilegrid schema fully mapped (66,385 tiles, 158 types;
  sbva484 and sfvc784 byte-identical, md5
  7cea605749f946f896338fa527fccf1e). HPIO_RIGHT: 6 tiles, col 103.
- IO_CONFIG is 55 bits, verified: 21 (IOB_X0Y0 IN) + 34 (IOB_X0Y1
  OUT), frames 2 and 3 only, one tile.
- INT rule CORRECTED, not just unconfirmed: on xczu3eg, CLEL_R serves
  INT at col -1 (4,500 tiles); CLEL_L/CLEM/CLEM_R serve INT at col +1
  (4,320 tiles). The xcku3p minors table independently agrees: 52 CLB
  columns have a 76-minor left neighbour (all CLEL_R), 61 have one on
  the right (all other types), zero overlap, zero neither. The
  golden's INT_X0Y0 is CLEL_R column 0, so its INT_PIP green is
  unaffected, but INT_COL_OFFSET_FROM_CLB = -1 must gain a tile-type
  branch before any other column uses it.

Orchestrator verification (independent re-runs, 2026-11-08 ~11:05 EST):
- Report 1,173 lines, grep -c em dash = 0.
- xcku3p.json pairs: HPIO_L present (8 site types), HDIO_BOT_RIGHT /
  HDIO_TOP_RIGHT present, HPIO_RIGHT absent. BitFiltrator files:
  grep -c HPIO_RIGHT = 0 in xcku3p.json and ULTRASCALE_PLUS.json;
  HPIO_L = 7 in xcku3p.json.
- find for *hpio_left* across vendored data and prjuray-db-ref: no
  segbits_hpio_left.db.
- Minors-table reproduction (all 4 FAR rows, 452 CLB cols): left only
  208 (all CLEL_R), right only 244 (CLEM 184, CLEL_L 36, CLEM_R 24),
  both 0, neither 0. Per row 52/61, first-5 entries identical to the
  report. The INT correction is independently confirmed.

Recommended scope (researcher) and orchestrator decision: the
tilegrid item STAYS BLOCKED for tier-1 green. Do not build a full
tilegrid for 55 bits, and do not copy xczu3eg numbers to xcku3p
(different part, 104 vs 230 standard columns, 3 vs 4 clock-region
rows). The exact missing data is the Q6 Vivado probe (report Q6.2):
step 1 tile inventory via extract_device_info.tcl plus the arbiter
queries (TILE_TYPE =~ HPIO*, HDIO*, CMT*, == HPIO_RIGHT, sites of
HPIO tiles), which alone settles HPIO_RIGHT vs HPIO_L; step 2 one
row-0 HPIO tile base FAR plus frame count via .ll/bitstream probe;
step 3 the offset ruling for the second tile in the row. Step 4: if
step 1 finds no HPIO_RIGHT, stop and escalate to the CTO. The probe
needs a license and cannot run from this VM (no lmutil/Vivado here);
it is a named blocker, not a skipped step. The HPIO bridge stays
exactly as is until the probe lands.

Builder worker spawned 2026-11-08 ~11:10 EST (worktree
~/workspace/helion-ultra-bitgen-tilegrid, branch
fm/ultra-bitgen-tilegrid, base fa58d5c): implement the corrected
type-dependent INT rule in bitgen_device.rs int_col_major (+1 branch
with the same 76-minor validation, -1 restricted to CLEL_R) and in
harness reference.py (INT_COL_OFFSET_FROM_CLB), with tests resolving
an INT tile on a CLEM column. Verification bar: full suite green,
clippy/fmt clean, harness run runs/tilegrid-int-rule diff.jsonl
IDENTICAL to runs/srval-complete (zero differing records, goldens
untouched). Out of scope for the builder: HPIO bridge, goldens,
tilegrid build, CARRY8 registry, BRAM golden, random-design
generator. IO_CONFIG stays blocked in this session.

### Builder DONE, merged 2026-11-08 ~11:35 EST: INT rule corrected (a63f49c, merge 060b230)

Worker re-ran Q5.3 before changing code (log
/tmp/tilegrid-int-rule/q53_check.log): all 4 rows 52 left (all
CLEL_R), 61 right (CLEM 46, CLEL_L 9, CLEM_R 6), 0 overlap, 0
neither. Commit a63f49c on fm/ultra-bitgen-tilegrid (2 files,
+71/-37, 0 em dashes): bitgen_device.rs int_col_major gains the +1
branch for non-CLEL_R CLB types with the same 76-minor validation
(-1 restricted to CLEL_R); harness reference.py replaces
INT_COL_OFFSET_FROM_CLB = -1 with int_col_offset_from_clb(tile_type),
used by resolve_int_pip; both comments cite the two sources and keep
the provisional flag, stating the +1 side has never been exercised
by a golden. Tests: INT_X1Y0 now resolves (CLEM col 1, major 6,
INT major 7, base FAR 0x700, the same INT column INT_X2Y0 resolves
to); INT_X7Y0 resolves (CLEL_L col 7, major 18, INT major 19, base
FAR 0x1300); tile_names counts updated to the corrected rule (xcku3p
INT 208 to 452, total 28,532; xcvu3p INT 364 to 791, total 51,731;
INT_X113Y0 asserted absent, tail columns 113..117 fail loud).
Harness: 2 new tests (type-dependent offset; BOUNCE resolves on a
CLEM column at INT major 7).

Orchestrator verification (all run by the orchestrator, not taken on
trust): branch diff scope exactly the 2 files, 0 em dashes, HPIO
bridge untouched (helion-bitgen.rs not in the diff); no stale
INT_COL_OFFSET_FROM_CLB references (grep exit 1); harness unittest
re-run 38 tests OK (36 + 2 new); diff
runs/tilegrid-int-rule/diff.jsonl vs runs/srval-complete/diff.jsonl
byte-identical (0 lines, plus identical ignoring placement_hash, 20
records each); worker logs sum to 153 passed, 0 failed, fmt log
empty, clippy no warnings. Merge commit 060b230 on
fm/ultra-bitstream (ort, no conflicts), tree identical to a63f49c
(empty diff). Post-merge cargo test -j2 -p helion-ultrascale on the
merged tip: 153 passed, 0 failed, exit 0 (log
/tmp/tilegrid-int-rule/cargo-test-postmerge.log).

Block status after this session: LUT_INIT, FF_INIT, INT_PIP, FRAMING
still green tier 1 on xcku3p for blinky and counter, unchanged. The
INT rule is now correct for all 113 CLB columns per row instead of
52, but INT placement stays absolute-provisional until the xcku3p
tilegrid lands. IO_CONFIG stays blocked (55 bits, tile-relative);
the tilegrid item stays blocked on the Q6 Vivado probe (step 1 tile
inventory first, settles HPIO_RIGHT vs HPIO_L; steps 2-3 one row-0
HPIO tile FAR plus offset ruling), which cannot run from this VM.
Per the SPEC greedy rule the INT fix is green only after a critic
re-run: critic session 16 (CTO-spawned) to re-review a63f49c plus the
harness-side reference.py change. Worktree
~/workspace/helion-ultra-bitgen-tilegrid kept at a63f49c for the
critic. Next queue items (not started): BRAM golden design,
random-design generator (scheduler drives next).

## 2026-11-08 ~11:27 EST: bitgen-100 driver run (hourly, quiet, no spawn)

- Base verified fresh: fm/ultra-bitstream tip fa58d5c, clean except untracked bitfiltrator-ref/.
- Critic session 15 PASS verified in CRITIQUE.md:2690: FRAMING green and the FF_INIT total change are formal, no defects. runs/srval-complete/diff.jsonl re-verified directly: blinky and counter both LUT_INIT, FF_INIT, INT_PIP, FRAMING green, diff 0; IO_CONFIG blocked 55; BRAM/DSP/URAM/CLK blocked.
- Orchestrator session 14 (tilegrid) in flight, no spawn: agent 38efe3e3 running ~33 min; builder work live in ~/workspace/helion-ultra-bitgen-tilegrid (uncommitted INT-rule changes, 71 insertions / 37 deletions) and harness/runs/tilegrid-int-rule in progress (emit.bit/roundtrip.bit written 11:21-11:27 EST, no diff.jsonl yet). Tilegrid itself stays blocked pending the Q6 Vivado probe; HPIO_RIGHT may not exist on xcku3p (HPIO_L instead).
- License server: TCP connect to license2.forest.usf.edu:27000 succeeded twice this run (DNS 198.18.111.127), same as 10:27. No lmutil/Vivado on this VM, so a real checkout is still unverified. Tier 2 still cannot run.
