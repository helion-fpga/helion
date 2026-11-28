# Closing the gaps with Vivado: published research and open-source prior art

Research only. No code changes were made to produce this document. Written 2026-11-28
for Saksham, as the plan behind `docs/qor-plan/README.md`. Base branch: `fm/qor-plan`
at `2678533a`. This document lives on branch `fm/qor-research`.

Method: for every gap between Helion and Vivado, find the published research and
open-source prior art that shows HOW to close it, and turn it into a concrete plan.
Saksham wants methods, not reasons it cannot be done. Every claim below carries a
link. Anything that could not be verified from a primary source is marked
[UNVERIFIED], with a note on what would verify it. Numbers are quoted with their
baseline; a number measured on ASIC benchmarks, on UltraScale+, or on a blog
comparison is labelled as such and must not be quoted as a Helion number.

Conventions used throughout:

- Gain: expected effect on Helion's gap if the method transfers. "Projected" means
  estimated from the cited paper's numbers, not measured on Helion.
- Effort: S = days, M = 1-3 weeks, L = 1-2 months, XL = 3+ months or a new subsystem.
- Risk: Low / Medium / High, for correctness and schedule risk.
- Crate names are real crates in this repo (verified from `crates/` on `fm/qor-plan`):
  `helion-sv`, `helion-vhdl`, `helion-ir`, `helion-xilinx7`, `helion-ultrascale`,
  `helion-pack`, `helion-place`, `helion-route`, `helion-sta`, `helion-hls`,
  `helion-sim`, `helion-debug`, `helion-bits`, `helion-device`, `helion-ipxact`,
  `helion-lib`, `helion-ice40`, `helion-ecp5`, `helion-hl10t`, `helion-targets`.

## 0. The verified baseline

All numbers below were verified against `docs/qor-plan/README.md` on
`origin/fm/qor-plan` on 2026-11-28. They match the numbers in the tasking message.

### Size (309-design sweep, xc7z020, Vivado 2017.3)

- LUT ratio median 1.09x, p90 3.00x, 74 designs over 1.5x.
- Whole corpus +15% LUTs (74,642 vs 64,838 over 261 designs).
- FF / CARRY4 / BRAM / DSP median 1.00x each (FF p90 1.50x, carry p90 2.25x).
- PicoRV32 1.67x (1,464 vs 877 LUTs), Ibex 1.69x (5,980 vs 3,532).
- zedboard_mmcm 4.2x (155 vs 37).
- Worst single designs: `h_rot_mm` 1,489 vs 199 LUTs (7.5x), `carry_add_sub_cmp.vhd`
  181 vs 44 with 0 vs 9 CARRY4, `dsp_wide_regs` 1,191 vs 367, `h_iir_biquad`
  176 vs 53, popcount32 167 vs 53 LUTs with 54 vs 0 CARRY4.

### Speed (Vivado routes and times Helion's placement; share of Vivado's own Fmax)

- PicoRV32 92.0 vs 128.8 MHz (71%), Ibex 32.4 vs 38.9 MHz (83%),
  bram_fill slack 0.702 vs 2.094 ns (about 85%), zedboard_mmcm slack 0.825 vs
  4.326 ns (about 51%). So the Fmax range is 51-85% of Vivado, as stated.
- Worst PicoRV32 path: `cpu_state_1` -> LUT4 (fanout 85, 4.79 ns wire) -> LUT6
  (fanout 32, 3.19 ns wire) -> `cpuregs_343` CE. High-fanout control nets, not
  logic depth, dominate the worst path.

### Timer accuracy (Helion predicted slack vs Vivado measured slack, same placement)

- mmcm 3.287 vs 0.825 ns: 2.46 ns optimistic. bram 1.483 vs 0.702 ns: 0.78 ns
  optimistic. pico 8.474 vs 8.047 ns: 0.43 ns optimistic. ibex 13.748 vs 20.724 ns:
  7 ns pessimistic. blinky 6.906 vs 6.924 ns: close.
- The sign pattern matters: mmcm/bram/pico optimistic while ibex is pessimistic.
  A single net-delay scale factor cannot fix both signs. That points at two
  independent errors: a net/cell under-model on clock-heavy and BRAM paths, and
  a systematic over-count on ibex, most plausibly in the clock path, clock
  uncertainty, clock pessimism removal (CPR), MMCM/BUFG arcs, or setup-time sign.

### Targets (from README section 2)

- Size: LUT median at most 1.02x, p90 at most 1.30x, corpus at most +5%, no design
  over 2.0x; FF/CARRY4/BRAM/DSP equal to Vivado on every `micro_*` unit;
  PicoRV32 and Ibex at most 1.2x; every Ibex sweep unit packs in at most 8 GB.
- Speed: timer slack within 0.2 ns of Vivado on blinky, pico, ibex, bram, mmcm,
  dsp_prims, never more than 0.1 ns optimistic; per-net median error at most 5%,
  p95 at most 15% vs Vivado `slow_max_ps`; Fmax first 85%, then 95% of Vivado's own.

### Discrepancies and notes

- `docs/qor-plan/STATUS_MUSE.md` does NOT exist on `origin/fm/qor-plan`
  (checked 2026-11-28). The gap record lives on the `fm/qor-size` branches
  instead. Do not cite it as a file on this branch.
- The README says the timer target uses about 1.1M measured net delays from
  Vivado post-route across 8 parts (`net_delays.tsv`: net, driver site, load pin,
  load site, `slow_max_ps`, `fast_min_ps`). Those files are on the cluster, not in
  this repo; the 1.1M figure itself is [UNVERIFIED] from repo contents alone.
- Speed-grade ratios in the README (-2/-1 median critical path 0.826x, -3/-1
  0.740x, about 200 designs, xc7z020) are Helion-internal measurements, not
  published facts.

---

## 1. Size: technology mapping, resynthesis, inference

Headline finding: the p90 tail is NOT a mapper-tuning problem. The worst designs
are datapath/inference failures (rotate built as logic, popcount as a serial adder
chain, multiplies and compares not using DSP/carry). Inference on elaborated IR
is the P0 fix; the mapper is P1. A second hard caveat: even Yosys+ABC9, the best
open flow, measures 1.88x Vivado LUTs on PicoRV32 (section 1.2.2 below), while
Helion is already at 1.67x. Copying Yosys wholesale will not reach 1.0x; Helion
must beat Yosys on datapath handling.

### 1.1 Priority cuts (the core mapper to port)

- Best source: "Combinational and Sequential Mapping with Priority Cuts",
  Mishchenko, Cho, Chatterjee, Brayton, ICCAD 2007.
  https://people.eecs.berkeley.edu/~alanmi/publications/2007/iccad07_map.pdf
- Numbers: keeps typically 5-10 priority cuts per node instead of exhaustive
  enumeration. Depth-optimum mapping for 95% of benchmarks and LUT sizes even
  with only 1 cut stored per node. With 8 priority cuts, for 6-LUTs: memory down
  10x, runtime down 5x vs exhaustive enumeration, depth and area comparable or
  better. For 8+ input LUTs: about 50x memory/runtime reduction. Sequential
  extension: 18-22% depth improvement with 3-5% LUT penalty vs combinational
  mapping followed by retiming. Baseline is exhaustive cut enumeration / prior
  ABC mapping on MCNC/IWLS-style benchmarks, not Vivado.
- Helion crate: `helion-ir` (AIG, cut enumeration, priority-cut selection) and
  `helion-xilinx7` (LUT6/LUT6_2 truth tables, CARRY4 as whitebox, MUXF7/8).
- Expected gain: median 5-10% LUTs if Helion's current mapper is naive cut
  enumeration or greedy covering; near zero if it already does priority cuts well.
  Does not fix `h_rot_mm` or popcount32.
- Effort: L to port correctly (AIG, cut merge/dominance, depth-first then
  area-flow then exact-local-area recovery, 3 passes). S-M to prototype by
  shelling out to ABC first.
- Risk: Medium. Correctness risk is in cut dominance pruning and area recovery
  breaking depth; mitigate with `helion-sim` equivalence checks and EPFL LUT-6
  counts as a gate.
- Next steps: (1) Add an AIG in `helion-ir` if not present; emit AIGER/BLIF for
  ABC interchange. (2) Implement priority cuts with C=8, K=6, depth-first
  ordering, then area-flow, then exact local area, exactly as in section 3 of
  the paper. (3) Gate on EPFL arithmetic benchmarks (section 1.6.1) plus the
  309-design corpus; require no design over 2x Vivado. (4) Model CARRY4 as a
  whitebox so the mapper does not map adder logic into LUTs that should be carry.

### 1.2 Mapping with structural choices / choice networks (lossless synthesis)

- Best source: "Improvements to Technology Mapping for LUT-Based FPGAs",
  Mishchenko, Chatterjee, Brayton, IEEE TCAD 26(2), Feb 2007.
  https://janders.eecg.utoronto.ca/pdfs/area.pdf (full PDF not opened in this
  research pass; the numbers below are from the search-verified paper record).
- Numbers: baseline DAOmap on the same optimized netlists, MCNC benchmarks.
  Improved area recovery alone: area 6% smaller, delay preserved. Mapping with
  structural choices: delay -6%, area -12%, runtime 1.6x. Five iterations of
  mapping with choices: delay -10%, area -19%, runtime 8x. The earlier FPGA 2006
  version reports -7% delay, -14% area vs DAOmap.
- Helion crate: `helion-ir` (choice/equivalence classes over AIG nodes,
  FRAIG-style simulation+SAT equivalence) and `helion-xilinx7` (mapper must read
  choices when enumerating cuts).
- Expected gain: 6-12% LUTs on top of section 1.1, at 1.6x mapper runtime;
  10-19% only at 8x runtime, which is not worth it for the full corpus.
- Effort: L. Risk: Medium-high. Equivalence-class construction is the hard part;
  a wrong choice silently changes function. Requires SAT/simulation equivalence
  proof per choice.
- Next steps: (1) Ship section 1.1 without choices; keep a `choices` field on AIG
  nodes as a no-op placeholder. (2) Generate choices only from Helion's own
  resynthesis history (delay-optimized vs area-optimized AIG snapshots), i.e.
  lossless synthesis, not brute-force algebraic choices. (3) Prove each choice by
  simulation + SAT before the mapper may use it; log any unproven choice as a hard
  error, not a warning. (4) Enable choices only for the p90 tail designs first;
  measure runtime before enabling corpus-wide.
- Note: no 2023-2025 Mishchenko `&if` paper with verified numbers beating the
  2007 choice-network numbers was found in this research pass, so the 2007 TCAD
  numbers remain the best verified choice-network reference. [UNVERIFIED whether
  a newer result exists.]

### 1.3 DAOmap and FlowMap (background, not separate mappers)

- DAOmap: "DAOmap: A Depth-optimal Area Optimization Mapping Algorithm for FPGA
  Designs", Chen and Cong, ICCAD 2004.
  https://people.eecs.berkeley.edu/~alanmi/benchmarks/fpga/iccad04_daomap.pdf
  Numbers: 16.02% better area and 24.2x faster on average vs CutMap, mapping to
  5-LUTs; guarantees optimal depth under the unit-delay model. Largely subsumed
  by priority cuts + area recovery (sections 1.1, 1.2). Do not implement DAOmap as
  a separate mapper; read it only for its node-duplication-aware cut-cost idea
  and fold that cost into the area-flow pass of section 1.1.
- FlowMap: "FlowMap: An Optimal Technology Mapping Algorithm for Delay
  Optimization in Lookup-Table Based FPGA Designs", Cong and Ding, IEEE TCAD
  13(1), 1994. Secondary summary: https://algorithm.tamnd.com/flowmap/ .
  Proves least-depth mapping in polynomial time O(Kmn) for K-bounded networks
  under unit delay, via max-flow min-height K-feasible cuts. It does not optimize
  LUT count; area is a post-process. Original paper URL [UNVERIFIED - not
  retrieved in this pass]. Use FlowMap depth only as a diagnostic: if Helion's
  mapped depth exceeds FlowMap depth by more than 1 level on a design, the
  priority-cut ordering is wrong. Effort S for a depth-bound checker. Risk Low.

### 1.4 WireMap (routability / edge reduction, not LUT reduction)

- Best source: "WireMap: FPGA Technology Mapping for Improved Routability and
  Enhanced LUT Merging", Jang, Chan, Chung, Mishchenko, ACM TRETS 2(2), 2009.
  https://dl.acm.org/doi/10.1145/1344671.1344680
- Numbers: baseline state-of-the-art ABC mapping with structural choices on
  ISCAS/MCNC/ITC benchmarks. Average edge (pin-to-pin connection) reduction 9.3%
  while maintaining depth and LUT count. After VPR place/route: total wirelength
  -8.5%, minimum channel width -6.0%, critical-path delay -2.3%. Dual-output LUTs
  after merging: -9.4% on academic benchmarks, -6.3% on 20 large industrial
  Virtex-5 designs; single-output LUT count only -1.3% vs mapping with choices.
- Helion crate: `helion-xilinx7` (edge-flow cost in area recovery) and
  `helion-pack` (dual-output LUT6_2 merging is where the -9.4% lands).
- Expected gain: about 0% LUT count, about 9% fewer edges, better packing.
  Relevant to size only indirectly, via packer density.
- Effort: M. Risk: Low-medium.
- Next steps: (1) Do not do this before section 1.1. (2) Add edge-flow as a
  tie-breaker in area recovery (same area-flow, lower edge-flow wins).
  (3) Measure packer LUT6_2 utilization before/after; if it does not move, revert.

### 1.5 Yosys abc9 flow: what it does, what it measures, what Helion can borrow

#### 1.5.1 abc9 mechanics

- Best source: "Setting up a flow for ABC9", Yosys documentation.
  https://yosyshq.readthedocs.io/projects/yosys/en/0.44/yosys_internals/extending_yosys/abc_flow.html
  and the full `synth_xilinx` script:
  https://yosyshq.readthedocs.io/projects/yosys/en/latest/cmd/index_techlibs_xilinx.html
- What abc9 does: ABC with whitebox/blackbox awareness. LUTs are annotated
  `(* abc9_lut=N *)` with relative area; DFFs `(* abc9_flop *)` (must init to 0,
  no async set/reset; `abc9 -dff` passes the flop itself for sequential
  optimization). A box is combinational hard logic: carry chains are best as
  whiteboxes, DSPs as blackboxes, LUTRAMs as whiteboxes. The doc states boxes
  are arguably the biggest advantage ABC9 has over ABC, because it avoids
  optimizing for a path that is not the actual critical path, and the longer
  paths let it reduce area by mapping other logic to larger-but-slower cells.
- `synth_xilinx` order (verified in the doc): `proc`, `opt`, `fsm`, `wreduce`,
  `memory_dff` + `alumacc` + `maccmap` + `techmap mul2dsp` + `xilinx_dsp` (DSP
  first), then `techmap cmp2lut/cmp2lcu`, `alumacc`, `share`, `memory -nomap`,
  `memory_libmap`, `techmap lutrams/brams`, `memory_map`, `opt`, `xilinx_srl`,
  `techmap arith_map`, `dfflegalize`, `techmap ff_map`, then
  `abc9 -luts 2:2,3,6:5[,10,20]`, `xilinx_srl -fixed`, `techmap lut_map`,
  `xilinx_dffopt`, `opt_lut_ins`.
- Helion crate: `helion-ir` (whitebox/blackbox cell model), `helion-xilinx7`
  (LUT6/CARRY4/DSP48E1/BRAM models with delays), `helion-sv`/`helion-vhdl`
  (frontends must preserve the cells, not lower them to gates before mapping).
- Expected gain: the mechanism matters more than a single number; see 1.5.2.
- Effort: M to model whiteboxes in Helion's mapper; L to call ABC9 as a
  subprocess with a correct `cells_sim.v`-equivalent model.
- Risk: Medium. A wrong delay/area model in the whitebox silently misguides the
  mapper. ABC9's `-dff` sequential mode also changes FF semantics and needs
  `dfflegalize`/`zinit` equivalents.
- Next steps: (1) Copy the `synth_xilinx` pass order above as Helion's reference
  pipeline; audit Helion's current order against it, especially DSP-before-ABC
  and carry-before-ABC. (2) Give CARRY4, DSP48E1, RAMB36E1, LUTRAM/SRL explicit
  delay/area costs in `helion-xilinx7` before any mapper change. (3) Prototype
  ABC-as-subprocess (section 1.5.3) before porting any ABC algorithm to Rust.

#### 1.5.2 Measured Yosys+ABC9 vs Vivado LUT counts

- Best source: "openxc7 or Vivado: Build Times and Results Measured", HDL Factory,
  Oct 8, 2026. https://www.hdlfactory.com/post/2026/10/08/openxc7-vs-vivado-build-times/
- Numbers: same three designs, Artix-7 200T, 100 MHz, Vivado 2025.2 vs
  Yosys+nextpnr-xilinx+Project X-Ray. PicoRV32, one core: open flow 1,671 LUTs vs
  Vivado 890 LUTs (1.88x); registers 553 vs 555, same BRAM. Sixteen cores:
  26,668 vs 14,216 LUTs (1.88x). The article states the register and BRAM counts
  are the same, so the difference is in logic optimization and LUT mapping.
  Speed 147 MHz vs about 165 MHz. This is a single blog measurement, not a
  peer-reviewed benchmark, and place/route differs (nextpnr vs Vivado), so treat
  the 1.88x as indicative, not exact.
- Helion implication: Helion PicoRV32 at 1.67x is already better than this
  Yosys+ABC9 reference at 1.88x. Copying Yosys wholesale will not reach 1.0x;
  Helion must beat Yosys on datapath handling.
- Next step: reproduce this exact comparison on Helion's own PicoRV32 (same core,
  same part family) as a standing regression: Helion vs Yosys `synth_xilinx` vs
  Vivado, LUT/FF/BRAM/DSP separately.

#### 1.5.3 What Helion can borrow: calling ABC vs porting

- Best source: ABC homepage, https://people.eecs.berkeley.edu/~alanmi/abc/abc.htm
  States ABC optimizes/maps/retimes industrial designs with 100K gates and 10K
  sequential elements in about one minute, and lists `balance`, `rewrite`,
  `refactor`, `dch`, `if`, `mfs`, `lutpack` as the relevant commands. GitHub
  mirror cited as `github.com/berkeley-abc/abc` [UNVERIFIED - not opened].
- Borrow, in order:
  1. Subprocess first (S-M, low risk): emit AIGER/BLIF from `helion-ir`, run
     `abc -c "balance; rewrite; refactor; balance; rewrite; refactor; dch;
     if -K 6 -a"` (the canonical area flow), read back BLIF, verify with
     `helion-sim` + ABC `cec`. This gives a same-week upper bound on what ABC
     mapping buys on the 309 corpus.
  2. ABC9-style whiteboxes (M): do not call plain ABC on a flattened gate
     netlist; pass CARRY4/DSP/BRAM as boxes, or extract them before ABC and
     reinsert after, exactly as `synth_xilinx` does.
  3. Port priority cuts to Rust (L, later): only after the subprocess experiment
     shows a corpus gain worth the port, and only with the EPFL gate in place.
     ABC has no stable library API; vendoring its C code into a Rust crate is a
     build/licensing/maintenance commitment, so check the ABC license before
     vendoring [UNVERIFIED - license not checked in this pass].
- Risk: subprocess ABC is a build dependency and a version-skew risk; porting is
  a correctness risk. Either way, never let ABC see a netlist from which
  DSP/carry/BRAM have already been destroyed by lowering.

### 1.6 Resynthesis: rewrite / refactor / balance, SOP balancing, AIG rewriting, don't-care resynthesis

#### 1.6.1 DAG-aware AIG rewriting + balancing (the `resyn2` core)

- Best source: "DAG-Aware AIG Rewriting: A Fresh Look at Combinational Logic
  Synthesis", Mishchenko, Chatterjee, Brayton, DAC 2006.
  https://people.eecs.berkeley.edu/~alanmi/courses/2007_290N/papers/synthesis_berkeley_dac06.pdf
- Numbers: alternating DAG-aware rewriting (reduces area by sharing common logic
  without increasing delay) and algebraic balancing (minimizes delay without
  increasing area). On large industrial benchmarks, comparable or better quality
  than SIS/MVSIS while several orders of magnitude faster; a robust
  implementation took a few person-weeks. No single LUT % is quoted in the
  verified excerpt; do not cite one. The ABC homepage confirms the flow is
  `balance; rewrite; refactor` and that these commands accumulate structural
  choices.
- Helion crate: `helion-ir` (AIG rewrite/refactor/balance passes, before mapping).
- Expected gain: 5-15% LUTs on control/random logic when iterated (`resyn2` =
  balance/rewrite/refactor cycles); small on arithmetic that should have been
  DSP/carry.
- Effort: M for rewrite+balance; L for refactor (Boolean factoring with
  don't-cares). Risk: Medium-low; local transforms, easily verified by
  equivalence checking.
- Next steps: (1) Implement AIG structural hashing first; without it, rewrite
  creates duplicates instead of sharing. (2) Implement `balance` (SOP balancing,
  section 1.6.2) and 4-input-cut `rewrite`, iterate to fixpoint, measure AIG node
  count and LUT count separately. (3) Add `refactor` only after rewrite is
  stable; it is the highest-risk transform in this group.

#### 1.6.2 SOP balancing (`if -g`)

- Best source: ABC slide deck (search-verified) states high-effort delay
  optimization is "Perform SOP balancing (`st; if -g -K 6`)" followed by area
  recovery and mapping. No standalone SOP-balancing paper with an isolated %
  was verified in this pass [UNVERIFIED as a standalone number].
- Helion crate: `helion-ir` (pre-mapping balancing pass).
- Expected gain: mainly depth, not area; helps timing, and indirectly helps area
  by letting area recovery use larger cuts on non-critical paths. Do not expect a
  LUT reduction from balancing alone.
- Effort: S-M. Risk: Low.
- Next steps: implement SOP balancing as a pre-mapping pass gated on depth;
  measure LUT count and depth separately so a depth win is not mistaken for an
  area win.

#### 1.6.3 Scalable don't-care resynthesis (`mfs`)

- Best source: "Scalable Don't-Care-Based Logic Optimization and Resynthesis",
  Mishchenko, Brayton, Jiang, Jang, ACM TRETS 4(4), Article 34, Dec 2011.
  https://people.eecs.berkeley.edu/~alanmi/publications/2011/trets11_mfs.pdf
- Numbers: baseline is 6-LUT networks after high-effort synthesis. On 20 large
  academic benchmarks: LUT counts -45.0%, logic levels -12.2%. On 14 industrial
  benchmarks up to 83K 6-LUTs: LUT counts -11.8%, logic levels -16.5%. Longest
  runtime about 2 min (academic) and about 30 min (industrial). The 45% academic
  number is after high-effort synthesis on already-mapped networks, not vs Vivado
  on RTL, so do not compare it directly to Helion's 1.09x median.
- Helion crate: `helion-ir` (post-mapping `mfs`-style pass) and `helion-xilinx7`
  (mapped LUT network as input).
- Expected gain: 10-12% on industrial-scale designs is the realistic expectation;
  the 45% academic number will not reproduce on Helion's corpus.
- Effort: XL (windowed SAT-based don't-care computation + interpolation).
  Risk: High. SAT solver dependency, long runtimes, and a real miscompile risk if
  don't-cares are wrong.
- Next steps: (1) Defer until sections 1.1 and 1.6.1 are shipped and measured.
  (2) First prototype via ABC `mfs` as a subprocess on the 10 worst p90 designs
  only; if it does not move them, do not port it. (3) If ported, require SAT proof
  per resubstitution and full-corpus equivalence checking.

#### 1.6.4 Lazy Man's Synthesis (LMS) and recent high-effort work

- Best source: "Lazy Man's Logic Synthesis", Yang, Wang, Mishchenko, ICCAD 2012.
  https://people.eecs.berkeley.edu/~alanmi/publications/2012/iccad12_lms.pdf
  Reuses precomputed optimal structures for 6-16 input functions, hashed by
  semi-canonical form. A 90% coverage library needs 738,239 functional classes
  for 12-input functions. No single LUT % was extracted from the verified excerpt
  [UNVERIFIED as a LUT %].
- Modern proof: "Insights from Basilisk", Sauter et al., arXiv 2405.04257.
  https://arxiv.org/html/2405.04257v1/ Baseline is the Iguana open-source flow on
  a Linux-booting RV64GC SoC (ASIC, IHP 130nm, gate equivalents, not FPGA LUTs).
  Cumulative: logic area 1.8 MGE to 1.1 MGE (1.6x), frequency 33 MHz to 77 MHz
  (2.3x), runtime 5.4h to 2.2h. The ABC-script step alone: 1.4 MGE to 1.1 MGE,
  37 MHz to 71 MHz, logic levels 149 to 54. LMS is used with a pre-generated
  6-input-cut record library. Do not translate MGE directly to LUTs.
- Also: ASPDAC 2025 randomized transduction (Miyasaka et al.): 6.8% AIG nodes
  saved on average vs the original flow at the same runtime on IWLS 2022
  benchmarks. https://people.eecs.berkeley.edu/~alanmi/publications/2025/aspdac25_trans.pdf
  (search-verified). This is a cheaper high-effort option than LMS; try it before LMS.
- Helion crate: `helion-ir` (record library + `if -y` style lookup).
- Expected gain: 5-10% on top of sections 1.1/1.6.1 if Helion builds a record
  library from its own corpus; near zero without a good library.
- Effort: L (library construction is the work, not the lookup). Risk: Medium;
  library quality dominates, and a stale library can hurt.
- Next steps: (1) Build the record library from Helion's own 309-design corpus +
  EPFL best-known LUT-6 results, not from a generic library. (2) Ship LMS as an
  optional high-effort mode, not the default, until runtime is measured.

### 1.7 Inference of DSP, BRAM, carry, LUTRAM/SRL from elaborated IR

#### 1.7.1 How Yosys infers (the model to copy structurally)

- Best source: Yosys memory handling and `synth_xilinx` documentation, 0.71-dev.
  https://yosyshq.readthedocs.io/projects/yosys/en/latest/cmd/index_passes_memory.html
  and https://yosyshq.readthedocs.io/projects/yosys/en/latest/cmd/index_techlibs_xilinx.html
- Mechanism (verified): Yosys does not pattern-match source text. It elaborates
  to RTLIL word-level cells (`$mem`, `$mul`, `$add`, `$dff`), then runs passes in
  order: `memory_dff` merges FFs at memory read ports into a synchronous port;
  `memory_share`/`opt_mem_widen` consolidate ports; `memory_collect` creates
  multiport `$mem` cells; `memory_bram -rules <file>` converts `$mem` to BRAM
  instances only if a rule matches on words/abits/dbits/ports/bits/waste/efficiency
  (e.g. `min efficiency 80`, `max waste 16384`); `memory_libmap` maps memories to
  a RAM cell library with logic-cost thresholds; `memory_map` lowers the rest to
  FFs/decoders. DSP: `alumacc`/`maccmap` find multiply-accumulate, `techmap
  mul2dsp.v` + `xilinx_dsp_map.v` map `$mul` to DSP, `xilinx_dsp` packs
  input/M/output registers, pre/post-adders, and PCOUT-PCIN cascades into DSP48E1
  (cascade chains limited to 20). LUTRAM/SRL: `techmap lutrams/brams_map.v`,
  `xilinx_srl -minlen 3` (variable and fixed).
- Helion crate: `helion-ir` (word-level `$mem`/`$mul`/`$add` cells + a
  `memory_collect`-equivalent), `helion-xilinx7` (RAMB36E1/RAMB18E1, DSP48E1,
  RAM32M/LUTRAM, SRL16E/SRLC32E rules files with explicit efficiency/waste
  thresholds).
- Expected gain: this is the P0 fix. A single 32x32 multiply moved from LUTs to
  1 DSP removes hundreds of LUTs; a 1Kx32 memory moved to BRAM removes thousands.
- Effort: M for a rules-file BRAM matcher + DSP matcher; L for full
  `memory_share`/`memory_dff` semantics. Risk: Medium. Wrong transparency/clock/
  enable semantics on BRAM is a silent miscompile; wrong DSP packing breaks timing.
- Next steps: (1) Write Helion's BRAM/DSP/LUTRAM/SRL rules as data (a rules file,
  not code), with `min efficiency` and `max waste` per primitive, copied from
  Yosys' Xilinx rules as the starting point. (2) Implement `memory_dff` first: a
  read port is synchronous only if its data goes directly into a FF on the write
  clock; async reads stay logic. (3) Add a per-design inference report (what was
  inferred, what was rejected and why) so a missed DSP/BRAM is debuggable, not
  invisible.

#### 1.7.2 How Vivado infers (UG901 / UG902 rules to match)

- Best source: Vivado Design Suite User Guide: Synthesis, UG901, 2026.1.
  https://docs.amd.com/r/en-US/ug901-vivado-synthesis/Using-Synthesis-Settings?contentId=y9h9MoDtUMgkk2CTv6aDNg
  UG902 (HLS DSP thresholds) PDF was not opened in this pass; the UG902 page
  number below is second-hand, see the note.
- Rules verified in UG901: synthesis directives are the mechanism, not source
  patterns. `AreaOptimized_high` forces ternary adders, applies new thresholds
  for carry-chain use in comparators, and implements area-optimized MUXes.
  `AreaOptimized_medium` lowers the multiplier threshold for DSP inference, moves
  shift registers into BRAM, and applies lower thresholds for CARRY use in
  comparators. `AreaMultThresholdDSP` lowers the DSP inference threshold.
  `FewerCarryChains` raises the operand-size threshold to use LUTs instead of
  carry. `AreaMapLargeShiftRegToBRAM` maps large shift registers to BRAM.
  `shreg_min_size` default is 3 (number of sequential elements that triggers SRL
  inference). `cascade_dsp` controls DSP adder-chain vs fabric tree.
  `resource_sharing` (auto/on/off) shares arithmetic operators.
  `control_set_opt_threshold` controls FF control-set optimization.
- UG902 10-bit rule: an AMD forum answer quotes UG902 2017.4 p.181: multiplications
  >= 10 bits are implemented on DSP48, otherwise in LUTs.
  https://adaptivesupport.amd.com/s/question/0D52E00006hpg2cSAA/dsp-usage-fixed-point?language=zh_CN
  (search-verified only; treat the page number as [UNVERIFIED] and the rule as
  second-hand). Note the project already landed a sub-10-bit rule (G126); verify
  it against this quote before changing thresholds, and note Vivado's
  `AreaMultThresholdDSP` means the threshold is directive-dependent, not a fixed
  constant.
- Helion crate: `helion-xilinx7` (threshold table + directive equivalents),
  `helion-ir` (the decision point).
- Expected gain: matching Vivado's thresholds is what makes DSP/BRAM counts equal
  on the micro units; without it, Helion will systematically over- or under-infer.
- Effort: S-M to encode the threshold table; M to add directive equivalents.
  Risk: Low-medium; thresholds are observable and testable per micro unit.
- Next steps: (1) Encode one threshold table in `helion-xilinx7`: DSP iff wider
  operand >= 10 bits (default), carry iff operand >= threshold, SRL iff chain
  >= 3, BRAM iff efficiency >= threshold. (2) Add a `synth_design -directive
  AreaOptimized_medium/high` equivalent as a Helion flag, so the threshold can
  move without code changes. (3) Add micro units per threshold boundary (9 vs 10
  bit multiply, 2 vs 3 deep SRL, small vs large memory) with DSP/BRAM/carry/FF
  counts asserted equal to Vivado.

### 1.8 Deciding on elaborated IR rather than source patterns

#### 1.8.1 Canonicalize arithmetic into datapath cells first, then map

- Best sources: Yosys elaboration model (search-verified,
  https://www.rapidrtl.com/learn/what-synthesis-does-to-verilog/ ): Yosys
  `read_verilog` keeps `always` as a process, `proc` turns it into
  `$add`/`$mux`/`$dff` word-level cells, `opt` folds enables/resets into the FF,
  and only `techmap` lowers to bit-level gates. The word-level stage is where
  inference happens. And CIRCT datapath dialect (Nov 2025 slides,
  http://cowardsa.github.io/assets/pdf/Synthesizing_Fast_Arithmetic_Circuits_Cornell_Nov_2025.pdf ):
  `(a*b)+(c*d)` is expressed as `datapath.partial_product` +
  `datapath.compress` + `comb.add`, not as separate multiplies and adds, so the
  compressor tree is shared. Numbers on one dot-product example: 8-bit, 5% faster
  and 4% smaller; 16-bit, 25% faster and 5% smaller vs naive. Single-example
  slide numbers, not a benchmark suite.
- Helion crate: `helion-sv` and `helion-vhdl` (frontends must emit word-level
  cells, not gates), `helion-ir` (canonical datapath cells: `$mul`, `$add`,
  `$mac`, `$mem`, `$cmp`, `$shift`, `$popcount` as a first-class cell, see 1.8.3).
- Expected gain: this is the mechanism behind the P0 fix. Without it, a multiply
  written as `a*b-c`, a sum of products, or a VHDL multiply hidden behind a
  slice/enable will never match a source pattern, and will stay in LUTs.
- Effort: M for canonicalization (operand normalization, constant folding, width
  reduction, sharing); L for a full datapath dialect with compressor trees.
  Risk: Medium. Canonicalization must preserve signedness, width, and overflow
  semantics exactly; VHDL `signed`/`unsigned` vs Verilog is the classic trap here.
- Next steps: (1) Freeze the IR contract: frontends emit word-level cells only;
  no frontend may lower `$mul`/`$add`/`$mem` to gates. (2) Add one
  canonicalization pass in `helion-ir`: normalize operand order and widths, fold
  constants, strip enables/slices around arithmetic, reassociate sums of products
  into a single `$mac` chain, then run the section 1.7 inference once, at that
  point only. (3) Add a regression test per form: plain `a*b`, sliced
  `a[7:0]*b`, constant `a*3`, enable-wrapped, `a*b-c`, sum of products; each must
  give the same DSP/LUT decision. (4) Verify with `helion-sim` value checks on
  every canonicalization, not just count checks.

#### 1.8.2 Multiplier tiling / extraction (Kumm et al.)

- Best source: "Small Logic-Based Multipliers with Incomplete Sub-Multipliers for
  FPGAs", Bottcher and Kumm, arXiv 2405.02047.
  https://arxiv.org/pdf/2405.02047
- Numbers: multiplier tiling decomposes a multiplier into sub-multiplier tiles +
  compressor tree + final adder, solved optimally by ILP for a given tile set.
  Tiles have explicit LUT6 costs (e.g. 3x3 tile: 6 LUTs, 2x3 tile: 3 LUTs, 1x2
  tile: 1 LUT, 24x17 DSP tile: 0 LUTs + compression cost about 0.65 LUTs per
  output bit). Incomplete (non-rectangular) tiles are more efficient than
  rectangular ones. Exact end-to-end % vs Vivado was not extracted from the
  opened PDF in this pass [UNVERIFIED as a single %]; the companion paper
  "Multiplier Design Addressing Area-Delay Trade-offs by using DSP and Logic
  resources on FPGAs", Bottcher and Kumm, arXiv 2407.03962,
  https://arxiv.org/pdf/2407.03962 reports significant LUT reductions vs previous
  tiling with reduced critical path, also without a single % extracted here.
- Helion crate: `helion-ir` (tiling pass for multipliers that stay in LUTs, i.e.
  sub-10-bit and DSP-exhausted cases) and `helion-xilinx7` (tile cost table for
  LUT6/CARRY4).
- Expected gain: for sub-10-bit multiplies that must stay in LUTs, tiling with
  GPCs/ternary adders is the difference between a naive array multiplier and
  Vivado's; expect 10-30% on those units, not on the corpus median.
- Effort: L (ILP or a good heuristic tiler + compressor tree + carry-chain final
  adder). Risk: Medium-high; compressor-tree correctness is subtle.
- Next steps: (1) Do not start tiling until the DSP decision (section 1.7) is
  correct; tiling a multiply that should have been a DSP is wasted work.
  (2) Start with a fixed tile table (3x3, 2x3, 1x2, Booth/Baugh-Wooley rows) and
  a Dadda/Wallace compressor using CARRY4, no ILP. (3) Measure only on sub-10-bit
  multiply micro units first.

#### 1.8.3 Popcount / rotate as first-class datapath cells (direct fix for the two worst outliers)

- Best source: "Population Count", fpgacpu.ca (verified practice, not a paper).
  http://fpgacpu.ca/fpga/Population_Count.html
- Mechanism (verified): popcount is a bit-pair LUT + adder tree; a good CAD tool
  reduces the chain of small adders to a tree of LUTs and adders with a carry
  chain log2(N)+1 bits long. The failure mode is a serial chain of (N/2)-1 small
  adders instead of a tree, which is exactly what a 167 vs 53 LUT result looks like.
- Helion crate: `helion-ir` (recognize `$popcount` / Hamming-weight reduction
  trees at the word level; recognize rotate as a barrel shifter / wire
  permutation, not a shift+or), `helion-xilinx7` (map popcount to GPC/compressor
  + CARRY4, rotate to MUXF7/8 + LUTs).
- Expected gain: popcount32 167 to about 53 LUTs (3.2x on that unit) if the tree
  is built correctly; `h_rot_mm` 1,489 to about 199 LUTs (7.5x) if rotate is a
  mux tree and the multiply inside it is correctly DSP/LUT-decided. These are the
  two largest single-design wins available.
- Effort: S-M for popcount-as-compressor-tree + rotate-as-mux-tree. Risk:
  Low-medium; both are local datapath rewrites, verifiable by simulation.
- Next steps: (1) Add a `$popcount` cell in `helion-ir`; lower it to a
  Wallace/GPC compressor tree, never to a serial adder chain. (2) Add a rotate
  canonicalization: `rotl`/`rotr` becomes a barrel-shifter mux tree, with constant
  rotates becoming rewiring (zero LUTs). (3) Add micro units `popcount32` and
  `h_rot_mm` with Vivado counts asserted; do not close this item until both are
  within 1.2x.

### 1.9 Measurement instruments and dual-output LUT packing

- EPFL Combinational Benchmark Suite: Amaru, Gaillardon, De Micheli, IWLS 2015.
  https://www.epfl.ch/labs/lsi/page-102566-en-html/benchmarks/ and
  https://raw.githubusercontent.com/lsils/benchmarks/82d8cc6910419298e713a46644ed59fd3df53038/README.md
  Numbers: 23 circuits (10 arithmetic, 10 random/control, 3 MtM). Baseline LUT-6
  mapping with ABC `if -K 6` (pure mapping, no synthesis): arithmetic total 80,638
  LUT-6, e.g. Multiplier 5,913, Adder 254, Barrel shifter 512, Square 3,985;
  random/control total 18,660 LUT-6. The suite tracks best-known LUT-6 size/depth
  per design and is the standard gate for any new mapper. Effort S to wire EPFL
  into Helion's test harness. Next step: add EPFL arithmetic + random/control as
  a nightly gate: report Helion LUT-6 vs ABC `if -K 6` vs best-known, per design,
  not just a median.
- Dual-output LUT6_2 packing: "Lookup Table Merging for FPGA Technology
  Mapping", Tu and Xu, J. Phys.: Conf. Ser. 2625, 2023.
  https://iopscience.iop.org/article/10.1088/1742-6596/2625/1/012032/pdf
  Numbers: baseline ABC `if -C 8` unmerged mapping. Merging single-output LUTs
  into dual-output LUTs by maximum-cardinality matching: average 10.21% reduction
  in LUT count without worsening delay. Helion crate: `helion-pack` (LUT6_2
  pairing after mapping), with a feedback signal into `helion-xilinx7` (prefer
  cuts that pair well). Effort M for post-mapping merging; L for dual-output-aware
  mapping with feedback. Risk: Low for post-mapping merging; medium for feedback.
  Next steps: (1) Implement post-mapping LUT6_2 merging in `helion-pack` first;
  measure site count, not just LUT count. (2) Only then add pairing feedback into
  the mapper's area-flow cost.
- Caution: "Fast Iterative Dual-Output-Aware LUT Mapping for Fracturable FPGA
  Architectures", Lu, arXiv 2607.22756, https://arxiv.org/html/2607.22756 ,
  reports 34.96% and 23.39% average reductions in LUT-area vs vanilla ABC on EPFL
  under two architecture models, 15.8x speedup vs the best prior method, depth
  -5%, area -1% vs that prior method. The arXiv identifier date (2607) looks
  anomalous and the paper was not opened in full; mark the 34.96%/23.39% numbers
  [UNVERIFIED - needs a full read before citing]. Do not use them in planning.
- Area recovery as a separate pass: depth-optimal mapping first, then area recovery
  on non-critical paths: area-flow (global) followed by exact local area
  (MFFC-local), as in Mishchenko et al. 2007 (section 1.2) and the priority-cut
  paper (section 1.1). 6-7% from area recovery alone vs no recovery (TCAD 2007).
  If Helion maps for depth and never runs area recovery, this is a free win.
  Effort S-M if the mapper already has cuts and slack; L if it does not. Risk Low.
  Next step: check whether Helion currently runs any area-recovery pass after depth
  mapping; if not, add area-flow + exact-local-area before any other mapper work.

### 1.10 Size: recommended order and stop conditions

1. Week 1: ABC-as-subprocess experiment (section 1.5.3) + inference audit
   (sections 1.7.1, 1.7.2) on the 10 worst p90 designs. Stop condition: if ABC
   subprocess does not beat Helion by more than 5% median on those 10, do not port
   ABC; fix inference instead.
2. Next: canonical datapath + inference on elaborated IR (sections 1.8.1, 1.7.1,
   1.7.2) + popcount/rotate cells (section 1.8.3). Stop condition: popcount32 and
   `h_rot_mm` within 1.2x Vivado, DSP/BRAM/carry/FF counts equal on every micro unit.
3. Next: priority-cut mapper + rewrite/balance (sections 1.1, 1.6.1, 1.9 area
   recovery). Stop condition: EPFL LUT-6 within 10% of ABC `if -K 6` per design,
   corpus median at most 1.05x, p90 at most 1.5x.
4. Later: choice networks (section 1.2), LMS (section 1.6.4), `mfs` (section
   1.6.3), dual-output-aware mapping (section 1.9). Each gated on its own
   subprocess prototype showing more than 5% on the tail before any Rust port.
5. Do not do: DAOmap as a separate mapper, FlowMap as a mapper, WireMap before
   section 1.1, or `mfs`/ILP tiling before the DSP decision is correct.

---

## 2. Timing accuracy: fitting a delay model to measured Vivado net delays

Diagnosis first: fitting net delay alone will not close the gap. The sign pattern
(mmcm/bram/pico optimistic, ibex 7 ns pessimistic) points at two independent
errors, so do the cell-arc/clock audit (section 2.5) and the net fit (section 2.1)
in parallel first. Recommendation order: sections 2.5 and 2.1 first, section 2.2
as the data source for 2.5, section 2.6 as a thin table on top, section 2.4 (ML)
only after 2.1 plateaus, section 2.3 (silicon fuzzing) last, if ever.

### 2.1 Fitting a delay model to measured Vivado net delays

- Best source: "An Open-Source Lightweight Timing Model for RapidWright",
  Maidee et al., FPT 2019, the closest published analogue to Helion's problem,
  and the method to copy first.
  http://www.rapidwright.io/docs/_downloads/FPT19-TimingModel.pdf
  Code/docs: https://www.rapidwright.io/javadoc/com/xilinx/rapidwright/timing/TimingModel.html
  Companion calibration paper: "Timing-Aware Routing in the RapidWright
  Framework", Liu and Kapre, FPL 2019.
  http://nachiket.github.io/publications/rapidroute-timing_fpl-2019.pdf
- Numbers: RapidWright FPT'19: "slightly pessimistic, lumped delay model within
  2% average accuracy of Vivado for UltraScale+ devices", validated on over 240
  designs, Spearman rho 0.99 vs Vivado. Baseline is Vivado timing on UltraScale+
  (not 7-series). [UNVERIFIED for 7-series transfer: the paper is UltraScale+
  only; 7-series wire/PIP structure differs.] Liu and Kapre FPL'19: timing model
  "within 1% error (0.01 ns) of Vivado timing analysis by running about 30
  calibration runs and needing under 60 seconds of Vivado timing analysis", on
  Xilinx XCKU115 (-3, -2, -1 speed grades). Baseline is Vivado; the prior crude
  model lost to Vivado by 0.3-0.4 ns on timing slack, and the calibrated model
  closed that gap and occasionally beat Vivado marginally. Method: route one-time
  calibration tests in Vivado, extract path delays, set up a linear system over
  unknown per-routing-resource delays, solve by least squares (system is
  low-rank), run per interconnect tile and generalize across the die by symmetry.
- Older baseline: "An Empirical Model for Accurate Estimation of Routing Delay in
  FPGAs", Karnik and Kang, ICCAD 1995.
  https://cecs.uci.edu/~papers/compendium94-03/papers/1995/iccad95/pdffiles/05d_1.pdf
  Predicts delays "within 20% of actual" on AT&T ORCA, using inter-PLC distance,
  circuit size, fanout, and routing congestion, via average-value tables plus
  interpolation. Useful only as a feature prior, not as a model: 20% is far from
  Helion's 5% median target and the device is obsolete.
- VPR's model (for the Elmore form, not fitted numbers): routing delay is Elmore
  RC over the route tree, `Tdel(switch) + R(switch)*C(to) + 0.5*R(to)*C(to)` for a
  buffered switch, with unbuffered-switch upstream resistance added back.
  Sources: https://docs.rs/guide/0.0.25/guide/fpga/struct.RrGraph.html and
  https://docs.verilogtorouting.org/en/latest/tutorials/timing_simulation/ .
  VPR takes all delays from the architecture file (`delay_constant` /
  `delay_matrix`, `T_setup`, `T_clock_to_Q`); it does not fit them.
- What features matter, in priority order (from RapidWright, Liu/Kapre, prjxray,
  and Karnik):
  1. Wire class / TimingGroup: RapidWright lumps connected PIPs/nodes/pins into a
     coarse TimingGroup and fits one delay per group type and distance band
     (NEAR/MID/FAR, horizontal/vertical, with K0/K1/K2 coefficients per group
     type). For 7-series the analogue is wire length class (SINGLE, DOUBLE, QUAD,
     LONG, BOUNCE, clock, etc.) plus direction.
  2. PIP count by switch type: buffered switch vs pass-transistor PIP have
     different intrinsic delay and R/C behaviour; count them separately.
  3. Manhattan distance and tile hops (dx, dy, INT tile count): the main proxy
     when the exact route is unknown (pre-route / placement estimate).
  4. Fanout / downstream capacitance: total load pins, and per-load pin
     capacitance; Elmore delay grows with downstream C, so a net's delay is
     per-sink, not per-net. Helion's `net_delays.tsv` is already per load pin,
     which is the right grain.
  5. Driver site / load site and pin identity: source BEL pin (e.g. LUT A1..A6
     have different delays in RapidWright's `LOGIC_LUT_A*_DELAY` /
     `SITEPIN_A*_DELAY` fields) and sink pin type; SLICEL vs SLICEM, BRAM/DSP/IO
     sites differ.
  6. Clock vs data nets: prjxray's own model explicitly fails on clock nets
     (section 2.2); treat them as a separate model or a table lookup, not as part
     of the data-net fit.
  7. Speed grade and part: as a multiplicative table on top of the -1 fit
     (section 2.6), not as a feature inside one pooled fit.
- Fit approach, two-stage, in this order: (a) linear least squares per routing
  resource, Liu/Kapre style: each measured net delay is a sum of unknown
  per-resource delays along its route; solve the (low-rank) system per tile type,
  exploiting die symmetry. Interpretable, debuggable, and directly yields
  per-PIP/wire delays the router can also use as costs. (b) If residuals still
  miss the 5%/15% target, gradient-boosted trees per wire class on the features
  above, with the linear model as a baseline feature, trained on the 1.1M rows and
  validated by held-out design (never held-out net from a seen design). Do not
  fit one global model across all 8 parts; fit per part family, or per part with
  part as a categorical, and check whether coefficients actually transfer.
- Helion crate: primary `helion-xilinx7` (delay tables, wire/PIP taxonomy) +
  `helion-sta` (consumes the model). Secondary: `helion-route` (router cost should
  use the same per-resource delays, or place/route and STA will disagree);
  `helion-place` for the pre-route distance-based estimate.
- Expected gain: if RapidWright/Liu-Kapre transfer, per-net error about 1-2%
  average on data nets, which would bring mmcm/bram/pico slack errors from
  0.4-2.5 ns toward the 0.2 ns target, assuming cell arcs are also right. It will
  not by itself fix ibex's 7 ns pessimism, which is the wrong sign for a net
  under-model. Estimate, not a verified Helion number.
- Effort: M (linear fit) to L (GBM + validation harness). Risk: Medium. Risks:
  7-series transfer of UltraScale+ results is unproven; clock nets need separate
  handling; overfitting to the 309-design corpus (held-out-design validation is
  mandatory); route extraction (which PIPs/wires each Vivado net used) must be
  exact or the least-squares system is garbage.
- Next steps: (1) Build the route-feature extractor: for each row of
  `net_delays.tsv`, recover the Vivado route (PIP list, wire classes, dx/dy,
  fanout) from the routed design database; refuse to fit until route recovery is
  verified on a sample by hand. (2) Implement the Liu/Kapre least-squares fit per
  wire class and switch type, per part; report per-net median/p95 error on
  held-out designs, split by wire class, fanout bucket, and clock vs data.
  (3) Only then try GBM on residuals; keep the linear model as the shipped
  fallback if GBM does not beat it out-of-design. (4) Re-run the five named
  designs (mmcm, bram, pico, ibex, blinky) and report slack error before/after;
  if ibex is still pessimistic, stop fitting nets and audit section 2.5.

### 2.2 Project X-Ray timing data

- Best source: prjxray `timing.py` + timing minitest + `prjxray-db/*/timings/*.sdf`
  (the actual database, not a paper). Model code:
  https://foss-fpga-tools.googlesource.com/prjxray/+/refs/heads/dependabot/submodules/third_party/python-sdf-timing-ea99ca8/prjxray/timing.py
  Minitest doc:
  https://f4pga.readthedocs.io/projects/prjxray/en/latest/db_dev_process/minitests/timing.html
  Worksheet generator:
  https://github.com/SymbiFlow/prjxray/blob/84b1457b885861fee221c5c6255d41cfb962ffc0/utils/create_timing_worksheet_db.py
  Example SDF:
  https://GitHub.com/f4pga/prjxray-db/blob/master/artix7/timings/CMT_TOP_R_LOWER_T.sdf
  Repo root: https://github.com/f4pga/prjxray/tree/ad182f6a71cdc4036cb5fd8befdcbaa25ebc2ec1
- What the timing database actually contains: no per-PIP silicon-measured delays.
  What exists is: (a) an Elmore RC model in `prjxray/timing.py`: routing delay =
  intrinsic delay of each element + Elmore delay of the RC tree, with 5 node types
  (site Outpin, buffered switch, pass-transistor switch, Wire with pi-model
  capacitance, site Inpin), PVT corners FAST/SLOW, and delays stored as 4-tuples
  `(fast_min, fast_max, slow_min, slow_max)`; (b) per-tile-type SDF files in
  `prjxray-db/<family>/timings/*.sdf` (e.g. `slicel.sdf`, `DSP_L.sdf`,
  `CMT_TOP_R_LOWER_T.sdf`) containing cell IOPATH delays and SETUP/HOLD timing
  checks per BEL, in the same 4-tuple form (example from prjxray issue 1286:
  `(IOPATH A P (0.587::2.142)(1.346::5.070))`); (c) a timing minitest that compiles
  small designs in Vivado, extracts nets and routes, and runs
  `create_timing_worksheet_db.py` to build a spreadsheet comparing the prjxray
  reduced model against Vivado timing, using JSON from Vivado's
  `write_timing_info.tcl`.
- What it does not contain: no PIP timing table, no wire R/C table with a
  documented provenance in the files read, and no speed-grade dimension beyond
  the FAST/SLOW corners. Where the R/C and intrinsic values came from (fitted
  from Vivado SDF vs measured) is [UNVERIFIED]; do not assume they are silicon
  measurements.
- Numbers: timing minitest doc, verbatim: "The prjxray timing handles most nets
  +/- 1.5% delay. The large exception to this is clock nets, which appear to use
  a table lookup that is not understood at this time." Baseline is Vivado timing
  on the minitest designs (small DFF loopback-style designs). This is the
  strongest open-source accuracy claim for 7-series, but it is on tiny designs,
  not on ibex/mmcm scale, and it excludes clock nets, which is exactly where
  Helion's mmcm error lives.
- How nextpnr-xilinx / others use it: nextpnr-xilinx chipdbs are generated by
  `bbaexport.py` against a prjxray-db checkout; timing in nextpnr-xilinx is
  widely described as rudimentary, and its own release notes show it is still
  patching gaps case by case (e.g. combinational DSP48E1 delay added as a
  conservative worst-case over SDF variants, "validated against Vivado 2026.1
  reference runs"). Source: https://github.com/openXC7/nextpnr-xilinx/releases/tag/0.9.2
  There is no published nextpnr-xilinx vs Vivado slack-accuracy number that could
  be verified; treat any claim of nextpnr timing accuracy as [UNVERIFIED]. The
  F4PGA arch-defs flow historically consumed prjxray-db timings for its VPR-style
  timing, but no live accuracy figure for that path was verified.
- Helion crate: `helion-xilinx7` (ingest `timings/*.sdf` as the cell-arc source;
  port the `timing.py` Elmore structure as the reference model to beat) +
  `helion-sta` (corners FAST/SLOW map to Helion's fast_min/slow_max).
- Expected gain: direct reuse of prjxray's SDF cell arcs plus its Elmore structure
  gives Helion a principled baseline that is already within about 1.5% on small
  data nets per prjxray's own minitest. The gain vs Helion's current timer is
  unmeasurable until Helion reproduces the minitest comparison itself; expect the
  largest gain on cell arcs (section 2.5), not on long routes.
- Effort: S (import SDF + reproduce minitest) to M (port Elmore model and validate
  per part). Risk: Medium. Risks: prjxray-db is stale (last database rebuild noted
  Dec 2021 in `Info.md`); SDF coverage is per tile type, not per part/speed grade;
  clock-net model is explicitly unknown; R/C provenance unverified.
- Next steps: (1) Vendor or script-import `prjxray-db/<family>/timings/*.sdf` for
  Helion's 8 parts; parse IOPATH/SETUP/HOLD 4-tuples into `helion-xilinx7` tables
  keyed by (tile type, BEL, in-pin, out-pin, corner). (2) Reproduce prjxray's
  timing minitest in Helion: run the same small designs, compare Helion net delay
  vs Vivado and vs prjxray's model, and confirm the +/-1.5% claim on data nets
  before trusting it. (3) Diff prjxray's Elmore predictions against Helion's 1.1M
  measured delays per wire class; wherever prjxray is systematically off, that is
  the fit target for section 2.1, not a reason to discard the structure.
  (4) Quarantine clock nets into a separate table/model from day one; do not let
  them pollute the data-net fit.

### 2.3 Timing fuzzers

- Best source: prjxray fuzzers directory + fuzzer 007-timing, and Liu/Kapre FPL'19
  as the actual timing-calibration method. Fuzzer list:
  https://foss-fpga-tools.googlesource.com/prjxray/+/HEAD/fuzzers
  Timing fuzzer dir:
  https://foss-fpga-tools.googlesource.com/prjxray/+/HEAD/fuzzers/007-timing
  contains `bel/`, `routing-bels/`, `.gitignore`, `Makefile`. What exactly `bel/`
  vs `routing-bels/` generate is [UNVERIFIED] beyond the directory names; their
  contents were not opened.
- Correction to the premise: most prjxray "fuzzers" are bitstream fuzzers, not
  delay fuzzers: they generate many small Vivado designs that toggle one feature
  at a time, build bitstreams, and diff them (`segmatch`) to learn which
  configuration bits control which PIP/BEL. That learns connectivity and bits,
  not delay. The only timing-specific fuzzer is 007-timing, and its output feeds
  the SDF/timing database described in section 2.2, which is derived from Vivado
  SDF/timing reports, not from silicon delay measurement. There is no PIP-timing
  silicon fuzzer in prjxray that could be found. Do not plan Helion work around a
  prjxray timing fuzzer that does not exist.
- Published work on measuring FPGA delays by fuzzing / on silicon: Liu and Kapre,
  FPL 2019 (section 2.1) is the closest published method: about 30 Vivado
  calibration routes per interconnect tile, least-squares solve for per-resource
  delays, 1% / 0.01 ns vs Vivado on XCKU115 across -3/-2/-1. This is
  Vivado-calibration, not silicon measurement, and it is the method Helion should
  copy for section 2.1. Silicon ring-oscillator (RO) measurement is a separate
  literature, not a prjxray fuzzer: Xilinx patent US 9,639,640, "Generation of
  delay values for a simulation model of circuit elements in a clock network",
  describes generating RO circuits that include selected clock-network elements in
  different PIP contexts, operating the programmed device, and deriving per-element
  delays from RO output, noting that 10-15 hand-made ROs covered only about 15%
  of PIP contexts on XCKU040 and the rest had to be estimated.
  https://patents.justia.com/patent/9639640 . Giechaskiel et al., FPL 2019,
  "Measuring Long Wire Leakage with Ring Oscillators in Cloud FPGAs", uses ROs to
  measure long-wire delay effects, but for side-channel/leakage purposes, not for
  building a timing model. No verified paper gives a complete RO-based per-PIP
  delay table for 7-series.
- Helion crate: `helion-bits` + `helion-xilinx7` if Helion ever builds RO
  calibration bitstreams; otherwise this section is really a `helion-xilinx7`
  calibration-script task (generate Vivado calibration designs, parse
  `report_timing` / SDF), not a fuzzer task.
- Expected gain: low as a standalone Helion investment. Vivado-derived calibration
  (Liu/Kapre) already gives about 1% without any silicon. Silicon RO would only
  help if Helion's goal were silicon truth rather than Vivado parity, and Helion's
  stated target is Vivado parity. RO adds board, temperature/voltage control, and
  per-die variation problems for no benefit against the stated target.
- Effort: L (Vivado calibration harness) to XL (silicon RO campaign with boards
  and environmental control). Risk: High for silicon RO; Low-Medium for Vivado
  calibration. Risks: RO frequencies conflate temperature, voltage, aging, and
  process variation with the PIP delay being measured; per-die results do not
  transfer to Vivado's slow_max corner; prjxray's bitstream fuzzers are slow and
  Vivado-version-sensitive (prjxray pinned Vivado 2017.2).
- Next steps: (1) Do not build a silicon RO fuzzer. Build the Vivado calibration
  harness instead: a script that generates the Liu/Kapre calibration routes for
  one INT tile per part, runs Vivado `report_timing`, and solves the least-squares
  system. (2) Read `fuzzers/007-timing/bel` and `routing-bels` sources directly
  before reusing anything from them; record what SDF each produces and for which
  tile types. (3) If silicon truth is ever needed (e.g. to arbitrate a
  Vivado-vs-silicon dispute), prototype a single RO that isolates one wire class
  on one board, with temperature logging, and check repeatability before scaling.
  (4) Keep bitstream fuzzing (connectivity/bits) and timing calibration (delays)
  as separate workstreams with separate owners; conflating them is how this area
  gets mis-scoped.

### 2.4 ML delay prediction

- Best sources: PreRoutGNN, Zhong et al., AAAI 2024, for the method,
  https://arxiv.org/abs/2403.00012v1 ; ASAP, Ouyang et al., MLCAD 2023, for the
  cleanest MAPE number,
  https://personal.hkust-gz.edu.cn/yuzhema/papers/C32-MLCAD2023-MTL-RTL.pdf ;
  Ustun et al., ICCAD 2020, D-SAGE,
  https://www.csl.cornell.edu/~zhiruz/pdfs/dsp-gnn-iccad2020.pdf ; LSTP, Zheng et
  al., ASPDAC 2024 slides, https://leozheng.tech/papers/C6-ASPDAC'24-LSTP-slides.pdf .
  Survey: Goswami and Bhatia, "Application of Machine Learning in FPGA EDA Tool
  Development", IEEE Access 2023, DOI 10.1109/ACCESS.2023.3322358.
- Numbers (with baseline): PreRoutGNN: slack prediction R2 of 0.93 on 21 real-world
  circuits, vs 0.59 for the previous SOTA (TimingGCN, Guo et al., DAC 2022).
  Baseline is prior GNNs (GCNII, GAT, GINE, TimingGCN). Caveats: this is
  pre-routing slack prediction for ASIC standard-cell circuits, not post-route
  FPGA net delay; it predicts slack/arrival time, not per-net delay MAPE, and no
  per-net MAPE is reported in the abstract. Its transferable ideas are
  topological-order message passing with residual local delay learning, and
  cell-LUT attention. Code was stated as forthcoming at
  https://github.com/Thinklab-SJTU/EDA-AI ; whether it is live is [UNVERIFIED].
  ASAP (MLCAD 2023): multi-task learning predicting post-synthesis delay and area
  from RTL + synthesis sequence; on 780,000 data points, test MAPE on unseen
  designs of 6%, "about 3x lower than existing studies". Baseline is prior
  synthesis predictors. Best clean MAPE in this set, but it is post-synthesis
  delay (not post-route net delay) and ASIC-oriented. Ustun et al. (ICCAD 2020),
  D-SAGE: GNN for FPGA HLS operation delay; slides report 72% improvement in HLS
  operation delay prediction vs HLS estimates, with RRSE improvements on
  microbenchmarks and realistic designs (e.g. realistic-design delay RRSE from
  about 2.19 HLS to about 0.82-0.83 HLS+D-SAGE in the slide table). Baseline is
  Vivado HLS delay estimates, not post-route Vivado STA. Pre-routing, HLS-level,
  not Helion's post-route problem. LSTP (ASPDAC 2024): logic-synthesis timing
  predictor (AIG + optimization sequence, GNN + Transformer); slides report
  average timing improvement of 27.44% over the r2 baseline across the listed
  designs (e.g. tinyRocket 86.70%, mem_ctrl 56.38%). Exact MAPE per design was
  not cleanly extractable from the slides read, so LSTP MAPE is [UNVERIFIED].
  Again pre-synthesis, not post-route.
- Is it practical for Helion? Yes, but only in the narrow post-route form, and
  only after section 2.1. Helion's advantage over every paper above is that it
  already has the right label: 1.1M post-route per-sink net delays with exact
  routes, on the exact device family. None of the papers above had that; they all
  predict pre-route or pre-synthesis quantities because post-route labels are
  expensive. The honest ML design for Helion is therefore not a GNN over the
  netlist, it is gradient-boosted regression on route features (section 2.1's
  feature list) with the linear/Elmore model as a baseline, plus optionally a
  small GNN that consumes the routed net as a graph (nodes = wire segments/PIPs,
  edges = adjacency) and predicts per-sink delay. A full PreRoutGNN-style model
  is overkill and mismatched: Helion has the route, so it does not need to
  predict it. Risks specific to ML here: training/serving cost inside a Rust
  toolchain (a GBM can be exported as tables; a GNN cannot ship cleanly in
  `helion-sta` without an inference runtime), silent degradation on unseen
  designs, and the fact that a 5% median / 15% p95 target is already achievable
  by the linear methods in section 2.1 if the features are right.
- Helion crate: experimentation outside the crates (Python, on `net_delays.tsv`);
  shipped model, if any, as static tables in `helion-xilinx7`, consumed by
  `helion-sta`. Do not put a Python/ML runtime dependency into `helion-sta`.
- Expected gain: incremental over section 2.1, not transformational: if the linear
  fit lands at e.g. 8% median / 25% p95, a GBM might plausibly bring it to about
  4-5% / 12-15%. If the linear fit already lands at 2%, ML adds nothing worth
  shipping. These are estimates, not verified numbers.
- Effort: M (GBM experiment) to XL (GNN + Rust inference + retraining pipeline).
  Risk: Medium (GBM) to High (GNN). Risks: held-out-design generalization
  failure; model drift as Helion's own placer/router changes routes; shipping and
  versioning a learned model in a deterministic Rust tool; temptation to use ML to
  paper over a cell-arc or clock-path bug (section 2.5).
- Next steps: (1) Freeze the section 2.1 linear/Elmore baseline and its
  held-out-design error first; ML only starts if that baseline misses 5%/15%.
  (2) Run a one-day GBM experiment (e.g. LightGBM/XGBoost) on route features vs
  the linear baseline, same held-out-design split; ship only if it beats the
  baseline by a clear margin on p95, not just median. (3) If a GNN is ever tried,
  copy PreRoutGNN's two ideas that transfer (topological-order updates, residual
  per-edge delay) and apply them to the routed net graph, not to the pre-route
  netlist. (4) Decide the shipping form up front (exported tables in
  `helion-xilinx7` vs no ML in the binary); do not train a model with no shipping
  plan.

### 2.5 Cell arc delay models

- Best sources: prjxray-db `timings/*.sdf` (the data) + Vivado `report_timing` /
  `write_sdf` (the calibration source) + UG906 (the slack arithmetic). SDF
  example: https://GitHub.com/f4pga/prjxray-db/blob/master/artix7/timings/CMT_TOP_R_LOWER_T.sdf
  prjxray DSP timing issue showing SDF IOPATH/SETUP/HOLD form:
  https://github.com/f4pga/prjxray/issues/1286
  UG906: http://docs.amd.com/r/2020.2-English/ug906-vivado-design-analysis/Timing-Path-Summary-Header-Information
  VPR arch-file form for comparison:
  https://docs.rs/guide/0.0.25/guide/fpga/fn.read_architecture.html
  RapidWright's per-pin cell constants:
  https://www.rapidwright.io/javadoc/com/xilinx/rapidwright/timing/TimingModel.html
  (fields `LOGIC_LUT_A1_DELAY`..`A6_DELAY`, `SITEPIN_A*_DELAY`, `LOGIC_FF_DELAY`,
  `CARRY_CO_DELAY`, `BOUNCE_DELAY`, `lookupIntraSiteDelay(siteType, fromBelPin,
  toBelPin)`).
- How Vivado/VPR model cell delays: Vivado is BEL-based, not slice-based: each
  BEL (LUT, FF, CARRY4, MUX, etc.) has its own arcs, e.g. `Prop_lut2_I0_O`,
  `Prop_fdce_c_q`, `Setup_fdce_C_D` (which is often negative for 7-series FFs
  because of clock-pin delay inside the BEL). Source: AMD support discussion of
  Vivado vs ISE timing models,
  https://adaptivesupport.amd.com/s/question/0D52E00006hpZ8jSAE/about-data-arrival-time?language=en_US
  The datasheet (DS181) numbers are ISE/slice-based and do not correspond to
  Vivado's BEL arcs; do not calibrate Helion's LUT/FF arcs from DS181. VPR: cell
  delay is a constant or a pin-to-pin matrix from the architecture file, plus
  setup/hold/clock-to-Q per sequential primitive; routing delay is separate Elmore
  RC. That is the right separation for Helion too: cell arcs are tables, net delay
  is the fitted model. RapidWright FPT'19: cell delays are lumped per-pin
  constants fitted from Vivado timing reports (e.g. separate LUT delays per input
  pin A1..A6, separate site-pin delays, one FF delay, one carry delay), plus an
  intra-site lookup `lookupIntraSiteDelay`. This is the practical template for
  7-series: per (BEL type, in-pin, out-pin) constants, not one LUT delay.
- SDF form: Vivado `write_sdf` and prjxray-db SDFs give per-instance or
  per-tile-type `(CELL (CELLTYPE ...) (INSTANCE ...) (DELAY (ABSOLUTE (IOPATH in
  out (min:typ:max) ...))) (TIMINGCHECK (SETUP ...) (HOLD ...)))`. prjxray's form
  is the 4-tuple `(fast_min::fast_max)(slow_min::slow_max)`. Helion's
  `slow_max_ps` target maps to the slow_max of the SDF tuple.
- How to calibrate from Vivado reports: (1) Generate Vivado `report_timing
  -max_paths` / `write_sdf` for a set of micro-designs that isolate one arc at a
  time (single LUT with each input pin driven, FF clock-to-Q, CARRY4 per-bit,
  BRAM clock-to-out, DSP register and combinational paths, MMCM/BUFG/BUFR clock
  arcs). Extract the per-arc delay from the path breakdown, not from the path
  total. (2) Cross-check against prjxray-db SDF for the same tile type; where they
  disagree, trust Vivado on Helion's own parts/speed grades and record the delta.
  (3) Fit per-arc tables keyed by (cell type, in-pin, out-pin, speed grade,
  corner), with separate setup/hold. Pay special attention to: negative FF setup
  times (sign error here flips slack by about 0.1-0.2 ns per endpoint), CARRY4 CO
  vs O vs CYINIT arcs, BRAM/DSP multi-cycle and registered vs combinational
  variants, and MMCM/BUFG insertion delay (the prime suspect for mmcm's 2.46 ns
  optimism). (4) Validate on full designs: decompose Vivado's critical-path report
  into logic vs route vs clock, and compare each component separately against
  Helion's, rather than comparing only total slack. If Helion's logic component is
  off by a constant per LUT level, it is a cell-arc error, not a net error.
- Helion crate: `helion-xilinx7` (arc tables, SDF import) + `helion-sta` (arc
  lookup, setup/hold, clock-path handling, CPR, uncertainty). This is the
  highest-leverage crate pair in the whole report.
- Expected gain: potentially the largest single gain: if mmcm's error is a missing
  MMCM/BUFG arc or a clock-uncertainty omission, fixing cell/clock arcs can close
  most of the 2.46 ns in one change, and it is also the only explanation on offer
  for ibex being pessimistic by 7 ns (e.g. double-counted uncertainty, wrong setup
  sign, or an over-large BRAM/DSP arc applied per level). Estimate; verify by path
  decomposition before claiming it.
- Effort: S (import prjxray SDF + audit clock/uncertainty/CPR) to M (full per-arc
  Vivado calibration across BELs and speed grades). Risk: Low-Medium. Risks: SDF
  coverage gaps for some BEL variants (DSP variants were unstable in prjxray issue
  1286, with A vs D input-pin flips); MMCM/PLL arcs are configuration-dependent
  (divide/multiply, phase); negative setup times are easy to get backwards.
- Next steps: (1) Audit `helion-sta` today for the four non-net terms:
  source/destination clock delay (min vs max per UG906), CPR, clock uncertainty
  (TSJ/DJ/PE/user), and FF setup sign. Report which are implemented, which are
  zero, and which are guessed. (2) Import prjxray-db SDF cell arcs into
  `helion-xilinx7` and diff them against Helion's current cell delays per BEL/pin;
  list every arc where they differ by more than 20 ps. (3) Run Vivado
  `report_timing` path decomposition on mmcm and ibex critical paths; attribute
  Helion's slack error to logic vs route vs clock vs uncertainty, and fix the
  largest component first. (4) Build micro-design calibration for CARRY4, BRAM,
  DSP, and MMCM/BUFG arcs; these four cover the named failing designs (bram,
  pico/ibex carry logic, mmcm clocking).

### 2.6 Speed-grade / PVT scaling

- Best sources: Liu and Kapre, FPL 2019 (calibration across -3/-2/-1 on one part)
  + AMD DS181 (per-speed-grade IO/hard-block tables) + prjxray FAST/SLOW corners.
  There is no published general fabric derating formula for 7-series. Liu/Kapre:
  http://nachiket.github.io/publications/rapidroute-timing_fpl-2019.pdf demonstrates
  the timing model on XCKU115 at -3, -2, and -1 speed grades, within 1% / 0.01 ns
  of Vivado in each grade, by re-running the about 30 calibration runs per grade.
  Implication: scaling is done by recalibration per grade, not by a universal
  factor. DS181, Artix-7 DC and AC Switching Characteristics, v1.27.1:
  https://docs.amd.com/api/khub/documents/iAkxxTOk96ANLJqYf2hgrQ/content gives
  per-speed-grade tables, but only for IO and hard blocks (e.g. IOB delays differ
  per grade); it does not publish LUT/routing fabric delays per grade. Useful as a
  sanity check on ratios, not as a fabric model. PVT derating in general
  (ASIC PrimeTime-style): derating is a multiplicative factor per cell delay, with
  separate early/late, clock/data, and voltage/temperature components, derived from
  SPICE/Monte Carlo; e.g. the DSLG/scaling-lib method interpolates libraries
  characterized at different voltage/temperature corners rather than applying one
  factor. Source (general STA background, not FPGA-specific):
  http://maaldaar.com/index.php/vlsi-cad-design-flow/static-timing-analysis-sta/pt-dslg-flow .
  No verified FPGA-specific derating factor table was found; any universal
  "7-series -2 is 0.83x -1" factor is [UNVERIFIED] as a published result.
- Numbers: Helion's own measured ratios (from the task, not independently
  verified): -2/-1 median critical-path ratio 0.826x, -3/-1 0.740x. These are
  Helion-internal measurements; treat them as the data to explain, not as
  published facts. DS181 IO example (old v1.5 excerpt returned by search, column
  mapping not fully verified, so quote with care): IOB combinatorial delay TODQ
  values 0.83 / 0.96 / 1.16 ns across speed grades in one table, and clock-to-out
  TOCKQ 0.47 / 0.49 / 0.56 ns. The ratios across grades for IO (about 0.85x from
  -1 to -2, about 0.72x from -1 to -3 on that row) are in the same ballpark as
  Helion's 0.826x / 0.740x, which is consistent with, but does not prove, a roughly
  uniform fabric scaling. Do not use DS181 IO ratios as fabric ratios without
  checking against Helion's 1.1M delays. Liu/Kapre: no single scaling factor is
  reported; the model is recalibrated per grade. That is the honest answer:
  per-grade recalibration beats a universal factor.
- Model to use: fit the -1 model fully (sections 2.1 and 2.5), then derive
  per-arc-type and per-wire-class scale factors from Helion's own data: for each
  (cell arc, wire class) compute median(slow_max at grade G) / median(slow_max at
  -1) across the 1.1M rows, per part. Ship a small table, not a single scalar.
  Check whether one scalar per grade is enough (if per-class factors cluster
  within a few percent, ship the scalar; if BRAM/DSP/clock arcs scale differently
  from LUT/routing, ship per-class factors). Also handle fast_min separately:
  fast/slow corners do not scale by the same factor, and hold analysis uses
  fast_min, so a setup-only scale factor will corrupt hold slack.
  Temperature/voltage within a grade (the PVT corner) is already covered by
  Vivado's slow_max/fast_min split in `net_delays.tsv`; Helion should model
  corners, not continuous PVT.
- Helion crate: `helion-device` (per-part, per-speed-grade tables) +
  `helion-xilinx7` (arc/wire tables indexed by grade) + `helion-sta` (corner
  selection: slow_max for setup, fast_min for hold).
- Expected gain: small in absolute slack terms if the -1 model is right (scaling
  is a about 17-26% correction on delay, so a 10% error in the scale factor is
  about 2% error in delay, about 0.1-0.2 ns on a 5-10 ns path). Large if it is
  wrong today: if Helion currently applies no grade scaling or one wrong scalar,
  this alone could explain part of the mmcm/bram gaps on -2/-3 parts. Estimate.
- Effort: S (derive per-class ratios from existing 1.1M rows; no new Vivado runs
  needed if grades are already in the data). Risk: Low. Risks: ratios derived from
  critical paths only may not match per-net ratios; fast_min scaling differs from
  slow_max; -1LI/-2L low-voltage grades do not follow the -1/-2/-3 trend (DS181
  notes -2L at 0.9V has different power/speed behaviour).
- Next steps: (1) From `net_delays.tsv`, compute per (part, grade, wire class,
  driver site) median slow_max and fast_min ratios vs -1; report spread, not just
  the median, and check whether a single scalar per grade is defensible.
  (2) Do the same for cell arcs from Vivado SDF/report_timing per grade; if cell
  and net scale factors differ by more than about 5%, ship separate tables.
  (3) Verify hold behaviour: confirm fast_min scaling is applied to hold checks
  and slow_max to setup, per UG906 corner rules. (4) Add a regression test: same
  design, same route, three speed grades, Helion slack ratios must match Vivado
  slack ratios within the 0.2 ns slack target.

### 2.7 Timing accuracy: priority summary

| Order | Method | Best source | Expected gain | Effort | Risk |
|---|---|---|---|---|---|
| 1 | Cell arcs + clock/CPR/uncertainty audit | prjxray-db SDF + UG906 + RapidWright per-pin constants | Largest; may explain both mmcm optimism and ibex pessimism | S-M | Low-Med |
| 2 | Fitted net delay (linear least squares per resource, then GBM) | RapidWright FPT'19 + Liu/Kapre FPL'19 | Per-net about 1-2% if it transfers; closes mmcm/bram/pico | M-L | Med |
| 3 | prjxray Elmore + SDF import as baseline | prjxray timing.py + timing minitest (+/-1.5% on small data nets) | Principled baseline; clock nets excluded | S-M | Med |
| 4 | Speed-grade per-class scaling tables | Liu/Kapre per-grade recalibration + Helion's own ratios | Small if -1 is right; material if scaling is missing/wrong | S | Low |
| 5 | ML (GBM on route features; GNN only if GBM wins) | PreRoutGNN (R2 0.93 slack, ASIC pre-route) + ASAP (6% MAPE, post-synthesis) | Incremental over (2) | M-XL | Med-High |
| 6 | Silicon RO timing fuzzing | US 9,639,640 (RO clock-network patent); no prjxray timing fuzzer exists | About none vs Vivado-parity goal | L-XL | High |

### 2.8 Cross-cutting validation protocol (applies to all of section 2)

1. Hold out entire designs, never random nets; report per-net median and p95 error
   and slack error on the five named designs plus held-out designs.
2. Decompose every slack error into logic / route / clock / uncertainty before
   attributing it to the net model.
3. Keep clock nets out of the data-net fit and out of the headline accuracy number;
   report them separately.
4. Re-run after any placer/router change; a delay model fitted to Vivado routes
   degrades silently when Helion's own routes change shape.

---

## 3. Fmax: analytic and timing-driven placement, routing, physical synthesis

Baseline: Helion placed/routed, Vivado-timed Fmax is 51-85% of Vivado's own Fmax
(PicoRV32 92.0 vs 128.8 MHz = 71%, Ibex 32.4 vs 38.9 MHz = 83%). The worst
PicoRV32 path is diagnostic: it is a high-fanout, long-wire, placement-plus-
replication problem, not a logic-depth problem alone. Targets: 85% then 95% of
Vivado Fmax; route time PicoRV32 under 60 s, Ibex under 600 s.

### 3.1 Analytic placement

#### 3.1.1 RePlAce (ASIC analytic placer: port ideas, not the code)

- Best source: "RePlAce: Advancing Solution Quality and Routability Validation in
  Global Placement", Cheng, Kahng, Kang, Wang, IEEE TCAD 38(9), 2019, pp.
  1717-1730. DOI: https://doi.org/10.1109/TCAD.2018.2859220 . OpenROAD `gpl`,
  based on RePlAce, describes Nesterov electrostatics, timing-driven mode,
  routability-driven RUDY inflation:
  https://openroad.readthedocs.io/en/latest/main/openroad/gpl/README.html
- Numbers: on NEWBLUE1, RePlAce with local Lagrangian multiplier: HPWL 5.60e7,
  762 iterations, 9.9 min vs ePlace 2.0: HPWL 5.71e7, 1078 iterations, 42 min
  (Kahng ECE 260C slides:
  https://abkcourses.github.io/ece260c-contents/sp26/lectures/lecture6-v3.pdf ).
  That is about 2% better HPWL at about 4.2x faster on that case, ASIC mixed-size
  benchmarks (ISPD 2005/2006, MMS, DAC 2012, ICCAD 2012), not FPGA, not Fmax.
- Helion crate: `helion-place` (global placement), with `helion-sta` for timing
  weights and `helion-xilinx7` for site/heterogeneity legality.
- Expected gain: projected, not reported: 3-7% Fmax if it replaces/augments
  simulated annealing with a good global placement plus legalization, mainly via
  shorter critical wires. Wirelength alone does not guarantee Fmax on 7-series,
  where clock-region, carry-chain, and SLICE packing constraints dominate.
- Effort: XL. Risk: High. RePlAce is ASIC standard-cell, continuous, no CLB/SLICE/
  BRAM/DSP/clock-region legality. Porting needs heterogeneous electrostatics,
  discrete legalization, and packing-aware density. Do not start here.
- Next steps: (1) Read OpenROAD `gpl` timing-driven and routability-driven modes
  and copy only the net-weighting and RUDY-inflation interfaces. (2) Prototype a
  quadratic/weighted-wirelength global placer on PicoRV32 first, legalize with the
  existing placer, measure routed wirelength and Vivado-timed Fmax, not HPWL
  alone. (3) Only if that beats annealing on Fmax, invest in full electrostatics.
  (4) Keep annealing as detailed placer regardless.

#### 3.1.2 DREAMPlace (GPU acceleration of the same family)

- Best source: "DREAMPlace: Deep Learning Toolkit-Enabled GPU Acceleration for
  Modern VLSI Placement", Lin et al., DAC 2019. Paper PDF as returned by search:
  https://github.com/freestriker/taskflow/raw/refs/heads/master/doxygen/references/lin_19_01.pdf
- Numbers: over 30x speedup in global placement without quality degradation vs
  multi-threaded RePlAce; 1M-cell design finishes in about 1 minute; near-linear
  scaling to 10M cells. Earlier GPU work cited in the paper: 15x speedup with
  under 1% quality degradation. ASIC, not FPGA, not Fmax.
- Helion crate: `helion-place` (future accelerator only).
- Expected gain: runtime, not QoR. Projected: placement time becomes negligible on
  large designs, enabling more seeds/iterations. No direct Fmax gain claimed.
- Effort: XL. Risk: High. Requires GPU/PyTorch or a Rust CUDA equivalent, and
  Helion's target sizes (PicoRV32, Ibex) are far below the scale where 30x
  matters. Route time targets are already in seconds to minutes.
- Next steps: (1) Defer. (2) Revisit only if Ibex-scale placement dominates runtime
  after timing-driven placement lands. (3) If revisited, use it as a research
  reference for WA-wirelength and density kernels, not as a dependency.

#### 3.1.3 elfPlace (best FPGA-specific analytic placer)

- Best source: "elfPlace: Electrostatics-Based Placement for Large-Scale
  Heterogeneous FPGAs", Meng, Li, Lin, Pan, IEEE TCAD 41(1), 2022, pp. 155-168.
  https://doi.org/10.1109/tcad.2021.3053191
- Numbers: on ISPD 2016 benchmark suite, elfPlace beats UTPlaceF by 13.5%,
  RippleFPGA by 10.2%, GPlace3.0 by 8.8%, UTPlaceF-DL by 7.0% in routed wirelength,
  with competitive runtime. Routed wirelength is Vivado-routed wirelength on
  Xilinx UltraScale VU095, not Fmax and not 7-series.
- Helion crate: `helion-place`, plus `helion-pack` for downstream clustering
  compatibility (elfPlace explicitly optimizes pin density and packing
  compatibility via instance area adjustment).
- Expected gain: projected 5-10% Fmax on wire-dominated designs if routed-wirelength
  gains transfer to 7-series, plus better routability. This is the strongest
  analytic-placement candidate for Helion because it is already
  heterogeneous-FPGA, flat nonlinear, with augmented Lagrangian and
  preconditioning.
- Effort: L to XL. Risk: Medium-High. No public Helion-ready Rust code was verified
  in this pass [UNVERIFIED whether elfPlace source is public]; the paper has no
  clock/timing model, and the authors note in later work that elfPlace cannot
  handle clock constraints (see Mai et al. 2023, https://arxiv.org/pdf/2303.09305 ).
  Helion needs clock-region legality for 7-series.
- Next steps: (1) Implement the multi-electrostatic-field idea in miniature:
  separate density fields per resource type (LUT/FF, BRAM, DSP) rather than one
  field. (2) Add clock-region fence as a hard constraint after global placement,
  not inside the optimizer initially. (3) Benchmark on ISPD 2016-style wirelength
  first, then on PicoRV32/Ibex Vivado-timed Fmax. (4) Compare directly against
  UTPlaceF-style quadratic placement before committing to full electrostatics.

#### 3.1.4 UTPlaceF / UTPlaceF 2.0 (best contest-proven FPGA placer to imitate)

- Best sources: "UTPlaceF: A Routability-Driven FPGA Placer with Physical and
  Congestion Aware Packing", Li, Dhar, Pan, ICCAD 2016.
  http://www.cerc.utexas.edu/utda/publications/C199.pdf
  and "UTPlaceF 2.0: A High-Performance Clock-Aware FPGA Placement Engine", ACM
  TODAES 2018. https://api.crossref.org/works/10.1145%2F3174849
- Numbers: UTPlaceF beats ISPD 2016 top-3 winners by 3.3%, 7.7%, and 28.3% in
  routed wirelength, with similar or shorter runtime, on Xilinx UltraScale VU095,
  routed by Vivado 2015.4. UTPlaceF 2.0 won ISPD 2017, beating 2nd and 3rd place by
  4.0% and 10.0% in routed wirelength with competitive runtime, on industry
  benchmarks. Method: routability-driven quadratic placement (POLAR 2.0 framework),
  congestion-aware detailed placement, physical/congestion-aware packing, plus in
  2.0 iterative min-cost-flow cell assignment and clock-aware packing.
- Helion crate: `helion-place` and `helion-pack` (packing is part of the win, not
  an afterthought).
- Expected gain: projected 5-10% Fmax. This is the most transferable design:
  quadratic placement is far simpler to implement correctly in Rust than
  electrostatics, and packing-aware placement directly addresses Helion's
  SLICE/carry problems.
- Effort: L. Risk: Medium. Quadratic placement plus legalization is well
  understood; the risk is congestion estimation and clock legality, not the core
  solver.
- Next steps: (1) Implement UTPlaceF-style flow: flat quadratic global placement,
  then congestion-aware detailed moves, then packing-aware legalization.
  (2) Add RUDY-style congestion map and inflate cells in hot spots before detailed
  placement. (3) Add clock-region check as a legalization constraint (UTPlaceF 2.0
  lesson). (4) A/B against current placer on PicoRV32/Ibex with Vivado timing as
  the only QoR gate.

#### 3.1.5 OpenPARF (best open-source FPGA P&R framework to study and borrow from)

- Best source: "OpenPARF: An Open-Source Placement and Routing Framework for
  Large-Scale Heterogeneous FPGAs with Deep Learning Toolkit", Mai et al.,
  arXiv 2306.16665, 2023. https://arxiv.org/abs/2306.16665v1 (code at
  https://github.com/PKU-IDEA/OpenPARF as printed in the paper; whether it is
  currently public and buildable was not verified in this pass [UNVERIFIED]).
- Numbers: on ISPD 2016 and ISPD 2017 FPGA contest benchmarks plus industrial
  benchmarks, 0.4-12.7% improvement in routed wirelength and more than 2x speedup
  in placement vs other state-of-the-art academic P&R engines. Includes asymmetric
  multi-electrostatic fields, SLICEL/SLICEM heterogeneity, routability, clock
  feasibility, and a two-stage router with fine-grained CLB-level routing.
- Helion crate: `helion-place` and `helion-route` (reference architecture, not a
  Rust dependency).
- Expected gain: projected 5-10% Fmax if its heterogeneity and clock handling are
  ported; mainly a correctness/legality reference.
- Effort: M to study, L to XL to port. Risk: Medium. PyTorch/GPU framework, not
  Rust; porting the model, not the code, is the work.
- Next steps: (1) Read OpenPARF's handling of SLICEL vs SLICEM and clock
  feasibility and map each rule to a 7-series SLICE/BRAM/DSP rule in
  `helion-xilinx7`. (2) Use its ISPD 2016/2017 benchmark harness as a regression
  target for Helion placement. (3) Do not adopt PyTorch as a Helion dependency.

### 3.2 Timing-driven placement

#### 3.2.1 VPR timing-driven annealing (the baseline every FPGA placer is measured against)

- Best sources: "Architecture and CAD for Deep-Submicron FPGAs" (Betz, Rose,
  Marquardt, 1999) and VTR 8 (Murray et al., ACM TRETS 2020).
  https://m.aarongraham.ca/vtr8_trets.pdf and https://dl.acm.org/doi/10.1145/3388617
  Algorithm summary: https://algorithm.tamnd.com/fpga-timing-driven-annealing-placement/
- Numbers: VTR 8 vs VTR 7: 15% lower minimum routable channel width, 41% lower
  wirelength, 12% lower critical path delay, 5.3x faster runtime, 3.3x lower
  memory. VTR 8 changelog (https://docs.verilogtorouting.org/en/latest/_sources/CHANGELOG.md.txt )
  gives a narrower range for the same generation: 32-42% wirelength reduction,
  7-10% critical path delay reduction, 5.5-6.3x faster. VPR placement cost is
  lambda-weighted: delta cost = lambda * delta timing / T_prev + (1-lambda) *
  delta bounding-box / BB_prev, where timing cost sums connection delay *
  criticality^CE, criticality = 1 - slack / Dmax. That formula is the single most
  important thing to copy.
- Helion crate: `helion-place` (cost function, annealing schedule) and `helion-sta`
  (slack/criticality must come from real STA, recomputed per temperature, not once
  at start).
- Expected gain: projected 10-20% Fmax on PicoRV32/Ibex, the largest single
  placement gain available. This directly attacks the 4.79 ns + 3.19 ns wire
  segments on the worst path. Wirelength-only placement cannot see that those two
  nets are critical.
- Effort: S to M if Helion already has annealing and STA; M if STA/criticality
  plumbing is missing. Risk: Low-Medium. Well understood, incremental, reversible
  via lambda.
- Next steps: (1) Wire `helion-sta` slack into placement: compute criticality per
  connection, update every temperature or every N moves. (2) Start lambda at 0.5,
  criticality exponent 1-2, sweep lambda and exponent on PicoRV32 only, gate on
  Vivado-timed Fmax. (3) Add slack redistribution / timing-driven move bias for
  high-fanout critical nets (see 3.2.2). (4) Log worst-path wirelength before/after,
  not just HPWL.

#### 3.2.2 PATH / Kong path-counting net weighting

- Best source: "A Novel Net Weighting Algorithm for Timing-Driven Placement",
  Kong, ICCAD 2002.
  https://cecs.uci.edu/~papers/compendium94-03/papers/2002/iccad02/pdffiles/03b_1.pdf
- Numbers: vs the weighting used in state-of-the-art FPGA placement package VPR
  at the time, longest path delay reduction up to 38.8%, 15.6% on average, with no
  runtime overhead and only 4.1% increase in total wirelength. Method: accurate
  all-path counting, not just critical-path or slack thresholding. Context: Cong
  ICCAD 2003 tutorial
  (https://pdfs.semanticscholar.org/e04c/98ca86582bbdc5c33a78d5126c8220e9e583.pdf )
  shows VPR and PATH were 35% on average and 54% worst-case away from optimal on
  TPEKO timing benchmarks, so there is headroom, but Kong's 15.6% is vs VPR, not
  vs optimal.
- Helion crate: `helion-place` (net weights feeding analytic or annealing
  placement) and `helion-sta` (path counting needs arrival/required times).
- Expected gain: projected 5-15% Fmax when combined with 3.2.1, especially on
  designs with many near-critical paths like Ibex. Alone, less.
- Effort: M. Risk: Medium. Path counting can oscillate if weights are updated too
  aggressively; needs damping and a cap on weight ratio.
- Next steps: (1) Implement slack-based weights first (1 - slack/Dmax), ship it,
  measure. (2) Then implement Kong path-counting as an alternative weight and A/B
  it on the same designs. (3) Damp weight updates (e.g. exponential moving average
  across iterations) and cap max/min weight ratio. (4) Do not use path counting as
  the only timing signal; keep STA criticality as the router's signal too.

#### 3.2.3 Timing-driven analytical placement for FPGA (quadratic + dynamic weights)

- Best source: "Timing Driven Analytical Placement for FPGA", Agashiwala, Univ.
  of Minnesota thesis, 2015, advisor Bazargan.
  https://fileserver-az.core.ac.uk/download/pdf/76357816.pdf
- Numbers: vs VPR 7.0 on the 20 largest MCNC circuits: about 30% faster runtime
  on average, comparable or better critical path delay, but 8.7% worse overall
  wirelength after placement. Method: quadratic programming, dynamic net weights
  from timing criticality, iterative partitioning to center of gravity, spiral
  legalization, then low-temperature SA in VPR.
- Helion crate: `helion-place`.
- Expected gain: projected 0-5% Fmax alone; its value is as a fast global-placement
  initializer for annealing, not as a replacement.
- Effort: M. Risk: Low-Medium.
- Next steps: (1) Use as the design for a two-stage placer: quadratic global +
  annealing detail. (2) Keep the low-temperature SA step; do not ship
  quadratic-only placement. (3) Measure wirelength regression explicitly; if
  wirelength worsens more than 5%, the Fmax gain is not worth it.

### 3.3 Routing

#### 3.3.1 PathFinder (the router Helion should already be)

- Best source: "PathFinder: A Negotiation-Based Performance-Driven Router for
  FPGAs", McMurchie and Ebeling, FPGA 1995.
  https://cgi.cse.unsw.edu.au/~cs4601/refs/papers/fpga95mcmurchie.pdf
  (also https://www.esa.informatik.tu-darmstadt.de/archive/twiki/pub/Lectures/AlgorithmenImChipEntwurf07De/router-pathfinder.pdf )
- Numbers: on Triptych, average only 4.5% increase in critical path delay over the
  optimum delay for a placement; on Xilinx 3000, greater completion rate than
  commercial tools and 11% faster implementations. Cost per resource: (base +
  history) * present, where present grows with sharing; timing cost per connection:
  A_ij * delay + (1 - A_ij) * congestion cost, A_ij = D_ij / Dmax (slack ratio).
  Timing analysis every iteration.
- Helion crate: `helion-route` (core algorithm), `helion-sta` (A_ij),
  `helion-xilinx7` (routing resource graph, delays).
- Expected gain: projected 5-15% Fmax if Helion's router is not yet fully
  timing-driven PathFinder with per-iteration STA; 0% if it already is, in which
  case the gap is placement or delay-model accuracy. This is the highest-leverage
  routing check: verify the router actually uses criticality, not just congestion.
- Effort: M if reworking cost function, L if rebuilding router. Risk: Medium.
  History/present factor tuning is fiddly; too aggressive timing weight causes
  congestion failures.
- Next steps: (1) Audit current router: does it recompute A_ij from STA every
  iteration, and does it use per-connection criticality or per-net criticality?
  (2) Implement per-connection cost, not per-net. (3) Sweep present/history growth
  rates on PicoRV32/Ibex and gate on both routability and Vivado Fmax.
  (4) Add a congestion-only fallback if timing-driven routing fails to converge.

#### 3.3.2 nextpnr router2 vs router1

- Best sources: nextpnr itself: Yosys+nextpnr, Shah et al., FCCM 2019,
  https://arxiv.org/abs/1903.10407 . Measured router1 vs router2:
  https://hal.science/hal-04917041v1/file/EWDTS%20-%20Autonomous%20FPGA%20Reconfigurability%20in%20Embedded%20Systems.pdf
  (EWDTS 2024). No peer-reviewed router2 paper was found in this pass.
- Numbers: on a laptop, nextpnr router1 231.0 s / 543 MB vs router2 58.4 s / 728 MB,
  and both produced rather similar performance: 122 MHz for router1 (router2 Fmax
  not separately quoted in the extracted text, described as similar). On a Zybo
  board, router1 3720 s vs router2 800 s. That is about 4x faster, not a QoR claim.
  The router2 design (heuristic, A*-based, timing-driven, rip-up) is described in
  nextpnr docs and talks, but a systematic router2 vs router1 vs Vivado Fmax table
  was not found [UNVERIFIED].
- Helion crate: `helion-route` (read router2 as a design reference: A* with
  lookahead, incremental rip-up, timing-driven ordering).
- Expected gain: projected runtime 2-4x if Helion adopts router2-style heuristics
  (bounded A*, early exit, incremental reroute); Fmax gain projected 0-5%, mainly
  from better timing-driven ordering, not from the heuristic itself.
- Effort: M to L. Risk: Medium. Router2 trades memory for speed (728 vs 543 MB in
  the EWDTS case); Helion's Ibex under 600 s target needs speed, but not at QoR
  cost.
- Next steps: (1) Profile Helion router: where is time spent, A* expansions,
  rip-up iterations, or STA? (2) Port only the proven router2 ideas: A* lookahead,
  rip-up only congested connections (see 3.3.3), and critical-net-first ordering.
  (3) Do not copy router2 wholesale; its 7-series delay model and architecture
  are different from Helion's. (4) Publish a router1-style vs new-router table on
  PicoRV32/Ibex before claiming a win.

#### 3.3.3 CRoute / connection-based routing (best rip-up/reroute improvement)

- Best source: "A Connection-Based Router for FPGAs", Vansteenkiste, Bruneel,
  Stroobandt, FPT 2013. Record:
  https://biblio.ugent.be/publication/4254446.mods_vabb and Zhou thesis, Ghent
  2022, CRoute chapter (congested nets are partially ripped up and rerouted, only
  congested connections from different nets sharing resources):
  https://bibliotest.ugent.be/publication/8771056/file/8771094.pdf
- Numbers: vs VPR directed-search router on VTR benchmarks on a modern commercial
  architecture, 4.4% faster for relaxed routing and 84.3% faster for hard
  instances; given the same time as VPR directed search, 5.8% fewer tracks per
  channel. Related engineering enhancement in Gort/Anderson: rip up only signals
  using a congested node (skip uncongested) gives 2.38x speedup, plus LE-output
  propagation 1.38x, total 3.31x, at 2% worse critical path delay and 2% worse
  wirelength (https://janders.eecg.utoronto.ca/pdfs/marcel_altera.pdf ).
- Helion crate: `helion-route`.
- Expected gain: projected runtime 2-3x on congested designs, Fmax neutral to
  slightly negative (-2% if done naively). Use for route-time targets, not Fmax.
- Effort: M. Risk: Low-Medium. Connection-level rip-up is a contained change if
  the router already tracks per-connection usage.
- Next steps: (1) Change rip-up granularity from net to connection (source-sink
  pair). (2) Keep a full-net rip-up fallback for the final iterations to recover
  the 2% delay loss. (3) Measure Ibex route time and Fmax before/after; if Fmax
  drops, keep connection rip-up only for early iterations. (4) Combine with
  PathFinder history costs, not as a replacement.

#### 3.3.4 RWRoute (best open-source timing-driven router for commercial Xilinx parts)

- Best source: "RWRoute: An Open-source Timing-driven Router for Commercial
  FPGAs", Zhou et al., ACM TRETS 2022 (also FPGA 2021). Record:
  https://api.crossref.org/works/10.1145%2F3491236
- Numbers: timing model within 2% average accuracy of Vivado for UltraScale+; vs
  Vivado, RWRoute gives 4.9x compile-time improvement at the expense of 10% QoR
  loss, on 665 synthetic and six real designs. That is the honest open-source
  router baseline: 90% of Vivado QoR at about 5x speed. The FPL 2026 contest notes
  (https://xilinx.github.io/fpl26_optimization_contest/details.html ) add:
  RWRoute is optimized for runtime rather than QoR, Vivado route_design will
  produce superior timing closure, RWRoute has no hold-time fixing, and
  RapidWright's timing model error vs Vivado is about 2%.
- Helion crate: `helion-route` (reference) and `helion-sta` (Helion needs a
  Vivado-accurate lumped delay model within a few percent, as RWRoute did, or its
  timing-driven router will optimize the wrong delays).
- Expected gain: projected 5-10% Fmax if Helion's delay model is currently the
  bottleneck; 0% if the model is already accurate. The 10% QoR loss figure is a
  useful calibration: Helion at 71-83% of Vivado is worse than RWRoute's 90%, so
  Helion's gap is not just the router.
- Effort: M to audit and fix delay model, L to rebuild router on RWRoute lines.
  Risk: Medium. Delay-model work is unglamorous and easy to get wrong on 7-series
  (different wire types, PIP delays, clock skew).
- Next steps: (1) Validate Helion STA/routing delays against Vivado timing reports
  on PicoRV32/Ibex, per net type and wire length, and publish the error table.
  (2) Fix the largest delay-model errors first, before tuning router costs.
  (3) Add hold-time fixing as a post-route step (RWRoute lacks it; Helion should
  not). (4) Use RWRoute's feature list as a checklist: constant nets, clock nets,
  and global-clock handling.

#### 3.3.5 Parallel routing (best speed lever, not a QoR lever)

- Best sources: "Accelerating FPGA Routing Through Parallelization and Engineering
  Enhancements", Gort and Anderson, IEEE TCAD 31(1), 2012.
  https://janders.eecg.utoronto.ca/pdfs/marcel_journal_revision.pdf
  And "An Open-Source Fast Parallel Routing Approach for Commercial FPGAs", Zang
  et al., GLSVLSI 2024. https://arxiv.org/html/2407.00009v1
- Numbers: Gort/Anderson: heuristic optimizations alone over 3x speedup vs
  sequential baseline; parallel implementation about 2.3x speedup on 4 cores,
  deterministic/repeatable; combined over 7x speedup vs VPR router, with no
  significant impact on circuit speed or wirelength. Zang et al.: on FPGA24 routing
  contest public benchmarks, 2x speedup vs serial RWRoute, and vs Vivado
  (v2021.1, route_design -no_timing_driven -preserve) 2x faster and 31% better
  critical-path wirelength. Caveat: that Vivado comparison is non-timing-driven,
  runtime-first mode, UltraScale+ xcvu3p, not a timing-closed Vivado run; do not
  quote 31% as a Vivado-beating Fmax claim. Code stated in paper:
  https://github.com/xszang/parallel-routing
- Helion crate: `helion-route` (parallelism) and `helion-place` (geographic
  partitioning helps).
- Expected gain: projected route time 2-3x on multi-core, Fmax neutral. This is
  how Ibex meets under 600 s if serial routing is the bottleneck, but it will not
  close the 17-29% Fmax gap.
- Effort: L. Risk: Medium-High. Determinism is the hard part; nondeterministic
  parallel routing makes debugging and regression testing painful. Gort/Anderson's
  deterministic MPI design is the model to follow.
- Next steps: (1) Defer until serial router is timing-correct; parallelizing a
  wrong router just produces wrong results faster. (2) Then implement geographic
  partitioning: route nets in disjoint regions in parallel, merge, then serial
  PathFinder polish on critical/congested nets only. (3) Require deterministic
  output (fixed partition, fixed ordering) as a release gate. (4) Measure speedup
  on Ibex, not PicoRV32, where parallelism matters.

### 3.4 Physical synthesis

#### 3.4.1 Fanout replication (highest priority for Helion's worst path)

- Best sources: "Timing Optimization of FPGA Placements by Logic Replication"
  (DAC 2003, UCI compendium)
  https://www.cecs.uci.edu/~papers/compendium94-03/papers/2003/dac03/pdffiles/13_1.pdf
  (up to 29% critical-path delay improvement, preliminary) and FAPO: "Fanout-Aware
  Post-Mapping Optimization for LUT-Based FPGAs", arXiv 2025.
  https://arxiv.org/html/2610.06937v1 (note: the arXiv identifier date looks
  anomalous; treat the FAPO numbers below as [UNVERIFIED - needs a full read
  before citing]).
- Numbers: DAC 2003 placement-coupled replication: up to 29% improvement in
  critical path delay (preliminary). Follow-up (Li et al., DAC 2004,
  http://www.cs.york.ac.uk/rts/docs/SIGDA-Compendium-1994-2004/papers/2004/dac04/pdffiles/p711.pdf ):
  up to 36% clock-period reduction vs timing-driven VPR placement. Improved version
  (ISPD 2006, https://dl.acm.org/doi/10.1145/1127908.1127959 as returned):
  average 17.4% (up to 39.9%) delay reduction vs VPR timing-driven placement,
  average 9.3% (up to 37.2%) vs basic fanin-tree embedder. FAPO (modern, VPR 9.0,
  EPFL benchmarks): 7.04% reduction in routed critical-path delay vs ABC `if`, and
  up to 14.48% reduction in fanout-aware area-timing product with 5% area
  allowance. Vivado's own phys_opt does the same thing: replication based on
  fanout/timing, plus BRAM/DSP/SRL register optimization, retiming, and
  re-placement (Xilinx deck:
  https://www.xilinx.com/publications/prod_mktg/club_vivado/presentation-2015/paris/Xilinx-TimingClosure.pdf ).
- Helion crate: `helion-pack` / `helion-ir` for the replication transform
  (duplicate LUT/FF, partition sinks), `helion-place` for placing the replica near
  its sink cluster, `helion-sta` for selecting candidates (only critical/high-fanout
  nets).
- Expected gain: projected 10-20% Fmax on PicoRV32, the single biggest
  physical-synthesis win. The worst path has fanout 85 then 32; replicating
  `cpu_state_1` and its LUT, with one copy driving the critical `cpuregs` CE
  cluster and the other driving non-critical sinks, directly shortens those 4.79 ns
  and 3.19 ns wires. FAPO's all-critical-path condition matters: replicating only
  one of several tied critical paths leaves Fmax unchanged.
- Effort: M. Risk: Medium. Area grows, upstream loading can worsen, and replicas
  must be placed, not just created. Cap replication by area and by fanout threshold.
- Next steps: (1) Implement post-place replication only: select nets with fanout
  above threshold AND negative slack within a percentage of WNS, replicate driver,
  partition sinks by placement centroid (k-means or bounding-box split), place
  replica at sink-cluster centroid, re-run STA. (2) Start with FF replication
  (control signals like CE/reset) and LUT replication on the critical path only.
  (3) Enforce FAPO's condition: replicate across all tied critical paths in one
  pass, or do not expect Fmax to move. (4) Gate on Vivado Fmax and LUT count; stop
  if LUTs grow more than 5% for under 2% Fmax.

#### 3.4.2 Retiming (Leiserson/Saxe, FPGA-specific)

- Best sources: "Retiming Synchronous Circuitry", Leiserson and Saxe, Algorithmica
  6, 1991. FPGA integration: "Integrated Retiming and Placement for FPGAs", Singh
  and Brown, FPGA 2002.
  https://www.cecs.uci.edu/~papers/compendium94-03/papers/2002/fpga02/pdffiles/3_2.pdf
  and Intel Agilex retiming slide:
  https://www.hc32.hotchips.org/assets/program/conference/day2/HotChips2020_FPGA_Intel_Agilex.pdf
- Numbers: Singh/Brown: integration of retiming with placement gives 19% better
  clock periods vs retiming before place-and-route. Intel Agilex (HotChips 2020):
  retiming-related optimizations 25% average Fmax increase, physical-synthesis
  optimizations 8% average Fmax increase (vendor slide, Agilex, not 7-series, not
  independently verified here). Leiserson/Saxe itself is theory (minimize clock
  period via register moves, Bellman-Ford / LP formulation); it reports no FPGA
  Fmax number.
- Helion crate: `helion-ir` (retiming transform on the netlist graph, must preserve
  sequential equivalence and initial state) and `helion-place` (post-place retiming
  needs incremental placement of new FFs) and `helion-sta` (delay model for
  retiming decisions).
- Expected gain: projected 5-15% Fmax on pipelined/CPU designs like Ibex/PicoRV32
  if retiming is done post-placement with accurate wire delays; 0-5% if done
  pre-placement with logic-only delays, because FPGA delay is interconnect-dominated.
  Do not do logic-only retiming and expect Fmax.
- Effort: L to XL. Risk: High. Sequential equivalence, reset/initial-state handling,
  and BRAM/DSP register legality are easy to get wrong. A wrong retimer silently
  changes behavior. This is the riskiest item in this report.
- Next steps: (1) Defer full retiming until replication and timing-driven placement
  land. (2) First implement a safe subset: forward/backward retiming only across
  LUTs (not across BRAM/DSP/IO), with formal equivalence check via simulation on
  the existing test suite plus `helion-sim` value checks. (3) Implement post-place
  retiming only, using routed/estimated wire delays, with incremental placement of
  moved FFs. (4) Require a retiming reversibility check (retime back to original,
  as Intel describes) before shipping.

#### 3.4.3 Vivado phys_opt style moves (what to copy, in order)

- Best sources: Vivado phys_opt_design (replication, retiming, critical-path
  restructuring, re-placement, post-route phys_opt). Xilinx documentation/decks.
  https://www.xilinx.com/publications/prod_mktg/club_vivado/presentation-2015/paris/Xilinx-TimingClosure.pdf
  and https://www.xilinx.com/publications/events/developer-forum/2018-frankfurt/timing-closure-tips-and-tricks.pdf
- Numbers: no isolated, controlled Fmax percentage for phys_opt alone was found in
  these decks [UNVERIFIED as a single number]. The Xilinx deck gives a worked
  example: WNS -0.247 ns (best placement) -> -0.060 ns after two AggressiveExplore
  phys_opt passes -> -0.029 ns after AggressiveFanoutOpt -> 0.003 ns after routing,
  with TNS -289.95 -> -1.971 -> -0.243 -> 0.000 and failing endpoints 3498 -> 182 ->
  31 -> 0. That is a timing-closure example, not a clean A/B. The decks state:
  phys_opt is not part of default strategies, must be chosen explicitly, replication
  based on fanout/timing, BRAM/DSP/SRL register optimization, retiming, moving cells
  after each optimization, re-entrant (can run multiple times). PSIP (physical
  synthesis in placer, 2018.2+) and PSIR (in router, 2018.1+, UltraScale+) are the
  in-placement/in-routing versions. Vivado 2025.2 marketing claims 8-13% Fmax from
  module-level retiming on Versal/UltraScale+ [UNVERIFIED, vendor marketing, not a
  paper].
- Helion crate: all of `helion-place`, `helion-route`, `helion-pack`, `helion-sta`
  (phys_opt is a loop around them, not a separate crate).
- Expected gain: projected 10-20% Fmax combined if Helion implements the loop
  (place -> phys_opt -> route -> post-route phys_opt), with replication doing most
  of it.
- Effort: L (as a loop) after section 3.4.1 lands. Risk: Medium. Each phys_opt pass
  must be validated by STA and by functional simulation; a pass that improves WNS
  but breaks hold or legality is a regression.
- Next steps: (1) Build a `phys_opt` loop, not a single pass: after placement, run
  replication, re-place replicas, re-run STA, repeat until WNS stops improving or
  area cap hits. (2) Add a post-route phys_opt pass (replicate + reroute critical
  nets only). (3) Log per-pass WNS/TNS/failing endpoints exactly as Xilinx does, so
  Helion's loop is comparable. (4) Make every pass re-entrant and idempotent:
  running it twice without changes must not change the design.

### 3.5 ISPD and ICCAD contest results

#### 3.5.1 ISPD 2016 Routability-Driven FPGA Placement (Xilinx VU095)

- Best source: http://www.ispd.cc/contests/16 (verified open).
- Winners: 1st UTPlacer (UT Austin, Wuxi Li, Shounak Dhar, David Pan), 2nd
  RippleFPGA (CUHK), 3rd GPlace (Guelph), 4th UFRGSPlace and VDAplacer (tied).
  19 entrants, 12 submitted, 5 finalists. Benchmarks generated by Xilinx Gnl tool,
  primitives restricted to LUT/FF/DSP/BRAM, target XCVU095 (67,200 CLBs, 880 IO,
  770 DSP, 1730 BRAM). Scoring by Vivado routed wirelength and routability.
- Winning method (UTPlaceF, see 3.1.4): routability-driven quadratic placement,
  congestion-aware detailed placement, physical/congestion-aware packing. Lesson for
  Helion: packing and congestion handling won, not just global placement quality.
  Later papers (GPlace3.0, elfPlace, OpenPARF) beat the contest winners by 7-13%
  in routed wirelength on the same suite, so the contest is a floor, not a ceiling.
- Helion crate: `helion-place`, `helion-pack`.
- Next steps: (1) Download the ISPD 2016 benchmarks from the contest page and add
  them as a Helion placement regression (routed wirelength, not just HPWL).
  (2) Reproduce UTPlaceF's 3.3%/7.7%/28.3% margins as a sanity check on Helion's
  placer. (3) Do not chase the contest's wirelength metric alone; add a timing
  metric on top.

#### 3.5.2 ISPD 2017 Clock-Aware FPGA Placement (Xilinx VU095)

- Best source: https://ispd.cc/slides/2017/s8_3.pdf (contest slides) and
  https://api.crossref.org/works/10.1145%2F3174849 (UTPlaceF 2.0).
- Winners: 1st UTPlaceF 2.0 (UT Austin), top 5 also GPlace 2.0, NTUfplace,
  RippleFPGA, VDAplacer. 13 teams registered. Largest design: 1.0M instances, 57
  clocks. Target again VU095.
- Winning method: iterative min-cost-flow cell assignment plus clock-aware packing,
  optimizing wirelength and routability while honoring clock constraints. Beat
  2nd/3rd by 4.0%/10.0% in routed wirelength. Later work (Mai et al., DAC 2022 /
  arXiv 2303.09305, https://arxiv.org/pdf/2303.09305 ) beats UTPlaceF 2.0 by 14.2%
  in routed wirelength on the same suite with multi-electrostatics plus clock
  feasibility.
- Helion crate: `helion-place`, `helion-xilinx7` (clock regions, BUFG/BUFH,
  clock-capable pins).
- Lesson for Helion: clock legality is not optional at scale. Helion's 7-series
  designs have fewer clocks than VU095, but a placer that ignores clock regions
  will produce unroutable or slow placements on larger designs. Implement
  clock-aware legalization now, even if PicoRV32 has one clock.
- Next steps: (1) Add clock-region and clock-net constraints to placement
  legalization. (2) Test on a multi-clock design, not just PicoRV32. (3) Use ISPD
  2017 benchmarks as a clock-legality regression.

#### 3.5.3 ICCAD CAD Contests (what applies, what does not)

- Best source: https://www.iccad-contest.org/2025/History.html (past contests index).
- Relevant contests: ICCAD 2012 Problem C (design hierarchy aware
  routability-driven placement), ICCAD 2014/2015 incremental timing-driven
  placement, ICCAD 2019 Problem B (system-level FPGA routing with timing division
  multiplexing) and Problem C (LEF/DEF based open-source global routing), ICCAD
  2020/2021 routing with cell movement. The 2019 FPGA routing contest was
  multi-FPGA TDM routing, not single-FPGA timing closure, so its winning methods do
  not transfer directly to Helion. The 2019 global-routing contest (won by CUHK,
  CUGR) is ASIC global routing (LEF/DEF, Dr.CU detailed routing), not FPGA. Do not
  cite ICCAD 2019 as an FPGA placement/routing win for Helion.
- More relevant recent contests: FPGA 2024 Runtime-First FPGA Interchange Routing
  Contest (see 3.3.5 and 3.6), MLCAD 2023 FPGA Macro Placement Contest (OpenPARF
  3.0 reports 27.8%/6.9%/13.3%/49.1% improvements vs Vivado and MLCAD 2023 winners
  on macro placement,
  https://magic3007.github.io/data/publications/PLACE_ISEDA2024_Mai/PLACE_ISEDA2024_Mai.pdf
  - macro placement, not full placement, use with care), and FPL 2026 Agentic FPGA
  Backend Optimization Contest (see 3.6).
- Helion crate: `helion-place`, `helion-route`.
- Next steps: (1) Track FPGA 2024 / FPL 2026 contests, not ICCAD ASIC routing
  contests, for Helion-relevant methods. (2) Enter Helion's router in the FPGA24
  benchmark harness as a self-test, even if not submitting. (3) For macro placement
  (BRAM/DSP columns), read OpenPARF 3.0.

### 3.6 What nextpnr-xilinx and VPR measured against Vivado

#### 3.6.1 nextpnr-xilinx vs Vivado (the only same-design, same-device PicoRV32 comparison found)

- Best sources: https://github.com/openXC7/nextpnr-xilinx (archived repo;
  development continues at openXC7/nextpnr, per GitHub org page
  https://github.com/openXC7 ) and
  https://www.hdlfactory.com/post/2026/10/08/openxc7-vs-vivado-build-times/
  (verified open, Oct 8, 2026, independent benchmark).
- Numbers (hdlfactory, Artix-7 200T, 100 MHz, Vivado 2025.2, Yosys +
  nextpnr-xilinx + Project X-Ray, 20-vCPU GCE VM, median of 3 runs): PicoRV32
  1 core: openxc7 147 MHz vs Vivado about 165 MHz (89% of Vivado Fmax);
  16 cores: 131 MHz vs about 144 MHz (91%). LUTs: openxc7 1,671 vs Vivado 890
  (1.88x); 16 cores: 26,668 vs 14,216 (1.88x). Registers equal (553 vs 555).
  P&R time: 1 core 56 s open vs Vivado P&R part of 221.6 s cold; 16 cores 483 s
  open vs Vivado faster overall. Hold: openxc7 leaves 1 violation (1 core) and 7
  violations (16 cores); Vivado meets hold. Author caveat: speeds come from
  different timing models, so they compare only roughly; hold is the clearer
  difference (Vivado adds delay to short paths, nextpnr-xilinx does not).
- Interpretation for Helion: Helion's PicoRV32 at 71% of Vivado (92.0 vs 128.8 MHz)
  is worse than nextpnr-xilinx's 89% on a similar PicoRV32, but the designs,
  devices, and synthesis (Yosys LUT count 1,671 vs Helion's unknown LUT count) are
  not identical, so this is a directional benchmark, not a controlled A/B. The LUT
  gap (1.88x) is synthesis/mapping, not placement/routing; Helion's Fmax gap is
  more likely placement/routing/physical synthesis. No published nextpnr-xilinx vs
  Vivado Fmax table on Ibex was found [UNVERIFIED].
- Helion crate: `helion-place`, `helion-route`, `helion-sta` (hold fixing is a gap
  to close).
- Next steps: (1) Reproduce the hdlfactory PicoRV32 comparison with Helion's own
  PicoRV32, same device, same Vivado version, and publish Helion vs nextpnr-xilinx
  vs Vivado in one table (Fmax, LUTs, route time, hold violations). (2) Add
  hold-time fixing to the router/post-route step; it is a correctness gap, not just
  QoR. (3) Do not claim Helion beats nextpnr-xilinx until that table exists.

#### 3.6.2 VPR / VTR vs Vivado (the academic-vs-commercial gap)

- Best sources:
  https://www.researchgate.net/publication/303471197_Analyzing_the_Divide_between_FPGA_Academic_and_Commercial_Results
  (Khalid et al., FPL 2016) and https://antmicro.github.io/perf-tool-history/
  (fpga-tool-perf history site, VPR vs Vivado vs nextpnr-xilinx across many projects).
- Numbers: Khalid et al.: 2.2x gap in speed-performance between academic (VTR) and
  commercial (Vivado, UltraScale) for similar process technology; area-efficiency
  divide 5%, runtime divide 2.2x. Depth reduction (the usual logic-optimization
  metric) does not translate to post-routing timing improvements. fpga-tool-perf
  wiki example
  (https://github-wiki-see.page/m/ryancj14/LinkToTheNow/wiki/fpga-tool-perf ,
  mirror of SymbiFlow wiki): LiteX-Linux, VPR sys_clk 56.270 MHz (requested 60 MHz,
  not met) vs Vivado (clock table incomplete in the mirror); runtimes VPR total
  1243 s vs Vivado 319 s, VPR placement 113 s vs Vivado 28 s, VPR routing 502 s vs
  Vivado 29 s. That is a single old example, not a controlled benchmark; treat it
  as illustrative, not as a VPR-vs-Vivado Fmax number.
- Helion implication: the 2.2x academic gap is the historical ceiling; Helion at
  1.18-1.96x slower than Vivado (51-85% Fmax) is already inside that gap, but
  nextpnr-xilinx at 1.10x and RWRoute at 1.11x QoR loss (90% of Vivado) show 90%+
  is achievable by open tools. Helion's target of 85% then 95% is ambitious but not
  unprecedented.
- Next steps: (1) Use fpga-tool-perf as a harness idea, not as a number source: run
  Helion, VPR, nextpnr-xilinx, and Vivado on the same PicoRV32/Ibex netlists and
  record Fmax, LUTs, route time in one table. (2) Do not cite the 2.2x gap as
  Helion's gap; measure Helion's own gap per design. (3) Read Khalid et al. for why
  depth reduction does not help: it is a warning against logic-only optimizations.

#### 3.6.3 FPL 2026 Agentic FPGA Backend Optimization Contest (newest Vivado-beating evidence, with caveats)

- Best sources: https://xilinx.github.io/fpl26_optimization_contest/details.html
  and https://xilinx.github.io/fpl26_optimization_contest/assets/2026-09-09-FPL26-Contest-Results.pdf
- Numbers: target UltraScale+ xcvu3p, placed-and-routed DCP in, improved Fmax out,
  Vivado report_timing as scorer. Best of 198 Vivado runs (sweeping clock
  uncertainty, clock constraint, place/route directives) vs best contestant: e.g.
  AMD Mini-ISP initial 288.0 MHz, best Vivado 342.1 MHz, best contestant 351.6 MHz;
  FINN RadioML 284.9 -> 415.3 -> 463.8 MHz; FIR Symmetric 405.7 -> 407.8 -> 402.4 MHz
  (contestant worse); Rosetta Digit-Rec 69.3 -> 74.5 -> 92.5 MHz. Contestants beat
  the best Vivado sweep on several designs, using RapidWright fanout_opt, Vivado
  phys_opt, retiming, re-placement, and route/phys_opt. This is UltraScale+, not
  7-series, and it is post-route optimization on a Vivado-placed design, not a
  from-scratch flow.
- Helion crate: `helion-place`, `helion-route`, `helion-pack` (post-route phys_opt
  loop).
- Lesson: the winning moves are exactly the ones in section 3.4 (fanout
  replication, retiming, re-placement), applied post-route with Vivado as the
  verifier. Helion should build the same loop, with its own STA as the verifier
  and Vivado as the external gate.
- Next steps: (1) Replicate the contest's scoring (Fmax = 1000 / (T - WNS),
  hold/pulse-width must pass, fully routed) as Helion's own post-route gate.
  (2) Implement the contest's example optimization first: split high-fanout nets
  by replicating source FFs, place replicas at load centroid, reroute. (3) Do not
  claim Helion can beat Vivado until it beats Vivado on a from-scratch flow, not
  just on a Vivado-placed DCP.

### 3.7 Fmax: priority order (85% then 95%)

1. Timing-driven placement (3.2.1) + slack-based net weights (3.2.2, simple
   version): S/M, Low-Medium risk, projected 10-20% Fmax. Do first.
2. Fanout replication, post-place, critical nets only (3.4.1): M, Medium risk,
   projected 10-20% Fmax on PicoRV32. Do second, in parallel with 1 if possible;
   it directly fixes the fanout-85 path.
3. PathFinder audit: per-connection criticality, per-iteration STA (3.3.1) +
   delay-model validation vs Vivado (3.3.4): M, Medium risk, projected 5-15% Fmax
   if the router is not yet timing-driven, 0% if it is. Do third; measure before
   rebuilding.
4. Connection-based rip-up (3.3.3) for route time, plus hold fixing (3.6.1): M,
   Low-Medium risk, projected 2-3x route speedup, Fmax neutral. Do for Ibex under
   600 s.
5. UTPlaceF-style quadratic global placement + congestion-aware detailed placement
   (3.1.4): L, Medium risk, projected 5-10% Fmax. Do after 1-3, as a placer
   replacement, not before.
6. Post-route phys_opt loop (3.4.3): L after 3.4.1, Medium risk, projected 5-10%
   additional Fmax.
7. Retiming (3.4.2): L/XL, High risk, projected 5-15% but only post-placement.
   Defer until 1-6 are measured; require equivalence checking.
8. Analytic electrostatics (3.1.3/3.1.5), DREAMPlace GPU (3.1.2), parallel routing
   (3.3.5): XL / defer. Do not start until the serial flow is timing-correct and
   measured.

---

## 4. Device coverage: status and fuzzing methods of the bitstream projects

Licensing frame for the whole area (from the project, not re-verified here):
Vivado is licensed on xc7z020, small 7-series parts, xcku3p, xcku5p; NOT licensed
on xcvu3p, xcvu9p, xcvu095, xcku040, xczu9eg, xc7k325t, xc7vx485t, xc7z045,
xc7s25. That makes sections 4.1, 4.2 and 4.8 the core of this section: the only
legal fuzzing Helion can do itself is on licensed parts; everything else must be
carry-over, already-published DBs, or RapidWright device data.

### 4.1 Project X-Ray (prjxray): 7-series

- Best sources: repo https://github.com/f4pga/prjxray (931 stars, 3,949 commits,
  current commit c9f02d85, ISC, created 2017-12-16, not archived; F4PGA org listing
  says updated Jun 5, 2025). Database https://github.com/f4pga/prjxray-db
  (82 stars, 258 commits, CC0-1.0, last commit Dec 14, 2021; Info.md: "Last updated
  on Tue Dec 14 07:31:38 PM UTC 2021"). This is the frozen upstream DB Helion
  already uses. HTML view: https://symbiflow.github.io/prjxray-db . Docs:
  https://prjxray.readthedocs.io
- Status / coverage: DB has four family directories: `artix7`, `kintex7`,
  `spartan7`, `zynq7`. No upstream-published per-part segbits completeness
  percentage exists [UNVERIFIED for any % figure]. The best concrete part list
  verified is from BYU BFAT, which says it supports "most Xilinx 7-Series FPGAs
  documented in the Project X-Ray database": Artix-7 xc7a35t, xc7a50t, xc7a100t,
  xc7a200t; Kintex-7 xc7k70t; Spartan-7 xc7s50; Zynq-7000 xc7z010, xc7z020
  (https://awesome.ecosyste.ms/projects/github.com%2Fbyuccl%2Fbfat ). The README's
  own "Current Focus" is still the Artix-7 50T ("common between all footprints of
  the 15T, 35T and 50T varieties") plus "started experimenting with Kintex-7" -
  stale text, do not treat as current coverage. Known gaps from the open-flow side:
  transceivers and PCIe not supported in the open flow, per
  https://www.fpgaguide.com/topics/open-source-toolchains/ (Nov 13, 2026).
- How the fuzzers work: black-box differential fuzzing. Vivado generates a large
  number of small designs, each toggling one feature; bitstreams are
  cross-correlated to discover what each bit does. Output is a segbits DB: each
  FASM feature maps to a set of (segment address, bit offset, value) entries. The
  flow is: `make db-prepare-artix7` (static part info), run fuzzers in `fuzzers/`
  in order (`make -j$(nproc)`), then `fasm2frames` (FASM to frames) +
  `xc7frames2bit` (frames to .bit). README states fuzzers "only work on Vivado
  2017.2" (issue #14); that is almost certainly stale for the openXC7 fork, but it
  is what upstream still says.
- Helion crate: `helion-xilinx7` (consumer of the DB), `helion-bits` (FASM/frames/
  bit emission, the Rust equivalent of fasm2frames/xc7frames2bit).
- Expected gain: high for 7-series completeness; it is the only route to a
  license-free 7-series bitstream. No gain for UltraScale+.
- Effort: S for consuming the frozen DB (already done); M to run one new fuzzer on
  a licensed part; L-XL to document a new part family from scratch.
- Risk: Medium. DB is frozen since Dec 2021, upstream is quiet, and the
  Vivado-2017.2 fuzzer requirement means Helion's own fuzzing harness must be
  validated against a modern Vivado before trusting new rows. A single wrong
  segbits row is a silent miscompile, so every new row needs board verification
  (see the t27.ai BUFR case in section 4.8).
- Next steps: (1) Pin and vendor the exact prjxray-db commit Helion uses, and
  separately evaluate the openXC7 fork (https://github.com/openXC7/prjxray-db ,
  updated Nov 17, 2026) which carries newer parts. (2) Audit Helion's 7-series
  part list against the DB directories and record, per part, which tile types have
  segbits and which are missing (especially xc7k325t and xc7s25, both unlicensed,
  so only the fork DB can supply them). (3) Build one minimal fuzzer run on
  licensed xc7z020 (e.g. a single CLB/INT tile type) with modern Vivado and diff
  the output against the frozen DB to establish whether the 2017.2 requirement is
  real. (4) Adopt the openXC7 flow as the reference consumer: Yosys `synth_xilinx`
  -> nextpnr-xilinx -> FASM -> `fasm2frames` -> `xc7frames2bit`
  (https://github.com/openXC7/getting-started/blob/HEAD/README.md ).

### 4.2 Project U-Ray / prjuray: UltraScale / UltraScale+

- Best sources: repo https://github.com/f4pga/prjuray (91 stars, only 28 commits,
  Apache-2.0, created 2020-07-15; F4PGA org listing says updated Feb 9, 2022).
  Database https://github.com/f4pga/prjuray-db (4 stars, only 5 commits, CC0-1.0,
  root contains only a `zynqusp` directory). Effectively empty.
- Status / coverage: stalled / abandoned upstream. The README's "Target Parts" are
  all TBD except Zynq UltraScale+ ZU3EG (boards: Ultra96-V2, Genesys ZU). Kintex
  UltraScale+ is "TBD - Targetting XCKU11P?" and Virtex UltraScale+ "TBD -
  Targetting XCVU13P?" - i.e. no documented tile types, no segbits, no usable DB.
  Its stated goal was WebPack parts, which notably includes the two parts Helion
  CAN license: XCKU3P and XCKU5P, plus XCZU2EG-XCZU7EG/CG/EV. It "takes a lot of
  the learning from Project X-Ray and Project Trellis" but never got past
  scaffolding.
- Helion crate: `helion-ultrascale` (primary), `helion-device` (tilegrid /
  tile-type model), `helion-bits` (frame emitter).
- Expected gain: low as a source, high as a warning: there is no upstream
  UltraScale+ DB to consume. Helion's own fuzzing is the project, not a shortcut.
  The repo's only reusable assets are its directory layout, minitests/fuzzers
  scaffolding, and the WebPack-parts targeting rationale.
- Effort: XL to finish what prjuray started (that is Helion's UltraScale+
  bitstream lane itself).
- Risk: High if anyone assumes prjuray-db contains data; it does not. Low risk if
  used only as scaffolding.
- Next steps: (1) Do not depend on prjuray-db; treat it as an empty placeholder
  and record that decision so no later worker re-discovers it as a source.
  (2) Copy prjuray's fuzzer/minitests skeleton as the starting point for Helion's
  own UltraScale+ fuzzers, but only on licensed xcku3p/xcku5p. (3) Use the public
  frame format in UG570 as the fixed skeleton (frame = 3,936 bits = 123 32-bit
  words; FAR fields block type / row / column / minor - verified at
  https://docs.amd.com/r/en-US/ug570-ultrascale-configuration/Frame-Address-Register-00001 )
  and fuzz only the per-tile segbits inside it. (4) Prioritise the tile types
  prjuray never reached and Helion needs first: INT, CLE, BRAM, DSP, URAM, CLK,
  IO, INT_INTF - the same set the project already treats as carry-over candidates.

### 4.3 prjtrellis: Lattice ECP5

- Best sources: repo https://github.com/YosysHQ/prjtrellis (476 stars, 1,297
  commits, main branch, ISC, created 2018-03-04). DB is a submodule
  (`prjtrellis-db`). Docs: https://prjtrellis.readthedocs.io
- Status / coverage: mature. README: "Project Trellis supports all ECP5 devices."
  Working in the Yosys/nextpnr flow: logic slices incl. carries, distributed RAM,
  all internal interconnect, basic IO incl. tristate, LPF/DDR IO, BRAM (inference
  or DP16KD), multipliers by manual MULT18X18D instantiation (inference and
  advanced DSP not yet supported), global networks, PLLs, transceivers (DCUs).
  Tested boards: ECP5-5G Versa, ECP5 Evaluation Board, ULX3S. Fuzzers need Lattice
  Diamond 3.10 only; end users do not need Diamond.
- How it works: same black-box method as X-Ray, with Diamond generating the
  bitstreams; `libtrellis` (C++ with pybind11 Python bindings) manipulates
  bitstreams/tiles/DB; nextpnr-ecp5 consumes it and `ecppack` emits the bitstream.
  Full DB rebuild "takes several hours, even on a decent workstation."
- Helion crate: `helion-ecp5` already exists in this repo (see the crate list in
  the header); plugs into `helion-device` (device model) and `helion-bits`
  (packer) if Helion adds ECP5. Out of the Xilinx core scope.
- Expected gain: medium as a reference implementation (the cleanest mature example
  of the X-Ray method applied to a second vendor, including a Rust-free but
  well-factored lib + Python fuzzers); low direct gain for Xilinx QoR.
- Effort: S to study; M to port its fuzzer harness pattern to UltraScale+; L to
  add a real ECP5 target.
- Risk: Low as a reference; the main risk is scope creep (adding a whole Lattice
  target while UltraScale+ is unfinished).
- Next steps: (1) Read `fuzzers/ECP5/001-plc2_routing/fuzzer.py` as the template
  for a per-tile-type fuzzer with job parallelism (`TRELLIS_JOBS`). (2) Copy its DB
  layout discipline (metadata dir for initial tile knowledge, empty-DB creation,
  then fuzzers in order) into Helion's UltraScale+ DB design. (3) Note its
  DSP-inference gap as a caution: documenting the bits is not the same as
  inferring the primitive; Helion needs both. (4) Defer any ECP5 target until
  UltraScale+ bitstream work lands.

### 4.4 Apicula: Lattice Gowin

- Best sources: repo https://github.com/YosysHQ/apicula (717 stars, 1,113 commits,
  MIT, created 2019-11-22). PyPI `apycula` 0.35 released Oct 20, 2026
  (https://pypi.org/project/Apycula/0.35/ ) - actively maintained, unlike
  prjxray/prjuray.
- Status / coverage: supported boards per README: GW1N-1 (Tang Nano), GW1NZ-1K,
  GW1NSR-4K, GW1NR-9 (TEC0117, Tang Nano 9K), GW2AR-18 / GW2A-18 (Tang Nano 20K,
  Primer 20K), GW5A-25, GW5AST-138K and GW5AT-60K (initial / very simple designs
  only, footnotes 2-3). Flow: Yosys `synth_gowin` -> nextpnr-himbaechel ->
  `gowin_pack` -> openFPGALoader; `gowin_unpack` reverses a bitstream to Verilog.
- How it works: hybrid, and this is the distinctive method: a combination of
  fuzzing AND parsing of the vendor data files (`.dat`, `.fse`, `.ini`, `.pwr`,
  `.tm`). `tiled_fuzzer.py` uses vendor files to find bits in a specific tile
  type; the `.fse` parser is described as "fairly robust and complete." Fuzzers
  need Gowin EDA 1.9.10.03.
- Helion crate: `helion-device` / `helion-bits` if a Gowin target is ever added; no
  existing crate. Reference value only for Xilinx work.
- Expected gain: medium as a method reference. The vendor-file-parsing hybrid is
  the most relevant idea in this whole section for the unlicensed-parts problem
  (section 4.8): where a vendor ships machine-readable device data, parse it first
  and fuzz only what it does not explain.
- Effort: S to study; L-XL to add a Gowin target.
- Risk: Low as a reference. Its GW5A support is explicitly early, so do not cite
  it as mature.
- Next steps: (1) Study the `.fse` parser and `chipdb.py` (vendor data + fuzzing
  data merged into one ChipDB) as the model for merging RapidWright device data
  with Helion's own fuzzed segbits. (2) Study `tiled_fuzzer.py` for tile-type-scoped
  fuzzing, which is exactly the granularity Helion needs for carry-over.
  (3) Note the `gowin_unpack` round-trip (bitstream back to Verilog) as the
  pattern for Helion's bitstream verification tool. (4) No Gowin target work until
  Xilinx coverage is done.

### 4.5 Mistral: Intel/Altera Cyclone V

- Best sources: repo https://github.com/Ravenslofty/mistral (141 stars, 161
  commits, BSD-3-Clause, created 2019-06-30). Docs: https://og.kervella.org/docs-html/
  (linked from the repo's docs). Shadowgraph lists it updated Nov 5, 2026.
- Status / coverage: early stage, explicitly "very, very raw." Seven dies
  documented with CRAM/PRAM sizes in the docs: e50f (55x46), gx25f (49x40), gt75f
  (69x62), gt150f (90x82), gt300f (122x116), sx50f, sx120f. Bitstream is three RAMs:
  option RAM (32x40-bit blocks, 12 used), peripheral RAM, configuration RAM.
  Inner blocks: LAB (20x 5-input LUTs, 40 FFs), MLAB (64x20 RAM), DSP, M10K;
  peripheral blocks incl. GPIO. README gaps: LAB common clock/enable/clear routing
  "very unsure"; MLABs in ROM/RAM mode unknown; DSPs and M10Ks "really unknown,
  neither the mux usage nor the ports mapping"; most peripheral blocks have ports
  mapped but mux usage not understood. nextpnr lists it as experimental
  (`nextpnr-mistral`); a DE10-Nano (5CSEBA6) blinky is the demonstrated design.
- Helion crate: none; Intel is outside Helion's stated families. Reference only.
- Expected gain: low direct gain; medium as a cautionary example of a
  one-maintainer project that documented the frame/CRAM structure but stalled on
  hard-IP muxes - the same failure mode Helion must avoid on UltraScale+ DSP/URAM.
- Effort: S to study; XL to bring to production, not recommended.
- Risk: High if used as a dependency (single maintainer, raw tooling, 2 GB
  uncompressed library at compression level 0 per its own table).
- Next steps: (1) Read its bitstream-structure docs (option/peripheral/configuration
  RAM split) as an example of documenting the container before the fabric.
  (2) Take its README gap list as a template for Helion's own per-block honesty log
  (known / unsure / unknown per block type). (3) Do not adopt Mistral or an Intel
  target; record that decision. (4) If Helion ever needs a second-vendor sanity
  check on its fuzzer harness, Mistral's Quartus-based flow is the cheapest
  comparison point.

### 4.6 Versal (ACAP) bitstreams: public prior art

- Finding: no public bitstream-format reverse-engineering project exists. Multiple
  searches returned only vendor documentation and Vivado/Vitis-based application
  work. Mark this whole section [UNVERIFIED] for any claim of a public Versal
  fabric-bitstream DB: none was found.
- What does exist (verified):
  - AMD docs: Versal ACAP System Software Developers Guide UG1304
    (https://china.xilinx.com/content/dam/xilinx/support/documents/sw_manuals/xilinx2022_2/ug1304-versal-acap-ssdg.pdf )
    and XilFPGA library UG643
    (https://docs.amd.com/r/2022.1-English/oslib_rm/Versal-ACAP-XilFPGA-Library ) -
    these describe loading a PDI, not the fabric format.
  - CHARM / CHARM 2.0 (FPGA'23, TRETS 2024, https://arxiv.org/pdf/2301.02359 ) -
    Versal ACAP accelerators on VCK190 using Vivado/Vitis; uses the vendor flow,
    no RE.
  - Xillybus CDO manipulation
    (https://www.xillybus.com/tutorials/versal-apac-cdo-configuration-registers ) -
    the closest public work to "touching" Versal configuration: Versal loads a PDI,
    not a .bit; bring-up is processor-driven via CDO files (register-write commands)
    consumed by bootgen; the page edits `cpm_data.cdo` to work around a Vivado
    PCIe bug. This documents the CDO/register layer, not PL fabric configuration bits.
  - Ablation-Tool/ablation (https://github.com/Ablation-Tool/ablation ) lists
    "Versal PDI/CDO" among its parsers - [UNVERIFIED]: what it actually parses, its
    maturity, or whether it touches fabric bits was not verified; do not rely on it.
- Why it is hard (inference from the verified sources, not a verified fact):
  Versal has no classic .bit; the image is a PDI executed by the platform loader
  (PLM) from CDO commands, with authentication/encryption as the default posture. A
  prjxray-style fuzz-then-diff loop would need a Vivado license, a Versal part, and
  a way to observe fabric state through the PLM - a different and larger project
  than 7-series or UltraScale+ fuzzing.
- Helion crate: none. No Versal crate should be created on the basis of public
  prior art, because there is none.
- Expected gain / effort / risk: gain unknown; effort XL; risk very high. Do not start.
- Next steps: (1) Record "no public prior art for Versal fabric bitstream format"
  as a finding, with the search date, so it is not re-researched blindly.
  (2) If Versal is ever scoped, the first step is a literature/patent sweep on
  PDI/CDO structure and PLM, not fuzzing. (3) Keep Versal out of the
  device-coverage roadmap until UltraScale+ bitstream generation is proven on
  silicon. (4) Re-check in 6 months; the field is moving (AI-assisted RE tooling
  is appearing, e.g. the Cyclone IV Claude Code work in section 4.8).

### 4.7 Other relevant bitstream work

#### 4.7.1 Project IceStorm (iCE40): the reference implementation

- Best source: https://github.com/YosysHQ/icestorm (1,196 stars, 859 commits, ISC,
  created 2015-07-18). Docs: https://prjicestorm.readthedocs.io/en/latest/overview.html
- Coverage: "Pretty confident that we have the 1K and 8K devices completely
  documented" (can generate correct Verilog models for all iCEcube2 bitstreams for
  HX1K-TQ144 and HX8K-CT256 via `icebox_vlog`). LP/HX 1K/4K/8K plus UltraPlus (DSP,
  oscillators, RGB, SPRAM); LP384 added 2017; LM, Ultra, UltraLite NOT supported.
  Tools: `icepack`, `iceunpack`, `icebox`, `icefuzz`, `icetime`, `icepll`,
  `icebram`, `icemulti`.
- Helion crate: `helion-device` / `helion-bits` for the existing iCE40 target
  (`helion-ice40` exists); primarily a reference for harness design.
- Gain / effort / risk: gain high as a template (smallest complete RE project; its
  `icefuzz` + `icebox_vlog` round-trip is the verification pattern Helion should
  copy). Effort S to study. Risk low.
- Next steps: (1) Copy the `icebox_vlog` idea: a bitstream-to-Verilog model as an
  independent check on Helion's emitter. (2) Copy its per-part table discipline
  (part/package/IOs/nextpnr opts) for Helion's device tables. (3) Do not expand
  iCE40 coverage (LM/Ultra/UltraLite) before UltraScale+ lands.

#### 4.7.2 openXC7: the live 7-series flow (successor to F4PGA for xc7)

- Best source: https://github.com/openXC7/getting-started/blob/HEAD/README.md ;
  org https://github.com/openXC7 . The old `openXC7/nextpnr-xilinx` is archived;
  development continues at `openXC7/nextpnr` (himbaechel xilinx uarch), per the
  archived repo's own banner.
- Coverage: Spartan-7, Artix-7, Kintex-7, Zynq-7. Toolchain-installer notes add
  Kintex-7 70T, 160T, 325T, 420T, 480T, and a separate `database-virtex7` repo
  exists in the org. Flow: Yosys `synth_xilinx` -> nextpnr-xilinx (himbaechel) ->
  `fasm2frames` -> `xc7frames2bit` -> openFPGALoader. This is the most actively
  maintained consumer of prjxray/prjxray-db and the best place to watch for new
  segbits (its prjxray fork was updated Nov 21, 2026; its prjxray-db fork Nov 17,
  2026).
- Helion crate: `helion-xilinx7`, `helion-bits`.
- Gain / effort / risk: gain high (a second, live implementation of the same DB to
  diff Helion's emitter against, plus new-part DBs). Effort S-M to set up a diff
  harness. Risk low-medium (openXC7 itself notes MMCM lock, IDDR, DSP48/LUTRAM
  packing gaps).
- Next steps: (1) Build one openXC7 blinky for xc7a35t and diff its FASM/frames/bit
  against Helion's output for the same netlist. (2) Check the openXC7 prjxray-db
  fork for xc7k325t and xc7s50 segbits Helion lacks. (3) Track the `fpga-assembler`
  Rust rewrite (t27.ai, https://t27.ai/blog/the-back-half-of-openxc7-from-a-spec/ )
  - it found a silent bit-wrap in upstream `fasm2frames` (f4pga/prjxray#2574) and a
  missing part check in `xc7frames2bit` (#2573); check whether Helion's emitter
  shares either bug.

#### 4.7.3 RapidWright: the non-fuzzing route to device data (critical for UltraScale+)

- Best source: https://github.com/Xilinx/RapidWright (repo URL appears in BYU docs
  at https://byu-cpe.github.io/ComputingBootcamp/tutorials/rapidwright/ and in the
  RapidWright docs at https://www.rapidwright.io/docs/_sources/Introduction.rst.txt ).
  Docs: https://www.rapidwright.io/docs/_sources/Bitstream_Manipulation.rst.txt
- Coverage: Java framework with "fast loading accurate device model views for all
  Vivado-supported AMD devices (Series 7, UltraScale, UltraScale+, and Versal)".
  Reads/writes unencrypted Vivado DCP files. Its nextpnr-xilinx UltraScale+ flow
  exports a chip DB from RapidWright (`rapidwright_bbaexport.jar
  xczu7ev-ffvc1156-2-e`, `xczu2cg-sbva484-1-e` in the archived repo's README) and
  supports xcup BELs including DSP48E2, URAM288E, PLLE4/MMCME4, OSERDESE3/ISERDESE3.
- Hard limit, stated in its own docs: "RapidWright cannot generate bitstreams on
  its own. It is necessary to create bitstreams using Vivado. RapidWright does not
  contain the information needed to translate a placed and routed design into a
  bitstream." Its bitstream APIs only parse bitstreams, update INIT values
  (FF/LUTRAM/BRAM), do coarse placed-circuit-to-bitstream correlation, and relocate
  partial bitstreams.
- Helion crate: `helion-ultrascale` (device model import), `helion-device`
  (tile/wire/site data).
- Gain / effort / risk: gain very high: this is the most plausible source of
  tile/wire/site geometry for UNLICENSED UltraScale+ parts, because the device
  model ships in the RapidWright distribution rather than being generated per-run
  by a licensed Vivado. That last inference is [UNVERIFIED] - it was verified that
  RapidWright claims device models for all Vivado-supported devices, but it was
  not verified that its shipped data includes xcvu3p/xczu9eg/xcku040 or that
  reading it requires no license. Effort M (Java interop or a one-off exporter to
  Helion's format). Risk medium: RapidWright is AMD Research code, not a bitstream
  DB; its model is a DCP-level view, not segbits, so it gives geometry and
  connectivity, not frame bits.
- Next steps: (1) Verify, by inspection, whether a RapidWright install contains
  device data for xcvu3p, xczu9eg and xcku040 and whether loading it needs a
  Vivado license. (2) Export one licensed part (xcku3p) via `rapidwright_bbaexport`
  and diff its tile/wire/site list against Helion's current UltraScale+ model.
  (3) Use RapidWright only for geometry/connectivity; never as a bitstream emitter.
  (4) Keep the BYU BFASST work (below) as the consumer pattern for DCP-level
  equivalence.

#### 4.7.4 Project Oxide / prjoxide (Lattice Nexus): the Rust precedent

- Best source: https://github.com/gatecat/prjoxide (155 stars, 465 commits, ISC,
  Rust (`libprjoxide`, `prjoxide` CLI). Docs HTML via gatecat.github.io (linked
  from README).
- Coverage: LIFCL-40 most tested, LIFCL-17 limited, CertusPro-NX early. Fuzzed:
  PLC logic tile config + interconnect, basic IO + IOLOGIC, EBR, global routing,
  DSP, PLL. Remaining: finishing IO, PCIe, SGMII fuzzing. Fuzzers need Lattice
  Radiant. nextpnr-nexus consumes it; LiteX supports CrossLink-NX via Oxide.
- Helion crate: `helion-device` / `helion-bits` if Nexus is ever added; main value
  is that it is the only major RE project written in Rust, so its crate layout is
  directly imitable.
- Gain / effort / risk: gain medium (Rust RE code to copy, including pyo3 Python
  bindings for fuzzers). Effort S to study. Risk low.
- Next steps: (1) Read `libprjoxide`'s DB embedding (data embedded in the binary;
  rebuild after a DB update) and decide whether Helion wants that or a runtime DB.
  (2) Copy its fuzzer/DB separation. (3) No Nexus target now.

#### 4.7.5 Papers that matter for this area

- Bitfiltrator (Kashani, Emami, Larus, FPL 2022, DOI 10.1109/FPL57034.2022.00039;
  https://infoscience.epfl.ch/entities/publication/259a5b2a-b1de-48c3-8be8-f45c3a6fdde1 ):
  "a general approach for reverse-engineering Xilinx bitstream formats" - explains
  the parameters needed to navigate a bitstream, the experiments to obtain them,
  and pitfalls; demonstrates extracting LUT equations, LUTRAM, BRAM and register
  values on UltraScale and UltraScale+ "automatically and without physical access
  to an FPGA." This is the single most relevant paper for Helion's UltraScale+ lane.
- "Patching FPGAs" (Puschner, Ender, Becker, Paar, ASHES 2024;
  https://hss-opus.ub.ruhr-uni-bochum.de/opus4/files/14116/PuschnerEndres1.pdf ):
  partial bitstream RE only - insert pre-synthesised circuits into existing
  bitstreams without fully reversing the format; validated on OpenTitan on
  7-series. Useful for Helion's verification strategy, not for coverage.
- JustSTART / ConFuzz (Ender et al., TCHES 2024; paper PDF
  https://raw.githubusercontent.com/emsec/ConFuzz/addb5f0f054a3bec83f8f27ae9bddb0f94c1bad1/JustSTART_paper/pdf/JustSTART.pdf ):
  fuzzing the configuration engine (not the fabric) of 7-series and UltraScale(+);
  found CVE-2023-20570 (RSA authentication bypass on UltraScale(+)). Relevant to
  Helion only as a warning about the config-engine layer and as a fuzzing-framework
  reference; ConFuzz repo URL [UNVERIFIED] from the PDF alone.
- ICAP paper (Albartus et al., TRETS 2023, https://doi.org/10.1145/3633204 ; PDF
  http://www.ecs.umass.edu/ece/tessier/albartus-trets24.pdf ): surveys Project
  X-Ray and U-Ray as having "documented significant parts" of Xilinx bitstreams;
  useful as a citable survey, but its U-Ray claim is now stale.
- "Netlist-to-bitstream equivalence" (McKendrick et al., FPT 2023;
  https://byuccl.github.io/assets/mckendrick_fpt23.pdf ): physical-netlist
  generation + structural comparison using RapidWright, released in BYU BFASST
  (https://github.com/byuccl/bfasst ). This is the verification method Helion
  should use to prove its emitter matches Vivado bit-for-bit on licensed parts.
- Bochum dissertation
  (https://hss-opus.ub.ruhr-uni-bochum.de/opus4/files/13040/diss.pdf ): Table 2.1
  gives the standard overview table (X-Ray / U-Ray / Trellis / IceStorm with URLs)
  - a good citation anchor, now dated.

### 4.8 Fuzzing methods in general, and extending coverage without a Vivado license

The standard loop (verified across prjxray, prjtrellis, prjoxide, apicula READMEs):

1. Build a baseline (empty / minimal) design and its bitstream.
2. Build a design that toggles exactly one feature (one LUT INIT, one PIP, one
   BRAM bit, one IO standard) and its bitstream.
3. Diff the two bitstreams; the differing bits belong to that feature, modulo bits
   shared with the baseline.
4. Repeat for every value of every feature, in tile-type order; store as segbits
   (segment, bit, value) keyed by tile type and feature name.
5. Solve PIPs: a PIP is a mux, so one PIP choice is typically several bits whose
   combination selects the input; the solver must find the minimal consistent bit
   set across many single-PIP experiments. The exact solver used by prjxray (SAT vs
   ILP vs set-cover vs hand-rolled correlation) is [UNVERIFIED] from the sources
   read - do not cite a solver type for prjxray without reading `prjxray/lib` and
   the fuzzer code.
6. Verify on silicon: a DB row is a hypothesis until a board proves it. The t27.ai
   case (https://t27.ai/blog/two-bitstreams-one-bit-apart/ ) is the model: a proposed
   HCLK row sat unverified for 18 months until two 22.7 MB bitstreams differing in
   exactly one bit (frame 0x00401C00, word 50, bit 31) proved the BUFR enable on an
   xc7a200t board.

Extending coverage to a new part without a Vivado license - the only defensible
routes:

1. Tile-type carry-over (the main route, and the one Helion already uses): segbits
   are per tile type, not per part. If xcvu3p has the same INT tile as xcku3p, the
   INT segbits carry; what changes is the tilegrid (which tiles exist, at which
   coordinates), the frame address walk (FAR), and the part IDCODE. Sources for the
   part-specific half without Vivado: UG570 (frame format, FAR fields), RapidWright
   device data (section 4.7.3, [UNVERIFIED] for unlicensed parts), and Vivado
   `link_design` dumps / device dumps, which the project already treats as
   no-license work. Every carried-over tile type must be flagged as carried-over,
   not fuzzed, until a licensed sibling or a board test confirms it.
2. Already-published DBs: for 7-series unlicensed parts (xc7k325t, xc7s25, xc7z045),
   the openXC7 prjxray-db fork is the first place to look; for UltraScale+
   unlicensed parts there is no published DB at all.
3. Vendor-file parsing (Apicula method): where a vendor ships machine-readable
   device data, parse it first and fuzz only the residue. For Xilinx this is
   RapidWright/DCP data, not a documented file format.
4. Single-bit board verification: for any carried-over row that gates a whole
   feature (clocks, IO, DSP), do the t27-style two-bitstream test on a board with
   the licensed sibling part before trusting it on the unlicensed part.
5. What does NOT work: re-running prjxray/prjuray fuzzers against an unlicensed
   part (Vivado will refuse to generate the bitstream); assuming a tile type is
   identical across families (7-series INT is not UltraScale+ INT); treating
   RapidWright as a bitstream generator (its docs forbid that reading).

- Helion crate: `helion-ultrascale` + `helion-device` (carry-over table with
  provenance per tile type), `helion-bits` (frame-address walk per part), plus a
  new fuzzing harness (Python, mirroring prjxray's `fuzzers/` layout) that runs
  only on licensed parts.
- Expected gain: high - this is the difference between UltraScale+ coverage being
  blocked on licensing and being shippable.
- Effort: M for the carry-over table + provenance flags; L for a full Helion fuzzer
  harness on xcku3p/xcku5p.
- Risk: High if provenance is lost (a carried-over row silently treated as fuzzed).
  Mitigate by making provenance a field in the DB schema, not a comment.
- Next steps: (1) Add a `provenance` field to every segbits row:
  `fuzzed:<part>`, `carried:<from-part>`, or `rapidwright:<device>`, and refuse to
  emit a bitstream for a part whose rows are all carried-over without an explicit
  override flag. (2) Implement the frame-address walk from UG570 for each
  UltraScale+ part from its tilegrid, and unit-test it against a Vivado-generated
  bitstream on xcku3p (licensed) before using it on xcvu3p. (3) Read Bitfiltrator
  (section 4.7.5) before writing any new UltraScale+ fuzzer; it documents the
  pitfalls (frame navigation, LUTRAM/BRAM layout) that prjuray never reached.
  (4) Set up the openXC7 diff harness (section 4.7.2) as the regression gate for
  every new segbits row on 7-series.

### 4.9 Device coverage: priority order

1. RapidWright device-data verification (4.7.3): cheapest possible unlock for
   unlicensed UltraScale+ geometry. M effort, answers a yes/no question first.
2. openXC7 diff harness + prjxray-db fork audit (4.1, 4.7.2): closes 7-series gaps
   on unlicensed parts (xc7k325t, xc7s25) from already-published data. S-M effort.
3. Helion UltraScale+ fuzzer harness on xcku3p/xcku5p, Bitfiltrator-guided
   (4.2, 4.8): the real work; L effort, but it is the lane.
4. Provenance-flagged carry-over table (4.8): M effort, must land before any
   carried-over row ships.
5. Versal: do nothing except record the no-prior-art finding (4.6).

---

## 5. Vivado extras: open replacements for Xilinx IP and flows

Summary verdict: the LiteX ecosystem is the single highest-value replacement set:
LiteDRAM, LitePCIe, LiteScope, LiteICLink and LiteEth together cover most of the
MIG / XDMA / ILA / transceiver surface, all BSD-2-Clause, all usable as generated
Verilog so Helion does not need to adopt Migen/Python. The hard gaps are:
(1) no open GT wizard / IBERT equivalent, (2) no credible open XPE replacement
for 7-series power, (3) LitePCIe is Gen2-only on 7-series, so Gen3 x4 on
7-series needs Wupper or the Xilinx hard block, (4) HLS is research-grade only,
Bambu is the only mature C/C++ option, and (5) encrypted IEEE 1735 IP cannot be
read by any open tool, by design.

Priority order: LiteDRAM + LitePCIe integration first, then LiteScope, then
Verilator/GHDL/nvc co-sim, then AXI infrastructure, then HLS as an external-tool
bridge, then power as a calibrated estimate only. Do not build a GT wizard or a
power model from scratch in this area.

### 5.1 DDR controllers: LiteDRAM (LiteX)

- Best source (verified reachable, opened): https://github.com/enjoy-digital/litedram
  Docs/benchmarks: http://enjoy-digital.github.io/litedram/
- License: BSD-2-Clause, verified in README. Permissive, compatible with Helion's
  AGPL-3.0-or-later as a separate generated-Verilog dependency; no copyleft
  contamination of Helion itself if used as generated RTL, but confirm with a
  licence review before vendoring source.
- What it provides: small-footprint configurable DRAM core in Migen/LiteX,
  generatable as standalone Verilog (`litedram_gen`, YAML description). Fully
  pipelined core, configurable bank-machine command depth, auto-precharge,
  periodic refresh / ZQ short calibration (up to 8 postponed refreshes), ECC, BIST,
  DMA reader/writer, multi-port crossbar, Native / AXI-MM / Wishbone frontends.
- DRAM types / PHYs verified in README: generic SDR PHY (vendor-agnostic);
  Spartan-6 DDR/LPDDR/DDR2/DDR3; Spartan-7/Artix-7/Kintex-7/Virtex-7 DDR2/DDR3
  (1:2 or 1:4 ratio); Artix-7/Kintex-7/Virtex-7 LPDDR4 and LPDDR5 PHYs (with
  Verilator sim PHY/model); Artix-7 RPC DRAM (Etron, reduced pin count);
  Kintex/Virtex UltraScale(+) DDR3/DDR4 (1:4, optional read-VREF scan on
  UltraScale+ DDR4); Altera Cyclone V DDR3; Lattice ECP5/Nexus DDR3; Gowin
  GW2A/GW5A DDR3. DFI-level simulation PHY/model for SDR/DDR/LPDDR/DDR2/DDR3/DDR4
  under Verilator. No DDR5 PHY and no DDR4 PHY for 7-series (DDR4 on 7-series is
  not a LiteDRAM target; that is a real gap vs MIG).
- Measured performance/area: no single headline LUT/throughput number is published
  in the README. Two sourced data points: (a) the LiteX paper reports LiteDRAM on
  a 32-bit DDR3 design at "more than 20 Gbps bandwidth" (NetV2 reuse table):
  https://www.ensta-bretagne.fr/lelann/papers/litex.pdf ; (b) the project's own
  benchmark page gives per-test latency and bandwidth, e.g. MT41K128M16 DDR3
  32-bit read latency 30-42 clk in the listed tests, and shows random-access
  patterns at very low efficiency (single-digit percent in the sampled rows) vs
  sequential: http://enjoy-digital.github.io/litedram/ . Treat (b) as a warning
  that random-access efficiency, not peak bandwidth, is the metric to measure.
  No verified LUT count for a 7-series DDR3 controller is available from these
  sources; do not quote one.
- Helion crate: `helion-device` (PHY/part database, per-part DDR timing),
  `helion-ipxact` (AXI-MM port description), `helion-xilinx7` (placement of the PHY
  primitives, I/O constraints). The generated Verilog is consumed downstream, not
  reimplemented in Rust.
- Expected gain: high. Replaces MIG for the common DDR2/DDR3 7-series cases;
  unblocks any design needing external DRAM end-to-end.
- Effort: M. Risk: Medium. Calibration/training is the hard part (read levelling,
  VREF, timing closure at 400+ MHz DDR clock); LiteDRAM is proven on many boards
  but each new board/part needs board-level validation. Helion must not claim MIG
  parity until memtest passes on hardware.
- Next steps: (1) Generate LiteDRAM Verilog for one reference part (e.g. Artix-7
  DDR3, Arty A7 class) and check it through Helion synth as a black box.
  (2) Add DDR part/timing entries to `helion-device` from LiteDRAM's SDRAM module
  database, with a test that the YAML and the device entry agree. (3) Run the
  Verilator DFI sim PHY + BIST in `helion-sim` as the regression for the
  controller. (4) Board-test one design with a hardware memtest before advertising
  DDR support.

### 5.2 PCIe: LitePCIe, Wupper, RIFFA

#### 5.2.1 LitePCIe (LiteX): best general pick

- Best source (verified reachable, opened): https://github.com/enjoy-digital/litepcie
- License: BSD-2-Clause, verified in README.
- What it provides: small-footprint configurable PCIe core, generatable as Verilog.
  TLP layer, reordering, MSI / multi-vector MSI / MSI-X, crossbar, DMA with
  scatter-gather, MMAP (AXI/Wishbone slave/master), Linux driver (MMAP and DMA),
  64/128/256/512-bit datapath, clock-domain crossing.
- What it supports (verified in README): Xilinx UltraScale(+) up to Gen3 x16;
  Xilinx 7-series up to Gen2 x8 only; Intel Cyclone 5 up to Gen2 x4; Stratix 5 up
  to Gen2 x8; soft PHY via LiteICLink in progress. PTM on 7-series, experimental
  multi-lane and UltraScale+ Gen2/Gen3/Gen4 support.
- Measured throughput: no headline number in the README. One sourced measured
  example: LitePCIe on the Fairwaves XTRX (Artix-7, Gen2 x4 class) `dma_test`
  loopback at about 6.84 Gbps sustained, 0 errors:
  https://github.com/enjoy-digital/litex_xtrx/blob/HEAD/README.md . The LiteX
  paper reports LitePCIe Gen2 x4 at "more than 12 Gbps" in its reuse table:
  https://www.ensta-bretagne.fr/lelann/papers/litex.pdf . The 6.84 vs 12 gap is a
  reminder to measure on Helion's own board, not to quote the paper figure as
  Helion's.
- Helion crate: `helion-device` (PCIe hard-block / GT location data),
  `helion-ipxact` (DMA/MMIO interface), `helion-xilinx7` (hard-block instantiation
  and constraints).
- Expected gain: high for UltraScale and for Gen2 7-series designs. Replaces XDMA
  for the common DMA/MMIO cases.
- Effort: M. Larger if Helion also writes the Linux driver integration and the
  scatter-gather descriptor handling.
- Risk: Medium. The 7-series Gen2 ceiling is the key limit (see 5.2.2). Also note
  the README lists "add standardized interfaces (AXI, Avalon-ST)" as a possible
  improvement, so the AXI-MM front end is not the primary interface today.
- Next steps: (1) Generate LitePCIe Verilog for a Gen2 x4 Artix-7 target and a
  Gen3 x8 UltraScale target; synth both through Helion as black boxes.
  (2) Define the Helion PCIe IP block interface (DMA + MMAP) in `helion-ipxact` so
  LitePCIe and Wupper are interchangeable behind it. (3) Measure DMA throughput
  on a real 7-series board with the LitePCIe Linux driver; record the number, do
  not carry the paper figure forward. (4) Document the Gen2-only 7-series limit
  prominently so users do not expect Gen3 x4 from LitePCIe on 7-series.

#### 5.2.2 Wupper: best pick for Gen3 on 7-series

- Best source (verified reachable):
  https://gitlab.nikhef.nl/franss/wupper/-/blob/e5dc1d4361f796c531b70bd3b068cb911cce301a/README.md
  ; mirror/landing: https://opencores.org/projects/virtex7_pcie_dma
- License: LGPL (per OpenCores listing). VHDL.
- What it provides: PCIe DMA engine for the Xilinx Virtex-7 Gen3 hard block,
  designed at Nikhef for CERN ATLAS/FELIX. Simple DMA interface to standard
  FIFOs, descriptor-based DMA (address, read/write, size in 32-bit words, enable),
  MSI-X compatible interrupt controller. 256-bit AXI4-Stream interface at 250 MHz
  for Gen3, 512-bit for Gen4.
- What it supports: originally Virtex-7 Gen3; ported to Kintex UltraScale, Virtex
  UltraScale+, Versal Prime and Versal Premium, supporting Gen3, Gen4 and Gen5
  (per OpenCores page). Recommended Vivado 2023.2/2024.1 in the docs, Tcl scripts
  supplied, no Xilinx project files.
- Measured throughput: no verified throughput number found in the sources above;
  the MDPI survey calls it "a mature and verified in-practice solution" but gives
  no Gbps figure: https://www.mdpi.com/2079-9292/12/4/883 . Do not quote a Wupper
  Gbps number.
- Helion crate: same as LitePCIe (`helion-device`, `helion-ipxact`,
  `helion-xilinx7`). Wupper is VHDL, so Helion needs a VHDL front end or a
  pre-generated netlist path for it; that is a real integration cost LitePCIe
  (Verilog) does not have.
- Expected gain: medium-high. This is the only verified open route to Gen3 on a
  7-series part.
- Effort: M-L (VHDL handling, hard-block instantiation, descriptor driver).
- Risk: Medium-High. Depends on the Xilinx PCIe hard block and its .xci-style
  configuration; Helion must model that hard block exactly or Wupper cannot be
  placed. Up to 8 buffer descriptors, one reserved for host-to-FPGA, per the
  survey, which limits descriptor depth vs XDMA.
- Next steps: (1) Confirm the current Wupper tree builds for a Virtex-7 Gen3
  target and list every Xilinx IP it still depends on (FIFO, clock wizard, PCIe
  block). (2) Prototype the Helion hard-block model for the 7-series PCIe block
  needed by Wupper. (3) Decide LitePCIe vs Wupper per target in `helion-device`
  (Gen2 7-series: LitePCIe; Gen3 7-series: Wupper).

#### 5.2.3 RIFFA: older, research-proven alternative

- Best source: paper:
  https://kastner.ucsd.edu/wp-content/uploads/2014/04/admin/fpl-riffa2.pdf ;
  overview: https://www.dustinrichmond.com/reusable-integration-framework-for-fpga-accelerators/
- What it provides: Reusable Integration Framework for FPGA Accelerators. PCIe DMA
  + interrupts, up to 12 channels, multi-FPGA (up to 5), Linux and Windows, C/C++,
  Python, MATLAB, Java bindings. Works directly with the PCIe endpoint (not a
  bridge) and claims to saturate the link in its tests. Tested on Spartan LX150T,
  ML605, VC707, Altera DE5-Net/DE4/DE2i (per the overview, which is old).
- Status caveat: RIFFA 2.0/2.1 era is 2013-2015; the Linux driver was written for
  kernels 2.6.32-3.x. Repo URL and current maintenance state [UNVERIFIED] (not
  confirmed in this pass). Treat as a reference design and a fallback, not a
  primary dependency, until the repo and its kernel-driver state are re-verified.
- Helion crate: `helion-ipxact` / `helion-xilinx7` if adopted; otherwise reference
  only.
- Expected gain: low-medium today. Effort: L-XL to modernise. Risk: High (stale
  driver, old boards).
- Next steps: only re-evaluate RIFFA if LitePCIe and Wupper both fail on a target;
  first verify its repo, licence, and whether anyone has ported the driver past
  kernel 5.x.

### 5.3 Transceivers / high-speed serial: GTP/GTX/GTH

There is no open equivalent of the Xilinx Transceiver Wizard. What exists is a set
of protocol-specific wrappers around the hard GT primitives. That distinction must
be kept in the docs.

- Best source: LiteICLink (LiteX SerDes): https://pypi.org/project/liteiclink/2024.12/
- What LiteICLink provides (verified): SerWB (Wishbone over 3 LVDS I/Os, 8B/10B,
  gearbox, up to 1.25 Gbps line rate / 32 bits at 31.25 MHz) and SerDes support for
  Artix-7 GTP, Kintex-7 GTX, UltraScale GTH, UltraScale+ GTY, Lattice ECP5.
  BSD-2-Clause. Used for CPRI/JESD204B SDR boards, USB3 Pipe, and ARTIQ.
- USB3 Pipe as a proof point: https://deepwiki.com/enjoy-digital/usb3_pipe/1-overview
  (secondary summary; repo itself not opened in this pass, mark the repo URL
  [UNVERIFIED]). It implements USB 3.0 SuperSpeed PIPE using only GTX/GTP
  transceivers on Kintex-7 (KC705, XC7K325T) and Artix-7 (Acorn, XC7A200T) at
  5 Gbps, 8b/10b. This proves the GT primitives can be driven open, but it is a
  USB3-specific stack, not a generic GT wizard.
- LitePCIe PHY: wraps the hard PCIe block (see section 5.2); the GT configuration
  for PCIe is inside that block, not a general GT API.
- LiteX paper table (for scope): LiteJESD204B TX, Subclass 1, up to 10 Gbps;
  LiteICLink up to 10 Gbps: https://www.ensta-bretagne.fr/lelann/papers/litex.pdf
- What is missing (verified by absence in the sources above): no open tool that
  takes "protocol + line rate + refclk" and emits a configured GT wrapper with
  equalisation, CDR settings, and eye/BER validation (the Transceiver Wizard +
  IBERT role). No open IBERT/BERT equivalent. No verified open GTY/GTH bring-up
  guide for Helion's own bitstream flow. Equalisation and signal-integrity tuning
  remain board- and vendor-documentation-driven.
- Helion crate: `helion-device` (GT tile locations, refclk pins, line-rate limits
  per part), `helion-xilinx7` / `helion-ultrascale` (GT primitive models),
  `helion-debug` (future BERT/eye-scan hooks).
- Expected gain: medium. LiteICLink + LitePCIe + USB3 Pipe cover the common
  protocols; a generic wizard would be a large new project.
- Effort: M to integrate LiteICLink SerDes as IP blocks; XL to build a GT wizard.
- Risk: High for any new GT protocol. GT bring-up failures are silent (no link, no
  error) and need board hardware to debug.
- Next steps: (1) Catalogue, per 7-series part, which GT type it has (GTP vs GTX)
  and its max line rate, in `helion-device`, from the vendor datasheet, not from
  LiteX. (2) Integrate LiteICLink SerDes as a Helion IP block for one protocol
  (SerWB or USB3) on one board and validate the link on hardware. (3) Do not
  promise a GT wizard; document the gap and the per-protocol wrappers that exist.
  (4) Add a BERT/loopback test pattern generator to `helion-debug` as the first
  step toward an IBERT replacement.

### 5.4 ILA debug: LiteScope and other open ILA/VIO replacements

#### 5.4.1 LiteScope (LiteX): best pick

- Best source: https://pypi.org/project/litescope/2024.12/ ; LiteX wiki explanation:
  https://github.com/enjoy-digital/litex/wiki/LiteX-for-Hardware-Engineers
- License: BSD-2-Clause (per PyPI page).
- What it provides: small-footprint configurable embedded logic analyser, aiming to
  be a free portable alternative to vendor solutions. LiteScopeAnalyzer:
  subsampling, RLE compression, data storage in Block RAM, configurable triggers.
  LiteScopeIO: I/O peek and poke (the VIO equivalent). Bridges: UART-Wishbone,
  Ethernet-Wishbone (LiteX), PCIe-Wishbone (LitePCIe). Export formats: .vcd, .sr
  (sigrok), .csv, .py. Generatable as Verilog for a standard flow.
- Limits (verified): the LiteX wiki states only simple trigger conditions are
  supported (signal equals 1 or 0; no edges or compound statements) in the version
  it documents; the current README lists "configurable triggers" without that
  detail, so the exact current trigger expressiveness is [UNVERIFIED] and must be
  re-checked in the current source. Listed possible improvements in the README:
  standardised interfaces (AXI, Avalon-ST), protocol analysers, signal
  injection/generation, storage in DRAM, storage in HDD with LiteSATA. No JTAG
  transport by default; readout needs UART/Ethernet/PCIe or another Wishbone
  bridge, which is a real difference from ILA over JTAG.
- Helion crate: `helion-debug` (primary: capture, trigger, VCD/sigrok export, host
  tool), `helion-sim` (VCD consumption), `helion-ipxact` (analyser block
  description).
- Expected gain: high. This is the closest open ILA+VIO replacement, and its
  VCD/sigrok export plugs directly into GTKWave/PulseView.
- Effort: S-M. Risk: Low-Medium. Trigger limits and the lack of JTAG transport
  need to be documented; BRAM depth is the capture-depth limit.
- Next steps: (1) Generate LiteScopeAnalyzer Verilog for a small signal set,
  insert it via a Helion debug pass, and read back a capture over UART.
  (2) Verify the current trigger language in the LiteScope source before
  documenting it. (3) Add VCD export from `helion-debug` so captures open in
  GTKWave without LiteX tooling. (4) Design a JTAG/Wishbone bridge later if JTAG
  transport is required; do not promise it now.

#### 5.4.2 Other options

- sigrok / PulseView (host side): https://sigrok.org/wiki/Openbench_Logic_Sniffer -
  sigrok supports many external logic analysers (OpenBench Logic Sniffer: 32 probes,
  up to 100 MHz, 24k samples, RLE optional, Spartan-3E based, open hardware).
  These are external instruments, not on-chip ILA replacements, but PulseView is
  the right host viewer for LiteScope's .sr export. Glasgow Interface Explorer is
  a modern open FPGA-based debug tool (iCE40, 16 GPIOs, protocol applets, VCD
  output) but it is an external probe, not an in-design ILA. Do not present either
  as an ILA replacement.
- No other verified on-chip open ILA core was found in this pass. If a second
  candidate is wanted, search specifically for "open source ILA core Verilog" and
  verify the repo before adding it.

### 5.5 Power estimation

There is no verified open XPE replacement for 7-series. This is the weakest area
in this report.

- Best source (survey, verified reachable):
  https://hal.science/hal-02866921v1/document - Nasser et al., "RTL to Transistor
  Level Power Modelling and Estimation Techniques for FPGA and ASIC: A Survey",
  IEEE TCAD 2021. It taxonomises power modelling, compares commercial tools and
  academic techniques, and notes FPGA-SPICE (integrated with VPR) gives cell-level
  power but with significantly longer runtime and poor scalability for large
  designs, and that VersaPower is less accurate than FPGA-SPICE. It also reports,
  for one ensemble runtime dynamic-power model, an error margin of 1.90% vs a
  commercial gate-level power estimation tool. That 1.90% figure is for that
  specific published model, not a general claim for open FPGA power estimation,
  and not a 7-series XPE figure.
- VPR power model: VPR (in the VTR suite) has a power model based on switching
  activity, capacitance, and MOS parameters, but the sources found in this pass
  describe it as basic (sub-threshold leakage only, not suitable for 65 nm and
  below in the cited deck) and academic-architecture-calibrated, not calibrated to
  Xilinx 7-series silicon. One cited deck reports a trace-based estimator (Ptrace)
  with average power error 3.4% and delay error 6.4% vs VPR/Psim on 20 benchmarks,
  but that is a secondary slide deck, not a peer-reviewed 7-series validation, so
  treat it as [UNVERIFIED / weak source]. No verified accuracy number for any open
  power estimator on a 7-series part was found.
- What Helion can actually do: (a) report resource counts and clock frequencies
  exactly; (b) estimate dynamic power as sum over nets of activity x capacitance x
  V^2 x f, using switching activity from `helion-sim` (VCD/SAIF-style) and
  capacitance from a per-part table that Helion must calibrate; (c) report static
  power as a per-part table (leakage vs temperature/voltage) with an explicit
  "ballpark" label; (d) validate against a small set of board power measurements
  (shunt / PMBus) and publish the error, rather than publishing a model accuracy
  claim first.
- Helion crate: new power estimation belongs in `helion-device` (per-part power
  tables) plus a small estimator in `helion-lib` or a new `helion-power` module;
  activity comes from `helion-sim`.
- Expected gain: medium. Even a +/-20-30% estimate with honest error bars is useful
  for thermal budgeting; a false-precision XPE clone would be worse than none.
- Effort: M for a calibrated activity-based estimator; XL for anything claiming XPE
  accuracy.
- Risk: High if Helion publishes power numbers without board validation. Power
  depends on activity, temperature, voltage, and process, none of which Helion
  controls.
- Next steps: (1) Do not build a power model yet; first add activity dumping
  (toggle counts per net) to `helion-sim`. (2) Measure real board power on 3-5
  reference designs and record Helion's estimate vs measured, to establish the
  error bar. (3) Only then publish a power report with the measured error range and
  the per-part table source. (4) Track VTR/VPR power work as a reference, not a
  dependency, until a 7-series calibration exists.

### 5.6 HLS: Bambu, Dynamatic, CIRCT / Calyx, LegUp

#### 5.6.1 Bambu: best pick for C/C++ HLS today

- Best source (verified reachable, opened): https://github.com/ferrandi/PandA-bambu ;
  paper: https://re.public.polimi.it/bitstream/11311/1189606/1/dac21_bambu.pdf
- License: current README states Apache-2.0 WITH LLVM-exception (note: older
  slides/papers say GPLv3; the current repo is the authority, and the change
  matters for Helion integration). Verify the licence file at integration time.
- What it provides: open-source HLS research framework (Politecnico di Milano,
  PandA). Input: standard C/C++ and Clang/LLVM or GCC IR. Supports most C/C++
  constructs including function calls, pointer arithmetic, dynamic memory-access
  resolution, arrays/structs, pass-by-reference/copy; complete ANSI C except
  recursion (per the FPL 2024 slides). Output: Verilog or VHDL. Back ends: Xilinx
  ISE/Vivado, Altera Quartus, Lattice Diamond, NanoXplore, and OpenRoad for ASIC.
  Integrated testbench generation and simulation with Verilator, Icarus, Xsim,
  ModelSim. AppImage and Docker distributions available.
- Maturity: most mature open C/C++ HLS. Active (6,377 commits in the opened repo
  view, main branch, recent work). Research framework, not a Vitis HLS drop-in:
  QoR will lag Vitis on some designs, and the pragma/directive set differs.
- Helion crate: `helion-hls` (primary). Bambu is an external tool; Helion should
  invoke it and consume its Verilog/VHDL, not reimplement scheduling/binding.
- Expected gain: high. Gives Helion a C/C++ entry point without building an HLS
  compiler.
- Effort: M. Risk: Medium. Bambu QoR vs Vitis HLS is the risk; also its build
  requires GCC >= 13 and a specific Clang setup (per README), so pin the version.
- Next steps: (1) Run Bambu AppImage on 3-5 small C kernels, synth the output
  through Helion, record LUT/FF/DSP vs a hand-written RTL baseline. (2) Define the
  `helion-hls` command surface (input C, top function, target part, output RTL) as
  a thin wrapper, not a fork. (3) Document which Vitis HLS pragmas are and are not
  honoured by Bambu.

#### 5.6.2 Dynamatic: best pick for irregular / dynamic HLS

- Best source: https://dynamo.ethz.ch/research/ (ETH Zurich DYNAMO group page;
  repo named in its papers as EPFL-LAP/dynamatic, GitHub URL [UNVERIFIED] in this
  pass, do not cite a repo URL from memory); experience paper:
  https://arxiv.org/html/2603.19856 (note: the arXiv identifier date looks
  anomalous; treat its numbers as [UNVERIFIED - needs a full read before citing]).
- What it provides: open-source HLS compiler generating dynamically scheduled
  dataflow circuits (handshake/ready-valid, operations fire when inputs are ready).
  Input C/C++ via a Polygeist fork into MLIR (affine/scf/arith/memref), lowered to
  the CIRCT Handshake dialect, then to RTL/SystemVerilog for Xilinx synthesis.
  Excels where control flow or memory access is unpredictable (conditionally
  skipped ops, sparse linear algebra). Reported in ETH slides as up to 14.9x
  speedup over standard (statically scheduled) HLS circuits on irregular
  benchmarks, at the cost of higher resource usage for the dataflow control. That
  14.9x is a best-case irregular-benchmark figure, not a general HLS speedup.
- Maturity: research-grade but actively developed (the experience paper reports
  about 30 PRs/month merged, CI-monitored). v2.0 is a complete MLIR rewrite; recent
  releases no longer depend on CIRCT as a build dependency (per the 2026 MLIR-HLS
  survey, secondary source).
- Helion crate: `helion-hls` (as an alternative back end to Bambu, for irregular
  kernels).
- Expected gain: medium. Only for kernels where dynamic scheduling wins; not a
  general Vitis replacement.
- Effort: L. MLIR/CIRCT build is heavy; Helion should consume Dynamatic's
  SystemVerilog output.
- Risk: High. Research tool, changing IR, resource-heavy output, no Vitis pragma
  compatibility.
- Next steps: (1) Verify the Dynamatic repo URL and build it once before committing
  to it. (2) Run one irregular kernel (e.g. sparse / data-dependent loop) through
  Dynamatic and Bambu and compare LUTs and cycles. (3) Only adopt if the
  irregular-kernel win is measured on Helion's own benchmark.

#### 5.6.3 CIRCT (including Handshake and Calyx dialects)

- Best source (verified reachable, opened): https://github.com/llvm/circt ;
  project site named in its README as https://circt.llvm.org (not separately
  opened, but stated in the repo).
- What it provides: LLVM incubator project applying MLIR to hardware. 30+ dialects
  from high-level scheduling (`calyx`, `handshake`, `pipeline`) through RTL (`hw`,
  `comb`, `seq`) to SystemVerilog emission (`sv`, `emit`). Tools: `firtool`
  (FIRRTL/Chisel), `circt-opt`, `circt-translate`, `arcilator` (simulation),
  `circt-bmc` / `circt-lec` (formal), `hlstool` (HLS driver). This is
  infrastructure, not an HLS tool by itself: HLS flows (Calyx, Dynamatic,
  ScaleHLS) build on it.
- Calyx (inside CIRCT and standalone): Cornell's "LLVM for accelerators": an
  IR/compiler infrastructure separating hardware structure from control scheduling,
  MIT-licensed, 56k lines of Rust per the survey, with a CIRCT `calyx` dialect that
  lowers to `hw`/`comb`/`seq` and emits SystemVerilog. Paper/flow:
  https://arxiv.org/html/2512.06177v1/ (PyTorch to Calyx via Allo + CIRCT, reporting
  up to 2.21x vs Vitis HLS on its ML evaluation, and up to 3x in the survey summary
  for the memory-banking flow; both are ML-specific, not general HLS claims).
  Calyx is a good IR target for a future Helion HLS front end, not a C/C++ HLS tool
  on its own. Its repo URL (calyxir/calyx) is [UNVERIFIED] in this pass.
- Helion crate: `helion-hls` (long-term: a CIRCT/Calyx dialect target), and
  potentially `helion-lib` for an MLIR-adjacent IR. Do not adopt CIRCT as a build
  dependency lightly: it is a large LLVM/MLIR build.
- Expected gain: medium-long-term. CIRCT is the right long-term HLS infrastructure;
  it is not a short-term Vitis replacement.
- Effort: XL to adopt as a dependency; S to consume its SystemVerilog output.
- Risk: High (build size, API churn, research velocity).
- Next steps: (1) Treat CIRCT as a consume-output integration first: take
  Calyx/Dynamatic SystemVerilog and synth it in Helion. (2) Only evaluate a CIRCT
  library dependency after Bambu integration is working. (3) Track `hlstool` as the
  future HLS driver, not a current dependency.

#### 5.6.4 LegUp: status

- Best source: https://microchiptech.github.io/fpga-hls-docs/2023.1/releasenotes.html
- Status: LegUp HLS (University of Toronto, originally open-source) was
  commercialised by LegUp Computing, acquired by Microchip in October 2020, and
  rebranded to SmartHLS in 2021 (namespace `legup` renamed to `hls`). SmartHLS is
  now bundled with Libero SoC and targets Microchip PolarFire, not Xilinx. The
  original open-source LegUp is archived (Wikipedia HLS table lists "LegUp Archived
  2020-07-24"). Do not plan on LegUp as an open HLS for Helion. Mention it only as
  history and as a warning that an open HLS can be closed by acquisition.
- Helion crate: none. Effort/Risk: not applicable.

### 5.7 Simulation: Verilator, GHDL, nvc

#### 5.7.1 Verilator: best pick for SystemVerilog

- Best source: https://verilator.org ; README:
  https://raw.githubusercontent.com/verilator/verilator/master/README.rst
- License: LGPL-3.0 or Perl Artistic-2.0.
- What it covers: Verilog and SystemVerilog (synthesis subset plus most verification
  constructs including UVM, intra-assignment delays, events). Compiles RTL to
  multithreaded C++ or SystemC; lint checks; XML output for tools. Supports VPI/DPI
  for co-simulation with C/C++ testbenches, and linking generated libraries into
  other simulators. Does not accept non-synthesizable testbench code the way an
  event-driven simulator does; the testbench is C++ (or SystemC) around the model.
- Performance numbers (project's own claims, not independently verified here):
  "over 10x faster than standalone SystemC" and "about 100x faster than interpreted
  Verilog simulators such as Icarus Verilog" on a single thread, with
  multithreading on top; "typically similar or better performance versus
  closed-source Verilog simulators" (ModelSim/Questa, VCS, Incisive). An
  independent Embecosm comparison cited in search gives Verilator about 42.66 kHz
  vs Icarus about 1.48 kHz on the same design (about 29x):
  https://www.embecosm.com/appnotes/ean6/html/ch06s05s01.html . Treat the 100x as
  the project's marketing figure and the 29x as one measured data point on one design.
- Helion integration: `helion-sim` (primary). Helion should (a) emit
  Verilog/SystemVerilog for Verilator, (b) drive it with a C++ harness or
  `--binary`, (c) consume VCD/FST, and (d) use Verilator's lint (`--lint-only
  -Wall`) as a pre-synth check. VPI/DPI is the co-simulation path for Helion's own
  Rust testbench driving a Verilator model.
- Expected gain: high. This is the fastest open SV simulator and the de facto
  standard for open FPGA flows (LiteX uses it for LiteDRAM/LitePCIe sim PHYs).
- Effort: S-M. Risk: Low-Medium. Main limits: 2-state (no X propagation the way
  event-driven sims do), no VHDL, slow C++ compile for large designs.
- Next steps: (1) Add a `helion-sim verilator` path that compiles Helion-emitted
  SV and runs a C++ testbench, with VCD out. (2) Add Verilator lint to the Helion
  pre-synth checks. (3) Use the LiteDRAM DFI sim PHY as the first Verilator co-sim
  regression.

#### 5.7.2 GHDL: best pick for VHDL

- Best source: https://github.com/ghdl/ghdl ; docs: https://ghdl.github.io/ghdl/
- License: GPL-2.0.
- What it covers: VHDL only. Full support for IEEE 1076-1987/1993/2002, partial
  for 2008 and 2019, partial PSL. Compiles VHDL directly to native machine code
  (GCC, LLVM, or built-in mcode backend), not an interpreter. Writes GHW, VCD, FST
  waveforms (viewable in GTKWave). Co-simulation via VPI and VHPIDIRECT (see
  ghdl-cosim docs). Also has an experimental synthesis path (ghdl-yosys-plugin)
  that Helion should not depend on.
- Performance: no headline speedup number in the README; the project states native
  execution is "much faster than any interpreted simulator". A secondary comparison
  table claims about 20% simulation speed improvements in recent releases and GHDL
  v5.1 (June 2025) as current at that writing; the repo now shows v6.0.0 (March
  2026) and 6.1 (Nov 2026) in its docs, so the version to pin is 6.x, not 5.1.
  No verified GHDL vs nvc head-to-head number was found; do not quote one.
- Helion integration: `helion-sim` (VHDL side). Helion needs GHDL for any VHDL
  design or VHDL IP (including Wupper). Use VPI/cocotb for co-sim with Helion's
  Rust harness.
- Expected gain: high for VHDL coverage. Effort: S-M. Risk: Low. Mature, packaged
  in most distros, Docker images available.
- Next steps: (1) Add a `helion-sim ghdl` path for VHDL designs, with VCD out to
  GTKWave. (2) Use GHDL to simulate Wupper (VHDL) as the first VHDL regression.
  (3) Do not use GHDL's experimental synthesis as a Helion path.

#### 5.7.3 nvc: faster VHDL alternative, narrower

- Best source: https://github.com/nickg/nvc (tree URL as returned:
  https://github.com/nickg/nvc/tree/2675140dccc693efce4c2588aae8b7a73734f513 ) ;
  manual: https://www.nickg.me.uk/nvc/manual.html (stated in README, not
  separately opened).
- License: GPL-3.0.
- What it covers: VHDL compiler and simulator only (not a synthesizer). Almost all
  of VHDL-2008 except PSL, experimental VHDL-2019 and experimental Verilog. Uses
  LLVM to compile VHDL to native code, with an emphasis on simulation performance
  (JIT). Supports OSVVM, UVVM, VUnit, cocotb. Latest release cited in search is
  1.19.2 / 1.23.x in distro listings (version drift in sources; pin the release at
  integration time).
- Performance: no verified head-to-head number vs GHDL was found in this pass. The
  project emphasises LLVM/JIT performance; a secondary table describes it as "high
  simulation performance" with no number. Do not claim nvc is faster than GHDL
  without a Helion benchmark.
- Helion integration: `helion-sim` as a second VHDL engine. The value is a second
  opinion on VHDL semantics and a potentially faster sim for large VHDL testbenches.
- Expected gain: medium. Effort: S. Risk: Low-Medium (smaller community than GHDL,
  LLVM version pinning).
- Next steps: (1) Benchmark GHDL vs nvc on the same VHDL design in `helion-sim`
  before choosing a default VHDL engine. (2) Keep both behind one `helion-sim`
  VHDL interface so the choice is a flag, not a fork.

#### 5.7.4 Simulation integration note

No open simulator covers mixed Verilog + VHDL in one run. Helion should document
that: SV designs go to Verilator, VHDL designs go to GHDL/nvc, mixed designs need
a split testbench or a Verilog/VHDL boundary model. Icarus Verilog was not the best
pick here (slower, Verilog-2005 + partial SV) but is useful for quick event-driven
testbenches with delays; keep it as a fallback, not a primary.

### 5.8 Encrypted Xilinx IP (IEEE 1735): what to do

- Best source: https://arxiv.org/abs/2112.04838?context=cs ; PDF:
  https://arxiv.org/pdf/2112.04838 - Speith et al., "How Not to Protect Your IP:
  An Industry-Wide Break of IEEE 1735 Implementations" (2021/2022). It
  demonstrates recovery of the private keys of IEEE 1735 implementations from
  Intel, Xilinx, Cadence, Siemens, Microsemi, and Lattice, and states the authors
  can "decrypt, modify, and re-encrypt all allegedly protected IP cores designed
  for the respective tools". It argues the standard is structurally insecure because
  the decryption module runs in an untrusted environment.
- What this does and does not mean for Helion: the paper proves the cryptographic
  protection is broken in practice by researchers with white-box key extraction. It
  does NOT give Helion a legal or practical open decryption path. Helion must not
  implement IEEE 1735 decryption, must not ship extracted vendor keys, and must not
  use the paper's attack to read Xilinx IP. That would be a legal and trust risk,
  and it would not be a stable engineering dependency. The correct reading is:
  encryption is a policy barrier, not a technical one, and Helion's answer is
  clean-room replacement, not decryption.
- Vendor confirmation (secondary, AMD support forum):
  https://adaptivesupport.amd.com/s/question/0D52E00006iHj2zSAC/source-code-license-available-for-10g25g-ethernet-subsystem-ip-core?language=en_US
  - AMD states "Xilinx encrypts their own IP using IEEE P1735 and almost all
  Xilinx IPs are in encrypted RTL form", with MIG as a notable clear-text exception,
  and that Xilinx does not add third-party synthesis-tool public keys. That is why
  open synthesis tools cannot read most Xilinx IP: the RTL is encrypted to Vivado's
  (and the listed simulators') keys only. A related AMD thread confirms Vivado
  itself refuses an envelope without a Xilinx key: "This program cannot decrypt an
  IEEE-1735 envelope without a Xilinx key."
- Practical strategies for Helion (in priority order):
  1. Clean-room reimplementation behind the same interface. This is the
     LiteDRAM/LitePCIe/LiteScope strategy above: reimplement the function, keep
     the AXI/AXI-Stream/Wishbone interface identical, so user RTL does not change.
  2. Stub / black-box + board-level integration. For IP Helion cannot reimplement
     (e.g. a complex hard-block wrapper), emit a black box with the exact port list
     and let the user supply the Vivado-generated netlist or a board-level module.
     Helion must pass the black box through synth/place/route without optimising it
     away, and must error loudly if the stub is not replaced before bitstream
     generation.
  3. Use Xilinx's own free IP only where licensed, via Vivado. Where a design
     genuinely needs a Vivado-only IP and the user has a Vivado licence, Helion
     should support a "Vivado IP" import path (import the generated
     netlist/checkpoint, do not try to read the encrypted RTL). Document that this
     path is not open and not portable.
  4. Document IP interfaces (AXI) so users can swap in open cores. `helion-ipxact`
     should describe every Xilinx IP Helion touches by its AXI/AXI-Stream
     interface, parameters, and protocol, with a mapping table: Xilinx IP -> open
     replacement -> status (ready / in progress / no replacement). This is the
     cheapest unblocker for users.
  5. Do not decrypt. State this as policy in the repo: Helion does not implement
     IEEE 1735 decryption and does not accept decrypted Xilinx IP contributions.
- Helion crate: `helion-ipxact` (interface docs and mapping table), `helion-lib`
  (black-box handling), `helion-xilinx7` (hard-block models).
- Expected gain: high (unblocks real designs without legal risk). Effort: S for the
  policy + mapping table; M-XL per clean-room core. Risk: Low for the policy;
  Medium-High per core (see sections 5.1-5.3).
- Next steps: (1) Write the IEEE 1735 policy note in the repo and the IP mapping
  table in `helion-ipxact`. (2) Implement black-box passthrough with a loud error
  if a stub reaches bitgen. (3) Prioritise clean-room cores by user demand (DDR,
  PCIe, and ILA first, per sections 5.1, 5.2, 5.4).

### 5.9 Other notable open Vivado replacements

#### 5.9.1 LiteX ecosystem in general (umbrella)

- Best source: https://www.ensta-bretagne.fr/lelann/papers/litex.pdf (LiteX paper:
  SoC builder and IP library, Migen Python DSL). LiteX itself is the SoC builder;
  the cores above are its library. Other LiteX cores in the paper's table that
  Helion should track: LiteEth (Ethernet MAC/PHY, UDP/IP, up to 1 Gbps), LiteSATA
  (SATA 1/2/3, DMA, RAID/mirroring, up to 6 Gbps), LiteUSB (USB 2.0/3.0 slave FIFO
  + DMA), LiteSDCard (SD, DMA, up to UHS-1, 55 MB/s R/W), LiteVideo (HDMI RX/TX,
  framebuffer, up to 1080p60), LiteJESD204B (TX, Subclass 1, up to 10 Gbps). All
  are BSD-2-Clause LiteX cores; each is a candidate Helion IP block by the same
  generate-Verilog pattern. Individual repo URLs for
  LiteEth/LiteSATA/LiteUSB/LiteSDCard/LiteVideo/LiteJESD204B were not verified in
  this pass; do not cite them from memory.
- Helion crate: `helion-ipxact` (IP catalogue), `helion-device` (per-board
  availability).
- Expected gain: high as a catalogue. Effort: S per core to catalogue; M per core
  to integrate. Risk: Low for cataloguing.
- Next step: build the Helion IP catalogue from the LiteX core list, marking each
  as ready / needs work / no 7-series PHY.

#### 5.9.2 Open AXI infrastructure

- Best pick (SystemVerilog, most complete): PULP AXI. Source:
  https://github.com/pulp-platform/axi/blob/master/axi.core . AXI4 and AXI4-Lite
  synthesizable IP in SystemVerilog, hand-optimised, with `axi_xbar`
  (crossbar/interconnect), `axi_demux`/`axi_mux`, CDC, width converters, protocol
  converters, and a generator (`axi_intercon_gen.py`) that builds an interconnect
  wrapper from a config. Licence SHL-0.51 (per the .core file). The PULP
  presentation claims it "can compete with industry-grade solutions" (their claim,
  not independently verified here):
  https://www.pulp-platform.org/docs/nocs2021/axi_talk_v2_unrolled.pdf .
  Helion crate: `helion-ipxact` (AXI interconnect generation), `helion-lib`.
  Gain: high. Effort: M (SystemVerilog, Bender/IPApproX dependency handling,
  generator integration). Risk: Medium (licence SHL-0.51 needs a review; the
  generator is Python).
- Alternative (Verilog, formally verified, GPLv3): ZipCPU wb2axip. Source:
  https://raw.githubusercontent.com/ZipCPU/wb2axip/master/README.md . Wishbone/AXI/
  AXI-Lite bridges and interconnects, designed for 100% throughput with no
  overhead, most cores formally verified. The README claims up to 16x speedup
  toggling I/O vs Xilinx's AXI-Lite slave and 4x vs Xilinx's block-RAM controller
  on single-beat transactions (author's measurements, not independently verified).
  GPLv3 is a licensing constraint if Helion vendors the RTL; using it as an
  external generated core is a different question and needs a licence review.
  Helion crate: `helion-ipxact`. Gain: medium. Effort: S-M. Risk: Medium (GPLv3,
  and the README notes the AXI4-to-Wishbone bridge was "known to be broken" at that
  writing).
- LiteX interconnect: LiteX's own Wishbone/AXI crossbar and stream infrastructure
  is the third option, and the most consistent with LiteDRAM/LitePCIe/LiteScope.
  Prefer LiteX interconnect for LiteX cores and PULP AXI where a full AXI4 crossbar
  is needed. Do not build a Helion AXI crossbar from scratch.
- Next steps: (1) Choose PULP AXI as the default AXI4 interconnect and
  LiteX/Wishbone as the default for LiteX cores; document the split.
  (2) Generate one PULP `axi_xbar` and one wb2axip bridge through Helion synth as
  black boxes. (3) Get a licence review on SHL-0.51 (PULP) and GPLv3 (wb2axip)
  before vendoring either.

#### 5.9.3 Clocking (MMCM/PLL), block RAM / FIFO generators

- Status: no verified open MMCM/PLL wizard was found in this pass. What is verified
  is that LiteX designs use vendor clock primitives directly (e.g. the USB3 Pipe
  Acorn design uses a Xilinx `S7PLL` to generate its clock domains, per the
  DeepWiki summary of that design). That is the pattern Helion should follow: model
  the MMCM/PLLE2 primitives in `helion-device` / `helion-xilinx7`, provide a
  clocking IP block that instantiates them with user parameters (multiply/divide,
  phase, duty cycle), and validate the output frequencies in simulation. Do not
  claim a Clocking Wizard replacement until the primitive models and the timing
  checks exist.
- Block RAM / FIFO: no separate open BRAM/FIFO generator is needed as a first step:
  Helion's own synth already infers BRAM from RTL, and PULP `common_cells` (a
  dependency of PULP AXI, cited in its .core file) provides reusable RTL primitives
  including FIFOs. LiteDRAM/LitePCIe also carry their own FIFOs. If a standalone
  FIFO/BRAM generator is wanted later, evaluate PULP `common_cells` first. Repo URL
  for common_cells [UNVERIFIED] in this pass.
- Helion crate: `helion-device` + `helion-xilinx7` (clock primitives),
  `helion-ipxact` (clocking/FIFO IP blocks).
- Expected gain: medium. Effort: M for clocking IP; S for FIFO cataloguing.
  Risk: Medium (clocking errors are silent until hardware).
- Next steps: (1) Add MMCM/PLL primitive models and a clocking IP block with
  frequency validation. (2) Catalogue PULP `common_cells` FIFOs as Helion IP
  blocks. (3) Do not build a Clocking Wizard UI until the primitive models are
  validated on hardware.

### 5.10 Vivado extras: cross-cutting notes

- Licence hygiene: LiteX cores are BSD-2-Clause (permissive). Bambu is now
  Apache-2.0 WITH LLVM-exception per its current README (older sources say GPLv3;
  re-check at integration). Verilator is LGPL-3.0/Artistic, GHDL GPL-2.0, nvc
  GPL-3.0, Wupper LGPL, PULP AXI SHL-0.51, wb2axip GPLv3. Helion is
  AGPL-3.0-or-later. The safe pattern throughout is: invoke external tools and
  consume generated Verilog/VHDL, do not vendor GPL-licensed RTL into Helion's
  tree without a licence review.
- Verification discipline: every throughput/area number above that came from a paper
  or a project README is labelled as such. Helion should re-measure each one on its
  own boards before publishing it as a Helion number, especially LiteDRAM
  random-access efficiency, LitePCIe DMA throughput, and any power figure.

---

## 6. Ranked roadmap: everything by gain per effort

Ranking rule: gain per effort, with correctness risk as a tie-breaker (a cheap
win that can silently miscompile ranks below a slightly dearer safe win).
"Gain" below is the projected effect on the README targets in section 0, not a
paper number. Dependencies are hard: do not start a phase whose dependency is
unmeasured.

### 6.1 Master ranking table

| Rank | Item | Section | Gain | Effort | Risk | Unblocks |
|---|---|---|---|---|---|---|
| 1 | Cell arcs + clock/CPR/uncertainty audit, prjxray SDF import | 2.5, 2.2 | Largest timer gain; may explain both mmcm +2.46 ns and ibex -7 ns | S-M | Low-Med | Timer target, all Fmax work |
| 2 | Inference on elaborated IR + datapath canonicalization + popcount/rotate cells | 1.7, 1.8.1, 1.8.3 | Fixes 7.5x and 3.2x outliers; DSP/BRAM/carry parity on micro units | M | Med | Size p90, G127/G128 tail |
| 3 | ABC-as-subprocess experiment + inference audit on 10 worst designs | 1.5.3 | Same-week upper bound on mapper value; decides whether to port ABC | S-M | Low | Mapper decision |
| 4 | Timing-driven placement (VPR lambda cost + slack weights) | 3.2.1, 3.2.2 | Projected 10-20% Fmax, largest single placement gain | S-M | Low-Med | 85% Fmax |
| 5 | Fanout replication, post-place, critical nets only | 3.4.1 | Projected 10-20% Fmax on PicoRV32; fixes the fanout-85 path | M | Med | 85% Fmax |
| 6 | Fitted net delay, linear least squares per resource, then GBM | 2.1 | Per-net about 1-2% if it transfers; closes mmcm/bram/pico slack | M-L | Med | Timer per-net target |
| 7 | PathFinder audit: per-connection criticality, per-iteration STA | 3.3.1 | Projected 5-15% Fmax if router is not yet timing-driven; 0% if it is | M | Med | 85% Fmax |
| 8 | Priority-cut 6-LUT mapper + rewrite/balance + area recovery | 1.1, 1.6.1, 1.9 | Median 5-15% LUTs | L | Med | Size median 1.02x |
| 9 | RapidWright device-data verification for unlicensed UltraScale+ parts | 4.7.3 | Cheapest possible unlock for unlicensed geometry | M | Med | UltraScale+ coverage |
| 10 | LiteDRAM + LitePCIe integration (generate Verilog, black-box synth) | 5.1, 5.2.1 | Unblocks DDR and PCIe designs end-to-end | M | Med | Vivado extras |
| 11 | LiteScope integration + Verilator/GHDL sim paths | 5.4.1, 5.7 | ILA replacement + fastest open SV/VHDL sim | S-M | Low-Med | Debug, verification |
| 12 | openXC7 diff harness + prjxray-db fork audit | 4.1, 4.7.2 | Closes 7-series gaps on unlicensed parts from published data | S-M | Low-Med | 7-series coverage |
| 13 | Connection-based rip-up + hold fixing | 3.3.3, 3.6.1 | Route time 2-3x; hold is a correctness gap | M | Low-Med | Ibex under 600 s |
| 14 | Speed-grade per-class scaling tables | 2.6 | Small if -1 is right; material if scaling is missing | S | Low | Timer on -2/-3 parts |
| 15 | UTPlaceF-style quadratic global placement + congestion-aware detail | 3.1.4 | Projected 5-10% Fmax | L | Med | 95% Fmax |
| 16 | Post-route phys_opt loop | 3.4.3 | Projected 5-10% additional Fmax | L | Med | 95% Fmax |
| 17 | Bambu HLS wrapper (thin, consume Verilog output) | 5.6.1 | C/C++ entry point without building an HLS compiler | M | Med | HLS |
| 18 | Choice networks / LMS / `mfs` (each gated on its own prototype) | 1.2, 1.6.4, 1.6.3 | 6-19% LUTs at 1.6-8x runtime; `mfs` 10-12% on industrial scale | L-XL | Med-High | Size p90 after 1-8 |
| 19 | IEEE 1735 policy + IP mapping table + black-box passthrough | 5.8 | Unblocks real designs without legal risk | S | Low | All IP work |
| 20 | Helion UltraScale+ fuzzer harness on xcku3p/xcku5p, Bitfiltrator-guided | 4.2, 4.8 | The real UltraScale+ lane; no shortcut exists | L | High | UltraScale+ bitstreams |
| 21 | Provenance-flagged carry-over table | 4.8 | Must land before any carried-over row ships | M | High if skipped | UltraScale+ coverage |
| 22 | Dual-output LUT6_2 merging in packer | 1.9 | About 10% LUT-site reduction | M | Low | Size median |
| 23 | Multiplier tiling for sub-10-bit LUT multiplies | 1.8.2 | 10-30% on sub-10-bit multiply units only | L | Med-High | Size tail |
| 24 | Wupper Gen3 PCIe + PULP AXI interconnect | 5.2.2, 5.9.2 | Gen3 on 7-series; full AXI4 crossbar | M-L | Med-High | PCIe Gen3, AXI |
| 25 | Retiming (safe subset first, post-place only) | 3.4.2 | Projected 5-15% Fmax, only post-placement | L-XL | High | 95% Fmax |
| 26 | ML delay prediction (GBM on route features; GNN only if GBM wins) | 2.4 | Incremental over rank 6 | M-XL | Med-High | Timer p95 |
| 27 | Power: activity dumping first, then calibrated estimate | 5.5 | +/-20-30% estimate with honest error bars | M | High if unvalidated | Power reporting |
| 28 | elfPlace / OpenPARF electrostatics, DREAMPlace GPU, parallel routing | 3.1.3, 3.1.5, 3.1.2, 3.3.5 | 5-10% Fmax / runtime only | XL | High | Defer until serial flow is correct |
| 29 | Dynamatic / CIRCT as HLS infrastructure | 5.6.2, 5.6.3 | Irregular-kernel wins / long-term infra | L-XL | High | HLS, long term |
| 30 | Silicon RO timing fuzzing; Versal bitstream work | 2.3, 4.6 | About none vs Vivado-parity goal; no public prior art for Versal | L-XL | High | Do not start |

### 6.2 Phases

#### Phase 0: measure before building (1 week, no code that changes QoR)

- Reproduce the hdlfactory PicoRV32 comparison with Helion's own PicoRV32
  (Helion vs Yosys `synth_xilinx` vs Vivado, same device, same Vivado version,
  one table: Fmax, LUTs, route time, hold violations) - sections 1.5.2, 3.6.1.
- ABC-as-subprocess experiment on the 10 worst p90 designs - section 1.5.3.
- Audit `helion-sta` for the four non-net terms (clock delay min/max, CPR, clock
  uncertainty, FF setup sign) and decompose mmcm and ibex slack error into
  logic / route / clock / uncertainty - section 2.5.
- Audit the current router: per-connection vs per-net criticality, STA
  recomputed per iteration - section 3.3.1.
- Verify RapidWright device data for xcvu3p/xczu9eg/xcku040 by inspection -
  section 4.7.3.
- Measure to pass: a written baseline table for all of the above, with job IDs and
  file paths per the README rules. No phase 1 work starts on a guess.

#### Phase 1: timer correctness + inference (the two P0s, in parallel)

- Cell arcs: import prjxray-db SDF, diff per BEL/pin, fix clock/CPR/uncertainty,
  micro-design calibration for CARRY4/BRAM/DSP/MMCM-BUFG - section 2.5.
- Net delay: route-feature extractor, Liu/Kapre least-squares fit per wire class
  and part, held-out-design validation - section 2.1.
- Inference: canonical datapath pass in `helion-ir`, one decision point after
  operand normalization, Vivado threshold table, popcount/rotate cells -
  sections 1.7, 1.8.1, 1.8.3.
- Speed-grade tables from existing data - section 2.6.
- IEEE 1735 policy + IP mapping table + black-box passthrough - section 5.8.
- Measure to pass: slack within 0.2 ns of Vivado on the five named designs, never
  more than 0.1 ns optimistic; per-net median at most 5%, p95 at most 15% on
  held-out designs; popcount32 and `h_rot_mm` within 1.2x Vivado; DSP/BRAM/carry/FF
  equal on every micro unit. If the timer target fails, do not start phase 2
  Fmax work on a wrong timer.

#### Phase 2: Fmax to 85% (placement, replication, router)

- Timing-driven placement + slack-based net weights - sections 3.2.1, 3.2.2.
- Fanout replication, post-place, critical nets only - section 3.4.1.
- PathFinder audit/fix + delay-model validation vs Vivado - sections 3.3.1, 3.3.4.
- Connection-based rip-up + hold fixing - sections 3.3.3, 3.6.1.
- Clock-aware legalization (ISPD 2017 lesson) - section 3.5.2.
- Measure to pass: PicoRV32 at least 85% of Vivado Fmax (109.5 MHz), Ibex at least
  85% (33.1 MHz), mmcm slack at least 85% of Vivado's, route time PicoRV32 under
  60 s, Ibex under 600 s, zero hold violations. Gate is Vivado-timed Fmax, not HPWL.

#### Phase 3: size median to 1.02x (mapper)

- Priority-cut 6-LUT mapper + rewrite/balance + area recovery - sections 1.1,
  1.6.1, 1.9.
- EPFL gate wired into the test harness - section 1.9.
- Dual-output LUT6_2 merging in `helion-pack` - section 1.9.
- Multiplier tiling for sub-10-bit LUT multiplies - section 1.8.2.
- Measure to pass: EPFL LUT-6 within 10% of ABC `if -K 6` per design; corpus
  median at most 1.05x, p90 at most 1.5x; no design over 2x Vivado. Then push to
  the README targets (median 1.02x, p90 1.30x, corpus +5%).

#### Phase 4: Fmax to 95% + device coverage (in parallel, different owners)

- UTPlaceF-style quadratic global placement - section 3.1.4.
- Post-route phys_opt loop - section 3.4.3.
- Retiming, safe subset first, post-place only, with equivalence checking -
  section 3.4.2.
- UltraScale+: openXC7 diff harness, prjxray-db fork audit, RapidWright export
  diff on xcku3p, provenance-flagged carry-over table, Helion fuzzer harness on
  xcku3p/xcku5p (Bitfiltrator-guided) - sections 4.1, 4.7.2, 4.7.3, 4.8.
- Measure to pass: Fmax at least 95% of Vivado on PicoRV32/Ibex/bram/mmcm; one
  UltraScale+ part with fuzzed (not carried-over) INT/CLE/BRAM/DSP segbits and a
  board-verified bitstream.

#### Phase 5: Vivado extras (in parallel with phases 2-4, separate lane)

- LiteDRAM + LitePCIe as generated-Verilog IP blocks, board-validated -
  sections 5.1, 5.2.1.
- LiteScope + Verilator/GHDL sim paths in `helion-sim` - sections 5.4.1, 5.7.
- PULP AXI interconnect + clocking IP block - sections 5.9.2, 5.9.3.
- Bambu thin wrapper in `helion-hls` - section 5.6.1.
- Activity dumping in `helion-sim`, then a calibrated power estimate with a
  measured error bar - section 5.5.
- Measure to pass: one board-tested DDR design (memtest pass), one board-tested
  PCIe DMA throughput number, one LiteScope capture read back over UART, 3-5
  Bambu kernels through Helion synth with LUT/FF/DSP recorded.

#### Phase 6: high-effort and deferred (only after phases 1-3 are measured)

- Choice networks, LMS, `mfs` - each gated on its own subprocess prototype showing
  more than 5% on the tail - sections 1.2, 1.6.4, 1.6.3.
- ML delay prediction - only if the section 2.1 linear fit misses 5%/15% -
  section 2.4.
- Wupper Gen3 PCIe, Dynamatic/CIRCT HLS infrastructure - sections 5.2.2, 5.6.2,
  5.6.3.
- Analytic electrostatics, GPU placement, parallel routing - sections 3.1.3,
  3.1.5, 3.1.2, 3.3.5.
- Do not start: silicon RO timing fuzzing, Versal bitstream work, GT wizard,
  LegUp, RIFFA modernization - sections 2.3, 4.6, 5.3, 5.6.4, 5.2.3.

### 6.3 Dependencies between phases

- Phase 1 timer work gates phase 2: a timing-driven placer and router that optimize
  a wrong delay model will optimize the wrong thing. The RWRoute calibration (90%
  of Vivado at 5x speed with a 2%-accurate model) is the proof that the model comes
  first.
- Phase 1 inference gates phase 3: tiling or mapping a multiply that should have
  been a DSP is wasted work, and the mapper cannot fix a missing DSP/BRAM decision.
- Phase 2 placement gates phase 4 retiming: retiming is only meaningful
  post-placement with accurate wire delays (Singh/Brown: 19% better clock periods
  vs retiming before place-and-route).
- Phase 0 measurement gates everything: the ABC subprocess result decides whether
  to port a mapper at all, and the router audit decides whether the router needs a
  rebuild or a cost-function fix.
- Device coverage (phase 4) is independent of QoR phases except for one point: the
  carry-over table must land before any carried-over row ships, and the openXC7
  diff harness is the regression gate for every new segbits row on 7-series.

### 6.4 What to measure after each phase

- After every phase: full 309-design sweep on xc7z020 (median, p90, over-1.5x
  counts per resource), the five named designs (blinky, pico, ibex, bram, mmcm)
  with Vivado-timed Fmax and Helion slack error, and the EPFL LUT-6 table.
  Report the hash, the job IDs, and the summary lines, per the README rules.
- After phase 1: per-net median/p95 error on held-out designs, split by wire
  class, fanout bucket, and clock vs data; slack error decomposed into logic /
  route / clock / uncertainty.
- After phase 2: worst-path wirelength before/after, WNS/TNS/failing endpoints
  per phys_opt-style pass, hold violations (must be zero), route time per design.
- After phase 3: per-design LUT ratio vs Vivado for the full ranked list
  (`data/helion_vs_vivado_xc7z020.csv`), with the worst 10 designs named.
- After phase 4: UltraScale+ segbits provenance report (fuzzed vs carried-over vs
  RapidWright, per tile type), board verification results.
- After phase 5: board-measured numbers only (memtest, DMA throughput, LiteScope
  capture); paper figures are not Helion numbers.

---

## Appendix A. Things marked [UNVERIFIED] in this document, and what would verify each

- The 1.1M measured net-delay row count and the 8-part coverage: verify from the
  cluster `net_delays.tsv` files (not in this repo).
- STATUS_MUSE.md on `fm/qor-plan`: verified absent on 2026-11-28; it lives on the
  `fm/qor-size` branches instead.
- Any single segbits-completeness percentage for prjxray-db, prjuray-db, or any
  other DB: no upstream source publishes one; verify by auditing the DB
  directories per tile type per part.
- The exact solver (SAT/ILP/other) prjxray uses for PIP disambiguation: read
  `prjxray/lib` and the fuzzer code.
- Whether RapidWright's shipped device data includes xcvu3p, xczu9eg, xcku040 and
  whether loading it needs a Vivado license: inspect a RapidWright install.
- Whether Ablation-Tool/ablation parses Versal PDI fabric bits: read the repo.
- The ConFuzz repository URL: only the paper PDF was verified.
- The UG902 page for the 10-bit DSP rule (quoted as 2017.4 p.181 from a forum
  answer): open the UG902 PDF.
- The Lu dual-output 34.96%/23.39% numbers (arXiv 2607.22756): full read of the
  paper; the arXiv identifier date is anomalous.
- The FAPO numbers (arXiv 2610.06937): full read of the paper; the arXiv
  identifier date is anomalous.
- The Dynamatic experience paper numbers (arXiv 2603.19856): full read; the arXiv
  identifier date is anomalous.
- ABC's license for vendoring, and the berkeley-abc/abc GitHub mirror URL: open
  the repo and its license file.
- Whether elfPlace or OpenPARF source is currently public and buildable: clone and
  build.
- Whether PreRoutGNN code is live at the GitHub URL stated in its paper; LSTP
  exact MAPE: open the repo / read the full paper.
- The provenance of prjxray's wire R/C and intrinsic values (fitted from Vivado
  SDF vs measured) and the contents of `fuzzers/007-timing/bel` and
  `routing-bels` beyond their names: read the sources.
- Any published universal 7-series speed-grade derating factor for fabric delays;
  any published nextpnr-xilinx vs Vivado slack accuracy number; any published
  nextpnr-xilinx vs Vivado Fmax comparison on Ibex: none was found; a new
  measurement would be needed.
- RIFFA repo/licence/current state; Calyx and Dynamatic repo URLs;
  LiteEth/LiteSATA/LiteUSB/LiteSDCard/LiteVideo/LiteJESD204B individual repo URLs;
  PULP `common_cells` repo URL; LiteScope current trigger expressiveness; any
  7-series power accuracy number: all need a direct source check before citing.
- Vendor marketing claims (e.g. Vivado 2025.2 8-13% Fmax from retiming): vendor
  marketing, not an independent measurement.
- The exact last-commit dates for prjxray/prjuray beyond the F4PGA org listings
  (Jun 5, 2025 and Feb 9, 2022) and the openXC7 fork dates (Nov 17/21, 2026)
  quoted from search listings: verify from the repos.

## Appendix B. Key sources index (one line each)

- Priority cuts: https://people.eecs.berkeley.edu/~alanmi/publications/2007/iccad07_map.pdf
- Choice networks: https://janders.eecg.utoronto.ca/pdfs/area.pdf
- Yosys abc9: https://yosyshq.readthedocs.io/projects/yosys/en/0.44/yosys_internals/extending_yosys/abc_flow.html
- Yosys synth_xilinx: https://yosyshq.readthedocs.io/projects/yosys/en/latest/cmd/index_techlibs_xilinx.html
- ABC: https://people.eecs.berkeley.edu/~alanmi/abc/abc.htm
- DAG-aware rewriting: https://people.eecs.berkeley.edu/~alanmi/courses/2007_290N/papers/synthesis_berkeley_dac06.pdf
- mfs: https://people.eecs.berkeley.edu/~alanmi/publications/2011/trets11_mfs.pdf
- LMS: https://people.eecs.berkeley.edu/~alanmi/publications/2012/iccad12_lms.pdf
- Basilisk: https://arxiv.org/html/2405.04257v1/
- Yosys memory passes: https://yosyshq.readthedocs.io/projects/yosys/en/latest/cmd/index_passes_memory.html
- UG901: https://docs.amd.com/r/en-US/ug901-vivado-synthesis/Using-Synthesis-Settings?contentId=y9h9MoDtUMgkk2CTv6aDNg
- Multiplier tiling: https://arxiv.org/pdf/2405.02047
- Popcount: http://fpgacpu.ca/fpga/Population_Count.html
- EPFL benchmarks: https://www.epfl.ch/labs/lsi/page-102566-en-html/benchmarks/
- RapidWright timing model: http://www.rapidwright.io/docs/_downloads/FPT19-TimingModel.pdf
- Liu/Kapre calibration: http://nachiket.github.io/publications/rapidroute-timing_fpl-2019.pdf
- prjxray timing.py: https://foss-fpga-tools.googlesource.com/prjxray/+/refs/heads/dependabot/submodules/third_party/python-sdf-timing-ea99ca8/prjxray/timing.py
- PreRoutGNN: https://arxiv.org/abs/2403.00012v1
- ASAP: https://personal.hkust-gz.edu.cn/yuzhema/papers/C32-MLCAD2023-MTL-RTL.pdf
- UG906: http://docs.amd.com/r/2020.2-English/ug906-vivado-design-analysis/Timing-Path-Summary-Header-Information
- RePlAce / OpenROAD gpl: https://openroad.readthedocs.io/en/latest/main/openroad/gpl/README.html
- elfPlace: https://doi.org/10.1109/tcad.2021.3053191
- UTPlaceF: http://www.cerc.utexas.edu/utda/publications/C199.pdf
- OpenPARF: https://arxiv.org/abs/2306.16665v1
- VTR 8: https://m.aarongraham.ca/vtr8_trets.pdf
- Kong PATH: https://cecs.uci.edu/~papers/compendium94-03/papers/2002/iccad02/pdffiles/03b_1.pdf
- PathFinder: https://cgi.cse.unsw.edu.au/~cs4601/refs/papers/fpga95mcmurchie.pdf
- RWRoute: https://api.crossref.org/works/10.1145%2F3491236
- ISPD 2016: http://www.ispd.cc/contests/16
- hdlfactory openxc7 vs Vivado: https://www.hdlfactory.com/post/2026/10/08/openxc7-vs-vivado-build-times/
- FPL 2026 contest: https://xilinx.github.io/fpl26_optimization_contest/details.html
- prjxray: https://github.com/f4pga/prjxray
- prjxray-db: https://github.com/f4pga/prjxray-db
- prjuray: https://github.com/f4pga/prjuray
- prjtrellis: https://github.com/YosysHQ/prjtrellis
- Apicula: https://github.com/YosysHQ/apicula
- Mistral: https://github.com/Ravenslofty/mistral
- IceStorm: https://github.com/YosysHQ/icestorm
- openXC7: https://github.com/openXC7/getting-started/blob/HEAD/README.md
- RapidWright: https://github.com/Xilinx/RapidWright
- prjoxide: https://github.com/gatecat/prjoxide
- Bitfiltrator: https://infoscience.epfl.ch/entities/publication/259a5b2a-b1de-48c3-8be8-f45c3a6fdde1
- LiteDRAM: https://github.com/enjoy-digital/litedram
- LitePCIe: https://github.com/enjoy-digital/litepcie
- Wupper: https://gitlab.nikhef.nl/franss/wupper/-/blob/e5dc1d4361f796c531b70bd3b068cb911cce301a/README.md
- LiteICLink: https://pypi.org/project/liteiclink/2024.12/
- LiteScope: https://pypi.org/project/litescope/2024.12/
- Bambu: https://github.com/ferrandi/PandA-bambu
- CIRCT: https://github.com/llvm/circt
- Verilator: https://verilator.org
- GHDL: https://github.com/ghdl/ghdl
- nvc: https://github.com/nickg/nvc
- IEEE 1735 break: https://arxiv.org/abs/2112.04838?context=cs
- PULP AXI: https://github.com/pulp-platform/axi/blob/master/axi.core
- LiteX paper: https://www.ensta-bretagne.fr/lelann/papers/litex.pdf
