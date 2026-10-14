# Helion v3.0.0 vs Yosys 0.33: Synthesis Benchmark

**Date:** 2026-10-14

## Results

| Design | Helion | Yosys | Speedup |
|--------|--------|-------|---------|
| PicoRV32 | 1.02s, 31MB, 1697 LUTs | 9.12s, 63MB, 1932 LUTs | 8.9x |
| SERV | 0.06s, 16MB, 323 LUTs | 2.73s, 37MB, 217 LUTs | 45x |
| Ibex | 0.01s, 7MB, 1112 LUTs | 0.60s, 37MB, 1110 LUTs | 60x |

## Method

- `/usr/bin/time -v`, 3 runs each, median
- Helion: `helion synth <design>.v` (commit 072b2ce)
- Yosys: `yosys -p "read_verilog <d>.v; synth -top <top>; abc -lut 6; stat"`
- Same RTL, same 6-LUT target, synthesis only
