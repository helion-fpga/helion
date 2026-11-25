# INT_INTF segbits lane report

Lane branch: fm/ultra-bitgen-intintf, code tip 24629800 (impl commit on counter base 839bca13). No merges into fm/ultra-bitstream, fm/zedboard-merge, or master. No critic dispatched by this lane; Session 39 is the main agent's to dispatch.

## 1. Outcome

The lane's headline acceptance criterion is NOT met, and cannot be met from the public corpus: the INT_INTF_R_PCIE4 IMUX PIP segbits are UNPINNABLE from the 14 public xczu3eg bitstream pairs. No corpus design routes a PL signal through any INT_INTF PIP, so there is no bit in any corpus file that a PIP could have set. The IOB placement lane therefore stays BLOCKED on B1, with Vivado named as the unblocker (a Vivado-built design that routes a signal through an INT_INTF_R_PCIE4 PIP, decoded the same way). This is a proven-blocked outcome with a named unblocker, not a failure to look: the probe attributes every unexplained bit in every INT_INTF frame, and the PIP pool is empty.

What the lane does deliver, all verified:

- The derived segbits for INT_INTF_R_PCIE4 OUTPUTS_ENABLED, cross-validated on 5 independent corpus designs, wired into the emitter. The emitter resolves the feature and sets exactly the 3 corpus bits.
- B2 fixed: the emitter resolves INT_INTF and HDIO tiles to base FARs through a tilegrid-derived table (15 entries, verified the same way the INT table was).
- The site_index checker bug fixed in both the Python checker and the Rust tileconn loader, with regression tests.
- The `helion-bitgen bits` diagnostic now classifies the new feature (IO_CONFIG) instead of failing loud on any bitstream containing it.
- Suites: helion-ultrascale 441 passed / 0 failed / 1 ignored, helion-cli 73/0/1, US-09 checkers 95 OK. Stash proof both sides for db.py, tileconn, and the emitter.

## 2. The corpus verdict: the PIP segbits are unpinnable

Method: a new probe binary (`crates/helion-ultrascale/src/bin/intintf_probe.rs`, built on the crate's own `decode_bitstream` plus a new public `tile_windows()` API in `bitstream_decode.rs`) decodes each of the 14 corpus pairs and attributes every unexplained bit to a tilegrid window (or a gap column). Probe logs `P-*.probe.log` and window dumps `intintf-windows.log`, `all-windows.log` in `~/workspace/intintf-logs/`. Full analysis in `~/workspace/intintf-logs/INTINTF_ANALYSIS.md`.

The probe reproduces the established total of exactly 54,907 unexplained bits across all 14 pairs. The INT_INTF attribution:

- The ONLY set bits in any INT_INTF tile are 120 bits, all in the 5 BNN designs (P-13 through P-17): 8 tiles per design, identical in all 5 (INT_INTF_R_PCIE4_X9Y120, X9Y122, X9Y154, X9Y158, X9Y168, X9Y172, X9Y176, X9Y178), each carrying exactly 3 bits at relative positions (frame 1, bit 21), (frame 2, bit 27), (frame 3, bit 24). That is 40 bits per relative position (8 tiles times 5 designs).
- Those 3 positions are exactly INT_INTF_LEFT_TERM_PSS.OUTPUTS_ENABLED.1 in the public `segbits_int_intf_left_term_pss.db` (01_21 02_27 03_24; OUTPUTS_ENABLED.0 is the same bits negated). The same 5 designs also decode 31 LEFT_TERM_PSS OUTPUTS_ENABLED.1 lines each.
- Zero gap bits in INT_INTF columns (col 0, 29, 75) across all 14 files: no bit is set in any odd-Y (unwindowed) INT_INTF tile either. Zero bits in any other INT_INTF tile type. The top gap columns (88, 42, 11, 57, 30) are owned by RCLK_DSP_INTF / HDIO tiles, not INT_INTF, verified from the window dump.
- Corroboration: only the 5 BNN designs use HDIO IO tiles (HDIO_BOT_RIGHT_X9Y120, HDIO_TOP_RIGHT_X9Y150); the other 9 designs use RCLK_HDIO clock tiles only. The 8 R_PCIE4 tiles sit in row 2, the same clock region as the two used HDIO tiles.

Conclusion: the 120 bits are OUTPUTS_ENABLED, not PIP bits, in 5 independent designs. No corpus design routes a PL signal through any INT_INTF PIP. The unblocker is Vivado: a Vivado-built design that routes a signal through an INT_INTF_R_PCIE4 PIP (for example the IOB blinky with a placed IOB), decoded the same way. Do not spin this: the PIP acceptance criterion is unmet and the IOB lane stays blocked until that lands.

Tilegrid facts from the window dump (275 INT_INTF windows, 15,712 windows total): INT_INTF_R_PCIE4 95 windows, INT_INTF_LEFT_TERM_PSS 180 windows (all X0, col 0, all rows, 16 frames, 3 words). INT_INTF_L (1,080 tiles), INT_INTF_R (900), INT_INTF_L_PCIE4 (360), INT_INTF_RIGHT_TERM_IO (180) have ZERO windows; the tilegrid records no baseaddr for them, so they stay unresolvable and fail loud. HDIO_BOT_RIGHT 4 windows, HDIO_TOP_RIGHT 4 windows, both 10 frames. HPIO_RIGHT has windows (col 103) but is out of scope for this lane.

## 3. The derived segbits: OUTPUTS_ENABLED

File `crates/helion-ultrascale/data/segbits_int_intf_r_pcie4.db` (new file, 2 lines plus a header marked Helion-derived, NOT upstream prjuray data, with derivation and cross-validation stated). Never edit a vendored prjuray DB in place; this is a new file. `data/PROVENANCE.md` gains a "Helion-derived segbits" section with a table entry, kept separate from the vendored table. `SEGBITS_FILES` in `src/bin/helion-bitgen.rs` goes from 9 to 10 entries, adding the derived file with a comment.

```
INT_INTF_R_PCIE4.OUTPUTS_ENABLED.1 01_21 02_27 03_24
INT_INTF_R_PCIE4.OUTPUTS_ENABLED.0!01_21!02_27!03_24
```

Mirroring the LEFT_TERM_PSS file verbatim (tile type changed). The loader skips `#` lines and fails loud on cross-file duplicates (verified in `segbits.rs`).

The decoder stays public-DB-only by design: adding the derived file to the decoder would emit 40 lines bit2fasm does not emit, breaking the exact-match claim. The derived file is emitter-side, and the probe demonstrates the attribution. The decoder reports the 3 emitted bits as unexplained (correct: no public DB explains them).

Emitter limitation, same class as the documented F3 INT limitation: the emitter's segbits path writes the listed bit directly as the absolute frame bit (no window offset added; verified in `bitgen.rs` `emit_frames`, `write_frame_bit` takes `r.bit` as-is). The derived bits therefore land correctly only on offset-0 tiles (X9Y120, X9Y60, X24Y0). The decoder (which adds offset times 16) is the authority for attribution. The emitter test uses X9Y120. The per-tile offset rule offset equals 3 times (Y mod 60) holds for the X9 R_PCIE4 column but NOT for X24 (48 violations); never infer offsets from a formula.

## 4. B2: the INT_INTF/HDIO base FAR table

`tools/verify_xczu3eg.py` gains section 11 (INTF/HDIO derivation, verification, `--write-intf-columns` flag, `SIDECAR_INTF_PATH` env; docstring updated). It derives (tile_type, row, X) to col from windowed tiles' baseaddrs, asserts one column per key, FAR row equals Y//60, base minor 0. Generated `crates/helion-ultrascale/data/xczu3eg_intf_columns.json` (15 entries, exactly the 4 types in section 2).

Verification run: `[PASS] 283 tiles, 15 entries, conflicts=0, bad=[]`; `[PASS] vendored table equals the tilegrid derivation (disk=15 derived=15)`; overall 37/40 checks passed, the 3 failures being pre-existing part.json std-column ones (confirmed identical on the base script via git stash round-trip). Note: section 11 initially copied the pre-existing double-brace f-string bug from lines 382/436 (prints literally); the two new lines were fixed to the correct single-brace form, the pre-existing lines left untouched.

`device_file.rs` gains `IntfColumnTable` (part plus BTreeMap of (String, u32, u32) to u32 columns) with `from_path` mirroring `IntColumnTable` (extra type level); `DeviceFile` gains the `intf_columns` field (initialized None in FromStr), plus `set_intf_columns` (part-match check, clears caches), `has_intf_columns`, `intf_column`, `intf_column_entry` (returns the table's own key string, so `resolve_tile` returns a device-lifetime type), `intf_column_entries`. `lib.rs` re-exports `IntfColumnTable` for parity with `IntColumnTable`.

`bitgen_device.rs` gains `parse_intf_tile` (parses `<TYPE>_X<x>Y<y>` into type, x, y), a `resolve_tile` branch for INT_INTF/HDIO names that looks up (tile_type, row=y/60, x) via the table, builds base FAR `Far(ClbIoClk, row, col, 0)`, and returns the canonical type from the table. A (type, row, X) the table does not list fails loud. No minors-span validation applies: the table is ground truth from recorded baseaddrs. `compute_tile_names` enumerates INTF/HDIO tiles (60 per row per (type, row, X), mirroring INT). Module docs updated (HDIO and INT_INTF now resolve with a table attached).

`bitgen.rs` gains `parse_intf_tile_y` and extends the parse-back row-start rule (F3) to INT_INTF/HDIO tiles: only Y%60==0 renders, so one set feature does not render 60 times, and HDIO tiles (which exist only at row-start Y) render exactly. `helion-bitgen.rs` `load_device` wires the table for xczu3eg next to the `set_int_columns` call.

The IOB investigation's claim that "the tilegrid HAS the baseaddrs" holds for HDIO tiles but NOT for the INT_INTF_R_PCIE4 tile the IOB path needs (INT_INTF_R_PCIE4_X9Y0 has empty bits, no baseaddr); B2 handles tiles with no bits entry via the shared column baseaddr, which is why X9Y0 resolves.

## 5. site_index fixes

### Python checker (`tools/us09v/us09v/db.py`)

`site_index()` claimed the tilegrid sites dict preserves the tile_type sites list order. False for HDIO tiles: it returned the tilegrid dict position 19 where the tile_type index is 4 for IOB_X0Y0. Consumers (`fabric.site_pin_wire`, `ports.trace_pad_to_slice_pin`, `routecheck._pin_wire`) index the tile_type sites list, so they need the tile_type index.

Fix: return the tile_type-list index computed by rank among same-type sites. The tilegrid sites dict and the tile_type sites list both order same-type sites by (X, Y) (verified globally: 0 violations on all 9,457 tiles), so the rank of the site among its type in the tilegrid dict selects the same entry in the tile_type list. The tile_type list interleaves site types per coordinate (HDIO_BOT_RIGHT X0Y0 entries: HDIOBDIFFINBUF, HDIOLOGIC_M, HDIOLOGIC_S, HDIO_VREF, HDIOB_M at index 4), which is why the dict position and the list index differ.

Regression tests in `tests/test_db.py`: `site_index("IOB_X0Y0")` is `("HDIO_BOT_RIGHT_X9Y0", 4)` with the tile_type entry being HDIOB_M; `site_index("IOB_X2Y130")` is `("HPIO_RIGHT_X32Y150", 4)` with the entry being HPIOB_M (IOBPAIR_26; note IOB_X2Y130 is HPIOB_M, not HPIOB_SNGL like the last site IOB_X2Y155). The existing tests still pass (SLICE_X10Y10 index 0, IOB_X2Y155 index 51, where both orders coincide).

### Rust loader (`crates/helion-ultrascale/src/tileconn.rs`)

Same flaw: `from_db_and_sites` looped over ALL same-type sites in the tile_type file and registered every one's pins under one UltraSite; `add_pin_wire` overwrites on duplicate (site, bel, pin), so the LAST (highest-Y) site's wire won silently. `int_tile_for_site` returned None on multi-site ambiguity instead of resolving.

Fix: `SiteTile` gains `site_index: usize`, the rank among same-type sites in the tile, computed at `TileconnDb::load` time by (X, Y) comparison of site names (the tilegrid sites map is a HashMap, so order is lost and the rank is derived from the names; a site name without coordinates fails loud via a new `BadSiteName` error). `from_db_and_sites` and `int_tile_for_site` select the rank-th same-type entry via a new `same_type_site` helper; a rank beyond the file's same-type entries fails loud via a new `SiteIndexOutOfRange` error. `pin_wire_for_pin` is untouched (its None-on-ambiguous behavior is by design; it takes no site). 467 tiles have more than 1 site of the same type.

Regression test in `tileconn.rs` (separate TWO_SITE fixture, the shared TINY fixture untouched): a tile type with 2 sites of the same type and distinct OP wires. `int_tile_for_site` returns each site's own INT wire (IMUX_E10 for IOB_X0Y0, IMUX_E12 for IOB_X0Y2); `WireDb::from_db_and_sites` registers each OP under its own UltraSite with its own wire.

## 6. Emitter test and diagnostics

Tests in `crates/helion-ultrascale/tests/device_file_xczu3eg.rs` (4 new):

- `xczu3eg_intf_table_loads_and_attaches`: 15 entries; anchors R_PCIE4 (2,9) to 29, (0,24) to 75, LEFT_TERM_PSS (0,0) to 0, HDIO_BOT_RIGHT (0,9) to 30, HDIO_TOP_RIGHT (0,24) to 76 (X24 exists only in row 0 for both HDIO types); INT_INTF_L and R_PCIE4 (2,10) absent; attach and re-check through the DeviceFile API.
- `xczu3eg_intf_table_rejects_wrong_part`.
- `xczu3eg_intintf_outputs_enabled_emits_corpus_bits`: the device's base FAR for INT_INTF_R_PCIE4_X9Y120 is 0x81D00; emitting OUTPUTS_ENABLED.1 sets exactly bits (1,21), (2,27), (3,24) at FARs 0x81D01/02/03 (the FARs the decoder reports for the 5 BNN designs). Emitting OUTPUTS_ENABLED.0 sets no bit anywhere (the frame entries are materialized even for a clear, so the assertion is on set bits, not map emptiness).
- `xczu3eg_intintf_pip_still_fails_unknown_feature`: the tile now resolves (B2 fixed), but the PIP line still fails loud with `BitgenError::UnknownFeature` (tile INT_INTF_R_PCIE4_X9Y0, feature PIP.IMUXOUT16.IMUX16), not UnknownTile. This test documents the blocked state; it flips to a pass only when a Vivado-built design pins the PIP segbits.

CLI confirmation (release binary, same FASM): emit exit 0, 5,982,532-byte bitstream; the probe attributes exactly 3 bits to INT_INTF_R_PCIE4_X9Y120 at (1,21), (2,27), (3,24); the decoder reports `unexplained: FAR 0x00081d01 bit 21`, `0x00081d02 bit 27`, `0x00081d03 bit 24` (expected: the decoder stays public-DB-only). Parse-back renders `INT_INTF_R_PCIE4_X9Y120.OUTPUTS_ENABLED.1` plus `.0` on the other row-start tiles (X9Y0, X9Y60, X24Y0), so the row-start rule round-trips.

`helion-bitgen bits` fix: `classify_feature` in `src/bin/helion-bitgen.rs` did not know INT_INTF tiles, so `bits` failed loud on any bitstream containing the new feature ("parse-back feature ... matches no block in the registry"). The IO_CONFIG arm now also matches `tile.starts_with("INT_INTF")` (the PIP arm runs first, so an INT_INTF PIP still classifies as INT_PIP). `bits` now lists `00081d01 21 1 IO_CONFIG`, `00081d02 27 1 IO_CONFIG`, `00081d03 24 1 IO_CONFIG`, exit 0. Test cases added to `classify_matches_harness_registry` (OUTPUTS_ENABLED.1 to IO_CONFIG, INT_INTF PIP to INT_PIP).

## 7. Suites and stash proofs

Suites on the merged impl commit 24629800 (re-run on final code after the classify fix; the fix touches only the bin, the lib and integration counts are unchanged):

- helion-ultrascale: 441 passed / 0 failed / 1 ignored (baseline 436/0/1 on the counter tip plus 5 new: 1 tileconn, 4 device_file_xczu3eg).
- helion-cli: 73 passed / 0 failed / 1 ignored (baseline, unchanged).
- US-09 checkers: 95 tests OK (baseline 93 plus 2 new db.py tests), run with Icarus 14.0 from oss-cad-suite (`~/workspace/oss-cad-suite/bin/iverilog`). Apt 12.0 could not be installed: the platform apt process held the dpkg lock across repeated retries (installing arachne-pnr, gir1.2-atk-1.0, graphviz, libopenconnect5 in sequence). Per the established note, oss-cad-suite 14.0 gives identical checker results to apt 12.0.

Stash proof both sides:

- db.py: with the fix stashed and the new tests kept, exactly the 2 new tests fail (the other 6 pass); restored, 8/8 pass.
- tileconn: with the three fix sites reverted (int_tile_for_site, from_db_and_sites main loop, SLICE alias loop) and the test kept, the new multi-site test fails (6 passed, 1 failed); restored, 7/7 pass.
- Emitter: with all tracked changes stashed and the binary rebuilt, emit of the OUTPUTS_ENABLED FASM fails loud `unknown tile INT_INTF_R_PCIE4_X9Y120` (exit 1); restored and rebuilt, emit succeeds (exit 0) and the probe confirms the 3 bits.

## 8. Known gaps and carried items

- B1 still open: the INT_INTF_R_PCIE4 IMUX PIP segbits are unpinned (section 2). The IOB placement lane stays BLOCKED until a Vivado-built design routes through an INT_INTF PIP. The PIP test in section 6 documents this state.
- Emitter segbits path writes the listed bit as the absolute frame bit (section 3): derived bits land correctly only on offset-0 tiles. Non-offset-0 INT_INTF tiles (for example X9Y122) would emit the offset-0 tile's bits. Same F3 class as INT tiles; the decoder is the authority.
- `helion-bitgen bits` on a bitstream with INT_INTF features now works (section 6), but only because OUTPUTS_ENABLED classifies as IO_CONFIG; any future INT_INTF feature that sets bits needs its own classification or `bits` fails loud on it.
- The derived file covers OUTPUTS_ENABLED only. No other INT_INTF feature is derivable from the corpus (the PIP pool is empty, and no other INT_INTF feature has set bits anywhere).
- Carried, unchanged from the counter lane: IOB placement (blocked on B1); set_int_columns for INT_X2+; INT_INTF tile model for DSP/BRAM routing; known-INIT Vivado datum; documented xczu3eg timing; remaining 54,907 non-band unexplained bits (the INT_INTF 120 are now explained as OUTPUTS_ENABLED by the probe, but the decoder still reports them unexplained since it stays public-DB-only); N16 parse-side open; N9 block types 2+ untested; N34 stale default decoder; N35/N36 router nondeterminism (acceptance pins verify outcomes, not .bit md5); N37 report omits required --top in verify command; N38 CLI suite counts a skipped flow test as passed; flags A-CTRL and A-F7F8 still unresolved, need a CTO ruling.

## 9. Incidents

- iverilog missing after a VM restart (checkers would not run); apt 12.0 install blocked by the platform apt lock across four retries over about 15 minutes; used oss-cad-suite iverilog 14.0 instead (identical checker results per the established note). The apt lock contention is a platform seeding process, not a lane defect.
- The cargo test build for the full ultrascale suite took about 18 minutes (all bin/example test targets, -j2 on this box); the lib test binary itself builds in about a minute. The classify fix landed after the first full-suite run, so the suite was re-run on final code.
- Two of my own test assertions were wrong on first write and were corrected against the data, not the code: the HDIO_TOP_RIGHT anchor (X24 exists only in row 0, not row 2) and the OUTPUTS_ENABLED.0 expectation (write_frame_bit materializes frame entries even for a clear, so the assertion is on set bits). The db.py HPIO test initially asserted the wrong site type (IOB_X2Y130 is HPIOB_M, not HPIOB_SNGL); the index 4 was right in all three cases.
- The full `cargo test -p helion-ultrascale` run was interrupted once by the long build time, not by a failure; it completed on re-run.

## 10. Files changed (impl commit 24629800, 15 files, +1171/-71)

- `crates/helion-ultrascale/src/bin/intintf_probe.rs` (new): the corpus probe.
- `crates/helion-ultrascale/src/bitstream_decode.rs`: `TileWindow` gains grid_x/grid_y; new public `TileWindowInfo` and `tile_windows(db_root)`.
- `crates/helion-ultrascale/data/segbits_int_intf_r_pcie4.db` (new): the derived OUTPUTS_ENABLED segbits.
- `crates/helion-ultrascale/data/PROVENANCE.md`: Helion-derived segbits section.
- `crates/helion-ultrascale/data/xczu3eg_intf_columns.json` (new): the 15-entry INTF/HDIO table.
- `crates/helion-ultrascale/tools/verify_xczu3eg.py`: section 11.
- `crates/helion-ultrascale/src/device_file.rs`: `IntfColumnTable`, DeviceFile intf_columns accessors.
- `crates/helion-ultrascale/src/bitgen_device.rs`: `parse_intf_tile`, resolve_tile branch, compute_tile_names, docs.
- `crates/helion-ultrascale/src/bitgen.rs`: `parse_intf_tile_y`, parse-back row-start rule for INTF/HDIO.
- `crates/helion-ultrascale/src/bin/helion-bitgen.rs`: SEGBITS_FILES 9 to 10, xczu3eg INTF wiring, classify_feature IO_CONFIG arm.
- `crates/helion-ultrascale/src/lib.rs`: `IntfColumnTable` re-export.
- `crates/helion-ultrascale/src/tileconn.rs`: `SiteTile.site_index`, `same_type_site`, `BadSiteName` / `SiteIndexOutOfRange`, fix sites, TWO_SITE fixture and test.
- `crates/helion-ultrascale/tests/device_file_xczu3eg.rs`: 4 new tests.
- `tools/us09v/us09v/db.py`, `tools/us09v/tests/test_db.py`: site_index fix and 2 new tests.
