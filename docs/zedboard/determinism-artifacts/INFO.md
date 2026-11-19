# T3 determinism artifacts (Linux x86_64 rerun, 2026-11-19)

Worktree HEAD (fm/zedboard-determinism): d6be0763a6af2a5ec2e5375e7c886ce3bc69ef73
Base: fm/zedboard-merge 7ec3192
uname -sm: Linux x86_64
rustc: 1.99.1 (58cf8f9c3 2026-10-13) (from ~/.cargo/bin/rustc)
Binary: target/release/examples/zedbit, built with
`cargo build --release -j1 -p helion-xilinx7 --example zedbit` (exit 0,
7m 03s, log zedbit-build.log). No rebuild before the rerun.
HELION_XILINX_DB=$HOME/workspace/xilinx-dbsrc

md5s:
- blinky.bit e13faaf255dc16c7c9463d748a6b5956 (4,045,671 bytes), Mac d622e9831c0fa76faa5cbaec1aea2267: MISMATCH
- pico.bit afe395dd2583f1264d81b409765cb9a4 (4,045,664 bytes), Mac 08ca33a6ff702841ca8851cf20b023c4: MISMATCH

Files:
- blinky.bit, pico.bit: bitstreams from the rerun
- blinky.fasm (570 lines), pico.fasm (37975 lines): FASM zedbit wrote next to them
- blinky-run.log, pico-run.log: rerun logs, transcribed from
  docs/zedboard/determinism.md (see the NOTE header in each file)
- zedbit-build.log: the zedbit build log (exit 0)
- zedbit-runs-first-attempt.log: first attempt the same day, both runs exit 1
  at device load (zynq7/mapping missing), before the DB restore
- manifest.txt: database manifest, made with
  `cd ~/workspace/xilinx-dbsrc/zynq7 && find . -type f | sort | xargs md5sum > manifest.txt`
  (334 lines). Caveat: manifest.txt lists itself (the redirect creates it
  before find runs), so its own line is not meaningful. The manifest confirms
  the restored mapping files (parts.yaml 1d0dbc93, devices.yaml 40a5419f) and
  confirms no xc7z020clg400-1 entry anywhere in the tree.
