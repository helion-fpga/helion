# Vendored BitFiltrator tables (LUT / FF / BRAM bit locations)

## What is here

`ultrascale_plus_lut_ff_bram.json`: the four bit-location tables the
lut-ff-bram worker consumes, extracted verbatim from the BitFiltrator arch
JSON. No value was changed, added, or recomputed. The extraction is
reproducible with `generate_tables.py`.

- `LutLoc`, `RegLoc` for all four CLB tile types (`CLEL_L`, `CLEL_R`,
  `CLEM`, `CLEM_R`): 60 `Y_ofst` entries each. Per Y: 8 LUTs with 64
  (`minor`, `frame_ofst`) pairs each, 16 FFs with one pair each.
- `BramMemLoc`: 24 `Y_ofst` entries, 16384 (`minor`, `frame_ofst`) pairs
  each. `BramMemParityLoc`: 24 `Y_ofst` entries, 2048 pairs each.

## Provenance

- Source file: `bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json`
  (BitFiltrator, EPFL VLSC).
- Source SHA-256:
  `c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c`
- License: MIT, Copyright (c) 2022 EPFL VLSC. Full text in the
  BitFiltrator clone (`bitfiltrator-ref/LICENSE`); the MIT permission
  notice covers this copy.
- Extracted: 2026-11-07, with `generate_tables.py`, which asserts the
  table counts above and warns if the source hash changes.

## Reading the tables

`minor` is the minor address of the configuration frame holding the bit;
`frame_ofst` is the bit offset within that 93-word frame. BitLocator in
the BitFiltrator reference (`src/bit_locator.py`, `src/arch_summary.py`)
is the authority for the lookup order:

- LUT: list index is the INIT bit index (0..63). `y_ofst` is the slice Y
  modulo 60 (clock-region height, `ArchSpec.num_clb_per_column`).
- FF: one (`minor`, `frame_ofst`) pair per FF name (`AFF`..`HFF2`).
  BitFiltrator names FFs `AFF`, Helion BELs name the same storage
  element `AQ` (UG574 Q output pin); the mapping is AFF=AQ, AFF2=AQ2,
  and so on through HFF2=HQ2.
- BRAM: list index is the absolute content bit, `INIT` word * 256 + bit
  (see `src/u-ray_bram_comparator.py`: `abs_idx = init_idx * 256 +
  bit_idx`). 16384 content bits are `INIT_00`..`INIT_3F` of one RAMB18;
  2048 parity bits are `INITP_00`..`INITP_07`. `y_ofst` is the RAMB18 Y
  modulo 24 (`ArchSpec.num_18k_bram_per_column`).

## What is NOT here

- No LUTRAM table exists in the source JSON, and the BitFiltrator paper
  section V-B does not state that LUTRAM uses the same INIT bits as
  LUTs (it describes LUTRAM/register formats as discovered separately
  from the logic location file, while standard LUTs were fuzzed). LUTRAM
  emission is therefore blocked, fail loud, until a LUTRAM table or
  Vivado logic-location data lands. See `src/bitloc.rs`.
- No INT PIP, IO, clock, DSP, or URAM encodings. Those belong to other
  workers.
- The `y_ofst` tables are per-column-relative. Absolute frame addresses
  also need the device file (row major, column major, SLR), owned by the
  device-file worker.

## Cross-checks performed at extraction time

- `LutLoc` and `RegLoc` are byte-identical across all four CLB tile
  types in the source file.
- The frame offsets are NOT a clean 48-per-Y formula: they jump by an
  extra 96 at Y_ofst 30 (clock-region boundary). Tables are vendored in
  full for that reason; do not replace them with a formula.
- Paper cross-check: section V-C works `SLICE_X136Y247/A6LUT` bit 0 as
  minor 11, frame offset 351. The table gives the same pair at
  `Y_ofst` 7 (247 mod 60), A6LUT, bit 0.
