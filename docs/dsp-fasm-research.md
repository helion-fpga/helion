# DSP48E1 FASM Emission Research Needed

## Current State
- IR: `Dsp48e1 { opmode, alumode }` defined with pins A/B/C/P
- Synthesis: emits Dsp48e1 with OPMODE=0b0000101 (P=A*B+C)
- Pack: `PackedDsp { name, opmode, alumode }`
- Place: `PlacedDsp { name, tile, site, opmode, alumode }`
- Emit: **NOT IMPLEMENTED** - `let _ = &placed.dsps;` (explicitly skipped)

## What's Needed

Project X-Ray documentation for DSP48E1 FASM features. The emit.rs
needs to generate FASM lines like:

```
TILE.DSP48E1_SITE.OPMODE[6:0] = 0b0000101
TILE.DSP48E1_SITE.ALUMODE[3:0] = 0b0000
TILE.DSP48E1_SITE.INMODE[4:0] = ...
```

### Required Research
1. Find Project X-Ray `db/` files for DSP48E1
   - Look in `crates/helion-xilinx7/src/db/` for DSP tiles
   - Check if X-Ray has DSP48E1 documented (it may be partial)

2. Key FASM features needed (from UG479):
   - `USE_MULT`: "MULTIPLY" vs "NONE"
   - `OPMODE[6:0]`: Operation mode
   - `ALUMODE[3:0]`: ALU mode  
   - `INMODE[4:0]`: Input mode
   - `AREG`, `BREG`, `CREG`, `DREG`, `MREG`, `PREG`: Pipeline registers (0/1/2)
   - `A_INPUT`, `B_INPUT`: "DIRECT" vs "CASCADE"

3. For Helion v1 (combinational multiply-add):
   - `USE_MULT = "MULTIPLY"`
   - `OPMODE = 0b0000101`
   - `ALUMODE = 0b0000`
   - All `*REG = 0` (combinational, no pipeline)
   - `A_INPUT = "DIRECT"`, `B_INPUT = "DIRECT"`

### Implementation Sketch
```rust
for dsp in &placed.dsps {
    let site = &dsp.site; // e.g., "DSP48E1_X0Y0"
    writeln!(fasm, "{tile}.{site}.USE_MULT = \"MULTIPLY\"")?;
    writeln!(fasm, "{tile}.{site}.OPMODE[6] = 0b{}", (dsp.opmode >> 6) & 1)?;
    // ... etc for each bit
}
```

### Blockers
- X-Ray DSP documentation may be incomplete
- Need to verify FASM feature names against X-Ray db
- Bitstream generation for DSP not yet tested

## X-Ray Database Findings (2026-10-15)

Cloned `https://github.com/SymbiFlow/prjxray` (shallow).
Analyzed `fuzzers/100-dsp-mskpat/generate.py`.

### Key Discoveries

1. **Site naming**: DSP sites use `DSP_0` or `DSP_1` (not `DSP48E1_X0Y0`).
   - Selection: `dsp = "DSP_0" if params['SITE'][-1] in "02468" else "DSP_1"`
   - Even Y coordinate -> DSP_0, odd Y -> DSP_1

2. **OPMODE/ALUMODE configuration**: Via `IS_*_INVERTED` parameters, not direct.
   - `IS_OPMODE_INVERTED[6:0]` (7 bits)
   - `IS_ALUMODE_INVERTED[3:0]` (4 bits)
   - `IS_INMODE_INVERTED[4:0]` (5 bits)
   - `IS_CARRYIN_INVERTED` (1 bit)

3. **Tag format**: `{DSP_0|DSP_1}.Z{IS_OPMODE_INVERTED}[{bit}]`
   - The `Z` prefix indicates inverted logic in X-Ray database
   - Example from generate.py: `add(segmk, site, dsp, 'OPMODEREG', 0, ...)`

4. **49 unique DSP tags** found in generate.py, including:
   - Pipeline regs: `ADREG`, `ALUMODEREG`, `AREG`, `BREG`, `CREG`, `DREG`, `MREG`, `PREG`, `CARRYINREG`, `CARRYINSELREG`, `INMODEREG`, `OPMODEREG`
   - Config: `AREG_0`, `AREG_2`, `BREG_0`, `BREG_2`, `ACASCREG`, `BCASCREG`
   - Modes: `USE_MULT`, `USE_DPORT`, `USE_PATTERN_DETECT`, `USE_SIMD`

### FASM Emission Strategy

For `Dsp48e1 { opmode: u8, alumode: u8 }`:
1. Determine site: `DSP_0` or `DSP_1` based on Y coordinate
2. For each opmode bit i (0-6):
   - If bit is 1: emit `{site}.ZIS_OPMODE_INVERTED[{i}]` (or omit if default?)
   - If bit is 0: omit (or emit inverted?)
3. Same for alumode bits 0-3 with `IS_ALUMODE_INVERTED`

**TODO:** Verify exact FASM syntax by examining a real X-Ray generated FASM file,
or by running the fuzzer (requires Vivado 2017.2, not available).

## References
- UG479: 7 Series DSP48E1 User Guide
- Project X-Ray: https://github.com/SymbiFlow/prjxray
- Fuzzer: `fuzzers/100-dsp-mskpat/generate.py` (analyzed 2026-10-15)
