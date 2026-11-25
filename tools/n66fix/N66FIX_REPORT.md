# N66 fix lane: correct the BRAM docs from 1460 to 1020 ps

Report commit: 7b4f2ee502cea58304e17f56a6fa14eeb9446776

## 1. Outcome

N66 is fixed. The two data docs that still described 1460 ps as the current
BRAM CLK to DOUT value now state 1020 ps at -2 / 0.85V, with the "-2 0.85V"
citations kept. The fix also corrects the column-mapping error that produced
1460 in the first place, and the sibling DS922/DS923 values quoted in the
same passages, which were read from the same wrong column. Doc-only lane:
no code, test, or DB value changed, and no flow number moves, by design.

- Base: fm/ultra-bitgen-n64fix at 80f3d4dd.
- Lane branch: fm/ultra-bitgen-n66fix at 64e91643 (merge of impl 0241fd95).
- Impl branch: fm/ultra-bitgen-n66fix-impl at 0241fd95 (local-only, reachable
  as an ancestor of the pushed lane branch).
- Diff: 2 files, +40/-23, 0 em dashes. Nothing merged into
  fm/ultra-bitstream, zedboard-merge, or master.

## 2. What N66 was

Critic Session 49, nit N66 (doc only, non-blocking): two data docs still
described 1460 ps as the current BRAM CLK to DOUT value in present tense,
after the N64 fix corrected the DBs to 1020 ps:

- `crates/helion-ultrascale/data/TIMING_XCKU3P.md` lines 76-81 and 113-117
- `crates/helion-ultrascale/data/TIMING_VERIFICATION_SESSION18.md` lines 16
  and 45

Root cause, confirmed by the Session 49 critic and by my own reads below:
the DS922/DS923 Table 33 row reads 0.91, 1.02, 1.11, 1.46, 1.53 ns across
the five numeric columns -3 at 0.90V, -2 at 0.85V, -1 at 0.85V, -2 at
0.72V, -1 at 0.72V. Session 18 read the fourth numeric column (1.46 ns)
as -2 at 0.85V; that column is -2 at 0.72V. The -2 at 0.85V column is the
second numeric column, value 1.02 ns = 1020 ps.

## 3. The fix

Two commits on the impl branch.

### 3a. Commit 7450c0fa: the N66 corrections

`TIMING_XCKU3P.md`:

- DS922 section: the column mapping is rewritten (five columns, -2 at
  0.85V is the second, not the fourth, with the misreading stated plainly).
  TRCKO_DO at -2 / 0.85V corrected from 1.46 ns = 1460 ps to
  1.02 ns = 1020 ps, `bram.clk_to_do` corrected to 1020 ps with the
  "-2 0.85V" citation kept, plus a note that 1.46 ns is the -2 at 0.72V
  value.
- DS923 section: TRCKO_DO 1.46 ns to 1.02 ns, `bram.clk_to_do` 1460 ps to
  1020 ps, with the same -2 / 0.72V note.

`TIMING_VERIFICATION_SESSION18.md`:

- Heading "TRCKO_DO 1.46 ns: CONFIRMED" to
  "TRCKO_DO 1.02 ns: CORRECTED (session 18 read 1.46 ns)".
- The column paragraph now states the corrected mapping, names the
  misreading, and records "store 1020 ps (corrected from 1460 ps by the
  N64 fix lane, 2026-11-25...)".
- The "exactly one value" bullet now reads "(BRAM CLK to DOUT 1020 ps,
  corrected from 1460 ps...)".
- The doc stays readable as history; the quoted row itself was correct and
  is unchanged.

`crates/helion-ultrascale/docs/xczu3eg_timing_sources.md` line 319 was
left untouched, per the Session 49 ruling: it is historical description of
the timing lane, not part of N66. The git diff on that file is empty.

### 3b. Commit 0241fd95: sibling values from the same wrong column

The impl worker flagged an open question: TRCKO_DO_REG 0.42 ns,
FMAX_WF_NC 585 MHz, FMAX_RF 510 MHz, TPW 577 ps, UltraRAM TPW 800 ps,
and the DSP FMAX values were all read in session 18 under the same wrong
mapping and left as-is. I resolved it with my own reads of the DS922 and
DS923 docs.amd.com pages on 2026-11-25 (Block RAM and FIFO, UltraRAM, and
DSP48 Slice Switching Characteristics). Every one of those values was a
fourth-column value, that is, -2 at 0.72V, not -2 at 0.85V. Corrected in
the same two files, second-column (-2 at 0.85V) values:

- DS922 BRAM: TRCKO_DO_REG 0.42 ns to 0.29 ns, FMAX_WF_NC 585 MHz to
  738 MHz, FMAX_RF 510 MHz to 637 MHz, TPW 577 ps to 542 ps.
- DS922 UltraRAM: FMAX 500 MHz to 600 MHz, FMAX_ECC_NOPIPELINE 312 MHz
  to 400 MHz, FMAX_NOPIPELINE 404 MHz to 500 MHz, TPW 800 ps to 700 ps.
- DS922 DSP: FMAX 644 MHz to 775 MHz, FMAX_NOPIPELINEREG 323 MHz to
  410 MHz.
- DS923 BRAM: TRCKO_DO_REG 0.42 ns to 0.29 ns (TRCKO_DO already 1.02 ns
  from commit 7450c0fa). Note: DS923 FMAX_WF_NC at -2 / 0.85V is 737 MHz,
  not the DS922 738 MHz; the doc cites no DS923 FMAX value, so nothing
  else changes there.
- Session 18 cross-check paragraph: TRCKO_DO_REG 0.42 ns to 0.29 ns,
  FMAX_WF_NC 585 MHz to 738 MHz, TPW 800 ps to 700 ps, with the
  fourth-column note.

Each corrected passage notes the old fourth-column values and where they
belong, so a reader can see what changed and why.

## 4. Re-grep: nothing beyond the two named docs

W-IMPL grepped the whole tree for `1460`, `1.46`, and `TRCKO_DO`
(138 hits, raw output in `~/workspace/n66fix-logs/impl/regrep-raw.txt`).
No additional file describes 1460 as the current BRAM CLK to DOUT value.
Excluded hits, each with a reason in the impl report: iCE40
`span_aliases.txt` wire indices; date-math `1460` constants in `zip.rs`,
both `bitstream.rs` copies, and vendored Rune date code; `1.46` SVG-path
fragments in vendored Rune icon catalogs; `timing_db.rs` and its tests
(already 1020 from the N64 lane); `xczu3eg_timing_sources.md` (already
correct, line 293 records the N64 correction); `XTIMING_REPORT.md` and
`N64FIX_REPORT.md` (historical lane reports that already state the
correct value); `GAPF5_STA_REPORT.md` / `EMITTER_XCZU3EG_REPORT.md`
(TRCKO_DO named only, no value claim). Remaining `1460` hits in the two
fixed docs are only the words "corrected from 1460 ps", that is, history,
not a current-value claim.

## 5. Acceptance

### 5a. Baseline (W-BASELINE, base tip 80f3d4dd)

Stale wording confirmed by quotation in the baseline report. Release build
exit 0 (cold, 70m 31s under a concurrent sibling build); helion
13,047,648 bytes md5 47e6017e20054bdf35a1284b42b15328, helion-bitgen
5,740,048 bytes md5 92efe2e331d2799e0be40af875597e45. Suites ultrascale
465/0/1, CLI release 92/0/1, checkers 100 OK in 129.019s. 12/12 flows
exit 0, 12/12 verify-bits PASS. Caveat, recorded in the baseline report:
`cargo test --release -p helion-cli` rebuilt `target/release/helion`
mid-run, so flows and verify-bits ran against that rebuilt binary; source
never changed and every number matches the N64 lane exactly.

### 5b. Impl (W-IMPL, branch 7450c0fa)

Scoped release build exit 0 in 22m 27s. Suites ultrascale 465/0/1,
CLI release 92/0/1, checkers 100 OK in 115.023s (4th attempt, see
section 6). 12/12 flows exit 0, 12/12 verify-bits PASS. The impl
acceptance ran before commit 0241fd95; the follow-up commit touches only
the same two .md files, so it cannot move a flow number, and the
coordinator acceptance below re-verifies the merged branch in full.

### 5c. Coordinator (merged lane branch 64e91643, own runs)

Release build exit 0 in 11m 38s; helion 12,955,176 bytes md5
8f23f466cea0250f8b70c9e33ef73c21, helion-bitgen 5,740,016 bytes md5
0d3d5e69c76bc61eefaaebd648c5e32a. (N54 recurs: the coordinator binary is
byte-identical in size to the N64 coordinator build; the baseline binary
differs, as expected for a different build command. Behavior below is
what certifies the lane.)

Suites: ultrascale 465/0/1 (35 ok-lines), CLI release 92/0/1
(11 ok-lines), checkers 100 OK in 122.008s (oss-cad-suite iverilog 14.0;
system iverilog absent, N49 recurs).

12/12 flows exit 0, numbers identical to baseline, impl, and the N64 lane:

- chain16 8570/362/4cf99124 twice
- shift32 8690/289/a7a070dd twice
- counter12 7370/752/b18112d3
- counter8 seed 0 8570/363/2902d6f0, seed 1 8570/342/01cb7f90
- counters2x4 8450/307/730fcc93
- counter5 9650/82/6fe2c80a
- counter 9650/58/4c99a51f
- blinky 9650/4/d82b70c8 unchanged
- counter16 5750/1278/a45c6efd

12/12 verify-bits PASS (800/0 each, counter16 80000/0), decoder built
from this branch, absolute --workdir, --top on every run.

I read the branch diff myself: only the two intended .md files change
(+40/-23 over the base), no .rs file is touched, so there are zero code
hunks for a rustfmt check, and no remaining present-tense 1460-as-current
BRAM wording in either doc. The audit file diff is empty.

## 6. Incidents

1. W-IMPL first started a full-workspace release build; after 60+ min it
   was still compiling GUI/zb crates (447 crates) against the N64 lane's
   114 compiles in 11m 14s. Killed it; the scoped build matching the N64
   lane was used instead. A concurrent baseline-lane build shared the CPUs.
2. W-IMPL checkers were OOM-killed three times (exit 137): concurrent
   baseline-lane build/checkers/verify on the 6 GiB cell, plus the impl
   worker's own error in attempts 2-3, running without `oss-cad-suite/bin`
   on PATH so iverilog was missing and 10 tests errored deterministically.
   Attempt 4, with the correct PATH after the baseline lane finished,
   passed cleanly. N60 recurs.
3. The impl worker's open question on the sibling DS922/DS923 values is
   resolved in this lane by my own page reads (section 3b), not carried.

## 7. New nits

None.

## 8. Carried

N65, N51, N53, N52, N54, N55, N56, N59, N60, N61, N62, N63. IOB placement
stays blocked on B1. INT_INTF tile model, known-INIT datum, documented
xczu3eg timing except the single BRAM field, 54,907 non-band bits, N16,
N9, N34, N37, N38, A-CTRL, A-F7F8. Fabric delays still blocked on Vivado
report_timing plus write_sdf. Tier 2 waits for Vivado. No critic
dispatched by this lane; the main agent dispatches Critic Session 50.

## 9. Honest limits

- Doc-only lane. No code, test, or DB value changed; nothing in the flow
  can move, and the acceptance above confirms nothing did.
- Proven on the measured design set only.
- The sibling-value corrections rest on my own reads of the DS922/DS923
  docs.amd.com pages on 2026-11-25 and the critic's Session 49 DS922/DS923
  reads for TRCKO_DO. No Vivado check exists for any of these values.
