# superorchestrator

Delegate coding work across harnesses and accounts. See [SPEC.md](SPEC.md) for what it does.

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## Layout

- `crates/`: the Rust workspace. `superorchestrator` is the binary; the `so-*` crates hold the libraries.
- `web/`: the React + Vite + TypeScript frontend, embedded into the binary at release.
- `docs/`: architecture decision records and plans.

## Development

```sh
scripts/setup-hooks.sh   # once: enables the pre-commit and pre-push hooks
scripts/check.sh         # full gate; --rust-only or --web-only to narrow
```

The full check needs pnpm and Node 24 (see `web/.nvmrc`); use `scripts/check.sh --rust-only` without them.

Set `SKIP_PREPUSH=1` to bypass the pre-push gate (CI still runs it).
