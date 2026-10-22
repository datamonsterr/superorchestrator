# Reviewing and landing runs

A run started with `review: true` stops at `pending_review` after its `checks` pass. The orchestrator decides what happens next.

## Review

`get_run(run, diff: true)` returns what the run changed, so you do not need to read its files:

- `diff.base`: the commit the run branched from.
- `diff.branch`: the run's branch (`so/<slug>-<id>`).
- `diff.stat`: `git diff --stat`, plus a line for each untracked file.
- `diff.patch`: committed and uncommitted changes, cut at 64 KiB (`diff.truncated` is `true` when it was cut).

From the terminal, use `superorchestrator runs show <run> --diff`. A run without a worktree has no `diff`.

## Accept, reject, land

- `reject_run(run, feedback)` sends the feedback to the same session and the run continues.
- `accept_run(run)` marks the run `succeeded`. The work stays on its branch. The result carries `branch` and `merge_command` so you can merge it.
- `accept_run(run, merge: true)` also merges the branch into the checkout the run was started from, then removes the worktree and branch. The merge is a fast-forward when possible, and otherwise a commit named `merge: <run title> (<short id>)`.

A merge is refused, and the run stays at `pending_review`, when:

- the base checkout is on another branch than the run started from;
- the base checkout has changes to tracked files (untracked files are fine);
- the run's worktree has uncommitted changes (they would not be merged);
- the merge conflicts. It is aborted, the base checkout is left as it was, and the error names the branch and the files.

Notes on the merge:

- It also refuses while a merge, rebase, cherry-pick or revert is in progress in the base checkout. That work is yours and is never aborted.
- Your repository's git hooks run (`pre-merge-commit`, `post-merge`, commit signing). The merge has no terminal: no editor, no password prompts, and a 120 second limit. A hook that refuses the merge, or a timeout, gives an error and the merge is undone.
- Accepting two runs into one repository at the same time is safe: the merges take turns. Accept and reject of the same run also take turns, so only one of them takes effect.
- If a run started while the base checkout had a detached `HEAD`, it is not merged. The error gives the command to run yourself.
- If `diff` cannot be made (for example the branch was pruned), `get_run` still works and `diff.error` says why.

For a run with isolation `none`, `merge: true` does nothing and says so in `note`. Nothing is merged without the flag, and nothing is pushed.
