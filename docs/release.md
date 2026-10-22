# Releasing

Releases are built by [dist](https://axodotdev.github.io/cargo-dist) (cargo-dist). Pushing a
version tag runs `.github/workflows/release.yml`, which builds, checksums and publishes a
GitHub Release. Nothing else triggers it. dist matches tags with `**[0-9]+.[0-9]+.[0-9]+*`;
the `release-check` job then rejects anything other than exactly `v` + the Cargo version.

## Gates

All gates are `global-artifacts-jobs` in `dist-workspace.toml` and `plan-jobs` is empty on
purpose. dist's generated `host` job (it creates the GitHub Release) runs when what it needs
is `success` **or `skipped`**. A failing plan-job makes every later job skip, and `host` would
then run with no artifacts. A failing global-artifacts-job has result `failure`, which blocks
`host`. dist 0.32's `dist host` does not check that artifacts exist, so this ordering is the
guard. `scripts/test-release-gate.py` simulates the generated job graph and fails if `host`
could run after any gate fails (CI runs it in the lint job).

The gates run in parallel after every target's archive is built, before `host`:

- `./ci` runs the whole CI workflow (callable with `workflow_call`).
- `./release-check` runs `scripts/check-release-tag.sh`: tag == `v<Cargo version>` and
  `CHANGELOG.md` has a `## [<version>]` section. A wrong tag therefore fails after the builds,
  not before.
- `./release-smoke` downloads the artifacts on a native runner for each target (Linux x86_64
  and aarch64, macOS x86_64 and aarch64), installs the archive with `install.sh` from a local
  copy (verifying its sha256) and runs `scripts/smoke-ci.sh` on it. A binary that embeds the
  placeholder UI fails here.

## What is built

- Targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`,
  `aarch64-apple-darwin`.
- Per target: `superorchestrator-<target>.tar.xz` and `superorchestrator-<target>.tar.xz.sha256`,
  plus `sha256.sum` and the source tarball.
- Config: `dist-workspace.toml`. The `superorchestrator` package opts in with
  `[package.metadata.dist]` because the workspace sets `publish = false`.
- dist's own installers are off. Users install with `install.sh` (checksum verified).

## The web UI is embedded

`crates/so-web/build.rs` embeds `web/dist`, or a placeholder page when it is missing. Release
builds must therefore build the frontend first. `.github/dist/build-setup.yml` holds those steps
(pnpm, Node 24, `pnpm -C web install --frozen-lockfile`, `pnpm -C web build`); dist inserts them
before `cargo build` in every build job (`github-build-setup` in `dist-workspace.toml`).
The CI `e2e` job runs `scripts/smoke-ci.sh`, which fails if the binary serves the placeholder.

## Changing the pipeline

`release.yml` is generated. Edit `dist-workspace.toml` or `.github/dist/build-setup.yml`, then run
`dist generate` (and `dist plan` to preview the release) and commit the result. Do not hand-edit
`release.yml`.

## Local checks

- `sh scripts/test-install.sh`: installer against a fake local release.
- `scripts/smoke-ci.sh [binary]`: scripted smoke of a built binary with throwaway config.
- `scripts/check-release-tag.sh vX.Y.Z`: the tag gate.
- `python3 scripts/test-release-gate.py` (needs PyYAML): simulates `release.yml` to prove `host` is gated.
- `scripts/check-binary-size.sh target/release/superorchestrator`: binary size budget (52,000,000
  bytes: measured 41,603,680 plus 25%).
- `shellcheck install.sh scripts/*.sh` and `actionlint` (`.github/actionlint.yaml` ignores
  shellcheck findings in the generated `release.yml` only), and `cargo deny check` (`deny.toml`).
