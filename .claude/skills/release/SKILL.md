---
name: release
description: Cut, publish, verify, or roll back a superorchestrator release.
---

# Release superorchestrator

1. Read `docs/release.md` and follow its release procedure.
2. Check the current workspace version, existing tags and GitHub releases. Choose the next
   unused SemVer version; never reuse a version or tag.
3. Update `CHANGELOG.md` with `## [X.Y.Z] - YYYY-MM-DD` and the release notes. Bump
   `[workspace.package].version` in `Cargo.toml`, update `Cargo.lock` without changing unrelated
   dependency versions, and synchronize `plugin/.claude-plugin/plugin.json`.
4. Run `scripts/check.sh` and `scripts/check-release-tag.sh vX.Y.Z`. Open a PR to `main`, wait
   for checks, and merge it.
5. Switch to updated `main`, fast-forward it, and verify a clean tree and intended commit. Never
   create a release tag from a non-`main` commit.
6. Before pushing a tag, obtain explicit user consent. A pushed tag starts release CI and can
   create a GitHub Release; obtain explicit consent before any separate release creation too.
7. After consent, create and push the annotated `vX.Y.Z` tag. Monitor its workflow with
   `gh run watch`, inspect it with `gh release view`, install with `./install.sh X.Y.Z`, and run
   `superorchestrator --version`.
8. Verify the installed executable's build hash against the release binary and compare CLI and
   supervisor build IDs with `superorchestrator status`.
9. For a bad tag, remove it locally and remotely, then delete the associated GitHub draft/release
   as appropriate. If published, communicate the rollback and correct it with a higher patch
   version; never silently reuse the version. A tag deletion does not stop a running workflow.
