# Helion Correctness Campaign: Handoff Document

**Date:** 2026-10-17
**Branch:** wip/ide-integration (HEAD: 66bac28)
**Scratch branch:** scratch/fixes (has uncommitted fixes)
**Goal:** 400-case corpus gate, 0 regressions from 5 commits

## The 5 Commits Under Gate

These 5 commits are on wip/ide-integration and need gate verification:

1. **33ecc9f** - "sv: Skip port fixup for submodules to prevent F2 multi-driven"
   - Changed `synth_rtl` to call `synth_rtl_inner(rtl, true)`, submodules use `false`
   - Port fixup now skips when `!is_top`
   - **Status:** Causes F2 regressions (see below)

2. **6a0570a** - "sv: Support unary reduction operators in const elaboration"
   - Added `|`, `&`, `^`, `!`, `~` to const_atom
   - **Latent bugs:** `&` uses u128::MAX (wrong for narrow), `~` not masked to width
   - **Status:** No regression found, but width bugs need fixing

3. **c68df0c** - "sv: Support 3-input adders with carry-in (a+b+cin)"
   - Extended packed_add_pair for Add(Add(a,b),cin), extended rexpr_has_nested_arith
   - **ALSO** added force-list restriction to port fixup (the regression!)
   - **Status:** REGRESSION - force-list causes 96 F6 failures

4. **7b33e24** - "Fix string parameter parsing in const_atom"
   - Added `Tok::Str` arm to const_atom using str_param_hash()
   - Added const_cond folding to procedural if handler
   - **Status:** Fixes serv_ctrl/serv_state string param bug. No regression.

5. **8acdb12** - "Fix whole-wire copy assigns (pc = r[0]) being silently dropped"
   - Expands whole-wire Ident/Bit/Range RHS to per-bit assigns before collect_net_copies
   - Removed Ident/Bit/Range from skip match
   - **Status:** Fixes pc bug. Creates identity LUTs (0xaaaaaaaaaaaaaaaa) instead of net aliasing - QoR issue, not correctness.

## Regression: c68df0c Force-List

**Proven by:**
```
$ git log --oneline -S "ONLY fix ports in the force list" -- crates/helion-sv/src/lib.rs
c68df0c sv: Support 3-input adders with carry-in (a+b+cin)
```

**Impact:**
- Baseline (e898994): picorv32 2543 cells, 0 fails
- HEAD (8acdb12): picorv32 2523 cells, 6 fails (F6: mem_la_wstrb unconnected)
- 232 cells disappeared, 16 u_tie_ cells gone
- 96 F6 failures across corpus, 32 F2 failures

**Root cause:** c68df0c restricted port fixup to only 6 ports in force list:
`pcpi_wait, pcpi_ready, s_axil_awvalid, s_axil_wvalid, s_axil_bready, ready`

All other unconnected ports no longer get tie-offs, exposing F6 failures.

**BUT:** The F6 cases are REAL BUGS, not false positives. Example:
- mem_la_wstrb IS driven in RTL (picorv32.v lines 406, 411, 419)
- Baseline masked it with tie-off (constant 0) - WRONG
- HEAD correctly exposes that the driver is lost

**The real bug:** `(* full_case *)` attribute causes entire always block to be dropped.
- Toy: with attribute = 0 cells, without = 8 cells
- picorv32.v has 10+ such attributes
- Parser does not handle `(* ... *)`, silently skips the block

**Do NOT:** Revert c68df0c or re-add blanket tie-offs. Fix the real bugs.

## Open Issues

### 1. full_case / parallel_case Attribute Parsing (CRITICAL)
**File:** crates/helion-sv/src/lib.rs, `parse_seq_item` (line ~8027)
**Problem:** `(* full_case *)`, `(* parallel_case *)` before case statements cause the entire always block to be dropped (0 cells).
**Attempted fix:** Added attribute-skipping loop in scratch/fixes, but didn't work. The tokenizer may not produce separate `(` and `*` tokens, or the code is in the wrong place.
**Test:** /tmp/fc_a.v (attribute only) should give 8 cells, currently gives 0.
**Impact:** picorv32, and 96 F6 cases.

### 2. Instance Parameter Override Miscompile (CRITICAL)
**Problem:** `#(.N(2))` overrides are ignored for procedural `if` folding. Both `#(.N(1))` and `#(.N(2))` give 65 cells (should differ).
**Root cause:** Modules parsed once with default params. `p.params` has defaults, not instance-bound values. Procedural if folds at parse time using defaults.
**Note:** Port widths (`#(.W(4))` in `[W-1:0]`) WORK because they're evaluated at elaboration with bound params. Only parse-time folding is broken.
**Design note:** ~/workspace/design_note_params.md has 3 options. Recommendation: Option C (don't fold params at parse time) as interim, Option B (fold at elaboration) as proper fix.
**Test files:** /tmp/num_override.v, /tmp/inst_override.v

### 3. Identity LUTs from 8acdb12 (QoR)
**Problem:** Whole-wire copies like `wire pc = a` now create Lut6 with init 0xaaaaaaaaaaaaaaaa (identity/buffer) instead of net aliasing.
**Cost:** +2 LUTs per copy. Affects axi_adapter (+196 cells) and axil_adapter (-179 cells, unexplained drop).
**Fix needed:** Peephole pass to remove buffer LUTs (single live input, identity INIT) by redirecting loads to source net. Do NOT remove when output is port, clock, or feeds carry/IOB.
**Tests needed:** Clock copy, reset copy, copy feeding carry, copy driving output port.

### 4. F2 Multi-Driven (from 33ecc9f)
**Problem:** 32 F2 failures on HEAD. 33ecc9f was supposed to FIX F2 by skipping fixup for submodules, but F2 persists.
**Examples:**
- serv_decode: `net o_sh_right u_clut59_s76/O u_iob/PAD` (LUT + port both drive)
- serv_aligner: `net o_wb_ibus_cyc` multi-driven
- serv_ctrl: was PASS, now F2 (regression from 33ecc9f)
**Root cause:** Port fixup creates duplicate drivers. The `!is_top` skip is insufficient.

### 5. axi_interconnect no_body (Separate Bug)
**Problem:** 0 cells, `no_body` diagnostic. Single 988-line module, generate blocks not producing logic.
**Status:** Not investigated. Separate from the 7 other blocked designs.

### 6. const_atom Width Bugs (from 6a0570a)
**Problem:** 
- `&` (AND reduction) compares against u128::MAX, wrong for narrow operands (`&1` should be 1, returns 0)
- `~` (NOT) is not masked to operand width
**File:** crates/helion-sv/src/lib.rs, const_atom function

## Test Files (in /tmp, will be wiped on restart)

Recreate as needed:
- `/tmp/fc_a.v` - full_case attribute only (0 cells, bug)
- `/tmp/fc_b.v` - default only, no attribute (8 cells, ok)
- `/tmp/fc_c.v` - both attribute and default (0 cells, bug)
- `/tmp/fc_d.v` - parallel_case in clocked always (0 cells, bug)
- `/tmp/num_override.v` - numeric param override test
- `/tmp/inst_override.v` - string param override test
- `/tmp/width_test.v` - width override test (works)
- `/tmp/both_override.v` - N and W override together

## Binaries (persistent)

- `~/workspace/bin/helion_head_release_8acdb12` - HEAD release (4.4M)
- `~/workspace/bin/helion_baseline_release_e898994` - Baseline release (4.4M)
- `~/workspace/helion-scratch-target/release/helion` - Scratch build (has attribute fix attempt)

**IMPORTANT:** /tmp is tmpfs (512M), wiped on restart. Do NOT store binaries in /tmp.

## Corpus

- Manifest: `~/workspace/bench/corpus/MANIFEST.yaml` (100 designs, 400 cases)
- Harness: `~/workspace/bench/functional_harness.py`
- Official runner: `~/workspace/bench/corpus_sweep.py`
  - **NOTE:** Does NOT run structural_checks on multi-file designs (line 105: "just check synth passed")
  - This is why baseline got 360/400
- My runner: `/tmp/par_corpus.py` (resumable, runs structural_checks on all)
  - This is why HEAD gets 47/400 (stricter)

## Key Commands

```bash
# Single-file HNF
helion hnf <file.v> -o <out.hnf>

# Multi-file synth
helion synth --part <part> --top <top> <files...> -o <out.hnf>

# Structural check
cd ~/workspace/bench
python3 -c "from functional_harness import parse_hnf, structural_checks; ports,cells,nets=parse_hnf('/tmp/x.hnf'); fails,warns=structural_checks(ports,cells,nets); print(f'fails={len(fails)}')"

# Stratified test
python3 /tmp/strat_test.py
```

## Scratch Worktree

**Location:** ~/workspace/helion-scratch (branch scratch/fixes)
**Changes:**
1. Removed force-list restriction from c68df0c (reverted to tie-off all)
   - **DO NOT COMMIT** - this masks real bugs
2. Added attribute-skipping in parse_seq_item (didn't work, needs fix)

**To continue:**
1. Fix the `(* ... *)` attribute parsing (find correct tokenizer handling)
2. Test on /tmp/fc_a.v (should go from 0 to 8 cells)
3. Test on picorv32 (mem_la_wstrb should get a driver)
4. Run stratified 40 to verify F6 flips to PASS
5. Then address F2, override miscompile, identity LUTs

## Gate Status

- **Baseline (corpus_sweep.py):** In progress, at 54/400. Must reproduce 360.
- **HEAD (par_corpus.py):** 47 PASS, 341 FAIL, 12 TIMEOUT out of 400.
  - FAIL: 96 F6 + 32 F2 + 125 parse_error + 88 cells=0
- **Stratified 40:** Baseline 20/20 PASS on F6/F2, HEAD 0/20. Regression confirmed.

## Files Modified (uncommitted in scratch)

- `crates/helion-sv/src/lib.rs`:
  - Line ~18662: Removed force-list (DO NOT COMMIT)
  - Line ~8028: Added attribute skip (NOT WORKING, needs fix)

## Next Steps (priority order)

1. **Fix `(* ... *)` attribute parsing** - This unblocks 96 F6 cases
2. **Verify picorv32** - mem_la_wstrb should be driven, F6 should clear
3. **Fix F2** - serv_decode, serv_aligner, serv_ctrl multi-drivers
4. **Fix override miscompile** - Don't fold params at parse time (Option C)
5. **Fix identity LUTs** - Peephole to remove buffer LUTs
6. **Fix const_atom widths** - `&` and `~` masking
7. **Investigate axi_interconnect** - no_body root cause
8. **Full gate** - corpus_sweep.py on HEAD, verify 360+ (should exceed baseline once bugs fixed)
