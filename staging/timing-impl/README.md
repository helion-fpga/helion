# Timing implementation (staging)

This directory contains the timing agent's implementation, designed to integrate
with device-model's `helion-ultrascale` crate (commit 7a4146f on fm/ultra-device).

## Integration plan

Device-model defines in `fabric.rs`:
```rust
pub trait TimingDb: Send + Sync {
    fn has_data(&self) -> bool;
}
pub struct EmptyTimingDb; // placeholder
```

This implementation provides:
- `UltraScaleTimingDb`: concrete struct implementing `TimingDb` with `has_data() -> true`
- `SourcedDelay`: provenance-tracked delay values (Option<ps>, source)
- `preliminary_xcvu3p()`: ballpark values, PRELIMINARY marked
- `TimingReport` + `sta_routed()`: STA with hang_diag-compatible output

## Files

- `src/timing_db.rs`: SourcedDelay, delay table structs, preliminary_xcvu3p()
- `src/sta.rs`: TimingReport, sta_routed(), StaNetlist trait
- `src/lib.rs`: module declarations + TimingDb trait impl
- `tests/sta_sanity.rs`: 3 regression tests (all pass)

## To integrate

Copy `src/timing_db.rs` and `src/sta.rs` into device-model's
`crates/helion-ultrascale/src/`, add `pub mod timing_db; pub mod sta;` to
their `lib.rs`, and implement their `TimingDb` trait for `UltraScaleTimingDb`.

## Test status

3 tests pass (run 2026-10-22):
- sta_counter_sane: 810ps path, WNS=9110 at 10ns period
- sta_long_path_names_endpoints: 4-LUT chain, 1530ps, correct endpoints
- sta_unknown_delay_fails_loud: UNKNOWN delays fail, never silently substitute

Command: `cargo test -p helion-ultrascale` (after integration)
