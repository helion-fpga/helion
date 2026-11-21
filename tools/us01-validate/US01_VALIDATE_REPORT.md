# US-01 Validation Report (W-VALIDATE)

- Lane: `fm/ultra-bitgen-us01-mine`, validation branch `fm/ultra-bitgen-us01-validate`
- Date: Sat 2026-11-21
- Worker: W-VALIDATE
- Base commit: `f46295a8542dedddda864cc917c44814d8d636a3` (lane branch tip)
- Report path: `~/workspace/ultra-bitgen-crew/US01_VALIDATE_REPORT.md`
- Scripts: `tools/us01-validate/` on the validation branch (new: `consistency.py`, `coverage.py`, `di-extract-probe.py`, `run-tieoff-all.sh`, `run-consistency-all.sh`; reused unchanged: `tools/us01-char/check-tieoff.py`, `carry-live.py`)
- Logs: `~/workspace/us01-validate-logs/`
- Rules: license-free only, no Vivado, no vendor data beyond what is already public in the repo and corpus. No em dashes anywhere in this report.

## Headline verdicts

1. Held-out tie-off: PASS. On P-40 (FINN) and P-28 (PYNQ-ComputerVision), pairs not used to develop the VCC-default rule, every needed LUT input pin is real-driven or VCC-tied, zero problems. P-40: 122,746 real-driven, 2,036 VCC-tied, 0 problems. P-28: 48,430 real-driven, 615 VCC-tied, 0 problems.
2. Full 14-pair consistency: ZERO contradictions. 12,335,283 decoded PIPs checked against `tile_types` and `tileconn`; all 12,335,283 agree on every check (C1 wire exists, C2 wire exists, C3 edge exists, C4 BEL-pin stitch agrees). 3,357,492 BEL-pin agreements. Not one decoded PIP or pin connection contradicts the static DBs.
3. DI-fed LUT semantics: SETTLED functionally. `CARRY8.DI{i}.DI{i}` means DI[i] is the same LUT O6 net as S[i]. The extractor fix is a one-line change in `_carry_di`, plus the VCC-default rule applied first. Proved by direct probe of the real extractor code on a real slice (section 4).
4. VCC-default rule re-check: HOLDS across all 14 pairs. 1,587,046 needed pins total: 1,559,317 real-driven, 27,729 VCC-tied, 0 truly undriven, 0 baseline-real-only, 0 problems.
5. Coverage rollup: the corpus validates the CLE map almost completely at pin-class granularity (210 of 214 CLE pins wire-verified, 98.1 percent) and validates none of the DSP/BRAM map (0 of 1,168 DSP/BRAM pins exercised at all). Overall: 210 of 1,382 pins wire-verified (15.2 percent). The map itself is complete in tileconn; the corpus only corroborates the CLE part of it.

## 1. Held-out tie-off checks

Method: `tools/us01-char/check-tieoff.py`, reused unchanged from W-CHAR, run on the fresh decodes in `~/workspace/us01-char-logs/decoded/` against the raw prjuray baselines in `~/workspace/ultra-bitgen-crew/decoder-validation/baseline/`. The VCC-default rule was developed on P-11 (resizer) and P-13 (BNN-PYNQ). The held-out pairs are from the other two design families: P-40 is FINN, P-28 is PYNQ-ComputerVision xv2Filter2DDilate. Neither pair was used in any rule development.

Command (per pair):

```
python3 tools/us01-char/check-tieoff.py ~/workspace/us01-char-logs/decoded/<pair>.fasm \
  ~/workspace/ultra-bitgen-crew/decoder-validation/baseline/<pair>.fasm
```

Batch: `tools/us01-validate/run-tieoff-all.sh`, log `~/workspace/us01-validate-logs/tieoff-all.log`, per-pair JSON `tieoff-<pair>.json`.

| Pair | Needed pins | Real-driven (decoded) | VCC-tied (baseline) | Baseline-real-only | Truly undriven | No INT partner | Problems |
| --- | --- | --- | --- | --- | --- | --- | --- |
| P-40 | 124,782 | 122,746 | 2,036 | 0 | 0 | 0 | none |
| P-28 | 49,045 | 48,430 | 615 | 0 | 0 | 0 | none |

Held-out verdict: the VCC-default rule generalizes. Zero problems on both held-out pairs, from two families the rule never saw during development. The map, as built from tileconn alone, is right for the tie-off question.

## 2. Full 14-pair consistency

Method: new script `tools/us01-validate/consistency.py`. For every PIP line in every decoded FASM pair it checks, against the static public DBs:

- C1: the dst wire exists in the tile type's wire list (`tile_types/tile_type_<type>.json`).
- C2: the src wire exists in the same wire list.
- C3: the (dst, src) edge exists in the tile type's PIP list.
- C4: for INT-tile PIPs whose dst or src is a BEL-facing wire (`IMUX_*`, `LOGIC_OUTS_*`, `CTRL_*`), tileconn must stitch that wire to a CLE site-pin wire (`CLE_CLE_[LM]_SITE_0_<pin>`). This is the BEL-pin agreement check: every decoded BEL-pin connection must agree with the tileconn stitch for that pin.
- C5 (informational only): whether a PIP wire appears in tileconn's (tile type, wire) universe. tileconn lists only inter-tile stitch wires; most INT-internal wires live in tile_types only, so C5 is a scope observation, never a contradiction.

A contradiction is a decoded PIP failing C3 or C4: a PIP edge or a pin connection the static DBs say cannot exist. Distinct failing edges are listed individually; there are none.

Two checker findings, both resolved in the script and recorded here rather than hidden:

- Bidirectional PIPs. The first run flagged 633 lines in P-28 (9 distinct edges), all of the form `PIP.BYPASS_E8.INT_NODE_IMUX_5_INT_OUT0.REV`. These are not contradictions. prjuray writes bidirectional PIPs with a `.FWD`/`.REV` suffix on the source wire in FASM, and the static DB stores the same edge with `<<->>` and `is_directional` 0 (example: `INT.INT_NODE_IMUX_5_INT_OUT0<<->>BYPASS_E8` in `tile_type_INT.json`). The raw baseline contains the identical 633 suffixed lines, so the decoder reproduces prjuray exactly here. The checker strips a trailing `.FWD`/`.REV` before the C2/C3 checks and counts the lines separately (18,890 across the corpus). The single RCLK edge of the same class (`CLK_HDISTR_R0` from `CLK_HDISTR_L0.FWD`) is the same phenomenon.
- Site PIPs. SLICE PIP lines name a site (`SLICE_X10Y102`), not a tile. The first run counted 78 of them as unknown tiles. The checker now resolves the site to its tile through tilegrid before the C1-C3 checks; the CIN site PIP edge (`CLE_CLE_L_SITE_0_CIN_PIN` to `CLE_CLE_L_SITE_0_CIN`) exists in the CLE tile types, as the char report states.

Command (per pair):

```
python3 tools/us01-validate/consistency.py ~/workspace/us01-char-logs/decoded/<pair>.fasm \
  ~/workspace/us01-validate-logs/consistency-<pair>.json
```

Batch: `tools/us01-validate/run-consistency-all.sh`, log `~/workspace/us01-validate-logs/consistency-all.log`.

| Pair | PIPs checked | Fully agree | C4 BEL agreements | Bidir-suffix lines | Contradictions |
| --- | --- | --- | --- | --- | --- |
| P-11 | 197,237 | 197,237 | 58,070 | 324 | 0 |
| P-13 | 923,459 | 923,459 | 255,399 | 1,289 | 0 |
| P-14 | 1,478,790 | 1,478,790 | 372,945 | 1,927 | 0 |
| P-15 | 1,237,600 | 1,237,600 | 337,499 | 1,945 | 0 |
| P-16 | 851,131 | 851,131 | 225,108 | 1,083 | 0 |
| P-17 | 1,539,214 | 1,539,214 | 385,074 | 1,913 | 0 |
| P-23 | 765,740 | 765,740 | 220,595 | 1,209 | 0 |
| P-28 | 381,563 | 381,563 | 110,884 | 633 | 0 |
| P-40 | 1,034,901 | 1,034,901 | 287,253 | 1,521 | 0 |
| P-41 | 1,097,818 | 1,097,818 | 301,872 | 1,651 | 0 |
| P-42 | 405,094 | 405,094 | 117,657 | 735 | 0 |
| P-43 | 921,912 | 921,912 | 252,918 | 1,868 | 0 |
| P-44 | 920,107 | 920,107 | 258,206 | 1,834 | 0 |
| P-45 | 580,717 | 580,717 | 174,012 | 958 | 0 |
| Total | 12,335,283 | 12,335,283 | 3,357,492 | 18,890 | 0 |

C1, C2, C3, and C4 failure counts are zero in every pair (verified by asserting each JSON: `c1_dst_wire_missing`, `c2_src_wire_missing`, `c3_edge_missing`, `c4_bel_no_site_partner` are all 0, `unknown_tile` 0, `no_tiletype_file` 0). Contradiction list: empty. Zero contradictions is the result, stated as such with the counts above.

Scope note (C5): 11,336,839 of the 12,335,283 PIP lines name at least one wire absent from tileconn's (tile type, wire) universe. This is expected, not a contradiction: tileconn stitches wires between adjacent tiles, and INT-internal wires (`INT_NODE_*`, `BOUNCE_*`, `INODE_*`, `SS1_*`, `WW2_*` and the like) are tile-internal and appear only in `tile_types`. The BEL-facing wires are the exception, and C4 shows every one of those stitches to exactly the site pin tileconn names. The reverse check the task asks for (a tileconn edge the decoder systematically contradicts) finds nothing: no decoded PIP drives a BEL-facing wire from a source the static DBs forbid, and no tileconn BEL-pin stitch is contradicted by any decoded PIP in any pair.

## 3. VCC-default rule re-check, all 14 pairs

Same method and batch as section 1 (`run-tieoff-all.sh`). The rule: an IMUX wire with no driving PIP in decoded FASM is VCC-tied if the baseline has a VCC PIP for it, undriven otherwise.

| Pair | Needed pins | Real-driven | VCC-tied | Baseline-real-only | Truly undriven | Problems |
| --- | --- | --- | --- | --- | --- | --- |
| P-11 | 26,569 | 25,869 | 700 | 0 | 0 | none |
| P-13 | 113,086 | 111,262 | 1,824 | 0 | 0 | none |
| P-14 | 183,242 | 180,145 | 3,097 | 0 | 0 | none |
| P-15 | 142,772 | 140,932 | 1,840 | 0 | 0 | none |
| P-16 | 108,013 | 106,162 | 1,851 | 0 | 0 | none |
| P-17 | 214,791 | 211,420 | 3,371 | 0 | 0 | none |
| P-23 | 100,432 | 98,574 | 1,858 | 0 | 0 | none |
| P-28 | 49,045 | 48,430 | 615 | 0 | 0 | none |
| P-40 | 124,782 | 122,746 | 2,036 | 0 | 0 | none |
| P-41 | 125,290 | 123,265 | 2,025 | 0 | 0 | none |
| P-42 | 55,476 | 54,568 | 908 | 0 | 0 | none |
| P-43 | 125,869 | 123,808 | 2,061 | 0 | 0 | none |
| P-44 | 128,599 | 126,578 | 2,021 | 0 | 0 | none |
| P-45 | 89,080 | 85,558 | 3,522 | 0 | 0 | none |
| Total | 1,587,046 | 1,559,317 | 27,729 | 0 | 0 | none |

The P-11 and P-13 rows reproduce W-CHAR's numbers exactly (25,869/700 and 111,262/1,824). The rule holds across all 14 pairs and all three design families (resizer, BNN-PYNQ, FINN, PYNQ-ComputerVision): 27,729 VCC-tied pins, zero misclassifications. No wire the rule calls VCC-tied has a real driver in the baseline, and no needed pin is left truly undriven. The rule is confirmed as the correct miner behavior: never emit a VCC PIP as a routed net; treat the tied input as constant 1.

## 4. DI-fed LUT semantics: recommendation

### Question

`CARRY8.DI{i}.DI{i}` feeds a LUT output into the carry DI mux. The extractor (`tools/us09v/us09v/extract.py`, `_carry_di`, lines 808-818) treats it as unmodelled and fails loud with `1'bx`. The char report counted such a LUT as live, but the exact Verilog the extractor should emit was not settled. DI-fed-only LUT counts per pair (`carry-live.py`, reused unchanged):

| Pair | DI-fed-only LUTs | COUT-only slices |
| --- | --- | --- |
| P-11 | 183 | 9 |
| P-13 | 657 | 4 |
| P-14 | 1,314 | 4 |
| P-15 | 724 | 4 |
| P-16 | 1,008 | 4 |
| P-17 | 1,904 | 4 |
| P-23 | 498 | 4 |
| P-28 | 73 | 2 |
| P-40 | 579 | 4 |
| P-41 | 585 | 4 |
| P-42 | 128 | 4 |
| P-43 | 543 | 4 |
| P-44 | 508 | 4 |
| P-45 | 2,791 | 4 |
| Total | 11,495 | 59 |

11,495 LUTs across the corpus are live only through a DI feed. This is not a corner case; any rebuild that fails on it fails on thousands of LUTs per large pair.

### Worked example: SLICE_X11Y16, P-11

The slice is in tile `CLEL_L_X7Y16`, grid (188, 169). Its CARRY8 features:

```
SLICE_X11Y16.CARRY8.CARRY_TYPE.SINGLE_CY8
SLICE_X11Y16.CARRY8.DI0.AX
SLICE_X11Y16.CARRY8.DI1.BX
SLICE_X11Y16.CARRY8.DI2.DI2
SLICE_X11Y16.CARRY8.DI3.DI3
SLICE_X11Y16.CARRY8.DI4.DI4
SLICE_X11Y16.CARRY8.DI5.DI5
SLICE_X11Y16.CARRY8.DI6.DI6
SLICE_X11Y16.CARRY8.DI7.DI7
```

DI2 through DI7 are LUT-fed; DI0 and DI1 take the X bypass pins. The slice has no FFMUX/OUTMUX selection for letters C, D, F, G, H; the only mux selections on the slice are `FFMUXE1.BYP` (the E FF takes the EX bypass pin, not the E LUT) and `OUTMUXA.D5` (the A LUT O5). So the C through H LUTs have no live output except through the carry DI. The slice above, SLICE_X11Y17, has the CIN site PIP, so this slice's COUT is consumed.

LUT INITs and their INIT-dependence sets (bit-flip test, A1 as LSB):

| LUT | INIT | Depends on |
| --- | --- | --- |
| C | 0x599aa665f330f330 | C1, C2, C3, C4, C5, C6 |
| D | 0x9696c3c355550000 | D1, D2, D3, D5, D6 |
| E | 0xaf5050af50505050 | E1, E3, E4, E5, E6 |
| F | 0x99aa665533003300 | F1, F2, F4, F5, F6 |
| G | 0x96a596a533003300 | G1, G2, G3, G4, G6 |
| H | 0xc6c639390a0a0a0a | H1, H2, H3, H5, H6 |

Every needed input pin of these six LUTs (31 pins) resolves through tileconn to an IMUX wire on INT_X7Y16, and every one is either real-driven by a decoded INT PIP (25 pins) or VCC-tied in the baseline (6 pins). The pattern is exact: all six LUTs have their sixth input VCC-tied (C6, D6, E6, F6, G6, H6). Detail for the C LUT:

| Pin | IMUX wire | Decoded driver | Baseline |
| --- | --- | --- | --- |
| C1 | INT_X7Y16.IMUX_W6 | INT_NODE_IMUX_45_INT_OUT1 | same |
| C2 | INT_X7Y16.IMUX_W12 | INT_NODE_IMUX_56_INT_OUT1 | same |
| C3 | INT_X7Y16.IMUX_W16 | INT_NODE_IMUX_62_INT_OUT1 | same |
| C4 | INT_X7Y16.IMUX_W28 | INT_NODE_IMUX_48_INT_OUT1 | same |
| C5 | INT_X7Y16.IMUX_W30 | INT_NODE_IMUX_41_INT_OUT0 | same |
| C6 | INT_X7Y16.IMUX_W20 | (none) | VCC_WIRE |

### Empirical probe of the real extractor

Script `tools/us01-validate/di-extract-probe.py` calls the real `us09v` `Extractor.pin_driver` and `Extractor._carry_di` on this slice. Log: `~/workspace/us01-validate-logs/di-extract-probe.log`.

On the decoded P-11 FASM:

```
pin_driver C1: ('site', 'SLICE_X10Y17', 'DQ2')
pin_driver C2: ('site', 'SLICE_X10Y14', 'DQ2')
pin_driver C3: ('site', 'SLICE_X10Y16', 'CQ')
pin_driver C4: ('site', 'SLICE_X10Y17', 'DQ')
pin_driver C5: ('site', 'SLICE_X11Y15', 'HQ2')
pin_driver C6: ('undriven',)
_carry_di(i=2, C): "1'bx"
errors after _carry_di: ['SLICE_X11Y16: CARRY8.DI2.DI2 source is not modelled']
_carry_di(i=0, A) (DI0.AX bypass): 's_SLICE_X12Y15__CQ2'
errors after bypass _carry_di: []
```

With the single missing VCC PIP injected (`PIP.IMUX_W20.VCC_WIRE` added to tile INT_X7Y16, the exact PIP the baseline carries and the decoded FASM drops), `pin_driver C6` returns `('const', 1)` and everything else is unchanged. The bypass DI path already works: DI0.AX resolves to a real net.

Two facts follow from the probe, not from reading the code:

1. The extractor already resolves every DI-fed LUT's inputs correctly through its normal LUT path. The LUT loop builds `lut_o6[c]` for every letter with an INIT, resolving each INIT-dependent input through `pin_driver`. The only failure is `_carry_di` returning `1'bx` for the DI feed itself.
2. The extractor fails on the VCC-tied inputs of a DI-fed LUT even after the DI fix, unless the VCC-default rule is applied first. In this example all six LUTs have a VCC-tied sixth input, so a DI-only fix still fails loud on C6 through H6.

### Recommendation

Treat `CARRY8.DI{i}.DI{i}` as DI[i] equal to the same LUT O6 net as S[i]. In `extract.py`, the extractor's own model already states S[i] is the LUT O6 output (`lut_o6.get(c)`, used for the XOR and CY wires), and the DI mux selection `DI{i}.DI{i}` names that same LUT output as the DI source. The fix is to pass `lut_o6` into `_carry_di` (it is in scope at the call site) and return `lut_o6.get(c)` for the `DI{i}.DI{i}` case instead of appending the unmodelled error and returning `1'bx`. If the letter has no INIT (`lut_o6` has no entry), keep the existing behavior: error plus `1'bx`, since the caller already reports that case for S[i] as well.

Concretely, for the worked example, the rebuild should emit (using the extractor's own naming, `s_SLICE_X11Y16__C` for the C LUT O6 net):

```
wire [63:0] s_SLICE_X11Y16__C_INIT = 64'h599aa665f330f330;
wire s_SLICE_X11Y16__C = s_SLICE_X11Y16__C_INIT[{C6, C5, C4, C3, C2, C1}];
wire s_SLICE_X11Y16__C_XOR = s_SLICE_X11Y16__C ^ C_in;
wire s_SLICE_X11Y16__C_CY = s_SLICE_X11Y16__C ? C_in : s_SLICE_X11Y16__C;
```

where C1 through C5 are the routed nets from the decoded PIPs and C6 is `1'b1` (VCC tie-off). The DI feed adds no new net: DI[2] is the same wire as S[2]. The resulting carry behavior for that bit is sum `O6 ^ C_in` and carry-out `O6 AND C_in` (since `CO = S ? C : DI` with `S = DI = O6`).

Which inputs a DI-fed LUT needs: exactly the full O6 INIT-dependence set (all six inputs when the INIT depends on them), resolved through tileconn plus the VCC-default rule. Not the dual-5LUT O5 set: a DI-fed LUT has no live O5 (no D5 selection exists for it by the definition of DI-fed-only), so A6 is a real input whenever the INIT depends on it, and in this example every DI-fed LUT depends on its sixth input. The extractor's LUT loop already computes this correctly (`dep6` over the full 64-bit INIT); no change is needed there.

Prerequisite, in order: apply the VCC-default rule before extraction (inject the VCC PIP for every IMUX wire with no driving PIP, or teach `driver()` the same rule), then apply the one-line DI fix. Either fix alone still fails loud on this slice: the DI fix alone leaves the six VCC-tied sixth inputs undriven, and the VCC fix alone leaves all six DI feeds at `1'bx`.

What the data cannot settle: bit-exact co-simulation of a rebuilt design containing a DI-fed slice. The functional semantics (DI[i] equals LUT O6) follow from the feature vocabulary, the extractor's own S[i] definition, and the probe, but no rebuilt netlist with a DI-fed LUT has been co-simulated against its bitstream yet. The experiment that would settle it: after applying both fixes, run `us09v verify-fasm` on P-11 with its RTL testbench and confirm zero DI errors and zero co-sim mismatches. P-11's source is the known-repo resizer (unpinned, per the char report), so that run is corroboration, not ground truth.

## 5. Coverage rollup

Method: new script `tools/us01-validate/coverage.py`. It scans all 14 decoded FASM files (PIP dst/src wires named, CARRY8 DI/CI config mentions, CIN site PIPs) and all 14 baseline FASM files (VCC_WIRE PIPs), then classifies every site pin of the five site types (from `site_types`, cross-checked against tileconn) into exactly one bucket:

- wire_verified: its tileconn partner wire appears as a PIP dst or src in a decoded FASM (and passed the C4 check).
- vcc_only: no decoded PIP ever names its partner wire, but a baseline has a VCC_WIRE PIP driving it.
- config_only: the pin appears in decoded FASM only via config features (CARRY8 DI/CI selections, CIN site PIPs), never via a wire. This is the exercised-but-unverified bucket.
- never_exercised: the pin appears nowhere in any decoded or baseline FASM in any form.

Buckets are per pin class across the corpus (a pin class counts as exercised if any slice instance exercises it anywhere). Per-pair JSON: `~/workspace/us01-validate-logs/coverage.json`. Supporting counts: 3,706 distinct sites carry a CIN site PIP across the corpus; 858,907 distinct (tile, wire) pairs are driven by VCC_WIRE across the baselines; 1,740 distinct (tile type, wire) pairs named by decoded PIPs.

| Site type | Total pins | Wire-verified | VCC-only | Config-only | Never exercised |
| --- | --- | --- | --- | --- | --- |
| SLICEL | 106 | 104 | 0 | 2 | 0 |
| SLICEM | 108 | 106 | 0 | 2 | 0 |
| DSP48E2 | 432 | 0 | 0 | 0 | 432 |
| RAMB181 | 203 | 0 | 0 | 0 | 203 |
| RAMBFIFO36 | 533 | 0 | 0 | 0 | 533 |
| Total | 1,382 | 210 | 0 | 4 | 1,168 |

The four config-only pins are CIN and COUT on SLICEL and SLICEM. They have no INT partner wire by construction: the carry chain is tile-internal, stitched CLE to CLE by tileconn (verified directly in the char report), and exercised through the CIN site PIP (3,706 sites) plus the tileconn CIN-to-COUT stitch, not through INT PIPs. Their bucket is a limitation of the wire-based definition, not a gap in the map.

The VCC-only bucket is empty at pin-class granularity: every pin class that is VCC-tied somewhere is also real-driven somewhere else in the corpus (for example, A6 is VCC-tied on the DI-fed slices of section 4 and real-driven on thousands of other slices). At pin-instance granularity the VCC tie-offs are real and counted: 27,729 VCC-tied needed-pin instances in section 3.

Plain statement of what the corpus validates: the corpus validates the CLE slice map almost completely, 210 of 214 CLE pins wire-verified (98.1 percent), with the remaining 4 pins (CIN, COUT on both slice types) exercised through site PIPs and tileconn stitching. It validates none of the DSP or BRAM map: 0 of 432 DSP48E2 pins and 0 of 736 BRAM pins are exercised in any form, which is expected (no DSP segbits exist, so zero DSP lines decode; BRAM site PIPs have no segbits encoding and never appear in FASM). Overall the corpus wire-verifies 210 of 1,382 pins (15.2 percent). The map itself is complete in tileconn (106/106, 108/108, 432/432, 203/203, 533/533 per the char report); the corpus corroborates the CLE part of it and says nothing about the DSP/BRAM part. Any claim that the bitstreams validate the DSP or BRAM pin map would be false.

## 6. Deviations and open items

1. No runtime execution-risk notice fired during this work. Nothing to record on that front.
2. The consistency checker needed two corrections during the run (bidirectional `.FWD`/`.REV` suffixes, site-named SLICE PIPs). Both are documented in section 2 with the evidence for each; the final numbers come from the corrected checker, and the P-28 rerun after the fix is in the log.
3. The raw prjuray baseline uses a different feature vocabulary from the decoded FASM (tile names like `CLEL_R_X11Y16`, `ALUT.INIT[63:0] = 64'b...` binary literals) that the decoder normalizes into site-named hex INIT lines. The baseline therefore cannot be fed to the extractor directly; the probe in section 4 injects the single VCC PIP instead. This is a property of the baseline files, not a defect in either tool.
4. The P-11 fresh decode has 48 fewer lines than the stored decode (default V0 CMT WIRE lines), per the char report. This validation used the fresh decodes throughout; the deviation does not affect any number here.
5. Flag A-F7F8 (F7/F8/F9 mapping) remains unresolved and needs a CTO ruling. It is out of scope for this lane; no F7F8 claim is made here.
6. IO/HDIO pins were not traced end to end in this lane either. The consistency check covers HDIO/RCLK PIP edges wherever they appear (they pass C1-C3 like any other PIP), but no IO site pin was traced pin to wire. Likely determined by the same construction, not verified here.
7. The -1-e vs -1-i pin-map identity assumption (same die, different speed grade) is still stated, not verified. Seven of the 14 pairs are -1-i parts decoded against the -1-e DB; every check in this report passes identically on the -1-i pairs (P-13 to P-17, P-28), which is consistent with the assumption but does not prove it.

## Appendix: commands and files

Validation runs (all under `nohup setsid`, logs in `~/workspace/us01-validate-logs/`):

```
# Held-out tie-off plus full VCC re-check (all 14 pairs)
sh tools/us01-validate/run-tieoff-all.sh            # log tieoff-all.log
python3 tools/us01-char/check-tieoff.py <decoded> <baseline>

# Full 14-pair consistency plus DI-fed counts
sh tools/us01-validate/run-consistency-all.sh       # log consistency-all.log
python3 tools/us01-validate/consistency.py <decoded> <out-json>
python3 tools/us01-char/carry-live.py <decoded>

# Coverage rollup
python3 tools/us01-validate/coverage.py ~/workspace/us01-validate-logs/coverage.json

# DI extractor probe
python3 tools/us01-validate/di-extract-probe.py    # log di-extract-probe.log
```

Key logs (`~/workspace/us01-validate-logs/`): `tieoff-all.log`, `tieoff-P-*.json` (14), `tieoff-P-*.err` (14), `consistency-all.log`, `consistency-P-*.json` (14), `consistency-P-*.err` (14), `carrylive-P-*.json` (14, inside `consistency-all.log`), `coverage.json`, `coverage.log`, `di-extract-probe.log`.

Inputs reused: fresh decodes `~/workspace/us01-char-logs/decoded/P-*.fasm` (14), raw baselines `~/workspace/ultra-bitgen-crew/decoder-validation/baseline/P-*.fasm` (14), DB `~/workspace/prjuray-db-ref/zynqusp/` (`tileconn.json`, `tilegrid.json`, `tile_types/`, `site_types/`).

References: `~/workspace/ultra-bitgen-crew/US01_CHAR_REPORT.md`, `~/workspace/ultra-bitgen-crew/decoder-validation/NORMALIZATION_REPORT.md` (rule R5), `~/workspace/ultra-bitgen-crew/decoder-validation/UNEXPLAINED_ATTRIBUTION.md`, `tools/us09v/us09v/extract.py` (`_carry_di` lines 808-818, `driver` lines 263-340, `inp` line 374, LUT loop lines 503-528, carry emission lines 568-595).
