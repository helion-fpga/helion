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

## References
- UG479: 7 Series DSP48E1 User Guide
- Project X-Ray: https://github.com/SymbiFlow/prjxray
- X-Ray db: `database/artix7/` (check for DSP tiles)

## X-Ray Feature Name Patterns (To Verify)

### OPMODE[6:0]
Based on X-Ray segbits database structure, expected patterns:
- `DSP48E1_X0Y0.DSP48E1.OPMODE0` through `OPMODE6`
- Each bit is a separate feature; set bits are listed in FASM

### ALUMODE[3:0]
- `DSP48E1_X0Y0.DSP48E1.ALUMODE0` through `ALUMODE3`

### Pipeline Registers
- `DSP48E1.AREG0`, `AREG1` (A input pipeline stages)
- `DSP48E1.BREG0`, `BREG1` (B input pipeline stages)
- `DSP48E1.CREG0` (C input pipeline)
- `DSP48E1.MREG` (Multiplier pipeline)
- `DSP48E1.PREG` (P output pipeline)

**Verification needed:** Clone prjxray, check `database/artix7/segbits_dsp_*.db`
for exact feature strings.
