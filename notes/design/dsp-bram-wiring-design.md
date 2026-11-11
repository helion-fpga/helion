# 7-Series DSP/BRAM End-to-End Wiring Design

## Current State

### DSP (DSP48E1)
- IR: `CellKind::Mac27` - simple marker, no pins, no config
- Pack: `PackedDsp { name: String }` - placeholder
- Place: DSP sites exist in device DB, but no pin-aware placement
- Route: No DSP pin routing
- Emit: No DSP configuration bits

Comment in pack.rs: "Pin-level DSP interface modeling (A/B/C/P routing)
is future work at the IR level."

### BRAM (RAMB18E1/RAMB36E1)
- IR: `CellKind::Bram18` - simple marker
- Pack: `PackedBram` - likely similar placeholder
- Status: Similar to DSP, needs full wiring

## DSP48E1 Interface (from UG479)

### Ports
- A[29:0]: 30-bit A input (concatenated A1[29:18] + A2[17:0])
- B[17:0]: 18-bit B input
- C[47:0]: 48-bit C input
- D[24:0]: 25-bit D input (pre-adder)
- P[47:0]: 48-bit output
- CLK: Clock
- CEA1, CEA2, CEB1, CEB2, CEC, CED, CEM, CEP: Clock enables
- RSTA, RSTB, RSTC, RSTD, RSTM, RSTP: Resets

### Key Config
- OPMODE[6:0]: Operation mode (multiply, add, accumulate, etc.)
- ALUMODE[3:0]: ALU mode
- INMODE[4:0]: Input mode (pre-adder config)

### For Helion v1
Simplify to combinational multiply-add:
- A[24:0] * B[17:0] + C[47:0] -> P[47:0]
- OPMODE = 0b0000101 (multiply-add)
- All registers bypassed (combinational)

## Implementation Plan

### Phase 1: IR Cell Definition (1-2 days)
Replace `CellKind::Mac27` with:
```rust
Dsp48e1 {
    a_width: u8,  // 1-30
    b_width: u8,  // 1-18
    // C always 48-bit, P always 48-bit
    opmode: u8,   // 7-bit OPMODE
}
```

Pin naming convention:
- `A[29:0]`, `B[17:0]`, `C[47:0]`, `P[47:0]`, `CLK`, etc.
- Use bus notation in net names

### Phase 2: Synthesis Inference (2-3 days)
In helion-sv, detect:
- `a * b` -> DSP (if widths fit)
- `a * b + c` -> DSP with C input
- `a * b + acc` (registered) -> DSP with MREG/PREG

Update `Mac27` emission to new `Dsp48e1` with proper pins.

### Phase 3: Pack (1 day)
Update `pack.rs`:
- Match `CellKind::Dsp48e1`
- Create `PackedDsp` with pin list
- Validate widths fit in DSP48E1

### Phase 4: Place (2-3 days)
- DSP sites are in device DB (check xdevice.rs)
- Place DSPs on DSP48E1 sites
- Respect columnar DSP placement (DSPs are in columns)

### Phase 5: Route (3-4 days)
- Route A/B/C inputs to DSP pins
- Route P output from DSP
- DSP pins have specific routing requirements

### Phase 6: Bitstream Emit (3-4 days)
- Generate FASM for DSP48E1 configuration
- OPMODE, ALUMODE, INMODE bits
- Need Project X-Ray DSP config documentation

## BRAM (RAMB18E1)

Similar phases:
1. IR: Replace `Bram18` with proper port definition
   - ADDRARDADDR[13:0], ADDRBWRADDR[13:0]
   - DIADI[15:0], DIBDI[15:0], DOADO[15:0], DOBDO[15:0]
   - CLKARDCLK, CLKBWRCLK, ENARDEN, ENBWREN, etc.
2. Synthesis: Infer from `reg [15:0] mem [0:1023]` patterns
3. Pack/Place/Route/Emit

## Testing
- Unit: DSP inference from simple multiply
- Integration: Full flow with DSP design
- Bitstream: Verify with openXC7 or hardware (when available)

## References
- UG479: 7 Series DSP48E1 User Guide
- UG473: 7 Series Memory Resources (BRAM)
- Project X-Ray: DSP/BRAM FASM documentation
