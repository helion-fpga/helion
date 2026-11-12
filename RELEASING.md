# Cutting a Helion release

Releases are cut from `master` by a maintainer. Bump the version, finish the
notes, tag, push; GitHub Actions builds the artifacts and publishes the
GitHub Release.

## Before you tag

On a clean checkout of `master`:

```bash
cargo test --locked -p helion-rune -p helion-hw
cargo test --locked --workspace
cargo run --locked -p helion-rune --bin helion-ide -- --headless examples/counter.sv   # WNS_PS=9640
cargo run --locked -p helion-cli -- run examples/counter.sv --cycles 16              # LED[16]=0000000111111110
```

Check that the screenshots in `docs/screenshots/` show the current IDE and
that the README, website and [ROADMAP.md](ROADMAP.md) describe the release.

## Steps

1. **Changelog.** In [`CHANGELOG.md`](CHANGELOG.md), turn the unreleased
   section into `## [X.Y.Z] — YYYY-MM-DD` (Keep a Changelog), point its link
   at `compare/vPREV...vX.Y.Z`, and add an empty `## [Unreleased]` stub above
   it for the next cycle. Gold numbers in the notes must match `helion qor`.
2. **Version.** Set `workspace.package.version` in `Cargo.toml` to `X.Y.Z`
   and refresh `Cargo.lock`. `scripts/build-macos-app.sh` stamps the same
   version into `Helion.app`'s `Info.plist`, so there is nothing to edit
   there.
3. **Commit** on `master`, for example `release: Helion X.Y.Z`.
4. **Tag and push:**

   ```bash
   git tag -a vX.Y.Z -m "Helion X.Y.Z"
   git push origin master
   git push origin vX.Y.Z
   ```

5. The `release` workflow (Rust 1.99.1) checks that the tag matches
   `Cargo.toml`, then builds and publishes:

   | Artifact | Contents |
   |---|---|
   | `Helion-X.Y.Z-macos-arm64.zip` | `Helion.app` for Apple Silicon, ad-hoc signed |
   | `helion-X.Y.Z-aarch64-apple-darwin.tar.gz` | `helion`, `helion-ide`, `helion-prog`, HAD, examples |
   | `helion-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz` | The same for Linux x86_64 |
   | `SHA256SUMS.txt` | Checksums |

   The release body is the changelog excerpt for the tag
   (`scripts/changelog-excerpt.sh`) plus install notes, including the Linux
   IDE's runtime libraries (`libxkbcommon0`, `libwayland-client0`,
   `libusb-1.0-0`).
6. Check the published release: download an artifact, verify it against
   `SHA256SUMS.txt`, and run `helion-ide --version`.

## Rules

- Never invent a version that `Cargo.toml` does not carry.
- Never attach vendor bitstreams to a release.
- Do not call the macOS app signed or notarized: it is ad-hoc signed only,
  and users clear the quarantine flag once (`xattr -cr /Applications/Helion.app`).
  Notarization needs an Apple Developer ID on the runner.
