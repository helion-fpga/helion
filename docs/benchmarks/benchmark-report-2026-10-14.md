# Helion v3.0.0 vs Yosys 0.33: Synthesis Benchmark Report

**Date:** 2026-10-14
**Helion version:** 3.0.0 (commit 072b2ce)
**Yosys version:** 0.33 (git sha1 2584903a060)
**Benchmark stage:** Synthesis only, generic 6-LUT mapping

## Summary

| Design | Lines | Helion | Yosys | Speedup |
|--------|-------|--------|-------|---------|
| PicoRV32 | 3,049 | 1.02s, 31MB, 1697 LUTs | 9.12s, 63MB, 1932 LUTs | 8.9x |
| SERV | 2,960 | 0.06s, 16MB, 323 LUTs | 2.73s, 37MB, 217 LUTs | 45x |
| Ibex | 468 | 0.01s, 7MB, 1112 LUTs | 0.60s, 37MB, 1110 LUTs | 60x |

## Methodology

### Measurement tool
- `/usr/bin/time -v` (GNU time 1.9)
- Wall clock time from "Elapsed (wall clock) time"
- Peak memory from "Maximum resident set size"

### Runs
- 3 runs per design per tool
- Median value reported
- Cold cache for first run, warm for subsequent (not separated in this report)

### Helion command
```
helion synth <design>.v
```
- Binary: `./target/release/helion` (release profile, optimized)
- Input: SystemVerilog RTL directly
- Output: `synth_design <top> cells=<n> luts=<n>`

### Yosys command
```
yosys -p "read_verilog <design>.v; synth -top <top>; abc -lut 6; stat"
```
- Synthesis script: `synth` (generic), `-top` set to design top module
- LUT mapping: `abc -lut 6` for 6-input LUTs
- Statistics: `stat` for cell counts

### Target
- Generic 6-LUT mapping for both tools
- No device-specific packing (happens later in full flow)
- Same RTL input files, same top modules

## Per-design details

### PicoRV32
- **Description:** RISC-V RV32IMC core by Claire Wolf
- **Lines:** 3,049
- **Top module:** `picorv32`
- **Helion:** 1.02s wall, 31MB peak, 3030 cells, 1697 LUTs
- **Yosys:** 9.12s wall, 63MB peak, 3529 cells, 1932 LUTs
- **Speedup:** 8.9x faster
- **Memory:** 2.0x less
- **LUT delta:** Helion 12% fewer

### SERV
- **Description:** SErial RISC-V CPU, bit-serial implementation
- **Lines:** 2,960
- **Top module:** `serv_top`
- **Helion:** 0.06s wall, 16MB peak, 323 LUTs
- **Yosys:** 2.73s wall, 37MB peak, 217 LUTs
- **Speedup:** 45x faster
- **Memory:** 2.3x less
- **LUT delta:** Yosys 33% fewer

### Ibex (reduced)
- **Description:** Reduced Ibex RISC-V core (468-line subset)
- **Lines:** 468
- **Top module:** `ibex_top`
- **Helion:** 0.01s wall, 7MB peak, 1112 LUTs
- **Yosys:** 0.60s wall, 37MB peak, 1110 LUTs
- **Speedup:** 60x faster
- **Memory:** 5.3x less
- **LUT delta:** Tie (0.2% difference)

## System information

- **OS:** Linux (container)
- **CPU:** x86_64
- **Helion build:** `cargo build --release`, rustc 1.85
- **Yosys install:** apt `yosys 0.33-5build2`

## Reproduction

```bash
# Clone and build Helion
git clone https://github.com/helion-fpga/helion
cd helion
cargo build --release -p helion-cli

# Run benchmark script
~/workspace/helion-vs-yosys.sh third_party/picorv32/picorv32.v picorv32
```

Script location: `~/workspace/helion-vs-yosys.sh`
