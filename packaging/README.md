# Packaging

Release payload for `Helion.app` and the unix tarballs. Device facts stay in HAD;
this directory does not invent a second part database.

| Path | What |
|---|---|
| `macos/Info.plist` | `fpga.helion.ide`, arm64-only. `CFBundleShortVersionString` / `CFBundleVersion` must equal `workspace.package.version` in `Cargo.toml`. |
| `macos/AppIcon.png` | Placeholder icon. `.icns` via `iconutil` on a Mac; Linux CI does not claim that. |
| `check-artifact-size.sh` | Fail if a release file exceeds 100 MiB. Commercial CAD is tens of GB; Helion must stay small. |

`scripts/build-macos-app.sh` and `scripts/pack-unix-release.sh` copy HAD + examples
and skip `examples/ip_ingest` (ingest corpus, not a stranger-clone lab).
`Helion.app/Contents/MacOS/helion-ide` is a wrapper that execs `Helion` so the IDE
bits are stored once.

Gold: empty-XDC `examples/counter.sv` → `WNS_PS=9640`. Stages and SOFT ≠ PASS:
[`docs/stages.md`](../docs/stages.md). How to cut a tag: [`RELEASING.md`](../RELEASING.md).
