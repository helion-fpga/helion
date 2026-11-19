# URAM288 config-bit and content encoding: search record

Status: MISSING in every public source searched. No URAM288 config-bit
or content encoding is derivable from public sources. Do not invent
one. Designs using a URAM288 fail loud (`BitgenError::UramBlocked`,
see `crates/helion-ultrascale/src/bitgen.rs` and
`crates/helion-ultrascale/src/uram.rs`) until Vivado fuzzing job 5
lands the encoding.

Searched 2026-11-19 by the URAM288 worker (session 18, second
replacement), on branch `fm/ultra-bitgen-uram`, worktree
`~/workspace/helion-ultra-bitgen-uram`. Every command below was run,
and every output below is the real output (trimmed only where noted).
Nothing here is inferred from memory.

## Sources searched

1. prjuray-db reference clone, `~/workspace/prjuray-db-ref`,
   commit `affbc5e` (2020-07-21). Its `COPYING` file opens
   "Creative Commons Legal Code / CC0 1.0 Universal". This is the
   same clone the segbits-vendor worker verified (COORD.md,
   2026-11-07).
2. BitFiltrator reference clone,
   `~/workspace/helion-ultra-bitstream/bitfiltrator-ref/`, commit
   `79b9bd6`. MIT License, Copyright (c) 2022 EPFL VLSC (same commit
   the segbits-vendor worker verified).
3. The vendored copies under `crates/helion-ultrascale/data/` in this
   worktree (same content as the clones for the files checked).
4. AMD documentation, read on docs.amd.com 2026-11-19: UG974
   (Vivado UltraScale Libraries, 2023.1 English, URAM288 page),
   UG573 (UltraScale Memory Resources, UltraRAM Summary), and WP477
   (UltraRAM white paper, v1.0, June 14, 2016).

## 1. prjuray-db has no URAM segbits file and no URAM mention

Command:

```sh
ls ~/workspace/prjuray-db-ref/zynqusp/ | grep -i uram
grep -ri uram ~/workspace/prjuray-db-ref/zynqusp/
```

Output: nothing. No file name contains `uram`, and no file content
mentions it (case-insensitive). There is no `segbits_uram*.db` and
no `segbits_rclk_uram*.db` (contrast DSP, which at least has
`segbits_rclk_dsp_intf_{l,r,clkbuf_l}.db` clock-interface files).
The full `zynqusp/` segbits listing contains `segbits_bram*.db`,
`segbits_clel_*.db`, `segbits_clem*.db`, `segbits_cmt_right.db`,
`segbits_hdio_*.db`, `segbits_hpio_right.db`, `segbits_int.db`,
`segbits_int_intf_left_term_pss.db`, `segbits_rclk_*.db`,
`segbits_xiphy_byte_right.db`, and nothing for a URAM tile type.

## 2. The vendored segbits DBs carry zero URAM lines

Command (in this worktree):

```sh
for f in crates/helion-ultrascale/data/segbits_*.db; do
  c=$(grep -ci uram "$f"); echo "$c $f";
done
```

Output: `0` for every one of the 18 vendored `.db` files
(`segbits_clel_l`, `segbits_clel_r`, `segbits_clem`,
`segbits_clem_r`, `segbits_cmt_right`, `segbits_hdio_bot_right`,
`segbits_hdio_top_right`, `segbits_hpio_right`, `segbits_int`,
plus their `.origin_info.db` provenance files). Total URAM lines: 0.

## 3. BitFiltrator ULTRASCALE_PLUS.json has no URAM table

Command (Python, on the vendored copy and the reference clone):

```python
d = json.load(open('crates/helion-ultrascale/data/ULTRASCALE_PLUS.json'))
sorted(d.keys())
open('crates/helion-ultrascale/data/ULTRASCALE_PLUS.json').read().count('URAM')
```

Output:

```text
top keys: ['BRAM', 'CLEL_L', 'CLEL_R', 'CLEM', 'CLEM_R']
uram mentions in json text: 0
```

The same file's `ultrascale_plus_lut_ff_bram.json` extraction
carries only the four CLB tile types plus BRAM. There is no URAM
content table to mirror the BRAM `BramMemLoc` / `BramMemParityLoc`
tables, and no URAM site-type JSON is vendored (contrast
`site_type_DSP48E2.json`, which at least documents DSP pins).

## 4. What the device file does know: URAM tiles exist, sites exist

The BitFiltrator device files name URAM tiles but carry no
encoding. Command (Python, on the vendored copies):

```python
d = json.load(open('crates/helion-ultrascale/data/xcku3p.json'))
[p for p in d['tileType_siteType_pairs'] if 'URAM' in str(p)]
```

Output (identical on `xcvu3p.json`):

```text
[['RCLK_RCLK_URAM_INTF_L_FT', 'BUFCE_ROW_FSR'],
 ['URAM_URAM_DELAY_FT', 'URAM288'],
 ['URAM_URAM_FT', 'URAM288']]
```

So the tile types `URAM_URAM_FT` and `URAM_URAM_DELAY_FT` hold
`URAM288` sites, and `RCLK_RCLK_URAM_INTF_L_FT` is a clock-interface
tile adjacent to a URAM column (BUFCE_ROW_FSR, clock routing, not
URAM configuration). Counts, already grounded in `src/device.rs`
from DS890: xcku3p 48 URAM288 blocks (13.5 Mb / 288 Kb), xcvu3p
320 blocks (90.0 Mb / 288 Kb).

## 5. UG974: the URAM288 primitive has 34 attributes and no INIT

Source: AMD UG974, 2023.1 English, URAM288 page
(`docs.amd.com/r/2023.1-English/ug974-vivado-ultrascale-libraries/URAM288`),
"Available Attributes" table, read in full 2026-11-19. The 34
attributes, with defaults, are:

| Attribute | Default |
|---|---|
| AUTO_SLEEP_LATENCY | 8 |
| AVG_CONS_INACTIVE_CYCLES | 10 |
| BWE_MODE_A | "PARITY_INTERLEAVED" |
| BWE_MODE_B | "PARITY_INTERLEAVED" |
| CASCADE_ORDER_A | "NONE" |
| CASCADE_ORDER_B | "NONE" |
| EN_AUTO_SLEEP_MODE | "FALSE" |
| EN_ECC_RD_A | "FALSE" |
| EN_ECC_RD_B | "FALSE" |
| EN_ECC_WR_A | "FALSE" |
| EN_ECC_WR_B | "FALSE" |
| IREG_PRE_A | "FALSE" |
| IREG_PRE_B | "FALSE" |
| IS_CLK_INVERTED | 1'b0 |
| IS_EN_A_INVERTED | 1'b0 |
| IS_EN_B_INVERTED | 1'b0 |
| IS_RDB_WR_A_INVERTED | 1'b0 |
| IS_RDB_WR_B_INVERTED | 1'b0 |
| IS_RST_A_INVERTED | 1'b0 |
| IS_RST_B_INVERTED | 1'b0 |
| OREG_A | "FALSE" |
| OREG_B | "FALSE" |
| OREG_ECC_A | "FALSE" |
| OREG_ECC_B | "FALSE" |
| REG_CAS_A | "FALSE" |
| REG_CAS_B | "FALSE" |
| RST_MODE_A | "SYNC" |
| RST_MODE_B | "SYNC" |
| SELF_ADDR_A | 11'h000 |
| SELF_ADDR_B | 11'h000 |
| SELF_MASK_A | 11'h7ff |
| SELF_MASK_B | 11'h7ff |
| USE_EXT_CE_A | "FALSE" |
| USE_EXT_CE_B | "FALSE" |

No `INIT_xxx` attribute appears anywhere on the page. That is the
key difference from RAMB36E2, whose INIT words are the BRAM content
sweep: a URAM288 has no per-word INIT attribute to vary, so the
BRAM content-sweep method cannot be copied. The content-sweep
mechanism job 5 uses instead is the XPM memory initialization file
(see section 7).

Trap avoided: do not copy INIT attributes from the Versal
URAM288E5 page (UG1485) or the Versal AM007 INIT_XX page. Those
document `INIT_000` through `INIT_3FFF` (1024 attributes of 288
hex digits) for the Versal primitive, not for UltraScale+
URAM288. The UltraScale+ UG974 page above is the authority for
this part, and it lists no INIT attribute.

## 6. UG573 and WP477: geometry, two independent sources

Both AMD sources agree, and both were read 2026-11-19:

- UG573, UltraRAM Summary: "Each UltraRAM block can store up to
  288K bits of data and is configured as a 4K x 72 memory block.
  UltraRAM has eight times the capacity of a block RAM." Also:
  "There are 16 UltraRAM blocks per clock region per column."
- WP477 (v1.0, June 14, 2016): "Every UltraRAM block is a dual-port
  synchronous 288Kb RAM with fixed configuration of 4,096 deep and
  72 bits wide."

So one URAM288 holds 4096 words x 72 bits = 294,912 content bits
(4096 x 72), with 64 data bits plus 8 parity/ECC bits per word when
ECC is on (WP477: "With ECC enabled, UltraRAM offers 64 bits wide
protected data words"). The 294,912 figure is the size of the
content table job 5 must land; it is not swept one bit at a time
(294,912 runs is not a job), so job 5 sweeps probe positions and
derives the structure, exactly as the job doc specifies.

## 7. Content initialization path for job 5 (sourced, not invented)

UG974 XPM_MEMORY_SPRAM (2024.1 English, families UltraScale and
UltraScale+), read 2026-11-19: `MEMORY_INIT_FILE` names a `.mem`
file of hex values "organized into the specified depth by narrowest
data width generic value of the memory", and `IGNORE_INIT_SYNTH`
0 means the initialization file "will appl[y] for both simulation
and synthesis" (1 means simulation only). The same page discusses
UltraRAM register control and ECC for this macro, so an XPM
single-port RAM with `MEMORY_PRIMITIVE` set to ultra is the
documented route to put chosen initial contents into a URAM288.
Job 5 pass B uses exactly that: one `.mem` file per probe variant,
and the run script asserts after synthesis that exactly one
URAM288 cell exists, so if synthesis maps the XPM macro elsewhere
the job fails loud on the cluster instead of silently diffing the
wrong cell. The `.mem` mechanism itself is documented; whether a
given Vivado version honors it for URAM on xcvu3p is a job-5
runtime check, recorded in `runs.txt`, not assumed here.

## 8. What is still unknown (the job-5 unknowns, stated plainly)

- Which frame addresses hold URAM288 content bits, and the
  (minor, frame_ofst) mapping for each of the 294,912 content
  bits. No public source states it; the FAR block type for URAM
  content frames is not stated in any public source either (UG570
  via BitFiltrator names block types 0 CLB_IO_CLK and 1
  BRAM_CONTENT; no URAM block type is named).
- Which bits each of the 34 attributes sets. No public source
  states any of them.
- The Vivado tile name for a URAM tile (the harness golden uses
  the provisional `URAM_X0Y0`, flagged provisional; job 5
  resolves the real site with `get_sites` at run time and does not
  depend on the golden spelling).
- The URAM column frame layout on xcvu3p (how many URAM columns,
  which frames each column occupies). Job 5 pass C enumerates it
  from Vivado `get_tiles` / the `.ll` files at run time.

## Consequence

Until job 5 lands, `uram_content_locations` in `src/bitloc.rs`
fails with `BitLocError::UramBlocked`, `emit_frames` in
`src/bitgen.rs` fails with `BitgenError::UramBlocked` for any
URAM tile, tile type, feature, or bare URAM288 attribute name, and
the harness reports the URAM288 block as blocked with the missing
data named. No bit is ever emitted for a URAM design.
