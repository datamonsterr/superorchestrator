# Task family routing, timeline and token efficiency

Deliver the improvements tracked in [task family routing](https://github.com/datamonsterr/superorchestrator/issues/53), [Claude token efficiency](https://github.com/datamonsterr/superorchestrator/issues/54), and [timeline and graph](https://github.com/datamonsterr/superorchestrator/issues/55) in one PR. Use isolated worktrees and review each implementation against its issue and the integrated branch.

| Task | Acceptance | Scope |
|---|---|---|
| Audit Claude sessions | Record the sample, deduplication method, cache and output counters, tool pairs, error categories, and a sanitized example for every improvement in the audit issue and its sub-issues. | Analyze prior project transcripts locally. Publish aggregate findings and sanitized examples. |
| Optimize MCP waits | Exercise both normal and quiet waits, including batched runs, questions, review, completion and timeout. Run `cargo test -p so-mcp -p so-supervisor --locked`. | Wait protocol, return behavior and delegation skill guidance. |
| Reduce MCP reply size | Test stat-only and full patches, cancellation receipts and opt-in free catalogs. Run `cargo test -p so-mcp -p so-supervisor --locked`. | MCP reply shape and supporting read handlers. Keep existing supervisor, CLI and web consumers working. |
| Improve failed calls | Test useful recovery for unavailable routing choices and unknown or ambiguous run IDs. Run targeted router and supervisor tests. | Diagnostics and usage guidance. Routing policy is part of the task family work. |
| Classify tasks and recommend models | Verify the OpenRouter Decisions request, task family preferences, eligibility, pins and legacy fallback. Run `cargo test -p so-core -p so-router -p so-agents --locked` and the CLI plugin skill tests. Document a matching workspace taxonomy and measured and estimated costs. | Reuse the live complexity request for classification. Keep workspace tagging as an opt-in admin setup. |
| Improve the timeline and add the graph | Verify filters, keyboard and pointer navigation, live updates, detail panel, and missing parents. Run `pnpm -C web lint`, `typecheck`, `test`, `build`, `size`, and browser smoke tests. | Existing web run data, Timeline and Graph views. |
| Stabilize and verify CI | Reproduce the reported fixture failures and verify portable binary lookup and deterministic test synchronization. | Test support for the Arch and macOS failures on the baseline CI run. |

Before merge, run formatting, workspace tests, Clippy and web checks against the integration branch, and verify all GitHub PR checks succeed. After merge, verify the related issues close and monitor the default-branch CI run.
