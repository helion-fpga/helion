## What

<!-- One or two sentences: what this pull request changes. -->

## Why

<!-- The problem it solves. Link the issue: "Fixes #123". -->

## How it was tested

- [ ] Added or updated a test: `cargo test -p <crate> --lib <TESTNAME>`
- [ ] `cargo test -p helion-rune` passes (if the IDE changed)
- [ ] `helion-ide --headless examples/counter.sv` still prints `WNS_PS=9640` (if CAD or the IDE changed)
- [ ] Screenshots attached (if the IDE looks different)

## Checklist

- [ ] `cargo fmt` and `cargo clippy` are clean on the crates I touched
- [ ] `CHANGELOG.md` updated for user-visible changes
- [ ] QoR numbers unchanged, or `crates/helion-cli/tests/qor.rs` updated in this PR with the reason

## Legal fence

- [ ] No proprietary vendor IP, libraries or databases; vendor-family backends use open bitstream documentation only
- [ ] No vendor trademarks as product names; no AXI interconnect as Helion IP (Helion-MM/ST)
