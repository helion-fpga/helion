# UltraScale+ Session 18 Handoff for Claude Code

Date: November 19, 2026 (updated with the Nov 19 sync corrections)
From: Muse (Saksham's agent)
Repo: https://github.com/helion-fpga/helion
Branch for this file: fm/ultra-bitgen-handoff-session18 (docs only, forked from fm/ultra-bitstream 07d0d7d)
Goal: license-free Virtex UltraScale+ backend support for Helion, bit-identical bitstreams, QoR at or above Vivado per design.

## Corrections since the first version of this handoff

- fm/helion-sv-ug901-dsp-bram-srl-fsm-infer tips at 9508202, not 11d65720. That hash (11d65720) is fm/helion-place-sa-detailed-swap-carry-bram-dsp-column. 9508202 is the commit "sv(ug901): implement DSP48E1, BRAM/ROM, SRL16E/SRLC32E, and FSM resource inference", touching crates/helion-sv/src/lib.rs, crates/helion-sv/src/ug901.rs (2,727 lines), and crates/helion-sv/tests/ug901_infer.rs. Verified: 9508202 is an ancestor of fm/zedboard-merge 7ec3192. It is the legacy front end inference, not v2, so the v2 DSP work (G3) is not a duplicate.
- fm/g3-dsp-v2 and fm/rung7-ps7 have no commits yet; agents are building and reviewing them now. They will be pushed only after review passes and Saksham says yes. The DSP48E2 extension will be sent as a patch on top of fm/g3-dsp-v2 once its hash is posted.
- Rebase: agreed, it is Saksham's call. No rebase is planned or authorized until he decides.

## How to get the code

All session 18 work is pushed to GitHub on `fm/` branches. Nothing is merged anywhere.

```sh
git clone https://github.com/helion-fpga/helion.git
cd helion
git fetch origin
git checkout fm/ultra-bitgen-bramgolden   # or any branch in the table below
```

Base branch for all bitgen work: `fm/ultra-bitstream` at `07d0d7d`.
Master: `566246a` (license: remove remaining MIT/Apache references). Do not touch master.

## Branch inventory (pushed Nov 19, 2026, remote tips verified to match local)

| Branch | Tip | Status |
| --- | --- | --- |
| fm/ultra-bitgen-bramgolden | fdd4e1f | BRAM golden, 161 tests passed, 0 failed, zero diff. Awaiting critic verdict (see below). Unmerged. |
| fm/ultra-bitgen-dsp | 091b715 | DSP48E2 done-pending-cargo-and-critic. Fail loud, fuzzing job 2 package ready. |
| fm/ultra-bitgen-ioconfig | 7465330 | IO_CONFIG done-pending-cargo-confirmation. Q6 HPIO probe package, 8/8 tests pass. |
| fm/ultra-bitgen-devmodel | d70467c | Device model done-pending-cargo. xcvu3p frame-gap extension, xcku3p slice-count corrections. |
| fm/ultra-bitgen-carry8 | f09c695 | CARRY8 work in progress, replacement worker verifying. |
| fm/ultra-bitgen-clk | 4709c98 | CLK audit plus Vivado clocking fuzz job (Job 4) docs, replacement worker in flight. |
| fm/ultra-bitgen-lutram | 01860ea | LUTRAM derivation recorded (US-04), replacement worker in flight. |
| fm/ultra-bitgen-route | 8ed62c0 | Router-to-PIP translator, WireDb routing graph (US-09/GAP-F4), replacement worker in flight. |
| fm/ultra-bitgen-uram | 0daab79 | URAM fail loud with named UramBlocked error, 2 commits. Second replacement queued. |
| fm/ultra-bitgen-packplace | 8ada374 | Pack/place promotion inventory for GAP-F2/F3. Replacement worker in flight. |
| fm/ultra-bitstream | 07d0d7d | Integration base. Nothing has been merged into it. |

Branches NOT pushed (no new commits yet, still at base 07d0d7d): fm/ultra-bitgen-synthmap, fm/ultra-bitgen-timing, fm/ultra-bitgen-multiseed. Their work is uncommitted or queued.

Older base branches already on origin: fm/ultra-bitgen-core (5e26453), device (33a4b8f), segbits (c1bef40), int (6bb30b6), lut (0e81099), lutff (7f80b54), tilegrid (a63f49c), emitter (9290ce6), integ (f5ebcf7), bitgendev (4fd7120), plus fm/ultra-device (7a4146f), fm/ultra-packplace (c9e96c4), fm/ultra-router (3d49b98), fm/ultra-timing (7ff0007), fm/ultra-multiseed (e1c2e47).

ZedBoard integration tip (Claude Code's side, not mine): fm/zedboard-merge at 7ec3192, verified on origin and fetched. The ZB-00 to ZB-12 list in docs/HANDOFF-INDEX.md is superseded by it; do not pick those sub-problems up.

## What is done

Tier 1 (zero diff against public segbits DBs, no Vivado needed) green on xcku3p for blinky and counter: LUT_INIT, FF_INIT, INT_PIP, FRAMING. Harness record: 60 records, 14 green, 46 blocked, 0 red.

BRAM golden (Nov 8): BRAM_CONTENT 0/16384 diff, BRAM_PARITY 0/2048 diff, full suite 161 passed, 0 failed. Branch fm/ultra-bitgen-bramgolden, commits 2e94fc3, 1b02834, fdd4e1f on base 8c9e801. One known clippy warning remains on the branch: too_many_arguments on write_frame_bit, bitgen.rs:853.

Session 18 completions (Nov 19, all unmerged, all on their own branches):

- DSP48E2 (091b715, 5 commits): independently re-verified that no DSP48E2 config-bit encoding exists in any public source (prjxray-db, BitFiltrator, vendored DBs, 0 hits). Fail loud via BitgenError::Dsp48e2Blocked with a zero-collision proof. Fuzzing job 2 package ready: 56 variants plus manifest. Caveat: cargo test never completed (OOM contention); direct rustc checks passed instead (lib compiles clean, dsp.rs 7/7, dsp48e2 integration 8/8, full lib unit-test binary 89/89). Outstanding: `cargo test -j1 --test dsp48e2`, plus critic re-review.
- IO_CONFIG (7465330, 2 commits): Q6 HPIO probe package for xcku3p (probe/q6-hpio/, Tcl scripts, XDC generator, tests), 8/8 tests pass, plus independent re-verification of the session 14 tilegrid research. Caveat: the -j1 cargo retry was still running at handoff; the fail-loud test result is not claimed passed. Outstanding: cargo confirmation, plus critic re-review.
- Device model (d70467c, 4 commits): re-verified all first-worker commits from primary sources, extended DEVICE_MODEL_GAPS.md (4-frames-per-FAR-row gap now holds on 8 devices with a direct UG570 Table 1 row; 9 XCU SSI files numerically consistent, recorded explicitly as numerical consistency only, never ground truth). Caveat: the new tests (tests/device_model_counts.rs, 6 tests, plus the license-note test) have never run green; first worker's cargo test died OOM. Outstanding: `cargo test -j1 -p helion-ultrascale --offline --test device_model_counts --test device_file_xcvu3p`.

## What is in flight (session 18, orchestrator still running as of Nov 19 ~10:47 EST)

- Critic session 17 redo on the BRAM golden branch: the original critic errored after about 90 minutes with no verdict written. A replacement critic is running; a Session 17 (redo, session 18) heading landed in CRITIQUE.md at 10:42 EST Nov 19, status IN PROGRESS, no verdict yet. This is the highest-priority item. It gates the BRAM branch being declared perfect and the US-07 random-design generator.
- Workers/replacements in flight: LUTRAM, route, CARRY8 (second replacement), CLK, critic replacement, packplace (second replacement). Queued: multiseed, synthmap (second), timing, URAM (second).
- Scope now covers the full flow, not bitgen only: synthesis mapping to UltraScale primitives (CARRY8, DSP48E2, RAMB36E2/RAMB18, URAM288, LUTRAM, SRLs, clocking), pack/place/route for the UltraScale fabric, STA, and 100-seed overnight timing closure. Staging branches: fm/ultra-packplace (c9e96c4), fm/ultra-router (3d49b98), fm/ultra-timing (7ff0007), fm/ultra-multiseed (e1c2e47). None of it is wired end to end yet.

## Synth mapping and the v2 front end

Session 18 synth mapping does NOT go through the v2 front end in crates/helion-sv. It is an inventory and mapping layer inside crates/helion-ultrascale (new src/synthmap.rs, still uncommitted, plus modified src/lib.rs, tests/corpus_legality.rs, and crates/helion-cli/src/ultra.rs; branch fm/ultra-bitgen-synthmap still at base 07d0d7d). No UltraScale mapper exists yet (GAP-F1), and no helion-sv files are in the session 18 plan. When inference needs to move into the front end, the DSP48E2 extension goes on top of fm/g3-dsp-v2 as a patch, not as a second inference. Guardrail: fm/qor2-mapper commit 47eed1e holds a proven miscompile (6-input AND to const 0, crates/helion-ir/src/priority_cut.rs, decompose_wide_luts_to_k) with the verified fix uncommitted in its worktree; do not touch fm/qor2-mapper without the pending CTO ruling. fm/qor2-mapper is local-only, not on GitHub.

## What is blocked, with the exact unblocker

- IO_CONFIG, 55 bits: no xcku3p tilegrid in any public source. Unblocker: the Q6 Vivado probe (package on fm/ultra-bitgen-ioconfig), to be run by Saksham on the USF cluster.
- DSP48E2 / URAM288 / CLK: no public encoding. Unblocker: Vivado fuzzing jobs (job 2 for DSP, job 5 for URAM, job 4 for CLK) run against a real Vivado checkout.
- LUTRAM, CARRY8: own blocks (US-04, US-05), in progress on branches.
- xcvu3p device file: frame-total gap (US-06). Remaining device-model blocks need Vivado confirmation (16-frame gap needs a KU3P debug bitstream; usable-site subset needs a get_sites probe; xcvu3p.json whole file needs Vivado confirmation; URAM schema needs FAR enumeration or a BitFiltrator schema extension).
- Tier 2 (zero diff against a real Vivado bitstream for the same design): BLOCKED. Vivado license state is UNKNOWN. Do not claim Vivado equivalence. Vivado is expected to arrive later; do not run or claim any Vivado check until it does. The PinPlanning probe package is prepared but not yet run on the cluster.
- ZedBoard is OUT OF SCOPE for this work. Do not spend energy on it; it is owned by Claude Code on fm/zedboard-merge now.

## Blockers clearable without Vivado or a board

GAP-B1 (BRAM critic verdict, redo in progress); GAP-B7 CARRY8 harness block (fm/ultra-bitgen-carry8 f09c695, public segbits only); GAP-F1 synth mapping inventory; GAP-F2/F3 pack/place staging (inventory plus legality against the device file); GAP-F4 route WireDb plus router-to-PIP translator (fm/ultra-bitgen-route 8ed62c0; only the INT PIP audit part 2 needs Vivado); GAP-F5 timing staging (structure on fm/ultra-timing 7ff0007; only the real xcku3p delay DB needs Vivado); GAP-F6 multiseed 100-seed flow (buildable and testable without Vivado on existing STA estimates); GAP-F7 device-model cross-checks (done on fm/ultra-bitgen-devmodel d70467c); GAP-F9 xcvu3p frame total (device-file bug class, partly workable from public sources); GAP-F10 harness speed; the clippy warning at bitgen.rs:853.

## Reuse for the 7-series side

- fm/ultra-router at 3d49b98: standalone router-scaffold crate (src/lib.rs 445 lines, mock.rs, tests.rs, 7 tests green, seed determinism). Reusable as a PathFinder scaffold shape, but not wired into helion-place, and it committed target/ build artifacts (180 files changed), so clean that before reuse.
- fm/ultra-timing at 7ff0007: staging/timing-impl (src/sta.rs, src/timing_db.rs). Reusable as STA structure and a timing_db seam, but its delays are preliminary xcvu3p values (critic flag F3), not 7-series numbers.
- fm/ultra-multiseed at e1c2e47: 389 lines in crates/helion-cli/src/main.rs, threads the seed through compile, adds a helion multiseed subcommand, rayon pool, JSONL checkpointing with resume, 6 unit tests, 10-seed proof WNS 8460 to 8820. Part-agnostic via --part but depends on place/route/STA working for the part.
- fm/ultra-packplace at c9e96c4: UltraScale-specific staging, limited reuse.

## Hard rules (Saksham's orders, do not break them)

1. NO MERGES WHATSOEVER. Do not merge anything into fm/ultra-bitstream or master, even on a critic PASS. Grind each block to absolute perfection on its own fm/ branch, critic-reviewed, then stop and report. Merges happen only when Saksham says so. Rebase onto fm/zedboard-merge is likewise Saksham's call, not yet decided.
2. Never push to master. Branches only.
3. No em dashes anywhere: not in chat, files, READMEs, artifacts, or code comments. Use hyphens, commas, colons, or restructure.
4. Never fabricate terminal output, test results, or numbers. Every claim needs the command plus output shown.
5. Builds: cargo -j1 or -j2 max (the box OOMs), via nohup setsid, log to files. Never clean target dirs.
6. One block per fix, one fix per commit. Re-run the full diff after each fix. Revert any fix that regresses a green block. A block goes green only on zero diff AND a critic re-run.
7. Only CC0/public-domain or MIT sources (prjxray-db CC0, BitFiltrator MIT). RapidWright.dat and FPGA Interchange DBs are REJECTED (Xilinx EULA). Never invent a bit value: fail loud with a named error instead.
8. The provisional HPIO bridge (HpioProvisionalFallback, base FAR 0x000) is an INVENTED value pinned by a test, never ground truth. It dies when the xcku3p tilegrid lands.

## Key files and how to verify

- Crew docs (in the repo on fm/ultra-bitstream): SPEC.md, INTERFACES.md, COORD.md, CRITIQUE.md, docs/ultra-bitgen-crew/HANDOFF.md, sub-problems US-01 to US-12 under docs/ultra-bitgen-crew/subproblems/. The ZedBoard rows in docs/HANDOFF-INDEX.md are marked SUPERSEDED by fm/zedboard-merge.
- Diff harness (outside the repo, not in git): ~/workspace/ultra-bitgen-crew/harness/, one command `python3 diff.py`, 38 tests OK. Golden record: harness/runs/bram-golden-final/diff.jsonl.
- Goal and gap list: workspace/goals/virtex-ultrascale-backend-support/GOAL.md (session 18 gap log GAP-B1 to B8, GAP-F1 to F10).
- Bitgen 100-design driver: hourly cron, family xcku3p (control part, xcvu3p second). Tier 1 diffs against public segbits DBs with no Vivado involved. Tier 2 waits for Vivado. Goal: 100 random designs green at BOTH tiers. Current: 60 records, 14 green, 46 blocked, 0 red.

Verify before trusting any claim:

```sh
git log --oneline -5 fm/ultra-bitgen-dsp
git rev-parse fm/ultra-bitstream   # must still be 07d0d7d; if it moved, a merge happened
python3 ~/workspace/ultra-bitgen-crew/harness/diff.py
cargo test -j1 -p helion-ultrascale --offline --test dsp48e2
```

## Known hazards in this environment

- Agents keep dying on sqlx pool timeouts (runtime transport, not work failures). Mitigation that works: keep tool calls small and few, avoid long polls, commit early and often so progress survives a transport death.
- Cargo builds OOM-kill at -j2 under load (rustc SIGKILL, exit 101). Use -j1, check `ps` for stray cargo/rustc processes before building.
- The box has about 6 GB RAM and 1.5 shared CPUs. Do not run parallel cargo builds across worktrees.

## What Claude Code should do next, in order

1. Read SPEC.md, INTERFACES.md, COORD.md, and the tail of CRITIQUE.md on fm/ultra-bitstream.
2. Check out fm/ultra-bitgen-bramgolden and re-run the harness and the full test suite yourself; confirm or dispute the 161 passed / 0 failed result and the zero diffs.
3. Run the three pending cargo confirmations (DSP dsp48e2 test, IO_CONFIG fail-loud test, devmodel device_model_counts plus device_file_xcvu3p) with -j1 when the box is quiet, and record the results on the branches.
4. Continue the in-flight blocks (CARRY8, LUTRAM, CLK, URAM, route/US-09, pack/place, synthmap, timing, multiseed) on their branches, one block per branch, with the greedy loop: fix the largest diff first, re-run the full diff after each fix.
5. Prepare (do not run) the Vivado-side work: Q6 HPIO probe and fuzzing jobs 2, 4, and 5, so they are ready the moment Vivado arrives.
6. Report back per branch: tip commit, tests run with output, diff numbers, what is still blocked and its named unblocker. Do not merge anything.
