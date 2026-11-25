# Documented xczu3eg timing lane: REPORT

- Outcome: (A-partial). Exactly one documented xczu3eg value now exists in the
  timing DB: BRAM CLK to DOUT, 1020 ps, from DS925 v1.30 Table 80 TRCKO_DO
  at -2 / 0.85V. Every other xczu3eg field stays Ballpark or UNKNOWN, the
  preliminary DB stays an honestly labeled estimate, and no fabric delay
  (LUT, FF, CARRY8, interconnect, DSP, URAM, IOB) has a documented public
  source. The fabric-delay unblocker is unchanged: Vivado report_timing
  plus write_sdf on a real design.
- Base: fm/ultra-bitgen-n58 at f6a606c8 (N58 tip, closed by Critic Session 47).
- Lane branch: fm/ultra-bitgen-xtiming at a6a7bd14 (local merge commit;
  NOT yet pushed at report write time). Impl branch:
  fm/ultra-bitgen-xtiming-impl at e4c7438e on origin (first impl commit
  64bb8b9c, follow-up commit e4c7438e). Merge commit a6a7bd14 merges
  origin/fm/ultra-bitgen-xtiming-impl into the lane branch.
- Report commit: cfa41a40 (the header commit states it; same two-commit
  pattern as N58 and STADIN).
- Report copy: tools/xtiming/XTIMING_REPORT.md on the lane branch,
  byte-identical to this file.
- Workers: W-BASELINE (agent 32040047-a546-4dd7-8dc5-1f07e9c5fffb),
  W-IMPL (agent 6b84d749-cd83-4301-bfdc-2bdf7efe3fb0),
  W-IMPL-FOLLOWUP (agent 3d0d9dd9-86b7-41de-abe9-395274e499ec).
  Coordinator: agent 9d6981c7-a7cb-4f90-afe6-c51c3e440ebe.
  No critic dispatched by this lane; the main agent dispatches
  Critic Session 48.
- Logs: ~/workspace/xtiming-logs/baseline/, impl/, coord/.
- Zero em dashes in this file.

## 1. Headline

The gap was item 5 of GAPF5_STA_REPORT.md and GOAL.md line 92: no
documented xczu3eg timing. The current DB, preliminary_xczu3eg() in
crates/helion-ultrascale/src/timing_db.rs, is an exact clone-and-rename
of preliminary_xcvu3p() using a generic UltraScale+ AMD forum ballpark
(120 ps LUT + 180 ps one INT hop = 300 ps per logic level), every value
Ballpark, labeled preliminary estimate everywhere (is_estimate(), flow
estimate=1 flag).

The lane first proved (B) BLOCKED with a complete source audit, then the
coordinator caught a hole in that audit (the DS925 Block RAM table itself
had never been read), and a follow-up worker closed the hole by reading
the table directly in two DS925 versions. The final outcome is
(A-partial): one documented value, everything else still blocked, with
the audit file recording exactly what was checked and what was not.

No timing number was invented anywhere in this lane. No value was
transplanted from a sibling datasheet. No Vivado equivalence is claimed;
Tier 2 waits for Vivado to arrive, and no Vivado exists on this VM.

## 2. The one documented value

- Field: bram.clk_to_do in documented_xczu3eg().
- Value: 1020 ps.
- Source: DS925 (Zynq UltraScale+ Data Sheet: DC and AC Switching
  Characteristics), current revision 1.30 English, release 2026-07-09,
  Table 80, parameter TRCKO_DO, Clock CLK to DOUT output (without output
  register), Max, speed grade -2 at 0.85V, second data column.
- Table row, all columns (-3 at 0.90V, -2 at 0.85V, -1 at 0.85V, -2 at
  0.72V, -1 at 0.72V): TRCKO_DO 0.91, 1.02, 1.11, 1.46, 1.53 ns.
  TRCKO_DO_REG (with output register) 0.27, 0.29, 0.30, 0.42, 0.44 ns
  is NOT stored: the DB has no registered-output field.
- Read twice, agreeing value-for-value: DS925 v1.30 docs.amd.com page
  (web per-page numbering shows the table as Table 1) and DS925 v1.23
  (Feb 28, 2023, DigiKey PDF mirror, Table 80, page 65). The code
  citation "DS925 v1.30 Table 80 TRCKO_DO, -2 0.85V" uses the PDF table
  number from v1.23; the v1.30 revision history touches Tables 3, 4, 7,
  8, 9 and PS sections only, so the BRAM table was not renumbered or
  changed between the two revisions. The reasoning is recorded in the
  audit file.
- The coordinator independently read the same table from the v1.23 PDF
  (9,242 lines, TRCKO_DO at line 5587) before dispatching the
  follow-up, and got the identical row. Three independent reads
  (coordinator v1.23, follow-up v1.30, follow-up v1.23) agree exactly.

## 3. Design point: the documented DB is unusable for STA, by design

documented_xczu3eg() follows the exact established pattern of
documented_xcku3p() and documented_xcvu3p(). sta_db_for_part() returns
the preliminary DB for all three parts ("xcku3p", "xcvu3p", "xczu3eg");
the documented DBs are deliberately unusable for STA (they fail loud
on the first UNKNOWN field; sta_routed fails with "timing: LUT6 delay
UNKNOWN in TimingDb"). So the new function is uncalled by any binary
by design, and the flow's timing numbers are unchanged by design. The
lane's gain is one documented value existing in the DB with a pinned
test, matching the other two parts.

The code change (4 files, +452/-15 vs base f6a606c8):

- crates/helion-ultrascale/docs/xczu3eg_timing_sources.md (330 lines,
  new): the full source audit, section by section, with the field-by-field
  map for all 20 DB fields and nit N64.
- crates/helion-ultrascale/src/timing_db.rs (+38/-10): new
  documented_xczu3eg() with exactly the one BRAM value; documented_db()
  now takes the BRAM picosecond value as a parameter; the xcku3p/xcvu3p
  calls still pass 1460 with unchanged citations; preliminary_xczu3eg()
  is behavior-identical, only its doc comment now points at the
  documented value; the module doc comment records the A-partial outcome.
- crates/helion-ultrascale/src/lib.rs: documented_xczu3eg added to the
  re-export, parallel to the other two documented constructors.
- crates/helion-ultrascale/tests/timing_db.rs: three new tests (see
  section 6).

## 4. What the audit proved (DS925, section by section)

Full detail lives in crates/helion-ultrascale/docs/xczu3eg_timing_sources.md.
The short version:

- AC Switching Characteristics: all values are based on the speed
  specifications in the AMD Vivado Design Suite; XCZU3EG is under speed
  specification version 1.30. This is the closest DS925 comes to
  admitting where fabric timing lives: in Vivado speed files, not in
  the data sheet. No delay values.
- DC Characteristics: supply voltages only. No timing values.
- Configuration Switching Characteristics: configuration timing only
  (milliseconds, and nanoseconds for the program pulse), not fabric delays.
- Clock Buffers and Networks: FMAX only (BUFG family 775 MHz at -2 /
  0.85V). A frequency is a period bound, not a pin-to-pin delay.
- Block RAM and FIFO: RETRIEVED. The only documented value (section 2).
- UltraRAM: FMAX, FMAX_ECC_NOPIPELINE, FMAX_NOPIPELINE, TPW, TRSTPW.
  No clock-to-out delay is published, so uram.clk_to_do has no
  documented source in any UltraScale+ datasheet.
- DSP48 Slice: FMAX rows only (FMAX 775 MHz at -2 / 0.85V in DS922).
  No A/B-to-P or clock-to-P delay is published.
- IOB HP/HD: per-standard, per-slew buffer delays (example DS922 at -2 /
  0.85V: DIFF_HSTL_I_12_F gives TINBUF 0.394 ns, TOUTBUF 0.423 ns).
  The DB fields iob.pad_to_fabric and iob.fabric_to_pad are generic with
  no standard or slew qualifier, so no single value from these tables
  can honestly fill them. The flow's designs do not fix one I/O standard.
- Input/Output Delay: IDELAY/ODELAY control parameters only (chain
  resolution 2.1 to 12 ps). A tap resolution is not a fabric path delay.
- Device Pin-to-Pin Output Parameter Guidelines: composite pin-to-pin
  values (TICKOF 5.28 ns at -2 / 0.85V for XCZU3, near clock region),
  not decomposable into DB fields; the tables direct the reader to the
  Vivado timing report for actual pin-to-pin values.
- UG572 / UG574 naming correction: UG574 is Configuration, UG572 is
  Clocking Resources. Neither carries fabric delay values. UG949
  describes clock path tap delays only qualitatively, no picosecond values.
- Structural conclusion: there is no CLB, LUT, flip-flop, CARRY8, or
  interconnect delay table in DS925, and none in DS922, DS923, DS926, or
  DS930 either. Fabric cell delays and routing delays are published
  nowhere in the public AMD documentation set; they exist only in the
  Vivado speed files. This is why the fabric-delay gap cannot be closed
  from documents, and why the unblocker stays Vivado report_timing plus
  write_sdf.

Sibling values observed but NOT used: DS922/DS923/DS926/DS930 all show
TRCKO_DO 1.02 ns at -2 / 0.85V and 1.46 ns at -2 / 0.72V. That
convergence was a hypothesis to check when the DS925 table was read,
not a citation. The follow-up lane read the DS925 table itself, so the
stored value is cited to DS925, not to any sibling.

## 5. Baseline (W-BASELINE, report 2026-11-25 00:21 UTC)

Base matches the N58 numbers exactly:

- Preliminary DB verbatim: LUT6 120, FF ckq 150, setup 80, hold 60,
  CARRY8 CI-CO 40, S-O 100, CYINIT-CO 60, interconnect single 180,
  PIP 50, all ps, source "AMD forum ~0.3ns/logic-level"; DSP, BRAM,
  URAM, IOB, double/quad/long interconnect UNKNOWN; speed grade -2;
  is_estimate() true.
- Build exit 0 (helion 12,955,240 bytes md5 d2e559381a184ff6142de57cc9ddf769;
  helion-bitgen 5,740,032 bytes md5 9127f0beddf78bef311551b30216c749).
- Suites: ultrascale 462/0/1 (35 lines), CLI release 92/0/1 (11 lines),
  checkers 100 OK (oss-cad-suite iverilog 14.0; no system iverilog installed).
- 12 flows all exit 0, estimate=1: chain16 8570/362/4cf99124 twice,
  shift32 8690/289/a7a070dd twice, counter12 7370/752/b18112d3,
  counter8 s0 8570/363/2902d6f0, s1 8570/342/01cb7f90, counters2x4
  8450/307/730fcc93, counter5 9650/82/6fe2c80a, counter 9650/58/4c99a51f,
  blinky 9650/4/d82b70c8, counter16 5750/1278/a45c6efd.
- Verify-bits 12/12 PASS (800/0 each, counter16 80000/0).
- Incident: one VM reboot killed the first combined suites run by
  SIGTERM after about 46 min mid-CLI-compile (partial CLI log preserved
  as suite-cli-release-partial1.log); CLI re-run alone completed 92/0/1.
  No OOM kills. Worktree left clean, no commits.

## 6. Impl workers

### 6a. W-IMPL: outcome (B) PROVE BLOCKED (commit 64bb8b9c)

- Diff 3 files +295/-0, no value changes. Committed audit file
  (first version): every DS925 section checked as in section 4 above;
  DS925 current revision 1.30 English, release 2026-07-09; older mirrors
  v1.23, v1.19, v1.14, v1.12, v1.9.
- Two new tests in timing_db.rs: the audit-presence test (file must exist
  and name DS925, TRCKO_DO, and the Vivado report_timing unblocker) has
  genuine fail-before proof (FAILED file-not-found on stashed base,
  passes on branch); the no-documented-value guard test passes on both
  (regression protection, stated honestly).
- Verification on branch binary: ultrascale 464/0/1, CLI 92/0/1,
  checkers 100 OK; 12 flows exit 0, chain16/shift32 md5s unchanged;
  verify-bits 12/12 PASS; blinky d82b70c8.
- Incidents: runtime restart killed the first ultrascale suite run
  mid-target (re-run in full); the baseline worker ran its CLI suite
  concurrently with the impl re-run (observed via ps, no OOM, serial
  rule broken by overlap); the first checkers run failed 15 tests from
  iverilog missing from PATH (re-ran correctly, 100 OK); pre-existing
  rustfmt deviations left untouched, own hunks fmt-clean.
- New nit N64 (found by this lane, NOT fixed): the existing
  documented_xcku3p() and documented_xcvu3p() BRAM values pin 1460 ps
  cited as "-2 0.85V", but the current DS922/DS923 tables show 1.46 ns
  is -2 at 0.72V and -2 at 0.85V is 1.02 ns (1020 ps). Needs a parent
  ruling before correcting pinned documented values for other parts.
  See section 9.

### 6b. Coordinator catch: the audit had a hole

The (B) audit was complete except for one table: the DS925 Block RAM
TRCKO_DO table itself was never read. One browser.open attempt on the
DS925 landing page failed with a local worker error and was not retried,
and more than ten targeted search queries never surfaced the table text.
The coordinator read it directly from the DS925 v1.23 PDF (DigiKey
mirror, 9,242 lines, TRCKO_DO at line 5587): Table 80, columns -3 at
0.90V, -2 at 0.85V, -1 at 0.85V, -2 at 0.72V, -1 at 0.72V; TRCKO_DO
0.91, 1.02, 1.11, 1.46, 1.53 ns; TRCKO_DO_REG 0.27, 0.29, 0.30, 0.42,
0.44 ns. So -2 at 0.85V is 1.02 ns, independently confirming N64.
Rather than patching the impl branch directly, the coordinator
dispatched a follow-up worker to do the read and the implementation,
so the lane's evidence chain stays worker-produced and checkable.

### 6c. W-IMPL-FOLLOWUP: outcome (A-partial) (commit e4c7438e)

- Re-verified TRCKO_DO 1.02 ns = 1020 ps at -2/0.85V in BOTH versions,
  agreeing exactly: DS925 v1.30 (2026-07-09, docs.amd.com page) and
  DS925 v1.23 (Feb 28, 2023, DigiKey PDF, Table 80, page 65).
- Diff 4 files +196/-54: new documented_xczu3eg() with exactly the one
  BRAM value; documented_db() now takes the BRAM ps as a parameter;
  xcku3p/xcvu3p still pass 1460 with unchanged citations; preliminary DB
  behavior-identical; lib.rs re-export added; audit file updated (BRAM
  table RETRIEVED, verdict A-partial, N64 confirmed by own re-read,
  Vivado unblocker still open for fabric delays).
- New test documented_xczu3eg_has_only_bram_clk_to_do pins 1020 ps,
  exact citation, 19 other fields UNKNOWN, not an estimate. Fail-before
  is a compile error (E0432 unresolved import, exit 101) on the stashed
  base, passes after restore. Full timing_db file 12 passed, 0 failed.
- Suites serial at -j1: ultrascale 465/0/1, CLI 92/0/1, checkers 100 OK
  in 133.606s. Release binaries byte- and md5-identical to the prior
  lane build (helion 12,955,192 bytes md5 c4af44ccd1f29ed5719a1ce810a4c9ea;
  helion-bitgen 5,740,016 bytes md5 fd113e3faebc7ee0a30cdb57f079bb8b).
- Flows: all 12 exit 0, every number unchanged (chain16 8570/362/4cf99124
  twice, shift32 8690/289/a7a070dd twice, blinky d82b70c8). Verify-bits
  12/12 PASS, 0 mismatches.
- Incidents: 3 call sites needed single-line form for edition-2024
  rustfmt; remaining fmt diffs in touched files are pre-existing; suites
  script took about 37 min wall clock, no failures. Zero em dashes.

## 7. Coordinator acceptance (merged lane branch, own runs)

Release build from the lane worktree at merge a6a7bd14, exit 0:
helion 12,955,176 bytes md5 5790323504e27a0a8d25cb2547408a7e,
helion-bitgen 5,740,016 bytes md5 331782f1e7d7e2102408475edec50d39.
The helion size differs 16 bytes from the impl build (worktree effect,
N54 class); helion-bitgen is the same size as the impl build. Log:
~/workspace/xtiming-logs/coord/build-release.log.

### 7a. Flows (12/12 exit 0, numbers identical to baseline and impl)

Scripts: ~/workspace/xtiming-logs/coord/run-coord-flows.sh, summary
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
- Every flow carries estimate=1. No flow number moved vs the N58 base.

### 7b. Verify-bits (12/12 PASS, 0 mismatches)

Script: ~/workspace/xtiming-logs/coord/run-coord-verify.sh (run from the
lane worktree tools/us09v, decoder built from this branch, nit N34;
absolute --workdir; --top on every run, nit N37). Summary
verify-summary.txt. Each: 800 comparisons, 0 mismatches, except
counter16: 80000 comparisons, 0 mismatches. Chain16, shift32, counter12,
counter8 s0/s1, counters2x4, counter5, counter, blinky, counter16 all PASS.

### 7c. Suites (coordinator-recounted from the logs)

Script: ~/workspace/xtiming-logs/coord/run-coord-suites.sh (serial, -j1).
Summary coord-suite-summary.txt.

- ultrascale: exit 0, 465 passed, 0 failed, 1 ignored (35 ok-lines),
  matching the impl follow-up worker exactly (465/0/1).
- CLI release: exit 0, 92 passed, 0 failed, 1 ignored (11 ok-lines).
- US-09 checkers: exit 0, Ran 100 tests, OK, in 120.935s
  (oss-cad-suite iverilog 14.0).
- No suite regressed vs the N58 base (462/0/1, 92/0/1, 100 OK); the
  ultrascale count rose by 3, the three new timing_db tests.

### 7d. Stash proof, both sides

fix.patch is the branch diff f6a606c8..a6a7bd14, saved at
~/workspace/xtiming-logs/coord/fix.patch, sha256
979e1f5de1bc4b12e3b8f9b1b6344c85ed9aef87a55ce09bc006e5daeabe9f09,
4 files +452/-15, applies cleanly to the base worktree.

- Pass side: full fix.patch applied on the base worktree (detached at
  f6a606c8): the full timing_db test target passes 12/12, including the
  new documented_xczu3eg_has_only_bram_clk_to_do (log
  stash-proof-pass-full.log; single-test run in stash-proof-pass.log).
- Fail side: test file only applied on the base worktree (lib without
  documented_xczu3eg): cargo test --test timing_db exits 101 with
  error[E0432]: unresolved import
  `helion_ultrascale::timing_db::documented_xczu3eg` (log
  stash-proof-fail.log). Note: filtering by the test name alone on the
  unpatched base exits 0 (zero tests match), so the fail side must run
  the whole target with the test file present; the coordinator's first
  fail-side attempt hit exactly that trap and was corrected.
- Baseline worktree restored clean at f6a606c8 after both sides.

### 7e. Audit spot-check

The coordinator read DS925 Table 80 directly from the v1.23 PDF
(section 6b) and the follow-up worker read it in v1.30 and v1.23.
The audit file's quoted row (0.91, 1.02, 1.11, 1.46, 1.53 ns) and
column order match all three reads. The N64 claim (1.46 ns is -2 at
0.72V, not -2 at 0.85V) follows directly from the same table's column
headers and is confirmed by the DS925 re-read, not only by the sibling
datasheets.

## 8. Tests

Three new tests in crates/helion-ultrascale/tests/timing_db.rs (12 total
in the file, all pass):

1. Audit-presence test: the audit file must exist and name DS925,
   TRCKO_DO, and the Vivado report_timing unblocker. Fail-before:
   FAILED file-not-found on the stashed base (W-IMPL).
2. No-documented-value guard: preliminary_xczu3eg() must carry no
   Documented delay value. Passes on both base and branch; regression
   protection, stated honestly (W-IMPL).
3. documented_xczu3eg_has_only_bram_clk_to_do: pins 1020 ps, the exact
   citation "DS925 v1.30 Table 80 TRCKO_DO, -2 0.85V", 19 other fields
   UNKNOWN, not an estimate. Fail-before: compile error E0432 on the
   stashed base (W-IMPL-FOLLOWUP, reproduced by the coordinator in
   section 7d).

## 9. New nits

- N64 (found by W-IMPL Nov 24, confirmed by W-IMPL-FOLLOWUP and by the
  coordinator Nov 25, NOT fixed): documented_xcku3p() and
  documented_xcvu3p() pin BRAM CLK to DOUT at 1460 ps, cited as
  "DS922 Table 33 TRCKO_DO, -2 0.85V" and "DS923 Table 33 TRCKO_DO,
  -2 0.85V". The current public DS922/DS923 Block RAM tables show
  TRCKO_DO 0.91, 1.02, 1.11, 1.46, 1.53 ns across the columns -3 at
  0.90V, -2 at 0.85V, -1 at 0.85V, -2 at 0.72V, -1 at 0.72V. So 1.46 ns
  is the -2 grade at 0.72V, and the -2 at 0.85V value is 1.02 ns
  (1020 ps). The 1460 ps value is real, but its cited conditions
  ("-2 0.85V") do not match the column it comes from. This lane did not
  change the value or the pinned tests: correcting a pinned documented
  value for the other two parts needs a parent ruling. The new
  documented_xczu3eg() value applies the correct column: 1020 ps from
  the -2 at 0.85V column.
- N65 (found by the coordinator, process only): the coordinator's flow
  and verify scripts duplicated fixture paths from the impl worker's
  scripts instead of sharing one fixture list, and three paths were
  wrong (counter12, counter, blinky live in critic42-runs/ and the
  worktree examples/, not intcols-logs/impl/). Three coord flows first
  exited 1 on missing files and were re-run from the correct paths with
  the exact expected numbers. Future lanes should keep one fixture list
  per lane, or copy the worker's script verbatim.

## 10. Honest limits

- Proven on the measured design set only (chain16, shift32, 8 regression
  designs, blinky, counter16). The documented value changes no flow
  number by design (section 3), so the risk surface is the new DB
  function and its test, not the flow.
- Exactly one of 20 DB fields is documented. LUT, FF, CARRY8,
  interconnect, DSP, URAM, and IOB delays have no public source and
  stay Ballpark or UNKNOWN. The preliminary DB stays estimate=1.
- The DS925 fabric-delay sections (everything except the BRAM table)
  were checked by search snippets only in the first lane; the follow-up
  lane re-read only the BRAM table directly. The structural conclusion
  (no fabric delay table exists in DS925) rests on the section-by-section
  search plus the AC section's own statement that fabric timing lives in
  Vivado speed files, not on a full-text read of every DS925 page.
- The "Table 80" number in the v1.30 citation comes from the v1.23 PDF;
  the v1.30 web page numbers the same table Table 1 under per-page
  numbering. The audit file records this reasoning; a critic checking
  the v1.30 PDF directly should confirm the table number there.
- N64 is unresolved and bounds the trust in the two older documented
  BRAM values' citations (the values are real; the cited conditions
  are wrong). Nothing in this lane corrects them.

## 11. Incidents, recorded honestly

- The lane setup exec result carried a system execution-risk notice on
  its git worktree output. The worktrees are checkouts of the user's own
  repo at the authorized base tip f6a606c8, and the lane proceeded in
  them as dispatched. Flagged to the parent agent in the coordinator's
  Nov 25 01:51 EST message; recorded here if it recurs.
- Runtime restarts killed one baseline combined suites run (SIGTERM
  after about 46 min), one impl ultrascale suite run mid-target, and
  the coordinator's first release build was killed the same way in the
  N58 lane; all re-ran in full, no test failed.
- The baseline worker's CLI suite ran concurrently with the impl
  worker's suite re-run (serial rule broken by overlap); no OOM, no
  failure, but the overlap is recorded.
- The impl worker's first checkers run failed 15 tests from iverilog
  missing from PATH; re-ran correctly, 100 OK.
- The coordinator's own flow script had wrong fixture paths for
  counter12, counter, and blinky (nit N65); three flows first exited 1
  on missing files and were re-run correctly. The coordinator's first
  verify script called a nonexistent verify.py path (all 12 exited 2);
  rewritten to mirror the impl worker's script exactly (python3 -m
  us09v from tools/us09v) and all 12 passed.
- The coordinator's first fail-side stash attempt filtered by the new
  test name on the unpatched base and exited 0 (zero tests match); the
  correct fail side applies the test file only and runs the whole
  target (exit 101, E0432). Recorded so the next lane does not repeat
  the trap.
- Pre-existing rustfmt deviations in touched files were left untouched;
  the lane's own new code is fmt-clean except where the follow-up worker
  noted 3 call sites reformatted to single-line form for edition-2024
  rustfmt.

## 12. Carried items

- N64: needs a parent ruling before correcting the pinned 1460 ps
  citations for documented_xcku3p() and documented_xcvu3p().
- Fabric delays still blocked: Vivado report_timing plus write_sdf on a
  real design. Vivado is not available on this VM; Tier 2 waits for it.
- N65: one shared fixture list per lane (process fix, no code).
- Carried unchanged: N51, N53, N54, N55, N56, N59, N60, N61, N62, N63,
  IOB placement blocked on B1, INT_INTF tile model for DSP/BRAM routing,
  known-INIT Vivado datum, remaining 54,907 non-band unexplained bits,
  N16 parse-side open, N9 block types 2+ untested, N34 stale default
  decoder, N37, N38, flags A-CTRL and A-F7F8 still unresolved, need a
  CTO ruling. N58 CLOSED by Critic Session 47 (qualified PASS).
- Nothing merged into fm/ultra-bitstream, fm/zedboard-merge, or master.
  Lane branch only. No critic dispatched by this lane; the main agent
  dispatches Critic Session 48.
