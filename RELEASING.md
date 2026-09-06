# Cutting a Helion release

Same shape as other public CAD trees: bump the version, write the notes, tag,
push. GitHub Actions builds the artifacts and opens the GitHub Release.

1. Edit `CHANGELOG.md`. Put a new `## [X.Y.Z] — YYYY-MM-DD` section at the top
   (Keep a Changelog). Gold numbers in the notes must match `helion qor`.
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
