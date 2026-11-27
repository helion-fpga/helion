# INT_INTF segbits lane report (second derivation attempt)

Lane branch: `fm/ultra-bitgen-intintf-segbits`, base `9c4917de` (the n76n77n78fix tip).
Worktree: `~/workspace/helion-ultra-bitgen-intintf-segbits`, HEAD `9c4917de`, clean.
Report: `~/workspace/ultra-bitgen-crew/INTINTF_SEGBITS_REPORT.md`
(copy committed byte-identical at `tools/intintf-segbits/INTINTF_SEGBITS_REPORT.md`
on the lane branch).
Date: Nov 26, 2026. Worker: agent b25be4c7-667f-4565-929c-08eeb74e8d1e.
Orchestrator session 20 closes the lane; no merges into fm/ultra-bitstream,
fm/zedboard-merge, or master. No critic dispatched by this lane; critic
dispatch stays with the main agent.

No em dashes anywhere in this file.

## 1. Outcome

The INT_INTF_R_PCIE4 IMUX PIP segbits are UNPINNABLE from the 14-pair
xczu3eg public corpus. No segbits line can be derived for any INT_INTF_R_PCIE4
IMUX PIP, including IMUX16 to IMUXOUT16. The attribution pool is empty: the
only unexplained bits in any INT_INTF frame in any corpus design are the 120
OUTPUTS_ENABLED bits already derived by the Nov 22 INT_INTF lane. No
(frame, bit) exists that appears exactly when an IMUX PIP must be used, so
the derivation criterion cannot be met. B1 stays blocked. Unblocker: the M2
Vivado fuzz job in flight on the cluster (xcku3p, per RECON_NEEDS.md line 131)
or a Q6 Vivado probe. This lane is the license-free corpus path and does not
wait on M2 or duplicate it.

## 2. What this lane is relative to the Nov 22 INT_INTF lane

This lane is an independent re-verification of the Nov 22 INT_INTF lane's
conclusion, not a first discovery. The Nov 22 lane
(`~/workspace/ultra-bitgen-crew/INTINTF_REPORT.md`, branch
fm/ultra-bitgen-intintf, code tip 24629800) already concluded the PIP segbits
are unpinnable from this corpus, derived the OUTPUTS_ENABLED segbits, and
wired them into the emitter. Session 19 dispatched this lane from the IOB
investigation's Option A without citing that report, so the overlap was found
by the worker during the work, not before dispatch. What this lane adds:

- A fresh re-run of the full probe from the new base 9c4917de, reproducing the
  Nov 22 canonical probe logs byte for byte (diff 0 lines on all 14 pairs).
- A fresh decode of P-13 with the current-base helion-bitgen, byte-identical
  (1,630,780 lines, diff 0) to the Nov 22 decode FASM.
- A deeper HDIO constraint analysis (section 5) that the Nov 22 lane did not
  do: the 7 configured HDIO outputs, the INT-side drives into the 8 receiving
  INT_INTF tiles, and why "configured" does not imply "the PIP is selected".
- The HDIO TFFM parallel gap (section 7), noted, not resolved.
- The correction that the derived R_PCIE4 segbits file exists since Nov 22
  (the IOB investigation's section 5 grep missed it because it looked for
  IMUXOUT16, which the file does not contain).

Nothing was derived and no emitter change was made. The worktree is unchanged
from its base except for this report.

## 3. Step 1: B1 reproduced

Commands run in the lane worktree at base 9c4917de:

- `grep -r -l IMUXOUT16 crates/helion-ultrascale/data/` gives zero hits,
  exit 1. No vendored segbits line mentions IMUXOUT16.
- `grep -r -l INT_INTF_R_PCIE4 crates/helion-ultrascale/data/` gives exactly
  two files: `segbits_int_intf_r_pcie4.db` and `xczu3eg_intf_columns.json`
  (a FAR table, not segbits).
- `segbits_int_intf_r_pcie4.db` is 15 lines: a header comment plus exactly 2
  features, `INT_INTF_R_PCIE4.OUTPUTS_ENABLED.0` (`!01_21 !02_27 !03_24`)
  and `INT_INTF_R_PCIE4.OUTPUTS_ENABLED.1` (`01_21 02_27 03_24`), 0 IMUX
  lines. Its own header states no INT_INTF PIP bit is set in any corpus
  design, so no PIP line is derived there.
- Prjuray-db harness reference copy
  (`~/workspace/ultra-bitgen-crew/harness/reference/prjuray-db/zynqusp/`):
  `grep -r -l IMUXOUT16` gives zero hits, exit 1;
  `grep -r -l INT_INTF_R_PCIE4` gives only `tilegrid.json`. The only INT_INTF
  segbits file in that copy is `segbits_int_intf_left_term_pss.db`, which
  carries OUTPUTS_ENABLED plus 48 DELAYWIRE PIPs and 0 IMUXOUT lines.
  Scope note, added by the orchestrator: in the fuller checkout at
  `~/workspace/prjuray-db-ref/zynqusp/`, IMUXOUT16 does appear in
  `tile_types/tile_type_INT_INTF_R_PCIE4.json` and in `tileconn.json`.
  Those are connectivity definitions (PIP existence, src/dst wires), not
  segbits (bit positions). No segbits line for any IMUX PIP exists in either
  copy. The operative B1 fact stands in both.
- Emitter still fails loud: `cargo test -j2 -p helion-ultrascale --test
  device_file_xczu3eg` gives 9 passed, 0 failed, including
  `xczu3eg_intintf_pip_still_fails_unknown_feature`, which expects
  `BitgenError::UnknownFeature` for tile `INT_INTF_R_PCIE4_X9Y0`, feature
  `PIP.IMUXOUT16.IMUX16`. Log: `~/workspace/intintf-segbits-logs/test-devicefile.log`.
- Lib suite at this base: `cargo test -j2 -p helion-ultrascale --lib` gives
  262 passed, 0 failed, 0 ignored. Log:
  `~/workspace/intintf-segbits-logs/test-lib.log`.
  Loose end from the worker, resolved by the orchestrator: the worker saw a
  `pgrep` hit for the cargo test command after the log already showed
  exit 0 with 262 passed. No cargo test process remains now, so the log
  result stands on its own.

The tile_type JSON confirms the PIP structure the corpus would have to pin:
176 PIPs in `tile_type_INT_INTF_R_PCIE4.json`, by src/dst wire class:
96 src IMUX (48 IMUX to IMUXOUT direct, 48 IMUX to DELAYWIRE_PCIE),
48 DELAYWIRE_PCIE to IMUXOUT, 32 LOGIC_OUTS_R to LOGIC_OUTS_L. Each
IMUXOUTn has two candidate drivers (IMUXn direct, DELAYWIRE_PCIE_n) and
each IMUXn has two candidate consumers (IMUXOUTn, DELAYWIRE_PCIE_n).
FASM PIPs are sink-first (`PIP.<dst>.<src>`, confirmed in
`crates/helion-ultrascale/src/int_pip.rs`), so the IOB lane's target reads
`PIP.IMUXOUT16.IMUX16`.

## 4. Step 2: fresh corpus probe

Probe: `intintf_probe`, built from this base
(`crates/helion-ultrascale/src/bin/intintf_probe.rs`), run over all 14
corpus pairs at `~/workspace/ultra-bitgen-crew/decoder-validation/bits/`.
Logs in `~/workspace/intintf-segbits-logs/`, `probe-run.log` plus
`probe-run2.log`, script `run-probe.sh`. All 14 fresh probe logs are
byte-identical (diff 0 lines each) to the Nov 22 canonical probe logs in
`~/workspace/intintf-logs/`.

### Per-pair table

| pair | unexplained | R_PCIE4 attributed | HDIO PIP lines | HDIO IOSTANDARD_OUT | INT_X9 IMUX_E drive tiles |
|---|---|---|---|---|---|
| P-11 resizer_ultra96 | 2166 | 0 | 0 | 0 | none |
| P-13 cnvW1A1-ultra96 | 5673 | 24 | 7 | 7 | Y120, Y126, Y154, Y158, Y168, Y172, Y174, Y178 |
| P-14 cnvW1A2-ultra96 | 6174 | 24 | 7 | 7 | same 8 tiles |
| P-15 cnvW2A2-ultra96 | 6519 | 24 | 7 | 7 | same 8 tiles |
| P-16 lfcW1A1-ultra96 | 3987 | 24 | 7 | 7 | same 8 tiles |
| P-17 lfcW1A2-ultra96 | 4075 | 24 | 7 | 7 | same 8 tiles |
| P-23 cnv-w1a1 | 2528 | 0 | 0 | 0 | none |
| P-28 xv2Filter2DDilate | 1830 | 0 | 0 | 0 | none |
| P-40 cnv-w1a2 | 2658 | 0 | 0 | 0 | none |
| P-41 cnv-w2a2 | 2620 | 0 | 0 | 0 | none |
| P-42 tfc-w1a1 | 576 | 0 | 0 | 0 | none |
| P-43 tfc-w1a2 | 1341 | 0 | 0 | 0 | none |
| P-44 tfc-w2a2 | 1458 | 0 | 0 | 0 | none |
| P-45 unsw_nb15-mlp-w2a2 | 13302 | 0 | 0 | 0 | none |

Total unexplained bits 54,907, exactly matching the N7_PORT_REPORT table
for every pair. The per-pair counts above were re-verified by the
orchestrator from the fresh probe logs (`grep -h "^# unexplained bits:"
*.probe.log`, sum 54,907).

### INT_INTF attribution

- INT_INTF_R_PCIE4 attributed bits: 24 per BNN design (P-13 through P-17),
  0 in the other 9 designs. Distinct positions: 8 tiles (X9Y120, X9Y122,
  X9Y154, X9Y158, X9Y168, X9Y172, X9Y176, X9Y178) times (frame 1, bit 21),
  (frame 2, bit 27), (frame 3, bit 24), each in all 5 designs. Total 120,
  all OUTPUTS_ENABLED.1 positions. No other position appears even once.
  (A grep for INT_INTF_R_PCIE4 in a BNN probe log returns 25 lines: the 24
  bit lines plus 1 summary line.)
- INT_INTF_LEFT_TERM_PSS attributed bits: 0 in every pair. All its bits
  are explained by the public segbits file.
- Gap bits total 24,404, in columns 57 (7682), 42 (6537), 88 (4313),
  11 (3562), 82 (1944), 101 (356), 30 (10). Zero gap bits in columns 0,
  29, 75 (the INT_INTF columns). So unwindowed INT_INTF tiles, including
  X9Y0 (the IOB lane's target tile, which has no tilegrid window), also
  carry no set bit in any design.
- Window counts verified fresh: 95 INT_INTF_R_PCIE4 windows, 275 INT_INTF
  windows total (`windows-rpcie4.log` 96 lines with header,
  `windows-intintf.log` 276 lines with header).

## 5. HDIO constraint, examined in full

This lane went further than the Nov 22 lane. The 5 BNN designs each
configure the same 7 HDIO outputs: identical 7 OPFF PIP lines in every
design (verified by the orchestrator: the sorted HDIO PIP line sets have
identical md5 across all 5 BNN FASM files) plus 7 identical
IOSTANDARD_OUT lines (same md5 check). The other 9 designs have 0 HDIO
lines. The 7 PIP lines:

- HDIO_BOT_RIGHT_X9Y120: pairs 14 (OPFFM) and 22 (OPFFS).
- HDIO_TOP_RIGHT_X9Y150: pairs 15, 22, 36, 42, 43 (OPFFS except pair 42,
  OPFFM).

Fresh decode of P-13 with the current-base helion-bitgen
(`P-13-fresh.fasm` in `~/workspace/intintf-segbits-logs/`, console log
`decode-p13.log`) is byte-identical (1,630,780 lines, diff 0) to the
Nov 22 decode FASM. Note: `decode-p13.log` itself is the console log
(1,636,455 lines) and is not the file compared; the comparison is on the
FASM output. The decode confirms the INT side drives IMUX_E0 and IMUX_E16
eastward from 8 INT_X9 tiles (Y120, Y126, Y154, Y158, Y168, Y172, Y174,
Y178; E0 on all but Y126 and Y174) into the adjacent INT_INTF tiles in all
5 BNN designs. The orchestrator verified the tile set is identical in all
5 BNN FASM files; the driving source wire varies per design. Tileconn,
verified by grep in `tileconn.json`:

- [INT, INT_INTF_R_PCIE4] delta [1,0] pairs IMUX_E16 with IMUX16.
- [HDIO_BOT_RIGHT, INT_INTF_R_PCIE4] delta [-1,0] and
  [HDIO_TOP_RIGHT, INT_INTF_R_PCIE4] delta [-1,-2] pair OPFFM_D1 with
  IMUXOUT16.

Yet no INT_INTF PIP bit appears in any of the 8 receiving tiles in any of
the 5 designs.

Why this does not yield a derivation: only 1 of the 7 configured HDIO
outputs (BOT pair 14, IMUXOUT16) has a matching INT-side drive on its
tile; the other 6 have no INT-side drive on the adjacent INT tile
(INT_X9Y150 has 0 PIPs; Y148/Y152 likewise show no IMUX_E drives), and 7
of the 8 eastward drives feed INT_INTF tiles with no HDIO neighbor at all.
So "configured" does not imply "the PIP is selected" for any given tile.
The OUTPUTS_ENABLED tiles (Y120, Y122, Y154, Y158, Y168, Y172, Y176,
Y178) and the drive tiles (Y120, Y126, Y154, Y158, Y168, Y172, Y174,
Y178) overlap in 6 and differ in 2 each, so OUTPUTS_ENABLED is not tied
to which tiles receive IMUX signals either. The cnv-w1a1 .hwh shows the
block design exposes no external PL ports (0 EXTPORTS; all accelerator
ports are AXI to the PS), so these HDIO outputs are platform
infrastructure with no observable pin behavior in the corpus.

## 6. Why no derivation, including no zero-bit line

The corpus is consistent with the direct IMUX to IMUXOUT PIP carrying no
configuration bit (a default selection), but it does not prove it, for
the reasons in section 5: the selection cannot be tied to any specific
tile, and the majority of the drives do not correspond to any configured
output. A zero-bit encoding is still an encoding claim, and the standing
rule forbids emitting guessed bits. The derivation criterion requires a
(frame, bit), and none exists.

Open question for a future lane with Vivado data, NOT a conclusion of
this lane: is the IMUX to IMUXOUT PIP bitless by default, with the
DELAYWIRE path (the only PIP class the public LEFT_TERM_PSS segbits file
carries, 48 one-bit lines) as the configured alternative? The public
LEFT_TERM_PSS file carries only the 48 IMUX_FT0 to DELAYWIRE_XIPHY PIPs;
the direct IMUX to IMUXOUT PIPs and the DELAYWIRE to IMUXOUT PIPs are
both absent from it. Their absence could mean they are bitless, or that
prjuray never fuzzed them. This lane cannot distinguish the two, and
does not try.

## 7. Parallel gap, noted not resolved: HDIO TFFM PIP

The HDIO TFFM PIP is likewise absent from the public HDIO segbits DB:
0 TFFM lines against 6 OPFFM lines in each of
`segbits_hdio_bot_right.db` and `segbits_hdio_top_right.db`
(`grep -c TFFM` 0, `grep -c OPFFM` 6, both files). Tileconn pairs
HDIO_LOGICPAIR_14_TFFM_D1 with IMUXOUT0, so the tristate control for a
configured HDIO output has the same shape of problem as the IMUX PIP:
its select PIP has no public segbits line, and 0 TFFM lines appear in any
decoded corpus FASM (0 in all 5 BNN designs). This is a parallel gap for
a future lane, not a resolved finding of this lane.

## 8. Acceptance scope

Full acceptance suites (CLI suite, US-09 checkers, 12 flows, 12
verify-bits) were NOT re-run. Per the dispatch brief, the full acceptance
list applies to the derivation-success path; this lane took the negative
path (proof table, no emitter change), so only the targeted PIP test and
the lib suite were run. No code changed, so no flow number can have
moved; the base 9c4917de suites were already critic-verified in Session 55.

## 9. Worktree-setup execution-risk notice

The lane worktree setup carried a system execution-risk notice; the
worktree is a checkout of the user's own repo at the authorized base tip
9c4917de, and the lane proceeded as dispatched. Recorded here per the N64
lane precedent.

## 10. Evidence index

- Logs: `~/workspace/intintf-segbits-logs/` (14 fresh `P-*.probe.log`,
  `probe-run.log`, `probe-run2.log`, `run-probe.sh`, `build-probe.log`,
  `build-bitgen.log`, `test-lib.log`, `test-devicefile.log`,
  `windows-intintf.log`, `windows-rpcie4.log`, `decode-p13.log`,
  `P-13-fresh.fasm`).
- Canonical Nov 22 probe logs: `~/workspace/intintf-logs/` (all 14 diff 0).
- Nov 22 decode FASM: `~/workspace/n7-port-logs/validate/corpus-decode/`.
- Prior lane report: `~/workspace/ultra-bitgen-crew/INTINTF_REPORT.md`.
- Worktree: `~/workspace/helion-ultra-bitgen-intintf-segbits`
  (branch fm/ultra-bitgen-intintf-segbits, HEAD 9c4917de, clean before
  this report commit; only new files are the logs and scripts under
  `~/workspace/intintf-segbits-logs/` outside the repo, plus build
  artifacts under the worktree `target/` directory).
- Worker session: `/home/hatch/agents/agent-b25be4c7-667f-4565-929c-08eeb74e8d1e/sessions/b25be4c7-667f-4565-929c-08eeb74e8d1e.jsonl`,
  final message seq 244 (timeout progress update; technical finding
  complete, deliverables completed by orchestrator session 20).

## 11. What remains

B1 stays blocked with its named unblocker: the M2 Vivado fuzz job in
flight on the cluster (xcku3p) or a Q6 Vivado probe. The bitless-default
hypothesis (section 6) and the HDIO TFFM parallel gap (section 7) are
open questions for future lanes with Vivado data. The IOB placement lane
remains blocked on B1.
