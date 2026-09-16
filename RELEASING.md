# Cutting a Helion release

Same shape as other public CAD trees: bump the version, write the notes, tag,
push. GitHub Actions builds the artifacts and opens the GitHub Release.

1. Edit `CHANGELOG.md`. Move items out of `## [Unreleased]` into a new
   `## [X.Y.Z] — YYYY-MM-DD` section at the top (Keep a Changelog). Gold numbers
   in the notes must match `helion qor`. Keep an empty Unreleased stub for the
   next cycle. Align the release story with [`ROADMAP.md`](ROADMAP.md) and the
   open [milestones](https://github.com/helion-fpga/helion/milestones).
2. Set `workspace.package.version` in `Cargo.toml` to `X.Y.Z`.
3. Set `CFBundleShortVersionString` and `CFBundleVersion` in
   `packaging/macos/Info.plist` to the same string.
4. Commit on `master`. Example subject: `release: Helion X.Y.Z`.
5. Tag and push:

   ```bash
   git tag -a vX.Y.Z -m "Helion X.Y.Z"
   git push origin master
   git push origin vX.Y.Z
   ```

6. The `release` workflow builds:

   - `Helion-X.Y.Z-macos-arm64.zip` — `Helion.app` (Apple Silicon, unsigned)
   - `helion-X.Y.Z-aarch64-apple-darwin.tar.gz` — CLI + IDE + HAD + examples
   - `helion-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz` — Linux CLI + headless IDE
   - `SHA256SUMS.txt`

   and publishes them on
   [Releases](https://github.com/helion-fpga/helion/releases) with the changelog
   excerpt.

Do not invent a version that `Cargo.toml` does not carry. Do not attach vendor
bitstreams. Do not claim a signed/notarized Mac app until we have an Apple
Developer identity on the runner.

Release jobs also:

- require empty-XDC `examples/counter.sv` gold **WNS_PS=9640** on the built IDE
- strip release binaries (`CARGO_PROFILE_RELEASE_STRIP=symbols`)
- omit `examples/ip_ingest` from the app and tarballs (test corpus)
- fail if any artifact exceeds 100 MiB (`packaging/check-artifact-size.sh`)

`Helion.app` stores the IDE once (`Contents/MacOS/Helion`); `helion-ide` is a
wrapper that execs it.

## Latest stable vs next

- **Latest stable on GitHub Releases:** `v2.0.1` (Helion 2.0.1) — macOS app +
  Darwin/Linux tarballs. Do not recreate the tag; rebuild via `workflow_dispatch`
  only if artifacts must be replaced.
- **Next product tag:** whatever sits under `## [Unreleased]` in `CHANGELOG.md`
  when it ships. `CFBundleShortVersionString` must equal `Cargo.toml` on that
  commit.
- Gold: empty-XDC `examples/counter.sv` **WNS_PS=9640**. Stages / SOFT ≠ PASS:
  [`docs/stages.md`](docs/stages.md).
