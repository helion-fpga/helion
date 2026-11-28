# Closing the size and speed gaps with Vivado: work brief for DeepSeek and Muse

Written 2026-11-28 for the two agents working on Helion: **Muse** (Meta's Muse app) and **DeepSeek** (the DeepSeek Harness app on the user's MacBook).
Base: `fm/release-4.5` at `ce268e05`, already on GitHub (`helion-fpga/helion`).
Vivado reference: 2017.3. Main part: `xc7z020clg484-1` (ZedBoard).

## 0. Rules for both of you

1. **Run everything on the USF cluster, never on the MacBook.** That means every `cargo build`, `cargo test`, `zedbit` run, QoR benchmark and Vivado job. On 2026-11-27, six parallel Helion builds froze the MacBook four times (16 GB RAM, disk 98% full, so no swap). The Mac is only for editing, git and ssh.
   - The cluster is `ssh usf` (host `sc.rc.usf.edu`, Slurm partition `sc`). It has 6 nodes, each with 24 CPUs and 64 GB, and no GPUs. Most of it is idle.
   - How to build there: see the last section of `~/helion-vivado-data/CLUSTER_GUIDE.md`. In short:
     - Rust 1.99.1 is in `~/.cargo/bin`. The Helion source is `~/helion-release`; rsync your branch there or `git fetch` it.
     - Run `cargo fetch` on the login node, which has internet. Then run `cargo build --offline` inside `sbatch`.
     - Python 3 is `/apps/anaconda3/5.3.1/bin/python3.9`.
     - The prjxray DB is `~/.cache/helion/prjxray-db`.
   - Always give `--mem`, because Slurm kills a job at its limit. Ibex needs more than 62 GB today (G130), so skip it with `--skip ibex` until G130 is fixed.
   - Job names start with `hvq_` (size) or `hvt_` (speed). Never run heavy work on the login node.
   - **Muse:** you run on your own small machine and may have no cluster access. Push your branch, and **DeepSeek runs the cluster check for each hash you push** (section 5). Your own machine is for unit tests only.
2. **One owner per file; never edit the other agent's files.** The split is in sections 3 and 4. If you need a change in the other side's files, ask; don't make it.
3. **Branches:**
   - Muse works on `fm/qor-size` and DeepSeek on `fm/qor-speed`, both cut from `fm/release-4.5`.
   - Push `fm/` branches only. Never master, never merges, and no force-push without asking the user. Rebase onto `fm/release-4.5` when it moves.
   - No AI attribution in commits or PR text.
4. **The gap ledger** (`~/untitled folder 7/HELION_GAPS.md` on the Mac) is the source of truth.
   - A gap is closed only when it is **fixed AND guarded**: a test, a benchmark threshold or a checker rule that would fail if the gap came back.
   - DeepSeek keeps the ledger. Muse writes its status to `docs/qor-plan/STATUS_MUSE.md` on its branch, and DeepSeek copies it into the ledger.
5. **Prove every claim with a number from a run** (a job ID and a file path). Claims without them are wrong until checked; that has happened before.
6. **Never bend a test design to fit Helion.** Every design that Vivado accepts must build on Helion unchanged.

## 1. Where Helion stands (measured 2026-11-27)

### Size: Helion's LUT count compared with Vivado's, over 298 designs on xc7z020 that both tools built
| | Helion / Vivado |
|---|---|
| LUT, median design | 1.09x (p90 3.00x; 74 designs over 1.5x) |
| LUT, whole corpus | +15% (74,642 vs 64,838 over 261 designs) |
| FF / CARRY4 / BRAM / DSP, median | 1.00x each (FF p90 1.50x, carry p90 2.25x) |
| PicoRV32 / Ibex | 1.67x (1,464 vs 877) / 1.69x (5,980 vs 3,532) |
| zedboard_mmcm | 4.2x (155 vs 37) |

### Speed: Vivado routes and times Helion's placement (`hx_*` jobs), compared with Vivado's own build (`ref_*`)
| Design | Helion's placement | Vivado's own | Share |
|---|---|---|---|
| PicoRV32 (clk_half, 20 ns) | 92.0 MHz | 128.8 MHz | 71% |
| Ibex (sys_clk, 50 ns) | 32.4 MHz | 38.9 MHz | 83% |
| bram_fill (10 ns) | slack 0.702 ns | slack 2.094 ns | about 85% |
| zedboard_mmcm (clk_b_mmcm, 8 ns) | slack 0.825 ns | slack 4.326 ns | about 51% |

### Helion's own timing estimates are not yet accurate
On the same placement, Helion's predicted slack against Vivado's measured slack:
- mmcm: 3.287 vs 0.825 ns, so Helion is **2.46 ns optimistic**.
- bram: 1.483 vs 0.702 ns, 0.78 ns optimistic.
- pico: 8.474 vs 8.047 ns, 0.43 ns optimistic.
- ibex: 13.748 vs 20.724 ns, **7 ns pessimistic**.
- blinky: 6.906 vs 6.924 ns, close.

## 2. The targets ("done" means all of these)

### Size (Muse)
- Over the 309-design sweep on xc7z020:
  - LUT ratio median **at most 1.02x** and p90 **at most 1.30x**;
  - the whole corpus **at most +5%** LUTs;
  - **no design over 2.0x**.
- FF, CARRY4, BRAM and DSP counts **equal to Vivado's** on every `micro_*` unit.
  - Vivado's rules are measured in `rtlsweep/SUMMARY.md`. For example: 8x8 multipliers never go to a DSP; 25x18 signed = 1 DSP; 18x25 unsigned = 2; 32x32 = 4 DSPs + 47 LUTs; popcount32 = 53 LUTs and 0 CARRY4.
- PicoRV32 **at most 1.2x** (1,052 LUTs), Ibex **at most 1.2x** (4,238), mmcm **at most 1.5x** (55).
- Every Ibex sweep unit packs in **at most 8 GB** (G130).

### Speed (DeepSeek)
- **The timer comes first.** On the same placement, Helion's slack is **within 0.2 ns of Vivado's** on blinky, pico, ibex, bram, mmcm and dsp_prims, and never more than 0.1 ns optimistic.
  - Per net, against Vivado's `slow_max_ps` in the corpus `net_delays.tsv` files: median error **at most 5%**, p95 **at most 15%**.
- **Fmax of Helion's placement, as timed by Vivado:**
  - first **85%** of Vivado's own (the `tools/qor/targets.json` G5 bar);
  - then **95%**;
  - the end goal is **Vivado's own: PicoRV32 128.8 MHz, Ibex 38.9 MHz, bram_fill 126.5 MHz, mmcm slack 4.326 ns at 8 ns, blinky 272 MHz**.
- Route time stays within `targets.json`: PicoRV32 under 60 s, Ibex under 600 s.

## 3. Muse: the size gap (synthesis and mapping)

**Your files:** `crates/helion-sv`, `crates/helion-vhdl`, `crates/helion-ir`, and the synthesis and mapping parts of `crates/helion-xilinx7`, up to and including the packer's cell counts.

**Not yours:** timing, place and route (DeepSeek's).

Work in this order, since the easy, certain wins come first. Each item is done when its guard passes on the cluster sweep.

1. **G126. Size threshold for DSP inference.**
   - Today `micro_mul8x8_c/_r/_p3` get 1 DSP + 2 LUT in Helion. Vivado gives 90 LUT + 10 CARRY4 and 0 DSP.
   - Copy Vivado's thresholds from `SUMMARY.md` (DSP48E1 section).
   - Guard: all 6 `micro_mul*` units have exactly Vivado's DSP count.
2. **G125. VHDL `*` on a DSP.**
   - `tools/x7v/tests/stress/mul12.vhd` is 759 LUT and 0 DSP in Helion, against 22 LUT and 1 DSP in Vivado.
   - G3 already does this for SystemVerilog (`is_mac_rhs`/`emit_mac27`); reuse it from the VHDL path, don't write a second inference.
   - Guard: 1 DSP, and at most 2x Vivado's LUTs.
3. **G127. Bit-count reductions as LUT trees.**
   - popcount32 is 167 LUT + 54 CARRY4 in Helion, against 53 LUT + 0 in Vivado, which uses a LUT3/LUT5 compressor tree. `h_popcount` and `h_hamming` show the same pattern.
   - Guard: popcount32 at most 1.2x Vivado's LUTs and 0 CARRY4.
4. **G130. Ibex needs over 62 GB.**
   - zedbit dies in `wide_cone module=ibex_multdiv_fast signal=mac_res_signed_6`.
   - The multiplier is being built as one giant cone instead of a DSP, and Helion builds logic that Vivado deletes first (Vivado keeps 4 LUTs of `ibex_obs_wrap__viv`).
   - Remove unobservable logic before mapping, and send the multiplier to the DSP path.
   - Guard: every Ibex sweep unit packs in 8 GB.
5. **G128. The rest of the LUT tail.** Find one cause at a time, and split each one off as its own G number. The worst designs:
   - `h_rot_mm`: 1,489 vs 199 LUTs.
   - `carry_add_sub_cmp.vhd`: 181 vs 44 LUTs, with 0 vs 9 CARRY4, because VHDL compare and subtract don't use carry chains.
   - `zedboard_mmcm`: 155 vs 46.
   - `h_iir_biquad`: 176 vs 53.
   - `dsp_wide_regs`: 1,191 vs 367 LUTs, with 305 vs 64 CARRY4.
   - `h_mask_gen`: 345 vs 137.
   - `zedboard_hdmi`: 403 vs 168.
   - The full ranked list is `data/helion_vs_vivado_xc7z020.csv`, sorted by `r_lut`.
6. **G1 / G8. The CPUs.** PicoRV32 and Ibex down to 1.2x, once items 1-5 are done.

**How to measure:**
- Push your branch, then ask DeepSeek to run the size check on that hash (section 5).
- For quick checks on your own machine: `cargo test -p helion-sv`, then the svcheck and x7v micro tests (`tools/x7v/README.md`).
- **Every change must keep the output correct:** the x7v verify and svcheck results are the guard against miscompiles. Muse's `fm/qor2-mapper` once turned a 6-input AND into constant 0.

## 4. DeepSeek: the speed gap (timing, place and route)

**Your files:** the timing, place and route code in `crates/helion-xilinx7`, plus `crates/helion-sta`, `crates/helion-place`, `crates/helion-route`, and `tools/qor`.

**Start from what landed on `fm/release-4.5`:**
- `c2798e36`: measured cell arcs and a net-delay correction (G89-G93).
- `dc83a66f`: the placer estimates connections the way the timer charges them (G93).
- `750b09fb` / `cb6481e2`: long hops ride long lines (G95).

Don't redo these; measure what they achieved first.

1. **G124 / G7. Calibrate the timer** until the timer target in section 2 holds.
   - Data: about 1.1 million measured net delays from Vivado's own routing, across 8 parts. See section 6.
   - Fit per wire class and per PIP. Check each fit on held-out designs, not the ones it was fitted on.
   - The mmcm case (2.46 ns optimistic) and the ibex case (7 ns pessimistic) are the two to explain first.
2. **G5. Placement quality**, from 71-85% of Vivado's Fmax up to the targets.
   - Read Vivado's worst paths on Helion's placement in `campus/reports/hx_*`. For example, PicoRV32's is `cpu_state_1` -> LUT4 (fanout 85, 4.79 ns of wire) -> LUT6 (fanout 32, 3.19 ns) -> `cpuregs_343` CE.
   - Fix the placer for each pattern: high-fanout control nets, keeping critical chains together.
3. **G129. A LUT output used as a clock** is refused, but Vivado accepts it.
4. **Run the cluster checks for Muse** (section 5), and keep the gap ledger up to date.
5. When the UltraScale+ sweeps finish (xcku3p, xcku5p, xczu3eg; still running as `hv_r_impl_*`), merge them with:
   ```
   ~/helion-vivado-data/tools/merge_impl.sh <part> <short>
   ```

**How to measure speed:**
- `zedbit ... --vivado=PREFIX` writes Helion's placement for Vivado (see `crates/helion-xilinx7/src/vivado_export.rs`).
- On the cluster, `~/vivado_jobs/helion_export.sh` and the `~/vivado_jobs/hx2/*.tcl` scripts route and time that placement in Vivado. Compare with `ref_*`.
- `tools/qor/run.py --set full` (on the cluster) checks Helion's own numbers against `baseline.json` and prints the `targets.json` goals.

## 5. The size check (DeepSeek runs it for every hash Muse pushes)
```
# on the cluster: build the hash, then 30 shards over the 309-design sweep, all in Slurm (survives the Mac being off)
cd ~/helion-release && git fetch origin && git checkout <hash> && cargo fetch
bash ~/hvv/submit.sh impl_xc7z020.csv           # build job + 30-shard array; results in ~/hvv/out/impl_xc7z020/
```
- Copy `~/hvv/out/impl_xc7z020` aside first, so each hash keeps its own results.
- Add `--skip ibex` until G130 is fixed.
- To get the summary, merge the shards as in `tools/helion_vs_vivado.py` (it prints the median, p90 and over-1.5x counts per resource).
- Report back to Muse: the hash, the job IDs, the summary lines, and any design that got worse.

## 6. Where the data is
| What | Where |
|---|---|
| Helion vs Vivado per design (xc7z020) | `data/helion_vs_vivado_xc7z020.csv` (this folder); script `tools/helion_vs_vivado.py` |
| Vivado post-route counts, WNS and WHS, 8 parts x 309 designs | `data/impl_<part>.csv` (this folder) |
| Vivado synthesis inference rules (carry, DSP, SRL, LUTRAM, FSM) | `data/rtlsweep_SUMMARY.md` (this folder) |
| Measured net delays (net, driver site, load pin, load site, slow_max_ps, fast_min_ps) | cluster: `~/vivado_jobs/out/recon/impl/<part>/units/<unit>/net_delays.tsv`, plus `timing.rpt`, `route.rpt`, `util.rpt` beside it |
| Vivado timing Helion's placement | Mac: `~/helion-vivado-data/campus/reports/hx_*`; cluster: `~/vivado_jobs/hx2/` |
| Vivado's own reference builds | Mac: `~/helion-vivado-data/campus/reports/ref_*`; `~/helion-zed-builds/vivado_jobs/COMPARISON.md` |
| Block goldens, SDF, power and other probes (DeepSeek's earlier jobs) | Mac: `~/helion-vivado-data/cluster_mirror/ds_jobs/` (38 `RESULTS.md` files under `out/us/`) |
| Speed-grade scaling | -2/-1 median critical path 0.826x; -3/-1 0.740x (about 200 designs, xc7z020) |
| Benchmark targets and the Vivado numbers | `tools/qor/targets.json`, `tools/qor/vivado.json` |
| Gap ledger | Mac: `~/untitled folder 7/HELION_GAPS.md` (G1, G3, G5, G7, G124-G130) |
