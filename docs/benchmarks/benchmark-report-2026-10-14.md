# Helion v3.0.0 vs Yosys 0.33: Synthesis Benchmark

**Date:** 2026-10-14
**Helion:** v3.0.0 (commit 072b2ce)
**Yosys:** 0.33 (git sha1 2584903a060)
**Stage:** Synthesis only, generic 6-LUT mapping

## Method

- Tool: `/usr/bin/time -v` (GNU time)
- Runs: 3 per design, median reported
- Helion command: `helion synth <design>.v`
- Yosys command: `yosys -p "read_verilog <d>.v; synth -top <top>; abc -lut 6; stat"`
- Same RTL input, same 6-LUT target, same synthesis stage for both tools

## Results

| Design | Helion time | Yosys time | Speedup | Helion mem | Yosys mem | Helion LUTs | Yosys LUTs |
|--------|-------------|------------|---------|------------|-----------|-------------|------------|
| PicoRV32 (3,049 lines) | 1.02 s | 9.12 s | 8.9x | 31 MB | 63 MB | 1,697 | 1,932 |
| SERV (2,960 lines) | 0.06 s | 2.73 s | 45x | 16 MB | 37 MB | 323 | 217 |
| Ibex (468 lines) | 0.01 s | 0.60 s | 60x | 7 MB | 37 MB | 1,112 | 1,110 |

## Read honestly

**Speed:** Helion is 8.9x to 60x faster wall-clock on identical RTL. The gap
widens on smaller designs where Yosys startup and ABC optimization dominate.

**Memory:** Helion uses 2x to 5x less peak RSS. Yosys loads heavier
optimization passes (ABC) that cost memory.

**Area (QoR):** Mixed.
- PicoRV32: Helion uses 12% fewer LUTs (1,697 vs 1,932).
- SERV: Yosys uses 33% fewer LUTs (217 vs 323). This is Helion's top
  optimization target.
- Ibex: Tie (1,112 vs 1,110).

**Caveats:**
- Yosys 0.33 is not the latest release. Results may differ with newer Yosys.
- Synthesis only. Full place-and-route comparison is separate work.
- Generic 6-LUT mapping. Device-specific packing happens later in the flow.
- The `ibex_reduced` input is 468 lines, not full Ibex. Labeled accurately here.

## Reproduction

```bash
# Helion
./target/release/helion synth third_party/picorv32/picorv32.v

# Yosys
yosys -p "read_verilog third_party/picorv32/picorv32.v; synth -top picorv32; abc -lut 6; stat"
```

Automated script: `~/workspace/helion-vs-yosys.sh`
