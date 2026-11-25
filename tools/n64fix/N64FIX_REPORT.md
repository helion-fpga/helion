# N64 fix lane: REPORT

- Outcome: N64 CLOSED. The xcku3p/xcvu3p BRAM CLK to DOUT value is corrected
  from 1460 ps to 1020 ps, keeping the "-2 0.85V" citations, exactly as
  Critic Session 48 ruled (CRITIQUE.md section 2). All three documented
  DBs now agree at 1020 ps for the same stated conditions. No flow number
  moves: the documented DBs are uncalled by any binary, by design.
- Base: fm/ultra-bitgen-xtiming at c6e7615d (timing lane tip, A-partial
  closed by Critic Session 48).
- Lane branch: fm/ultra-bitgen-n64fix at 9f5fcd93 (merge; impl commit
  8ccf7c0c). Report commit: PENDING. Header commit: PENDING.
- Report copy: tools/n64fix/N64FIX_REPORT.md on the lane branch,
  byte-identical to this file.
- Workers: W-BASELINE (agent 8c076f14-e24e-4976-8d56-f260ee091627),
  W-IMPL (agent 64f787ee-087b-4234-b8fa-c7c4b0dc6d8f).
  Coordinator: agent 491e9926-9f3a-4509-a0fe-8476a4a8d7e0.
  No critic dispatched by this lane; the main agent dispatches
  Critic Session 49.
- Logs: ~/workspace/n64fix-logs/baseline/, impl/, coord/.
- Zero em dashes in this file.

## 1. Headline

Nit N64, found by the timing lane and confirmed by Critic Session 48's
own DS922/DS923 reads: documented_xcku3p() and documented_xcvu3p()
pinned BRAM CLK to DOUT at 1460 ps cited as "-2 0.85V", but 1.46 ns is
the -2 grade at 0.72V; -2 at 0.85V is 1.02 ns. The ruling: correct the
VALUE 1460 to 1020 ps, keep the "-2 0.85V" citation. The fix is 4 files,
+10/-8, and every pinned test now agrees at 1020 ps for all three parts.
The documented_xczu3eg() value (1020 ps, DS925 Table 80) was already
correct and is untouched. Preliminary DBs untouched. No citation text
changed anywhere.

## 2. The fix

Commit 8ccf7c0c on fm/ultra-bitgen-n64fix-impl, merged as 9f5fcd93:

- crates/helion-ultrascale/src/timing_db.rs: doc comment line 247
  (xcku3p, 1460 ps to 1020 ps), value line 255 (documented_db("xcku3p",
  1460, "DS922 Table 33 TRCKO_DO, -2 0.85V") to 1020), doc comment line
  262 (xcvu3p, "Max: 1460 ps" to "Max: 1020 ps"), value line 264
  (documented_db("xcvu3p", 1460, "DS923 Table 33 TRCKO_DO, -2 0.85V")
  to 1020).
- crates/helion-ultrascale/tests/timing_db.rs: asserts lines 134 and 152
  (Some(1460) to Some(1020)) and header comment line 7 ("1.46 ns at -2
  / 0.85V as cited" to "1.02 ns at -2 / 0.85V").
- crates/helion-ultrascale/tests/sta_sanity.rs:165: extra edit NOT in
  the critic's list. The critic prescribed 6 edits and missed this pin
  (assert_eq!(db.bram.clk_to_do.ps, Some(1460)) on documented_xcku3p()
  in sta_documented_db_fails_loud_on_lut6). The impl worker's first full
  suite run failed on it (464 passed, 1 failed, exit 101) and the worker
  corrected it with the same value change. A repo grep confirms no other
  file pins the value: remaining 1460 hits are date-math constants
  (doe / 1460) and a frame count (41460), all unrelated.
- crates/helion-ultrascale/docs/xczu3eg_timing_sources.md: one line
  added at the end of the N64 section noting the correction. The audit
  text itself was not reworded; the 1460 ps mentions there remain as
  correct history of the mis-citation.

The coordinator verified the diff itself: 4 files, +10/-8, no remaining
1460 in the three touched code/test files, all three documented DBs pin
1020 with unchanged citations.

## 3. Baseline (W-BASELINE, report 2026-11-25)

Base matches the xtiming lane exactly:

- The 1460 pins are exactly the four source lines and three test lines
  listed in section 2 (timing_db.rs 247, 255, 262, 264; tests/timing_db.rs
  7, 134, 152).
- Suites: ultrascale 465/0/1 (35 lines), CLI release 92/0/1 (11 lines),
  checkers 100 OK in 105.096s (system iverilog 12.0, as in the xtiming
  lane; verify-bits used oss-cad-suite 14.0).
- 12 flows all exit 0, numbers identical to the xtiming lane: chain16
  8570/362/4cf99124 twice, shift32 8690/289/a7a070dd twice, counter12
  7370/752/b18112d3, counter8 s0 8570/363/2902d6f0, s1 8570/342/01cb7f90,
  counters2x4 8450/307/730fcc93, counter5 9650/82/6fe2c80a, counter
  9650/58/4c99a51f, blinky 9650/4/d82b70c8, counter16 5750/1278/a45c6efd.
- Verify-bits 12/12 PASS (800/0 each, counter16 80000/0).
- Incidents: a runtime restart killed the first suites run after
  ultrascale finished (CLI re-run from scratch, same numbers); checkers
  OOM-killed 3 times (exit 137) at 97 dots, each coinciding with the
  sibling impl worker running heavy concurrent jobs, and passed on the
  4th attempt in an idle window (100 OK); two interim summary lines
  reading "checkers exit=137" came from a $? capture bug in the
  worker's inline retry echo and were rewritten to the true values.

## 4. Impl worker

- Fail-before proof: test-file edits applied with source still at base.
  Both documented-DB tests fail, exit 101: left Some(1460), right
  Some(1020), at timing_db.rs:134 and :152 (log fail-before.log). After
  the source value change the same tests pass.
- The omitted sta_sanity.rs:165 pin (section 2): first full ultrascale
  run failed 464/1/1 on it; corrected; clean re-run 465/0/1. The first
  run's log was overwritten by the re-run; the failure is recorded in
  the impl report only.
- Verification on branch binary: ultrascale 465/0/1, CLI 92/0/1,
  checkers 100 OK; 12 flows exit 0, every number identical to baseline;
  verify-bits 12/12 PASS; blinky d82b70c8.
- Incidents: runtime restart killed the first suites run mid-CLI-compile
  (re-run from scratch, about 60 min wall time); helion-bitgen did not
  exist after the CLI suite (it is the helion-ultrascale bin target, not
  part of the CLI build) and was built explicitly before verify-bits;
  concurrent builds in the sibling baseline worktree slowed the suites
  (environment noise). The impl report's incident 4 misattributes that
  concurrent build to "the coordinator baseline worktree"; no
  coordinator build ran in that worktree, the concurrent jobs were the
  baseline worker's own. Corrected here; the underlying fact (concurrent
  heavy jobs slowed the suites) is real.

## 5. Coordinator acceptance (merged lane branch, own runs)

Release build from the lane worktree at merge 9f5fcd93, exit 0:
helion 12,955,176 bytes md5 61735f07aee246703610a27667f741d9,
helion-bitgen 5,740,016 bytes md5 25058cb6c031bf78f4eca04142d3c13a.
The helion size matches the xtiming coordinator build exactly with a
different md5 (N54 class, worktree effect). Log:
~/workspace/n64fix-logs/coord/build-release.log.

### 5a. Flows (12/12 exit 0, numbers identical to baseline and impl)

Script: ~/workspace/n64fix-logs/coord/run-coord-flows.sh (copied
verbatim from the xtiming coord scripts, only worktree-root and log-dir
paths changed; diff shows only those lines). Summary
coord-flow-summary.txt.

- chain16 twice: exit 0, WNS_PS=8570, 362 PIPs, md5 4cf99124 (both runs).
- shift32 twice: exit 0, WNS_PS=8690, 289 PIPs, md5 a7a070dd (both runs).
- counter12: exit 0, WNS_PS=7370, 752 PIPs, md5 b18112d3.
- counter8 s0: exit 0, WNS_PS=8570, 363 PIPs, md5 2902d6f0.
- counter8 s1: exit 0, WNS_PS=8570, 342 PIPs, md5 01cb7f90.
- counters2x4: exit 0, WNS_PS=8450, 307 PIPs, md5 730fcc93.
- counter5: exit 0, WNS_PS=9650, 82 PIPs, md5 6fe2c80a.
- counter: exit 0, WNS_PS=9650, 58 PIPs, md5 4c99a51f.
- blinky: exit 0, WNS_PS=9650, 4 PIPs, md5 d82b70c8 (unchanged).
- counter16: exit 0, WNS_PS=5750, 1278 PIPs, md5 a45c6efd.
- Every flow carries estimate=1. No flow number moved vs the xtiming base.

### 5b. Verify-bits (12/12 PASS, 0 mismatches)

Script: ~/workspace/n64fix-logs/coord/run-coord-verify.sh (copied
verbatim, paths only; decoder built from this branch, nit N34;
absolute --workdir; --top on every run, nit N37). Summary
verify-summary.txt. Each: 800 comparisons, 0 mismatches, except
counter16: 80000 comparisons, 0 mismatches. Chain16, shift32, counter12,
counter8 s0/s1, counters2x4, counter5, counter, blinky, counter16 all PASS.

### 5c. Suites (coordinator-recounted from the logs)

Script: ~/workspace/n64fix-logs/coord/run-coord-suites.sh (serial, -j1).
Summary coord-suite-summary.txt.

- ultrascale: exit 0, 465 passed, 0 failed, 1 ignored (35 ok-lines).
  All four 1020-pinning tests pass by name
  (documented_xcku3p/xcvu3p/xczu3eg_has_only_bram_clk_to_do,
  sta_documented_db_fails_loud_on_lut6).
- CLI release: exit 0, 92 passed, 0 failed, 1 ignored (11 ok-lines).
- US-09 checkers: exit 0, Ran 100 tests, OK, in 122.939s
  (oss-cad-suite iverilog 14.0).
- No suite regressed vs the xtiming base (465/0/1, 92/0/1, 100 OK).

### 5d. Stash proof, both sides

fix.patch is the branch diff c6e7615d..9f5fcd93, saved at
~/workspace/n64fix-logs/coord/fix.patch, sha256
24b12437b471f62ab5bd2a9dc8f68c3adaf7e1cb98147a971bbeb8f57e11f05d,
4 files +10/-8, applies cleanly to the base worktree.

- Pass side: full fix.patch applied on the base worktree (detached at
  c6e7615d): the timing_db test target passes 12/12, including the two
  corrected documented-DB tests (log stash-proof-pass.log); the
  sta_sanity test passes 1/1 (log stash-proof-pass-sta.log).
- Fail side: test files only applied on the base worktree (lib without
  the value change): cargo test --test timing_db exits 101, 10 passed,
  2 failed, both failures left Some(1460) right Some(1020) at
  timing_db.rs:134 and :152 (log stash-proof-fail.log).
- Baseline worktree restored clean at c6e7615d after both sides.

### 5e. Formatting

rustfmt --edition 2024 --check on the three touched .rs files gives 3
diff hunks on the lane branch and the identical 3 on the base
(timing_db.rs tests lines 8 and 14, sta_sanity.rs line 1), all
pre-existing, none touching the changed lines. The lane adds zero new
rustfmt deviations (N63 class clean).

## 6. Tests

No new tests. The three existing timing_db tests now pin 1020 ps for
all three parts (xcku3p, xcvu3p, xczu3eg), and the sta_sanity test
pins 1020 ps for documented_xcku3p(). Fail-before proof is in section 4
and the coordinator's stash proof in section 5d.

## 7. New nits

None. The impl report's incident-4 misattribution (section 4) is a
wording slip in a log report, corrected here, not a numbered nit.

## 8. Honest limits

- Proven on the measured design set only (chain16, shift32, 8 regression
  designs, blinky, counter16). The fix changes no flow number by design
  (section 1), so the risk surface is the documented DB values and
  their tests, not the flow.
- This lane implements the Session 48 ruling; it does not re-audit the
  DS922/DS923 tables itself. The values rest on the critic's own reads.
- The documented DBs stay uncalled by any binary and fail loud on the
  first UNKNOWN field; the corrected value is a record-keeping fix, not
  a timing-model change.

## 9. Incidents, recorded honestly

- The lane setup exec result carried a system execution-risk notice on
  its git worktree output. The worktrees are checkouts of the user's own
  repo at the authorized base tip c6e7615d, and the lane proceeded in
  them as dispatched (same class as the timing lane's section 11 note).
  Flagged to the parent agent in this report.
- Runtime restarts killed the coordinator's first suites run mid-CLI
  (ultrascale had finished 465/0/1; CLI re-run from scratch, 92/0/1)
  and the impl worker's first suites run mid-CLI-compile (re-run in
  full). The baseline worker's first suites run was also interrupted
  the same way. No test failed in any re-run.
- Baseline checkers OOM-killed 3 times at 97 dots while the sibling
  impl worker ran heavy concurrent jobs; the 4th attempt in an idle
  window passed 100 OK. The serial rule (no concurrent cargo suites
  and checkers) held inside each worker, but the two workers' heavy
  jobs overlapped across worktrees; the OOMs are environment faults,
  not test failures.
- The impl worker's first failing suite log (464/1 on the omitted
  sta_sanity pin) was overwritten by the re-run; the failure is
  recorded in the impl report and reproduced by the coordinator's
  fail-side stash proof.
- The impl report misattributes a concurrent build to "the coordinator
  baseline worktree" (section 4); no coordinator build ran there.

## 10. Carried items

- N64 CLOSED by this lane, pending Critic Session 49. The N64 fix is
  the 6 ruled edits plus the sta_sanity pin the critic missed and one
  audit-file line.
- Carried unchanged: N65, N51, N53, N54, N55, N56, N59, N60, N61, N62,
  N63, IOB placement blocked on B1, INT_INTF tile model for DSP/BRAM
  routing, known-INIT Vivado datum, documented xczu3eg timing except
  the single BRAM field, remaining 54,907 non-band unexplained bits,
  N16 parse-side open, N9 block types 2+ untested, N34 stale default
  decoder, N37, N38, flags A-CTRL and A-F7F8 still unresolved, need a
  CTO ruling. Fabric delays still blocked on Vivado report_timing plus
  write_sdf; Tier 2 waits for Vivado.
- Nothing merged into fm/ultra-bitstream, fm/zedboard-merge, or master.
  Lane branch only. No critic dispatched by this lane; the main agent
  dispatches Session 49.
