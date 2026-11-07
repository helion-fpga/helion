# demo.py reconstruction validation on xcku3p (control)

Date: 2026-11-07. No real xcku3p bitstream exists in the workspace, so
the control is a synthetic minimal bitstream built by
`make_synth_xcku3p_bit.py` from the reference BitLocator. It carries
exactly the frames the reference resolves for three BELs, one per
kind, each frame with a marker bit set at every located frame offset.

## Command

```sh
cd crates/helion-ultrascale/scripts
python3 make_synth_xcku3p_bit.py
```

That script builds `synth_xcku3p.bit`, re-parses it with the reference
`Bitstream` class (self-check 1), then runs demo.py itself (self-check
2). The demo.py invocation inside the script is:

```sh
cd ~/workspace/helion-ultra-bitstream/bitfiltrator-ref/src
python3 demo.py <path>/synth_xcku3p.bit \
  --luts SLICE_X23Y45/B6LUT \
  --ffs SLICE_X23Y45/BFF \
  --brams RAMB18_X0Y0
```

## Output

Script output, verbatim:

```text
BELs: 3, distinct FARs: 261
  SLICE_X23Y45/B6LUT: 64 pairs, slr SLR0
  SLICE_X23Y45/BFF: 1 pairs, slr SLR0
  RAMB18_X0Y0: 18432 pairs, slr SLR0
wrote .../scripts/synth_xcku3p.bit (100369 bytes)
self-check 1 PASS: 261 FARs, all marker bits set, byte offsets exact
self-check 2: demo.py exited 0, 18503 output lines
self-check 2 PASS: demo.py output matches the reference locator and
the built byte offsets on all 18,497 located bits
wrote .../scripts/synth_xcku3p_expected.json
wrote demo_xcku3p_lut_ff.log, demo_xcku3p_bram_summary.txt
ALL CHECKS PASSED
```

demo.py output for the LUT and FF, verbatim (`demo_xcku3p_lut_ff.log`,
first lines; the full log is committed next to this doc):

```text
SLICE_X23Y45/B6LUT
INIT[ 0] -> SLR0, frame address: 0x00003007 (BLOCK_TYPE = CLB_IO_CLK, ROW_ADDR =   0, COL_ADDR =  48, MINOR_ADDR =   7), frame offset: 2271, frame byte offset in bitstream: 1285
INIT[ 1] -> SLR0, frame address: 0x00003006 (BLOCK_TYPE = CLB_IO_CLK, ROW_ADDR =   0, COL_ADDR =  48, MINOR_ADDR =   6), frame offset: 2271, frame byte offset in bitstream: 901
INIT[ 2] -> SLR0, frame address: 0x00003005 (BLOCK_TYPE = CLB_IO_CLK, ROW_ADDR =   0, COL_ADDR =  48, MINOR_ADDR =   5), frame offset: 2271, frame byte offset in bitstream: 517
INIT[ 3] -> SLR0, frame address: 0x00003004 (BLOCK_TYPE = CLB_IO_CLK, ROW_ADDR =   0, COL_ADDR =  48, MINOR_ADDR =   4), frame offset: 2271, frame byte offset in bitstream: 133
...
INIT[63] -> SLR0, frame address: 0x00003004 (BLOCK_TYPE = CLB_IO_CLK, ROW_ADDR =   0, COL_ADDR =  48, MINOR_ADDR =   4), frame offset: 2256, frame byte offset in bitstream: 133

SLICE_X23Y45/BFF
INIT -> SLR0, frame address: 0x0000300c (BLOCK_TYPE = CLB_IO_CLK, ROW_ADDR =   0, COL_ADDR =  48, MINOR_ADDR =  12), frame offset: 2260, frame byte offset in bitstream: 1669
```

BRAM output (`demo_xcku3p_bram_summary.txt`; the full 18,434-line
output is verified programmatically by the generator, not pasted):

```text
demo.py --brams RAMB18_X0Y0: 18434 lines (1 header + 16384 INIT + 2048 INIT_P + 1 trailing blank)
first INIT line: INIT[    0] -> SLR0, frame address: 0x01000000 (BLOCK_TYPE = BRAM_CONTENT, ROW_ADDR =   0, COL_ADDR =   0, MINOR_ADDR =   0), frame offset:    0, frame byte offset in bitstream: 2053
last INIT_P line: INIT_P[ 2047] -> SLR0, frame address: 0x010000ff (BLOCK_TYPE = BRAM_CONTENT, ROW_ADDR =   0, COL_ADDR =   0, MINOR_ADDR = 255), frame offset:   59, frame byte offset in bitstream: 99973
```

## What this proves

- The reference demo.py, run on a bitstream built from reference
  FARs, resolves the same FARs, frame offsets, and byte offsets the
  generator computed: all 18,497 located bits (64 LUT + 1 FF + 16384
  BRAM content + 2048 BRAM parity) match exactly, including every
  frame byte offset in the file. The synthetic bitstream is therefore
  a valid control input for demo.py, not just a file the parser
  happens to accept.
- The Helion Rust `BitLocator` agrees with the same reference values:
  `tests/device_file_demo.rs` compares the loader against
  `synth_xcku3p_expected.json` (LUT and FF pair-for-pair; BRAM by
  distinct-FAR sets, first 8 pairs, and FNV-1a digests of the full
  ordered pair lists). `cargo test -p helion-ultrascale` runs it.
- The xcku3p device file loads and resolves real BELs end to end:
  SLICE_X23Y45 maps to col major 48, tile CLEL_L, row 0; RAMB18_X0Y0
  maps to BRAM content col major 0, 256 distinct content FARs and 256
  distinct parity FARs.

## What this does NOT prove

- Vivado agreement (tier 2). The marker bits are ours, placed at
  reference-computed offsets; nothing here says Vivado sets those
  bits for these BELs. Tier 2 stays blocked on the license server
  (connection refused since Oct 22, 2026).
- The 16-frame UG570 gap (41,460 file frames vs 41,476 listed for
  KU3P) is unchanged by this run: the synthetic bitstream only
  carries the 261 FARs the three BELs need, not the full frame set.
  See `data/README.md`.
- No last-of-row FAR was exercised: none of the 261 FARs is the last
  FAR of its row, so the generator's 2-zero-padding path for
  last-of-row FARs is untested. It is implemented per the reference
  parser's rule (`NUM_END_OF_ROW_PADDING_FRAMES = 2`) and will be
  exercised the first time a validated BEL lands on a last-of-row FAR.
