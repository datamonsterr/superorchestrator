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

## Cut a release

1. Choose the next [Semantic Version](https://semver.org/) (`MAJOR.MINOR.PATCH`). At the time
   this flow was added, the workspace is `0.1.0`, tag `v0.1.0` exists, `main` is 149 commits
   ahead of that tag, and there is no GitHub Release yet. Check the current branch, tags and
   releases before choosing a version; do not reuse an existing version or tag.
2. Add a `## [X.Y.Z] - YYYY-MM-DD` section near the top of `CHANGELOG.md`, above
   `## [Unreleased]`, with the user-facing changes for this release.
3. Set `version = "X.Y.Z"` under `[workspace.package]` in `Cargo.toml` and update the lockfile.
   Prefer `cargo update --workspace --offline`; if that cannot update only the workspace package
   versions from the local lockfile, edit those package version entries in `Cargo.lock` without
   changing dependency resolutions. Keep `plugin/.claude-plugin/plugin.json` at the same version:
   the release gate checks it too.
4. Run `scripts/check.sh`, then open a pull request to `main`. Wait for its checks and merge it.
   Cut the tag only after the merge, from the updated `main` commit:

   ```sh
   git switch main
   git pull --ff-only
   git status --short
   scripts/check-release-tag.sh vX.Y.Z
   git tag -a vX.Y.Z -m "Release vX.Y.Z"
   git push origin vX.Y.Z
   ```

   Confirm the working tree is clean and `HEAD` is the intended merged `main` commit before
   tagging. The tag push starts the release workflow and allows it to create a draft GitHub
   Release. Get explicit user consent before pushing a tag; the push triggers that publication.
5. Find the run for the pushed tag with `gh run list --workflow release.yml` and monitor it with
   `gh run watch RUN_ID`. Wait for all build and release gates to pass. Check the published
   release and install the pinned version:

   ```sh
   gh release view vX.Y.Z
   ./install.sh X.Y.Z
   superorchestrator --version
   ```

   The installer verifies the downloaded archive checksum. After upgrading, compare the
   installed executable's SHA-256 with the matching platform archive's binary (or otherwise
   verify its build hash) to confirm the installed build is the one from this release. Also run
   `superorchestrator status` and check the CLI and supervisor build IDs; restart the supervisor
   after active runs finish if they differ.
6. To roll back a bad tag, delete it locally and remotely, then delete or mark the associated
   draft/release as appropriate. For example, after confirming the tag is the bad release:

   ```sh
   git tag -d vX.Y.Z
   git push origin :refs/tags/vX.Y.Z
   gh release delete vX.Y.Z --yes
   ```

   If it is only a draft, deleting it removes the draft as well. If a release has already been
   published, communicate the rollback and publish a corrected higher patch version; do not
   silently reuse the deleted version. Deleting a tag does not cancel a workflow run already in
   progress.

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
builds must therefore build the frontend first. `.github/dist/build-setup.yml` is the template for
those steps (pnpm, Node 24, `pnpm -C web install --frozen-lockfile`, `pnpm -C web build`).
Because `allow-dirty = ["ci"]` stops dist from regenerating `release.yml`, the same steps and action
pins are mirrored into `release.yml` by hand, and the two files must stay in sync.
The CI `e2e` job runs `scripts/smoke-ci.sh`, which fails if the binary serves the placeholder.

## Changing the pipeline

`release.yml` is generated by dist, then hand-edited for supply-chain hardening: actions are pinned
to commit SHAs, the dist and rustup downloads are sha256-verified, and permissions are
least-privilege. `allow-dirty = ["ci"]` in `dist-workspace.toml` makes dist skip the file, so
`dist generate` neither updates nor checks it.

Pipeline changes therefore go into `release.yml` by hand. `.github/dist/build-setup.yml` is kept as
the template for the web-UI steps: mirror any change there into `release.yml`, and keep its action
pins identical. Use `dist-workspace.toml` for dist settings other than CI.

When bumping cargo-dist, update all three in lockstep: `cargo-dist-version` in
`dist-workspace.toml`, `DIST_VERSION` in `scripts/ci/install-dist.sh`, and that script's four
archive hashes from the release's `sha256.sum`. `scripts/test-release-gate.py` fails if the two
versions differ. Bump the action pins and the rustup version and its two checksums deliberately.

## Local checks

- `sh scripts/test-install.sh`: installer against a fake local release.
- `scripts/smoke-ci.sh [binary]`: scripted smoke of a built binary with throwaway config.
- `scripts/check-release-tag.sh vX.Y.Z`: the tag gate. It also checks that
  `plugin/.claude-plugin/plugin.json` carries the same version.
- `claude plugin validate plugin --strict`: validates the Claude Code plugin. Bump its `version`
  with the Cargo version.
- `scripts/smoke.sh`: a manual test on a real machine. It runs real harnesses and spends real
  quota, so it is never part of CI. Run it before tagging when you can.
- `python3 scripts/test-release-gate.py` (needs PyYAML): simulates `release.yml` to prove `host` is gated.
- `scripts/check-binary-size.sh target/release/superorchestrator`: binary size budget (52,000,000
  bytes: measured 41,603,680 plus 25%).
- `shellcheck install.sh scripts/*.sh scripts/ci/*.sh` and `actionlint`
  (`.github/actionlint.yaml` ignores shellcheck findings in `release.yml`, which is generated by dist
  and hand-edited, only), and `cargo deny check` (`deny.toml`).
