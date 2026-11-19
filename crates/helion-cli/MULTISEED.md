# Multiseed timing closure: verification record (GAP-F6)

Branch: `fm/ultra-multiseed`, staging tip `e1c2e47`, worktree
`~/workspace/helion-ultra-multiseed`. No merges, no pushes. Vivado
license state UNKNOWN; nothing here runs Vivado or claims Tier 2.

## What the inherited code is (verified by reading, 2026-11-19)

- The multiseed proof is exactly one commit, `e1c2e47`
  ("feat(cli): overnight multi-seed timing-closure flow"), touching
  only `crates/helion-cli/src/main.rs` (+372 lines).
- `helion multiseed <design> --part P --seeds N --seed-start S --jobs J
  --out FILE --threshold T`:
  - Synths once (`synth_any`), then runs full `compile_design` per
    seed on a rayon pool of J threads.
  - Each seed appends one JSONL record
    `{seed, wns_ps, tns_ps, runtime_s, status, reason}` under a file
    mutex, flushed per record, so a killed run keeps finished seeds.
  - Resume: seeds already in the JSONL are skipped.
  - Best seed: highest `wns_ps` among `status == "ok"` and
    `wns_ps >= threshold`; ties break on lowest `tns_ps`, then
    lowest seed. Deterministic over the JSONL on disk.
  - 6 unit tests: best-seed tie-breaks, threshold/filter, JSONL
    roundtrip + escaping, checkpoint parsing.
- Seed mechanism (verified in `crates/helion-place/src/lib.rs`):
  `PlaceOpts.seed` drives `seeded_order`, a SplitMix64 Fisher-Yates
  shuffle of the LUTFF processing order. Seed 0 is the legacy order.
  The shuffle applies only when the timing guide carries no
  criticality info; with default (empty) constraints, as `multiseed`
  uses, it applies. Sites are recorded by packed index, never in
  visitation order, so the shuffle cannot corrupt INIT/router
  indexing.
- Per-seed work inside `compile_design` is pack + place + route +
  DRC + bitgen + STA. Pack and bitgen are seed-independent and repeat
  per seed; only place (via the seeded order) and its downstream
  route/STA vary with the seed.

## What is NOT on this branch (verified by listing/grep)

- No `crates/helion-ultrascale` crate and no `xcku3p`/`xcvu3p` part
  file anywhere in the tree. `Device::load_part` reads
  `devices/helion/parts/<name>.toml`; only HL10T/HL10M/HL10S parts
  exist. `helion multiseed --part xcku3p` therefore fails at device
  load before any seed runs.
- No UltraScale pack/place/route/STA staging is ported here
  (fm/ultra-packplace, fm/ultra-router, fm/ultra-timing live on
  their own staging branches). The driver is part-agnostic: it
  sweeps whatever `compile_design` supports for the named part.

## Proven vs blocked (status at staging tip)

- PROVEN (mechanism, generic part): seed sweep, JSONL checkpoint
  and resume, deterministic best-seed selection, per-seed failure
  recording. Reproduction run on HL10T below.
- NOT PROVEN: the GOAL.md "10-seed, WNS 8460 to 8820" claim has no
  run log attached to the commit; reproduction is pending.
- BLOCKED on xcku3p: 100-seed flow cannot run until GAP-F7 (device
  model / xcku3p part) plus GAP-F2/F3/F4/F5 (pack/place/route/STA
  on that part) land. This driver must not invent a device model,
  delays, or routing to work around that.
- The 100-seed flow itself is unverified until an end-to-end run
  happens on xcku3p; this branch can only harden the driver
  mechanics on the generic part.

## Defects found by reading (to fix on this branch)

1. Pack and bitgen repeat per seed although neither depends on the
   seed. For 100 seeds that is 99 wasted packs and 99 wasted
   bitgens. Fix: pack once, run place/route/STA per seed, bitgen
   only the best seed.
2. No per-seed logs. Only a one-line stderr summary per seed and
   the JSONL record. Fix: `--log-dir` with one log file per seed
   (placement/route/STA summary).
3. `file.lock().unwrap()` panics the whole sweep if the mutex is
   poisoned (a panicking worker thread kills every other seed).
   Fix: recover the inner file on poisoning.
4. The end-of-run report dedups by seed keeping the FIRST record.
   If a seed ever has two records (concurrent runs sharing one
   out file, or a hand-edited file), the stale one wins. Fix:
   last record wins.
5. `to_jsonl` escapes backslash, quote, and newline but not CR,
   tab, or other control characters. Fix: escape all control chars.
6. No best-seed artifact: the best seed is only printed, so
   reproducing it needs a manual `helion impl --seed N` rerun.
   Fix: `--best-out` writes the best seed, its numbers, and the
   exact reproduce command; `--emit-best` re-runs the full
   compile for the best seed and writes its bitstream.
7. Seed-range overflow: `seed_start + n_seeds` can wrap for
   extreme inputs. Fix: checked arithmetic, fail loud.

## Reproduction runs

(Commands and outputs appended here as they are run. Nothing in
this file is claimed without a shown run.)
