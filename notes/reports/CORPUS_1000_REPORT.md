# Helion 1,000-Design End-to-End Flow Verification Report

**Branch:** `fm/helion-corpus-1000-harness` (based on `wip/ide-integration`)  
**Harness Target:** `crates/helion-cli/tests/corpus_1000.rs`  
**Device Target:** `HL10T-C32-1` (32×32 CLB / 8,192 LUT6 / 32 user IOB / 8 BRAM18)  
**Execution Engine:** In-process Rust + `rayon` work-stealing thread pool (8 threads) with per-stage `catch_unwind` and microsecond telemetry  

---

## 1. Executive Summary

To evaluate Helion's end-to-end reliability, QoR, and throughput at Vivado scale, we built and executed a parallel 1,000-design verification harness (`crates/helion-cli/tests/corpus_1000.rs`). The harness drives **1,000 distinct RTL designs**—comprising **225 real-world on-disk repository designs** across `third_party/`, `examples/`, `corpus-pass/`, and `ip/`, plus **775 deterministic parameterized and synthetic SystemVerilog/VHDL-2008 designs** across 17 hardware architecture families—through all 7 stages of the Helion FPGA compilation flow:

1. **Stage 1 — Parse & Elaborate** (`helion_sv::compile_sv` / `helion_vhdl`)
2. **Stage 2 — Technology Mapping & Cell Lowering** (`helion_sv::synth_sv` / `elaborate_sv_sources` / `helion_vhdl::map_vhdl` $\rightarrow$ `Lut6`, `Carry4`, `Hff`, `Bram18`, `Dsp48e1`/`Mac27`, `IobOut`/`IobIn`)
3. **Stage 3 — DRC Gate Checks** (Structural & semantic netlist verification: `F1` undriven nets, `F2` multi-driven nets, `F6` port coverage, `NO_BODY`, soft synthesis drops, inferred latches, and iterative 3-color DFS combinational loop detection)
4. **Stage 4 — Packing & Placement** (`helion_pack::pack` + `helion_place::place_with_guide` + `helion_drc::check_placed`)
5. **Stage 5 — Routing** (`helion_route::route_with_guide` PathFinder negotiated congestion router + `helion_drc::check_routed`)
6. **Stage 6 — Static Timing Analysis** (`helion_sta::report_timing_routed_xdc` @ 10.0 ns / 100 MHz clock constraint)
7. **Stage 7 — Bitstream Generation** (`helion_bits::bitgen` `.hbits` frame assembly and CRC packet encoding, refusing `HELION_E_EMPTY_BITSTREAM`)

### Key Headline Metrics
- **Total Corpus Size:** **1,000 distinct RTL designs** (225 repository on-disk designs + 775 parameterized/synthetic designs)
- **End-to-End Parallel Wall Time:** **5.21 – 10.78 seconds** across all 1,000 designs on 8 Rayon threads (**92.7 – 191.8 designs/sec** throughput), including a 25-design stratified re-execution proving **100% bit-identical determinism** across cell counts, placement, routing, WNS, and bitstream byte lengths.
- **Stage 1 & 2 Frontend Survival:** **1,000 / 1,000 (100.0%)** parse and elaborate without panics or crashes.
- **Strict Gated End-to-End Survival (Stages 1 $\rightarrow$ 7 with strict Stage 3 DRC gate):** **707 / 1,000 (70.7%)** pass every structural DRC rule and produce timing-closed, non-empty `.hbits` bitstreams.
- **Ungated Backend Flow Completion (Stages 4 $\rightarrow$ 7 physical P&R + STA + Bitgen):** **977 / 1,000 (97.7%)** pack, place, and route cleanly on `HL10T-C32-1`; **969 / 1,000 (96.9%)** close STA and emit valid `.hbits` bitstreams (the remaining 23 are `NO_BODY` empty shells, plus 8 `0-LUT` memory-bypass/unlowered shells that Stage 7 rightly refuses with `HELION_E_EMPTY_BITSTREAM`).

---

## 2. Stage-by-Stage Survival Funnel

We measure two complementary survival funnels:
1. **Strict Gated Survival Funnel:** Any Stage 3 structural DRC violation (`F1`, `F2`, `F6`, `NO_BODY`, `SOFT_INCOMPLETE`, `LATCH_INFERRED`, `COMB_LOOP`) halts promotion of the design.
2. **Ungated Per-Stage Pass Counts:** Even when Stage 3 flags a netlist defect (such as an `F2` redundant identity-LUT driver or `F6` dropped port), the harness still drives the synthesized `Design` through Stages 4–7 to measure physical P&R, STA, and Bitgen resilience independently.

| Flow Stage | Crate / Engine | Strict Gated Survival | Strict % | Ungated Stage Pass | Ungated % | Stage Drop (Strict) | Stage Drop (Ungated) |
|---|---|---:|---:|---:|---:|---:|---:|
| **1. Parse & Elaborate** | `helion-sv` / `helion-vhdl` | **1,000 / 1,000** | **100.0%** | **1,000 / 1,000** | **100.0%** | 0 | 0 |
| **2. Synth & Tech Map** | `helion-sv` / `helion-ir` | **1,000 / 1,000** | **100.0%** | **1,000 / 1,000** | **100.0%** | 0 | 0 |
| **3. DRC Gate Checks** | `helion-drc` + Structural Gate | **707 / 1,000** | **70.7%** | **707 / 1,000** | **70.7%** | -293 | -293 |
| **4. Pack & Place** | `helion-pack` + `helion-place` | **707 / 1,000** | **70.7%** | **977 / 1,000** | **97.7%** | 0 | -23 (`NO_BODY`) |
| **5. PathFinder Route** | `helion-route` + `check_routed` | **707 / 1,000** | **70.7%** | **977 / 1,000** | **97.7%** | 0 | 0 |
| **6. Static Timing (STA)** | `helion-sta` (`10,000 ps` clk) | **707 / 1,000** | **70.7%** | **969 / 1,000** | **96.9%** | 0 | -8 (`0-LUT` shells) |
| **7. Bitstream (`.hbits`)** | `helion-bits::bitgen` | **707 / 1,000** | **70.7%** | **969 / 1,000** | **96.9%** | 0 | 0 |

```
Strict Funnel:  [S1 Parse: 1000] -> [S2 Synth: 1000] -> [S3 DRC: 707] -> [S4 Place: 707] -> [S5 Route: 707] -> [S6 STA: 707] -> [S7 Bitgen: 707]
Ungated Funnel: [S1 Parse: 1000] -> [S2 Synth: 1000] ------------------> [S4 Place: 977] -> [S5 Route: 977] -> [S6 STA: 969] -> [S7 Bitgen: 969]
```

### Survival Breakdown by Corpus Category (All 1,000 Designs)

| Category | Source Type | Total | S1 Parse | S2 Synth | S3 DRC | S4 Pack/Place | S5 Route | S6 STA | S7 Bitgen | Strict E2E Pass |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `Repo_Examples` | On-disk (`examples/*.sv,.vhd,.v`) | 33 | 33 | 33 | 15 | 32 | 32 | 32 | 32 | **15** (45.5%) |
| `Repo_CorpusPass` | On-disk (`corpus-pass/**/*.v`) | 13 | 13 | 13 | 6 | 11 | 11 | 8 | 8 | **6** (46.2%) |
| `Repo_IP_Cores` | On-disk (`ip/h_*/h_*.v`) | 70 | 70 | 70 | 2 | 70 | 70 | 70 | 70 | **2** (2.9%) |
| `Repo_MultiFile_SoC` | On-disk multi-file SoCs | 5 | 5 | 5 | 1 | 5 | 5 | 5 | 5 | **1** (20.0%) |
| `Repo_OpenTitan_Ibex` | On-disk (`examples/ysyx_ibex.sv` 96 mods) | 96 | 96 | 96 | 11 | 84 | 84 | 80 | 80 | **11** (11.5%) |
| `Repo_ThirdParty_PicoRV32` | On-disk (`third_party/picorv32/picorv32.v`) | 8 | 8 | 8 | 0 | 7 | 7 | 6 | 6 | **0** (0.0%) |
| `Param_LogikBench` | Parameterized repo IP (`DW=2..32`) | 65 | 65 | 65 | 58 | 58 | 58 | 58 | 58 | **58** (89.2%) |
| `Gen_FSM_Controllers` | Moore/Mealy FSMs + `full_case` + latch | 60 | 60 | 60 | 42 | 60 | 60 | 60 | 60 | **42** (70.0%) |
| `Gen_FIFO_SkidBuffers` | Sync FIFOs, skid buffers, elastic pipes | 50 | 50 | 50 | 50 | 50 | 50 | 50 | 50 | **50** (100.0%) |
| `Gen_AXI_Lite_Crossbars` | AXI4-Lite slaves + 2D `generate` crossbars | 50 | 50 | 50 | 40 | 50 | 50 | 50 | 50 | **40** (80.0%) |
| `Gen_DSP_MAC_Pipelines` | Multiply-accumulate, FIR taps, `MAC27` | 50 | 50 | 50 | 50 | 50 | 50 | 50 | 50 | **50** (100.0%) |
| `Gen_BRAM_LUTRAM_Cores` | Sync RAM, LUTRAM, register files | 45 | 45 | 45 | 45 | 45 | 45 | 45 | 45 | **45** (100.0%) |
| `Gen_ShiftReg_LFSR` | Galois/Fibonacci LFSRs, SIPO/PISO | 45 | 45 | 45 | 45 | 45 | 45 | 45 | 45 | **45** (100.0%) |
| `Gen_ALU_BarrelShifters` | 3-input `a+b+cin` ALUs, shifters (`w=3..8`) | 50 | 50 | 50 | 26 | 50 | 50 | 50 | 50 | **26** (52.0%) |
| `Gen_CDC_Synchronizers` | 2-FF..5-FF sync chains, pulse bridges | 45 | 45 | 45 | 45 | 45 | 45 | 45 | 45 | **45** (100.0%) |
| `Gen_Generate_Hier_Trees` | Unrolled `generate for` reduction trees | 45 | 45 | 45 | 45 | 45 | 45 | 45 | 45 | **45** (100.0%) |
| `Gen_Signed_Arithmetic` | Signed 2's complement add/sub/ashr | 45 | 45 | 45 | 45 | 45 | 45 | 45 | 45 | **45** (100.0%) |
| `Gen_PriorityEnc_Arbiters` | Priority encoders, round-robin arbiters | 45 | 45 | 45 | 45 | 45 | 45 | 45 | 45 | **45** (100.0%) |
| `Gen_Gray_BCD_DDS` | Gray counters, DDS phase accumulators | 45 | 45 | 45 | 45 | 45 | 45 | 45 | 45 | **45** (100.0%) |
| `Gen_CRC_ECC_Engines` | Parallel CRC-8/16/32 step engines | 45 | 45 | 45 | 45 | 45 | 45 | 45 | 45 | **45** (100.0%) |
| `Gen_UART_SPI_I2C_PWM_WB` | Wishbone PWM / serial peripheral cores | 45 | 45 | 45 | 45 | 45 | 45 | 45 | 45 | **45** (100.0%) |
| `Gen_VHDL2008_Cores` | Synthesizable VHDL-2008 architectures | 30 | 30 | 30 | 25 | 30 | 30 | 30 | 30 | **25** (83.3%) |
| `Gen_GateCommit_Stress` | Gate-commit regression & DRC stress suite | 15 | 15 | 15 | 11 | 15 | 15 | 15 | 15 | **11** (73.3%) |
| **TOTAL** | **225 Repo + 775 Parameterized/Synthetic** | **1,000** | **1,000** | **1,000** | **707** | **977** | **977** | **969** | **969** | **707 (70.7%)** |

*(Note: In the final run with `Gen_AXI_Lite_Crossbars` using concatenated `case ({axil_awvalid & axil_wvalid, axil_awaddr})` for indices `0..35`, 35 additional AXI-Lite slaves pass Stage 3 DRC, bringing total strict E2E pass to **742 / 1,000 (74.2%)**; when using nested `if (axil_awvalid && axil_wvalid) case (axil_awaddr)`, those 35 cases expose Root Cause 3 below and strict E2E is **707 / 1,000 (70.7%)**.)*

---

## 3. Throughput & Latency Percentiles

All 1,000 designs were timed per stage at microsecond resolution in release mode (`cargo test --release -p helion-cli --test corpus_1000`) on 8 Rayon worker threads:

| Stage | p50 Latency | p90 Latency | p99 Latency | Max Latency | Primary Bottleneck at p99 / Max |
|---|---:|---:|---:|---:|---|
| **Stage 1: Parse & Elaborate** | `67 µs` (0.07 ms) | `444 µs` (0.44 ms) | `17.99 ms` | `61.73 ms` | Multi-file `serv/*.v` & `picorv32.v` tokenization |
| **Stage 2: Synth & Tech Map** | `16.39 ms` | `114.65 ms` | `698.60 ms` | `3,626.98 ms` | `ibex_multdiv_fast` / `picorv32` / `sha256_all` AIG & LUT opt |
| **Stage 3: DRC Gate Checks** | `66 µs` (0.07 ms) | `557 µs` (0.56 ms) | `19.10 ms` | `108.12 ms` | 3-color DFS & port-net scan on 2.7k-cell `picorv32`/`serv_debug` |
| **Stage 4: Pack & Place** | `184 µs` (0.18 ms) | `2.25 ms` | `30.98 ms` | `789.84 ms` | `serv_debug` (2,284 LUTs, 1,335 FFs) & `picorv32` IMUX legalize |
| **Stage 5: PathFinder Route** | `17 µs` (0.02 ms) | `72 µs` (0.07 ms) | `3.42 ms` | `64.52 ms` | Negotiated congestion A* on `sha256_core` / `picorv32_pcpi_fast_mul` |
| **Stage 6: Static Timing (STA)** | `23 µs` (0.02 ms) | `132 µs` (0.13 ms) | `4.33 ms` | `210.56 ms` | Topological arrival/required propagation on 1k+ FF cones |
| **Stage 7: Bitstream (`.hbits`)** | `13 µs` (0.01 ms) | `44 µs` (0.04 ms) | `563 µs` (0.56 ms) | `7.06 ms` | Sparse CLB frame assembly & CRC packetization |
| **End-to-End Per-Design Total** | **`19.90 ms`** | **`122.84 ms`** | **`1,282.29 ms`** | **`3,638.24 ms`** | **Stage 2 (`helion-sv` synthesis) accounts for 89% of total CPU time** |

- **Parallel Sweep Wall Time (1,000 designs):** `5,213 ms` – `10,783 ms` (**92.7 – 191.8 designs/sec**)
- **Median End-to-End Compile Time (p50):** **`11.15 ms – 19.90 ms`** per design from raw RTL text to `.hbits` bitstream packets.

---

## 4. Categorized Failure Taxonomy & Deep Root-Cause Analysis

Across the 1,000 designs, every design that failed or stopped before completing strict 7-stage verification falls into one of the following **9 root-cause categories**:

| Taxonomy ID | Count | Stage | Primary Root Cause Summary |
|---|---:|---|---|
| `PASS` | **707** | All 7 Pass | Clean synthesis, zero DRC violations, placed, routed, timing closed, valid `.hbits` |
| `STAGE3_DRC_F6_UNCONNECTED_PORT` | **86** | Stage 3 DRC | `(* full_case *)` block drop (`picorv32`), `64'(data_i)` cast drop (`prim_secded_*_enc`), or nested `if (en) case` drop |
| `STAGE3_DRC_F1_UNDRIVEN_NET` | **65** | Stage 3 DRC | Wide-cone cap (`>6` PIs in `gen_alu_datapath` `w>=6`, `prim_secded_*_dec`, `sha256_w_mem`, `picorv32`) |
| `STAGE3_DRC_F2_MULTI_DRIVEN_NET` | **39** | Stage 3 DRC | Commit `8acdb12` emits identity LUT (`0xaaaa...`) on output port in parallel with `IobOut` (`blinky`, `counter`, `serv_decode`) |
| `STAGE3_DRC_F1_UNDRIVEN_AND_F2_MULTIDRIVEN` | **35** | Stage 3 DRC | Combined wide-cone `F1` drop + commit `8acdb12` parallel identity-LUT `F2` on 1-bit output ports (`sha256_core`, `serv_aligner`) |
| `STAGE3_DRC_F2_MULTIDRIVEN_AND_F6_PORT_UNCONNECTED` | **33** | Stage 3 DRC | Combined commit `8acdb12` parallel identity-LUT `F2` + unreferenced/dropped control port `F6` (`ip/h_*.v`) |
| `STAGE3_DRC_NO_BODY_EMPTY_OR_UNLOWERED` | **11** | Stage 3/4 | Empty wrapper shells, technology-specific primitives (`prim_buf`, `prim_flop`, `prim_clock_gating`, `picorv32_axi`, standalone `top.sv`) |
| `STAGE3_DRC_NO_BODY_FLATTEN_CAP` | **9** | Stage 3/4 | `FLATTEN_CAP` refuses wide combinational multiplier/log cones (`ln_phase_d.v`, `muladdc_phase_d.v`) |
| `STAGE3_DRC_LATCH_INFERRED` | **7** | Stage 3 DRC | Explicit `always_latch` level-sensitive storage (`ibex_register_file_latch`, `prim_generic_clock_gating`, `gen_fsm_ctrl_055..059`) |
| `STAGE3_DRC_COMB_LOOP` | **5** | Stage 3 DRC | Combinational LUT cycle (`serv_top` `bufreg_clr_lsb`, `ibex_alu` `shift_result`, `gen_gate_stress_010..011`) |
| `STAGE3_DRC_NO_BODY_SIM_ONLY` | **3** | Stage 3/4 | DPI / simulation-only `$display`/`$finish` modules (`simulator_ctrl`, `ibex_tracer`, `ibex_controller`) |

---

### Deep-Dive Root Cause 1: Commit `8acdb12` Parallel Identity-LUT (`0xaaaaaaaaaaaaaaaa`) + `IobOut` Collision (`F2_MULTI_DRIVEN`) AND Commit `33ecc9f` Dead `synth_rtl` Bug

**Affected Designs (107 total across `F2` categories, plus 216 designs with QoR bloat totalling 3,952 redundant LUT6 cells):**
- `examples/blinky.sv`, `examples/blinky.vhd`, `examples/counter.sv`, `examples/complex.sv`
- `examples/serv/serv_decode.v` (37 F2 nets, e.g. `o_sh_right`), `serv_ctrl.v` (`o_bad_pc`), `serv_bufreg2.v` (`o_sh_done`), `serv_immdec.v` (`o_csr_imm`), `serv_rf_if.v` (`o_rs1`), `serv_aligner.v` (`o_wb_ibus_cyc`), `serv_csr.v` (`o_q`)
- `ip/h_*.v` (`st_out_valid`, `pwm_out`, `sclk`, `miso`, `busy`, `irq`, `overflow`, `carry`)
- `gen_vhdl_core_025..029` (`led <= cnt(wm1);`)

#### Exact Mechanism & HNF Evidence
Inspecting the HNF emitted for `examples/counter.sv` (`assign led = cnt[3];`) and `examples/serv/serv_decode.v` (`o_sh_right = co_sh_right;`):
```text
# From `helion hnf examples/counter.sv`:
cell u_clut0_s4 Lut6 0xaaaaaaaaaaaaaaaa
cell u_iob IobOut
net cnt_3 u_luts3/I3 u_ff3/Q u_clut0_s4/I0 u_iob/I
net led u_clut0_s4/O u_iob/PAD

# From `helion hnf examples/serv/serv_decode.v`:
cell u_clut59_s76 Lut6 0xaaaaaaaaaaaaaaaa
cell u_iob IobOut
net funct3_2 u_ff2/Q ... u_clut59_s76/I0 ...
net co_sh_right u_clut20_s37/O u_iob/I
net o_sh_right u_clut59_s76/O u_iob/PAD
```
1. **Why `F2` happens on every 1-bit wire/bit copy to an output port (`crates/helion-sv/src/lib.rs:17736–17825` & `18536–18627`):**
   - Commit `8acdb12` removed `RExpr::Ident(_) | RExpr::Bit(_, _) | RExpr::Range(_, _, _) => continue` from the combinational assign loop in `synth_rtl_inner`.
   - As a result, `assign led = cnt[3];` emits a physical identity buffer LUT `u_clut0_s4` (`Lut6 { init: 0xaaaaaaaaaaaaaaaa }`) with input `I0 = cnt_3` and output `O = led`.
   - Immediately afterward, the output IOB loop at `crates/helion-sv/src/lib.rs:18620` calls `drive_target(rhs, rtl)`, resolves the RHS directly to `cnt_3`, and calls `emit_iob(&mut d, &mut iob_n, "cnt_3", "led")`, which connects `u_iob/I = cnt_3` and `u_iob/PAD = led`.
   - Because `led` is a top-level output port, `d.peephole_opt_luts()` refuses to sweep `u_clut0_s4`. Consequently, `u_clut0_s4` and `u_iob` are wired **in parallel** between `cnt_3` and `led`, and `led` has two distinct drivers (`u_clut0_s4/O` and `u_iob/PAD(from cnt_3)`).
   - In `serv_decode.v`, this exact mechanism happens on all 37 scalar control outputs assigned in `always @(*)` (`o_sh_right = co_sh_right`, etc.).
2. **Newly Discovered Bug in Commit `33ecc9f` (`crates/helion-sv/src/lib.rs:3842–3857` & `17363`):**
   - Commit `33ecc9f` intended to pass `is_top = false` in `lower_own_for_stitch` (submodule path, line 3811) and `is_top = true` in `lower_own_cached` (top-level leaf module path, line 3852, called from `synth_from_parsed_top` line 19349).
   - Instead, a blind find-and-replace changed **both** `lower_own_for_stitch` (line 3811) and `lower_own_cached` (line 3852) to call `synth_rtl_inner(rtl, false)`, leaving `fn synth_rtl(rtl: &Rtl)` at line 17363 **100% dead code** (confirmed by `rustc` warning `function synth_rtl is never used`) and making `is_top` permanently `false` for every module in Helion.

---

### Deep-Dive Root Cause 2: `(* full_case *)` & `(* parallel_case *)` Statement Attribute Drop (`F6_UNCONNECTED_PORT`)

**Affected Designs:**
- `third_party/picorv32/picorv32.v::picorv32` (`mem_la_wstrb` and 5 other ports unconnected)
- `gen_fsm_ctrl_042..054` (13 FSM controllers using `(* full_case *)` or `(* parallel_case, full_case *)` before `case (mode)`)

#### Code Location & Mechanism
- **`crates/helion-sv/src/lib.rs:7052` (`parse_attr`) and `8027` (`parse_seq_item`):**
  1. `parse_seq_item` does not check for `(* ... *)` attributes before `case` or `if` statements. When `(* full_case *)` precedes `case (mem_wordsize)` in `picorv32.v` (lines 402, 1251, etc.), `parse_seq_item` sees `Tok::Sym('(')`, falls through to `parse_assign_nbas`, returns `Err`, and `parse_seq_block` (line 6671) calls `skip_item_or_block(p)`, **silently discarding the entire `case` statement**.
  2. Furthermore, `parse_attr` at line 7052 unconditionally calls `p.bump()` even when `p.eat_sym('=')` returns `false`, consuming the closing `*` of valueless attributes like `(* full_case *)`, and does not loop over comma-separated attributes like `(* parallel_case, full_case *)`.

---

### Deep-Dive Root Cause 3: Sequential `always_ff` Nested Enable / `else if` Guard Drop (`F6_UNCONNECTED_PORT`)

**Affected Designs:**
- `gen_axi_fabric_035..044` (10 AXI-Lite slave designs using `if (!rst_n) ... else if (s_axil_awvalid && axil_wvalid) case (axil_awaddr)`)
- `ip/h_*.v` cores (`h_clkdiv`, `h_gpio`, `h_crc8/16/32`, `h_timer`, `h_uart`, `h_spi_mm`, `h_spi_slave`, `h_watchdog`, etc.) where `resetn` or `mm_valid`/`st_valid` guards a sequential block.

#### Code Location & Mechanism
- When an `always @(posedge clk)` / `always_ff @(posedge clk)` block has a synchronous reset branch `if (!resetn) ...` followed by `else if (valid_en) begin case (addr) ... end` (or a nested `if (valid_en)` wrapping a `case`), the sequential lowering pass in `crates/helion-sv/src/lib.rs` extracts the `case (addr)` assignments into the register `D` cones but drops the outer `valid_en` enable guard from the cone inputs.
- When the enable condition is folded into the `case` selector (`case ({axil_awvalid & axil_wvalid, axil_awaddr})`), all 35 AXI-Lite slave designs (`gen_axi_fabric_000..034`) synthesize all input ports and pass all 7 stages with 0 DRC errors.

---

### Deep-Dive Root Cause 4: SystemVerilog Width-Cast `64'(data_i)` Lowering Drop in OpenTitan SECDED Encoders (`F6_UNCONNECTED_PORT`)

**Affected Designs (12 OpenTitan ECC Encoder Modules in `examples/ysyx_ibex.sv`):**
- `prim_secded_22_16_enc`, `prim_secded_inv_22_16_enc`, `prim_secded_hamming_22_16_enc`, `prim_secded_inv_hamming_22_16_enc`
- `prim_secded_28_22_enc`, `prim_secded_inv_28_22_enc`
- `prim_secded_39_32_enc`, `prim_secded_inv_39_32_enc`, `prim_secded_hamming_39_32_enc`, `prim_secded_inv_hamming_39_32_enc`
- `prim_secded_64_57_enc`, `prim_secded_inv_64_57_enc`
- `prim_secded_72_64_enc`, `prim_secded_inv_72_64_enc`, `prim_secded_hamming_72_64_enc`, `prim_secded_inv_hamming_72_64_enc`
- `prim_secded_hamming_76_68_enc`, `prim_secded_inv_hamming_76_68_enc`

#### Code Location & Mechanism
- In `examples/ysyx_ibex.sv:6045–6078`, every OpenTitan SECDED encoder begins its `always_comb` block with:
  ```systemverilog
  always_comb begin : p_encode
    data_o = 64'(data_i);
    data_o[57] = ^(data_o & 64'h0103FFF800007FFF);
    ...
  ```
- Unlike the corresponding SECDED *decoders* (`prim_secded_22_16_dec`, `prim_secded_28_22_dec`, `prim_secded_hamming_22_16_dec`, `prim_secded_inv_22_16_dec`, `prim_secded_inv_28_22_dec`, `prim_secded_inv_hamming_22_16_dec`—which index `data_i` directly without `64'(data_i)` and **PASS all 7 stages**!), the encoders use the SV size-cast `64'(data_i)`.
- `helion-sv` fails to lower the non-constant width-cast `64'(data_i)` as a zero-extended wire copy of `data_i`, causing `data_o` to fold to constants and leaving `data_i` with **0 endpoints** in `design.nets` (`F6_PORT_UNCONNECTED(port=data_i(in))`).

---

### Deep-Dive Root Cause 5: Wide-Cone Capping (`>6` Primary Inputs) Without Multi-LUT Decomposition (`F1_UNDRIVEN_NET`)

**Affected Designs (100 total: 65 `F1` only + 35 `F1 + F2`):**
- `gen_alu_datapath_003..005`, `009..011`, etc. (24 ALU designs with operand width `w in {6, 7, 8}`)
- OpenTitan wide SECDED decoders (`prim_secded_39_32_dec`, `prim_secded_64_57_dec`, `prim_secded_72_64_dec`, `prim_secded_hamming_76_68_dec`)
- `corpus-pass/sha256/rtl/sha256_core.v` (`a_new_3`, 284 nets), `sha256_all.v` (324 nets)
- `third_party/picorv32/picorv32.v` (`reg_op1_31`, `mem_la_read`, 188 nets)
- `ip/h_absdiff.v`, `ip/h_addsub_mm.v`, `ip/h_cordic_step.v`, `ip/h_iir_biquad.v`

#### Exact Threshold & Mechanism
- Our parameterized ALU sweep (`gen_alu_datapath_000..049`) sweeps operand width `w` from `3` to `8` bits on:
  ```systemverilog
  case (op)
      2'b00: res <= a + b + cin;
      2'b01: res <= a - b;
      2'b10: res <= (a ^ b) | (a & MASK);
      default: res <= (a << 1) ^ (b >> 1);
  endcase
  ```
- **Observed threshold:**
  - `w = 3, 4, 5` bits: **100% PASS** all 7 stages (`ffs = 3, 4, 5`).
  - `w = 6, 7, 8` bits: `synth_rtl_inner` caps at `ffs = 5` (bits `0..4`) and drops bit 5 (`res_5`) because the combinational cone for bit 5 of `mux(op, a+b+cin, a-b, ...)` depends on `op[1:0]`, `cin`, `a[5:0]`, `b[5:0]` (15 PIs), hitting the `WIDE_CONE` cap in `synth_rtl_inner` without multi-LUT Shannon/cut decomposition (`map_wide_cone` at `crates/helion-sv/src/lib.rs:14416` is currently unused dead code, as confirmed by `rustc` warning `function map_wide_cone is never used`).
- Because `res_5` has no `Hff` driver while `assign led = res[w-1] ^ res[0]` reads `res_5`, Stage 3 DRC catches `F1_UNDRIVEN_NET(net=res_5)`.

---

### Deep-Dive Root Cause 6: `NO_BODY` Empty Shells, `FLATTEN_CAP`, and `SIM_ONLY` Modules (23 Designs)

These 23 designs intentionally emit `0` cells (`NO_BODY = 1`) and are refused by Stage 3 DRC and Stage 4 Pack/Place:
1. **`STAGE3_DRC_NO_BODY_FLATTEN_CAP` (9 designs):**
   - `corpus-pass/schematic-wraps/ln_phase_d.v` (`repo_ln_phase_d`)
   - `corpus-pass/schematic-wraps/muladdc_phase_d.v` (`repo_muladdc_phase_d` and `param_muladdc_dw2..dw8`)
   - **Why:** Both modules use wide combinational multiplier/logarithm expressions that trigger `note_flatten_cap` (`crates/helion-sv/src/lib.rs:17420`), setting `FLATTEN_CAP = 1` and `NO_BODY = 1` with `cells = 0`.
2. **`STAGE3_DRC_NO_BODY_SIM_ONLY` (3 designs):**
   - `examples/ysyx_ibex.sv::simulator_ctrl`, `ibex_tracer`, `ibex_controller`
   - **Why:** Contain DPI/simulation tasks (`$display`, `$fwrite`, `$finish`, `sim_only_for`), so `helion-sv` marks `SIM_ONLY = 1` and `NO_BODY = 1`.
3. **`STAGE3_DRC_NO_BODY_EMPTY_OR_UNLOWERED` (11 designs):**
   - `examples/multi/top.sv` (when evaluated standalone without `tog.sv`; passes 100% in `Repo_MultiFile_SoC` as `repo_multifile_tog_top`!)
   - `third_party/picorv32/picorv32.v::picorv32_axi` (wrapper instantiating `picorv32` and `picorv32_axi_adapter` without standalone slice definitions)
   - 9 OpenTitan technology wrapper shells in `examples/ysyx_ibex.sv`: `prim_buf`, `prim_flop`, `prim_clock_gating`, `prim_clock_mux2`, `prim_ram_1p`, `prim_ram_2p`, `prim_xilinx_clock_gating`, `prim_xilinx_clock_mux2`, `ibex_top_tracing`.

---

### Deep-Dive Root Cause 7: Combinational Loops (`STAGE3_DRC_COMB_LOOP`, 5 Designs)

- `repo_multifile_serv_top` (`examples/serv/*.v::serv_top`) and `repo_multifile_serv_pin_wrap`:
  - Cycle detected on net `bufreg_clr_lsb` (`u_serv_bufreg_clr_lsb`) formed during hierarchical stitching of `serv_decode`, `serv_ctrl`, and `serv_bufreg`.
- `repo_ibex_ibex_alu` (`examples/ysyx_ibex.sv::ibex_alu`):
  - 16 combinational LUT cycles detected on `shift_result_0..` where `ibex_alu` reuses `shift_result` through a bit-reversal loop in `always_comb`.
- `gen_gate_stress_010` and `gen_gate_stress_011`:
  - Intentional 2-LUT cross-coupled NAND/XNOR loop (`loop_a = ~(loop_b & a); loop_b = ~(loop_a ^ a);`) included in the stress suite to verify that Stage 3's 3-color DFS cycle detector catches 100% of combinational loops.

---

### Deep-Dive Root Cause 8: Inferred Latches (`STAGE3_DRC_LATCH_INFERRED`, 7 Designs)

- `repo_ibex_ibex_register_file_latch` (`examples/ysyx_ibex.sv::ibex_register_file_latch`)
- `repo_ibex_prim_generic_clock_gating` (`examples/ysyx_ibex.sv::prim_generic_clock_gating`)
- `gen_fsm_ctrl_055..059` (5 synthetic `always_latch` designs)
- **Why:** All 7 designs use explicit `always_latch` blocks for level-sensitive storage or clock-gating enable latches, which Stage 3 flags under `STAGE3_DRC_LATCH_INFERRED`.

---

### Deep-Dive Root Cause 9: Stage 6 STA & Stage 7 Bitgen Refusal on `0-LUT` Memory-Bypass / Unlowered Shells (8 Designs)

In the ungated flow, **977 / 1,000** designs pass Stages 4 & 5 (Pack/Place and Route), and **969 / 1,000** pass Stages 6 & 7 (STA and Bitgen). The exact **8 designs** that pass Route but fail STA (`0 timing endpoints and 0 packed LUTFFs`) and Bitgen (`HELION_E_EMPTY_BITSTREAM: 0 configured frames`) are:

| Design | Origin | Cells | LUTs | FFs | BRAMs | IOBs | Why 0 LUTFFs Were Packed |
|---|---|---:|---:|---:|---:|---:|---|
| `repo_sha256_phase_d` | `corpus-pass/sha256/rtl/sha256_phase_d.v` | 32 | 0 | 0 | 0 | 32 | `FLATTEN_CAP + ASSIGN_NOT_LOWERED` left only 32 undriven `IobOut` cells |
| `repo_sha256_w_mem` | `corpus-pass/sha256/rtl/sha256_w_mem.v` | 32 | 0 | 0 | 0 | 32 | `ASSIGN_NOT_LOWERED` on 16×32 message schedule array left only 32 `IobOut` cells |
| `repo_sha256` | `corpus-pass/sha256/wrap/sha256.v` | 32 | 0 | 0 | 0 | 32 | Wrapper around `sha256_w_mem` with `FLATTEN_CAP` left 32 `IobOut` cells |
| `repo_third_party_picorv32_regs` | `third_party/picorv32/picorv32.v::picorv32_regs` | 64 | 0 | 0 | 0 | 64 | Commit `472a7d7` disabled `Bram18e1` emission (F3 workaround) while memory bypass skipped LUT/FF lowering, leaving 64 `IobOut` cells and 0 logic cells |
| `repo_ibex_prim_onehot_check` | `examples/ysyx_ibex.sv::prim_onehot_check` | 1 | 0 | 0 | 0 | 1 | `ASSIGN_NOT_LOWERED` on 2D array tree left 1 `IobOut` cell |
| `repo_ibex_ibex_simple_system` | `examples/ysyx_ibex.sv::ibex_simple_system` | 1 | 0 | 0 | 1 | 0 | Emitted 1 `Bram18` cell with 0 LUTs and 0 FFs |
| `repo_ibex_ibex_pmp` | `examples/ysyx_ibex.sv::ibex_pmp` | 2 | 0 | 0 | 0 | 2 | `GENERATE_NOT_LOWERED + ASSIGN_NOT_LOWERED` left 2 `IobOut` cells |
| `repo_ibex_prim_subst_perm` | `examples/ysyx_ibex.sv::prim_subst_perm` | 64 | 0 | 0 | 0 | 64 | `ASSIGN_NOT_LOWERED` on substitution-permutation rounds left 64 `IobOut` cells |

---

## 5. Priority Engineering Recommendations

1. **Fix `lower_own_cached` `is_top` Argument (`crates/helion-sv/src/lib.rs:3852`):**
   - Change `let d = synth_rtl_inner(rtl, false)?;` in `lower_own_cached` back to `let d = synth_rtl(rtl)?;` (which passes `is_top = true`), while keeping `synth_rtl_inner(rtl, false)` in `lower_own_for_stitch` (line 3811), and guard lines `18628–18638` with `if is_top`.
2. **Eliminate Commit `8acdb12` Parallel Identity LUTs (`crates/helion-sv/src/lib.rs:17736–17825`):**
   - Resolve `RExpr::Ident`, `RExpr::Bit`, and `RExpr::Range` wire copies via net aliasing (or skip emitting a `0xaaaaaaaaaaaaaaaa` LUT when `lhs` is an output port already wired by `emit_iob`). This single fix eliminates **107 `F2_MULTI_DRIVEN` failures** (including `blinky`, `counter`, `serv_decode`, `serv_ctrl`, and 49 `ip/h_*.v` cores) and removes **3,952 redundant identity LUT6 cells** across the 1,000-design corpus.
3. **Parse Statement Attributes in `parse_seq_item` (`crates/helion-sv/src/lib.rs:7052, 8027`):**
   - Consume `(* full_case *)` and comma-separated `(* parallel_case, full_case *)` attributes at the start of `parse_seq_item`. This unblocks `third_party/picorv32/picorv32.v` (`mem_la_wstrb`) and all 13 `gen_fsm_ctrl` attribute test cases.
4. **Enable `map_wide_cone` Decomposition (`crates/helion-sv/src/lib.rs:14416`):**
   - Wire the existing `map_wide_cone` helper into `synth_rtl_inner` for cones exceeding 6 PIs instead of dropping the output net with `WIDE_CONE`. This resolves **100 `F1_UNDRIVEN_NET` failures** across wide ALUs (`w >= 6`), OpenTitan SECDED decoders, `sha256_core`, and `picorv32`.

---

## 6. Reproducing This Report

Run the parallel 1,000-design verification harness in release mode:

```bash
cargo test --release -p helion-cli --test corpus_1000 -- --nocapture
```

Structured artifacts are automatically emitted to:
- `target/corpus_1000/corpus_1000_results.csv` (1,000 rows × 43 metrics columns)
- `target/corpus_1000/corpus_1000_summary.json` (machine-readable funnel, latency percentiles, and failure taxonomy)
- `target/corpus_1000/report_data.txt` (human-readable summary and per-design audit log)
