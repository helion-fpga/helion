# Timing Model Calibration Plan

## Current State

Timing models in `crates/helion-backend/src/lib.rs` (`struct TimingModel`):

| Device | lut_ps | ff_ckq_ps | setup_ps | pin_ps | iob_ps | hop_ps | Status |
|--------|--------|-----------|----------|--------|--------|--------|--------|
| HL10T | 150 | 80 | 50 | 20 | 100 | 40 | Gold (WNS_PS=9640) |
| Xilinx 7s | 180 | 380 | 120 | 30 | 1000 | 70 | Preliminary, NOT correlated |
| iCE40 | ? | ? | ? | ? | ? | ? | TBD |
| ECP5 | ? | ? | ? | ? | ? | ? | TBD |

The 7-series values follow DS181 (-1 speed grade) ranges but have NOT been
validated against Vivado timing reports.

## Calibration Methodology

### Phase 1: Vivado Correlation (needs Vivado access)

1. **Reference designs**: Create N small designs (counter, shift register,
   simple FSM, adder chain) that exercise:
   - LUT6 A->O arcs (all 6 inputs)
   - FD C->Q, D setup/hold
   - IOB pad delays (various IOSTANDARDs)
   - Routing hops (short, medium, long nets)

2. **Vivado flow**: For each design:
   - Synthesize with Vivado
   - Place and route
   - Generate timing report (`report_timing -delay_type max`)
   - Extract per-arc delays from report

3. **Fit model**: For each TimingModel field, compute:
   - Mean and stddev across measurements
   - Per-pin variation (LUT inputs A1-A6 have different delays)
   - Temperature/voltage corners (if available)

4. **Validate**: Run Helion STA on same designs, compare WNS/slack.
   Target: within 10% of Vivado.

### Phase 2: Hardware Measurement (needs hardware)

1. **Ring oscillators**: Implement RO with N LUTs, measure frequency.
   - LUT delay = 1 / (2 * N * freq)
   - Vary N to isolate LUT vs routing

2. **FF characterization**: 
   - Clock-to-Q: launch from FF, capture with FF, measure max freq
   - Setup/hold: sweep data vs clock phase

3. **IOB**: Loopback test, measure pad delay.

### Phase 3: Per-Device Models

For each new device family:
1. Obtain vendor timing documentation (DS181 for 7-series, etc.)
2. Run Phase 1 correlation if vendor tools available
3. Otherwise, use datasheet values with derating margins
4. Document confidence level for each value

## Current Blockers

- No Vivado access (USF cluster requires VPN + SAML, not available from sandbox)
- No hardware yet (ZedBoard pickup pending)
- iCE40/ECP5: need IceStorm/Trellis timing data or nextpnr reports

## Immediate Actions (no hardware needed)

1. Review 7-series values against DS181 tables more carefully
   - LUT6 delays vary by input pin (A6 fastest, A1 slowest)
   - Consider per-pin LUT model instead of single value
2. Add timing model validation tests
   - Check that WNS is monotonic with logic depth
   - Check that placement affects WNS via hop_ps
3. Document confidence levels in code

## Files
- Model: `crates/helion-backend/src/lib.rs` (`TimingModel`)
- STA: `crates/helion-sta/src/lib.rs`
- Gold test: `crates/helion-cli/tests/qor.rs` (WNS_PS=9640)
