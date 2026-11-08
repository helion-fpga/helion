# Helion 7-Series Backend: Line-by-Line Investigation Report

**Date:** 2026-10-22
**Investigator:** Code audit subagent
**Scope:** `~/workspace/helion-zed-bringup/crates/helion-xilinx7/src/` (5017 lines, 14 files)
**Method:** Read every file, every function. Asked: what does it do, what does it NOT do, are there silent failures?

## Summary

The backend has systematic incomplete-primitive-configuration bugs. The pattern: primitives are marked IN_USE but missing critical config bits, producing bitstreams that program (done=1) but silently fail to function. This is worse than a build failure.

**Critical findings:** 8
**High findings:** 6
**Medium findings:** 5
**Low findings:** 3

---

## CRITICAL

### C1. BUFGCTRL clock does not pass (emit.rs:163-178)
**What it does:** Emits 10 FASM features for BUFGCTRL (IN_USE, INIT_OUT, IS_IGNORE0/1_INVERTED, PRESELECT_I1, ZINV_CE0/CE1, ZINV_S0/S1, ZPRESELECT_I0).
**What's wrong:** The ZINV_ bit polarity is a guess, never validated on hardware. The clock does not reach flip-flops on the ZedBoard. Either the polarity is wrong, or additional routing bits are needed, or the wrong BUFG site is selected.
**Evidence:** ZedBoard blinky programs (done=1) but LEDs do not blink. Vivado bitstream on same hardware blinks.
**Severity:** CRITICAL - every clocked design is broken.

### C2. CARRY4 emits invalid FASM (emit.rs:194-201)
**What it does:** Emits `TILE.SLICEL_X0.CARRY4.ACY0` (and BCY0/CCY0/DCY0).
**What's wrong:** The carry chain configuration is incomplete. Missing: PRECYINIT (CYINIT source select: AX/CIN/C0/C1), DI input muxing, S input configuration, and inter-CARRY4 chaining (CI from previous CO). The PlacedCarry struct (place.rs:86) only has name/tile/site, no configuration data. The pack stage (pack.rs:103) captures no DI/S/CYINIT info from the IR.
**Evidence:** X-Ray DB has PRECYINIT.AX/CIN/C0/C1 and CARRY4.ACY0/BCY0/CCY0/DCY0. Helion emits only the mux bits, no init.
**Severity:** CRITICAL - any arithmetic (adders, counters) is broken. The 27-bit blinky counter cannot work.

### C3. FF clock-enable and reset silently ignored (emit.rs:112-124)
**What it does:** Emits `FF.ZINI` and `FFMUX.AX/BX/CX/DX` for each flip-flop.
**What's wrong:** The PackedFf struct (pack.rs:28) has `ce_net: Option<String>` and `sr_net: Option<String>`, and pack.rs correctly captures them from the IR (lines 236-240). But emit.rs never emits them. The X-Ray DB has `ZRST`, `CEUSEDMUX`, `SRUSEDMUX` features. They are silently dropped.
**Evidence:** pack.rs:236-240 captures ce_net/sr_net. emit.rs:112-124 does not reference them.
**Severity:** CRITICAL - any FF with CE or SR will ignore those pins. Silent wrong behavior.

### C4. DSPs silently ignored (emit.rs:196)
**What it does:** `let _ = &placed.dsps;` - explicitly discards DSP placements.
**What's wrong:** If a design uses DSPs (Mac27 cells), they are placed on sites but no configuration is emitted. The bitstream will program but the DSP will not function.
**Evidence:** emit.rs:196. The comment says "future work" but there is no loud error.
**Severity:** CRITICAL - silent miscompile for any DSP design. Should fail loudly.

### C5. Multiple BUFGs collide on same site (xdevice.rs:118-120, place.rs:147-156)
**What it does:** `bufg_site()` returns `self.device.bufg_sites().into_iter().next()` - always the first site.
**What's wrong:** The `used` set is not consulted. If a design has 2 clocks, both BUFGs are placed on the same site. place.rs inserts into used_sites, but the next call still returns the first site.
**Evidence:** xdevice.rs:118-120. Same bug in `ramb_site()` and `dsp_site()`.
**Severity:** CRITICAL - multi-clock designs will have overlapping placements. Silent corruption.

### C6. CARRY4s placed on random slices, breaking chain (place.rs:167-178)
**What it does:** Places each CARRY4 on a free slice via `slice_near(ax, ay, &used_sites)`.
**What's wrong:** Carry chains must be vertically adjacent in the same column (CO of one drives CI of the next). Placing them on random free slices breaks the chain.
**Evidence:** place.rs:167-178. No vertical adjacency constraint.
**Severity:** CRITICAL - even with correct FASM, the physical chain is broken.

### C7. BRAM init data silently zeroed on parse failure (pack.rs:231-236)
**What it does:** Parses INIT hex strings with `u64::from_str_radix(t.trim(), 16).unwrap_or(0)`.
**What's wrong:** On parse failure, silently uses 0. A malformed INIT attribute produces a zeroed BRAM with no error.
**Evidence:** pack.rs:236.
**Severity:** CRITICAL - silent data corruption. Should be a loud error.

### C8. BRAM only emits IN_USE, no port config (emit.rs:180-187)
**What it does:** Emits `TILE.RAMB18.SITE.IN_USE` only.
**What's wrong:** No port configuration (clocks, enables, address/data widths), no INIT data programming. The comment admits "future work".
**Evidence:** emit.rs:180-187.
**Severity:** CRITICAL - BRAM will not function. Should fail loudly instead of emitting a non-functional marker.

---

## HIGH

### H1. OLOGIC bypass not configured for outputs (route.rs:140-145, emit.rs)
**What it does:** route.rs comment says "The output path must go through the OLOGIC bypass." The router traverses pseudo-pips for connectivity.
**What's wrong:** emit.rs emits no OLOGIC configuration. If the OLOGIC needs explicit bypass configuration bits, they are missing.
**Evidence:** route.rs:140-145 mentions OLOGIC bypass. emit.rs has no OLOGIC features.
**Severity:** HIGH - outputs may not drive correctly. Needs X-Ray DB verification.

### H2. `mux_for_ff` is a guess (emit.rs:280-315)
**What it does:** Selects FFMUX input (AX/BX/CX/DX) based on BEL letter pairing.
**What's wrong:** The comment says "the router/emitter can refine" - this is not verified against hardware. If the LUT is not on the same BEL letter, the mux select is wrong.
**Evidence:** emit.rs:280-315, comment at line 314.
**Severity:** HIGH - wrong mux select means FF.D is disconnected. Silent failure.

### H3. IBUF PAD connection is implicit (pack.rs:275-315)
**What it does:** `insert_clock_spine` synthesizes a PackedIob for the clock input with `net: format!("{}_ibuf", port.name)`.
**What's wrong:** There is no explicit connection from the top-level port to the IBUF's PAD pin in the netlist. The router sees the fabric-side net but the pin-to-IBUF path is assumed, not modeled.
**Evidence:** pack.rs:295-310. No `connect()` call for the PAD.
**Severity:** HIGH - works by convention, but fragile. If the port name doesn't match, the IBUF is dangling.

### H4. `site_tile` may return empty string (place.rs:216)
**What it does:** `let tile = site_tile.get(site).cloned().unwrap_or_default();`
**What's wrong:** If a site is reused but not in site_tile map, tile is empty string. This produces invalid placement with empty tile name.
**Evidence:** place.rs:216.
**Severity:** HIGH - corrupt placement data. Should be an error, not default.

### H5. Undriven nets silently dropped (route.rs:165-167)
**What it does:** `if loads.is_empty() { continue; }`
**What's wrong:** A net with a driver but no loads is silently skipped. This is probably fine, but a net with no driver and loads would have been caught earlier (drivers.len() != 1 check). The silent skip masks potential issues.
**Evidence:** route.rs:165-167.
**Severity:** HIGH - likely benign, but masks design issues.

### H6. LUT input tie-off not verified (emit.rs:96-105)
**What it does:** Emits LUT INIT with 64-bit truth table. Unconnected inputs are "tied low at emission" per pack.rs comment.
**What's wrong:** The INIT value assumes unconnected inputs are 0, but this is not verified. If the synth produces a different assumption, the LUT function is wrong.
**Evidence:** pack.rs:20 comment: "None = unconnected, tied low at emission". No verification in emit.rs.
**Severity:** HIGH - potential functional mismatch.

---

## MEDIUM

### M1. `is_clock_net` trusts device query blindly (route.rs:175-177)
**What it does:** If `device.is_clock_net(net)`, uses ClockSpine routing.
**What's wrong:** No verification that the net is actually driven by a BUFG. If xdevice.rs has a bug in clock_nets registration, non-clock nets use the spine.
**Evidence:** route.rs:175-177, backend.rs:62-67.
**Severity:** MEDIUM - works if backend.rs is correct.

### M2. Test `routed_blinky` has empty bufgs (emit.rs:340)
**What it does:** The unit test constructs Packed7 with `bufgs: vec![]`.
**What's wrong:** The test does not cover BUFG emission. The BUFG code path is untested.
**Evidence:** emit.rs:340.
**Severity:** MEDIUM - test gap.

### M3. `canonical_feature` strips leading zeros (frames.rs:253-258)
**What it does:** `FOO[00]` becomes `FOO[0]`.
**What's wrong:** If the X-Ray DB distinguishes `FOO[00]` from `FOO[0]` (unlikely but possible), this breaks lookup.
**Evidence:** frames.rs:253-258.
**Severity:** MEDIUM - probably fine, but unverified.

### M4. Timing model is preliminary (timing.rs:1-12)
**What it does:** STA with Manhattan distance * hop delay.
**What's wrong:** The doc admits "NOT a Vivado-correlated signoff STA; the model values themselves are preliminary."
**Evidence:** timing.rs:11-12.
**Severity:** MEDIUM - timing numbers are estimates, not signoff.

### M5. `insert_clock_spine` only handles FF clocks (pack.rs:268-270)
**What it does:** Finds nets driving FF CLK pins.
**What's wrong:** If a clock drives only BRAM, DSP, or combinational logic (no FFs), no BUFG is inserted. The clock stays on general routing.
**Evidence:** pack.rs:275-280.
**Severity:** MEDIUM - probably correct behavior, but undocumented.

---

## LOW

### L1. `let _ = p;` suppresses unused variable (emit.rs:160)
**What it does:** Silences the unused `p` (PackedIob) variable.
**What's wrong:** The variable is fetched but not used. Indicates incomplete implementation.
**Evidence:** emit.rs:160.
**Severity:** LOW - code smell.

### L2. `let _ = fi;` suppresses unused loop var (place.rs:230)
**What it does:** Silences unused `fi` (FF index).
**What's wrong:** The index is not needed, but the pattern suggests incomplete logic.
**Evidence:** place.rs:230.
**Severity:** LOW - code smell.

### L3. `let _ = internal;` (route.rs:188)
**What it does:** Silences unused `internal` loads.
**What's wrong:** Site-internal loads are correctly skipped, but the explicit suppression suggests the logic was not fully thought through.
**Evidence:** route.rs:188.
**Severity:** LOW - benign.

---

## Recommendations

1. **Fail loudly for DSP, BRAM, and unconfigured CARRY4.** Silent miscompiles are unacceptable. Return Err until implemented.
2. **Fix `bufg_site()`, `ramb_site()`, `dsp_site()` to respect the `used` set.** Multi-instance designs are broken.
3. **Place CARRY4s with vertical adjacency.** The chain must be contiguous.
4. **Hardware-validate every BUFGCTRL bit.** The ZINV_ polarity is a guess.
5. **Emit FF CE/SR bits.** The data is in PackedFf but dropped in emit.
6. **Add BUFG to the emit unit test.** The current test has empty bufgs.
7. **Make BRAM INIT parse failures loud.** `unwrap_or(0)` is silent corruption.

## Files Reviewed
- lib.rs (37 lines): module declarations, clean.
- flow.rs (23 lines): thin shim, clean.
- backend.rs (185 lines): stage orchestration, clean. Correctly registers BUFG outputs as clock nets.
- emit.rs (409 lines): **8 findings** (C1, C2, C3, C4, C8, H2, M2, L1).
- pack.rs (364 lines): **3 findings** (C7, H3, M5).
- place.rs (383 lines): **4 findings** (C5, C6, H4, L2).
- route.rs (395 lines): **3 findings** (H1, H5, L3).
- fasm.rs (501 lines): parser, clean.
- frames.rs (699 lines): frame assembly, clean (address 0 and index 0 are valid defaults).
- bitstream.rs (542 lines): .bit file writer, clean (timestamp unwrap_or(0) is fine).
- device.rs (931 lines): DB loader, clean (unwrap_or(0) and unwrap_or_default are for counts/collections, fine).
- xdevice.rs (235 lines): **1 finding** (C5 - bufg_site/ramb_site/dsp_site ignore used set).
- timing.rs (313 lines): STA, preliminary model documented, clean.
