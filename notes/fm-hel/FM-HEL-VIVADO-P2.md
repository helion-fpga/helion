# FM-HEL-VIVADO-P2 — Trusted bitstream → board DONE honesty

**Branch:** `fm-hel-vivado-bits` (NO MERGE)  
**Goal:** Full bitgen path for counter + board DONE only on programmer/part confirm.
No FTDI/cable → honest detect/program fail. **Never** soft-hold or invent DONE
(especially not `program_hw` sim DONE=1 with USB=0). Linux honesty first.

## What changed

| Area | Before | After |
|------|--------|-------|
| `resolve_cable("auto")` | USB=0 → **sim** (invents fabric DONE on program) | USB=0 → **ofl** (program refuses DONE) |
| `Session::program_hw_cable` OFL/native | `Ok(soft-hold)` | Real `program_hbits_with_cable`; **Err** when no probe |
| Bare `program_hw` / Tcl | auto→sim DONE=1 | auto board path; USB=0 **refuses DONE** |
| Explicit `cable=sim` / `mpsse-sim` | sim DONE | sim fabric DONE **labeled** `(… not board DONE)` |
| GUI Program rail | soft-hold banner; auto Program could sim-DONE | `board:no-cable`; auto/usb blocked without probe; **Program Device (sim)** uses `cable=sim` |

## Legal fence

No UNISIM, no AMD IP/JTAG/XSim, no Vivado trademarks, no Kintex clones.

## Gold

`helion report_timing examples/counter.sv --sdc examples/counter.sdc` → **WNS_PS=9640**.

## Evidence commands

```bash
cargo test -p helion-hw --lib --features usb-native
cargo test -p helion-proj --lib tcl_session_steps
cargo test -p helion-gui --lib tcl_client_tree_console_flow
cargo run -q -p helion-cli -- bitstream examples/counter.sv -o /tmp/counter.hbits
cargo run -q -p helion-cli -- hw program --cable auto -b /tmp/counter.hbits   # expect refuse
cargo run -q -p helion-cli -- hw program --cable sim -b /tmp/counter.hbits    # sim fabric DONE
cargo run -q -p helion-cli -- report_timing examples/counter.sv --sdc examples/counter.sdc
```

## Honesty

- Board DONE requires OFL programmer-ok or native live STAT TDO DONE=1.
- USB=0 / no FTDI → detect `physical_had=0`; program **Err** (no soft-hold Ok).
- Sim / mpsse-sim remain available **only** via explicit cable; labeled not board DONE.

## Bitgen harden (Linux bits branch)

| Check | Behavior |
|-------|----------|
| `bitgen` empty pack | **Err** — refuses empty/fake `.hbits` (no LUTFF/IOB/DSP/BRAM) |
| `bitgen` zero frames | **Err** — INIT=0 / no IOB / no IMUX that set bits → refuse |
| `Session::write_bitstream` | **Err** if no design / not routed / empty frames |
| CLI `helion bitstream` | Exits 1 if configured frames == 0 |
| Counter gold | **185 B** sparse `.hbits`; frames/packets deterministic across runs |
| CRC / hash | Header CRC32C + body hash always set; in-stream `CRC_CHECK` (0x21) remains 0 stub (device CRC not modeled) — size-stable |
| Program path | Unchanged: `auto` USB=0 refuses DONE; `sim` labeled not board DONE |

```bash
cargo test -p helion-bits --lib
cargo test -p helion-proj --lib write_bitstream_refuses
cargo run -q -p helion-cli -- bitstream examples/counter.sv -o /tmp/counter.hbits   # 185 B
```

