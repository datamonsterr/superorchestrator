# The terminal UI

`superorchestrator` with no command, or `superorchestrator tui`, opens a full-screen terminal UI. It is the quickest way to see what is set up, manage accounts, watch runs and finish a first-run setup without leaving the terminal. Without a terminal (a pipe, a script, CI) the bare command prints the help instead, and `tui` exits with a usage error.

It needs a terminal of at least 60x16 and works best at 80x24 or larger. It reads the same files and database as the CLI, so everything it shows is also available from `status`, `account list` and `runs list`. It does not need the supervisor to be running for most things.

## Layout

A header (version, supervisor state, config directory), the tabs, the body for the current tab, a status line (progress, results, errors, or how old the data is) and the keys for the current view. The data reloads every 2 seconds.

| Key | Action |
| --- | --- |
| `Tab` / `Shift-Tab`, `1` to `4` | Switch tab |
| `j` / `k` or `Down` / `Up`, `g` / `G` or `Home` / `End`, `PgUp` / `PgDn` | Move the selection, jump to the top or bottom (or scroll an open run). Same in every list. |
| `R`, `F5` | Reload now |
| `?` | Help for the current tab |
| `q`, `Esc` | Close a pane, or quit from the top level |
| `Ctrl-C` | Quit |

## Tabs

**1 Overview.** Four counters (accounts, active runs, runs waiting for review, checklist progress), the setup checklist, the latest runs and a Levels block. The Levels block lists each level as `name  min-max  ref, ref` (the model refs in order). It is shown when rows remain after the recent runs, so at 80x24 with runs listed it may be out of view. When no level lists models, the block shows one line: no levels list models (nothing routes until one does). The checklist covers the config, the harnesses found on `PATH`, accounts, sign-ins, whether superorchestrator is registered with Claude Code, and the OpenRouter router key. Each missing item shows the fix beside it.

**2 Accounts.** The account table: name, harness, signed in, enabled, cap, active runs and headroom.

| Key | Action |
| --- | --- |
| `a` | Add accounts: pick the harness with the arrow keys, type a count (1 to 99). Names follow the same rule as `account add --count`. |
| `l` | Sign in. The UI steps aside and runs the login: for codex and opencode the harness's own login, for claude and agy a session of the account's wrapper (type `/login` in Claude, sign in inside agy, then exit). Press Enter afterwards to return. If the wrapper is not on `PATH` it shows the command instead. |
| `e` | Enable or disable the account. |
| `f` | Flag a cooldown (asks for hours), or clear one that is set. |
| `x` | Remove. You must type the account name to confirm. |
| `i` | Scan for logins already on this machine and import them. |

**3 Runs.** Recent runs, newest first: id, state, account, age and title. `Enter` opens a run: brief, route decision (its route line is `source / choice_id / account`, where the choice id is the model ref), recent events, result and, for a run waiting for review, its diff, all scrolling with `j` / `k`.

| Key | Action |
| --- | --- |
| `c` | Cancel the run (asks first). |
| `o` | Open the run's session (join): the UI steps aside and starts the harness, and you return when it exits. |
| `a` | Accept a run that is waiting for review. A run with a worktree asks "accept and merge?" and merges its branch into the base checkout (refused, leaving the run at the gate, if the checkout is dirty, on another branch or the merge conflicts). |
| `A` | Accept without merging. |
| `r` | Reject it with feedback; the run resumes with your text. |

The list is read from the store. The diff, cancel, join, accept and reject go through the supervisor and never start it: when it is not running they say "supervisor not running — start with any delegation or `superorchestrator daemon`".

**4 Setup.** The first-run steps one by one with live status: `init`, `harnesses`, `import`, `add accounts`, `sign in`, `install` and `doctor`. `Enter` runs the selected step, `a` runs them all in order (it stops at the first failure), and `i` and `d` jump to `install` and `doctor`. Steps do only what is missing, so running them again is safe. `sign in` lists the accounts that are signed out and how to sign in; use the Accounts tab for that.

## Colour and fallbacks

Colour is never the only signal: states have a symbol (`✓ ✗ ! ● ○`) next to the word. `NO_COLOR` (any non-empty value) and `TERM=dumb` turn colour off and use bold, dim and reverse video instead. A terminal without a UTF-8 locale, and the Linux console, get ASCII symbols and frames. The terminal is restored on every way out, including a panic, `SIGTERM` and `SIGHUP`.

## Limits

There is no mouse support, and screen readers cannot follow a full-screen UI. For a plain-text view use `status`, `account list --json` and `runs list --json`.
