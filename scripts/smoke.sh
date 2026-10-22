#!/usr/bin/env bash
# Real-machine smoke test of superorchestrator (SPEC 21). You run it by hand.
#
# WARNING: this script runs real harnesses on real accounts and spends real quota. CI never
# runs it. Use accounts you can afford to spend a few small turns on.
#
# What it does, in order:
#   1. Creates an isolated config dir, socket and wrapper dir under a temp directory.
#   2. Imports one account per harness you name (see the options below).
#   3. Asks `claude -p` to delegate one trivial task to each harness through
#      `superorchestrator mcp` (a strict MCP config; only mcp__superorchestrator__* tools).
#   4. Starts a longer codex run and prints its `join` command.
#   5. With two codex accounts, flags the first one and checks the run goes to the second.
#   6. With an OpenRouter (Jev) key, runs `route eval --require-live`.
#   7. Prints a summary table and exits non-zero if any step failed.
#
# Isolation: every superorchestrator command runs with SUPERORCHESTRATOR_CONFIG_DIR, the
# socket and the wrapper dir (accounts.wrapper_dir) set to throwaway paths, and with
# SUPERORCHESTRATOR_SKIP_SKILL_REFRESH set. Your real config dir, your real wrappers (for
# example ~/.local/bin/codex-acc2) and your installed skill and MCP registrations are never
# read or written. HOME stays real, because agy needs it for the system keyring. The
# credential homes you pass are linked, not copied, and the harnesses read and write them
# as they always do. The script never runs `install`.
#
# The `claude -p` orchestrator also runs with your real HOME and login. It is started with
# `--setting-sources project` from an empty directory, so it does not load your user
# settings, and with them your hooks and enabled plugins. Check `claude --help` if a future
# version renames that flag. Your user CLAUDE.md and auth are not settings, so they still apply.
#
# Usage: scripts/smoke.sh [options]
#   --binary PATH         superorchestrator binary (default: the one on PATH)
#   --codex-home DIR      codex credential home   (env SMOKE_CODEX_HOME)
#   --codex-home2 DIR     second codex home, for the failover step (env SMOKE_CODEX_HOME2)
#   --opencode-home DIR   opencode credential home (env SMOKE_OPENCODE_HOME)
#   --claude-home DIR     claude credential home   (env SMOKE_CLAUDE_HOME)
#   --agy-home DIR        agy home that holds .gemini (env SMOKE_AGY_HOME)
#   --keep                keep the temp directory
#   -h, --help            show this help
# Other environment: SMOKE_TIMEOUT (seconds per delegation, default 900),
#   OPENROUTER_API_KEY (enables the route eval step).
# Name at least one harness home. Every account must already be signed in.
# No `set -e` on purpose: a failing step is recorded and the run goes on, so the summary
# table lists every step. Setup that guards isolation uses `die` instead.
set -uo pipefail

usage() { sed -n '2,/^set -uo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }

bin=superorchestrator
codex_home=${SMOKE_CODEX_HOME:-}
codex_home2=${SMOKE_CODEX_HOME2:-}
opencode_home=${SMOKE_OPENCODE_HOME:-}
claude_home=${SMOKE_CLAUDE_HOME:-}
agy_home=${SMOKE_AGY_HOME:-}
keep=0
timeout_s=${SMOKE_TIMEOUT:-900}

while [ $# -gt 0 ]; do
  case $1 in
    --binary) bin=${2:?--binary needs a path}; shift 2 ;;
    --codex-home) codex_home=${2:?}; shift 2 ;;
    --codex-home2) codex_home2=${2:?}; shift 2 ;;
    --opencode-home) opencode_home=${2:?}; shift 2 ;;
    --claude-home) claude_home=${2:?}; shift 2 ;;
    --agy-home) agy_home=${2:?}; shift 2 ;;
    --keep) keep=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) echo "smoke: unknown option $1" >&2; usage >&2; exit 2 ;;
  esac
done

die() { echo "smoke: $*" >&2; exit 2; }

[ -n "$codex_home$opencode_home$claude_home$agy_home" ] ||
  die "name at least one harness home (--codex-home, --opencode-home, --claude-home, --agy-home)"
command -v python3 >/dev/null 2>&1 || die "python3 is needed to read JSON"
command -v claude >/dev/null 2>&1 || die "the claude CLI is needed as the orchestrator"
bin=$(command -v "$bin" 2>/dev/null) || die "superorchestrator not found; pass --binary"
case $bin in /*) ;; *) bin=$PWD/$bin ;; esac

tmp=$(mktemp -d "${TMPDIR:-/tmp}/so-smoke.XXXXXX")
# Unix socket paths are short on macOS, so the socket gets its own short directory.
sockdir=$(mktemp -d /tmp/so.XXXXXX)
mkdir -p "$tmp/wrappers" "$tmp/work" "$tmp/logs"

# Isolation first: cleanup runs `status` and must never reach a real supervisor.
# Nothing below may point at the real config dir, socket or wrappers.
export SUPERORCHESTRATOR_CONFIG_DIR=$tmp/config
export SUPERORCHESTRATOR_SOCKET=$sockdir/s.sock
export SUPERORCHESTRATOR_SKIP_SKILL_REFRESH=1
unset XDG_RUNTIME_DIR SUPERORCHESTRATOR_RUN_ID
case $SUPERORCHESTRATOR_CONFIG_DIR in "$tmp"/*) ;; *) die "isolation check failed" ;; esac

so() { "$bin" "$@"; }

cleanup() {
  local pid i
  pid=$(so --json status 2>/dev/null | sed -n 's/.*"pid": *\([0-9][0-9]*\).*/\1/p' | head -n 1)
  if [ -n "${pid:-}" ]; then
    kill "$pid" 2>/dev/null
    for i in $(seq 1 50); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.2
    done
    # Still alive after 10 s: it would keep the temp dir busy, so stop it for good.
    kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null
  fi
  if [ "$keep" = 1 ]; then
    echo "smoke: kept $tmp (logs in $tmp/logs)"
  else
    rm -rf "$tmp"
  fi
  rm -rf "$sockdir"
}
trap cleanup EXIT


names=()
results=()
notes=()
failures=0
record() { # record NAME PASS|FAIL|SKIP NOTE
  names+=("$1"); results+=("$2"); notes+=("${3:-}")
  [ "$2" = FAIL ] && failures=$((failures + 1))
  echo "smoke: [$2] $1 ${3:+- $3}"
}


bounded() { # bounded SECONDS CMD...
  local s=$1; shift
  if command -v timeout >/dev/null 2>&1; then timeout "$s" "$@"
  elif command -v gtimeout >/dev/null 2>&1; then gtimeout "$s" "$@"
  else "$@"; fi
}


# Ids of runs matching harness, account and state (empty string matches any), one per line.
run_ids() { # run_ids HARNESS ACCOUNT STATE
  so --json runs list --all 2>/dev/null | python3 -I -c '
import json, sys
h, a, s = sys.argv[1:4]
for r in json.load(sys.stdin):
    if r["harness"] == h and (not a or r["account"] == a) and (not s or r["state"] == s):
        print(r["id"])
' "$1" "$2" "$3"
}

new_run_id() { # new_run_id BEFORE_FILE HARNESS ACCOUNT STATE -> newest id not in BEFORE_FILE
  run_ids "$2" "$3" "$4" | grep -vxFf "$1" | tail -n 1
}

echo "smoke: this spends real quota on the accounts you named"
echo "smoke: config $SUPERORCHESTRATOR_CONFIG_DIR, wrappers $tmp/wrappers"

# Wrappers must land in the isolated dir. Without it they would go to the real ~/.local/bin.
so init --yes >"$tmp/logs/init.log" 2>&1 || die "init failed; see $tmp/logs/init.log"
so config set accounts.wrapper_dir "$tmp/wrappers" >>"$tmp/logs/init.log" 2>&1 ||
  die "could not set accounts.wrapper_dir; see $tmp/logs/init.log"
[ "$(so config get accounts.wrapper_dir 2>/dev/null)" = "$tmp/wrappers" ] ||
  die "accounts.wrapper_dir is not the isolated directory; refusing to import accounts"
record init PASS

# MCP config for the orchestrator. The server runs with the isolated environment.
python3 -I - "$tmp/mcp.json" "$bin" "$PATH" <<'PY'
import json, os, sys
out, binary, path = sys.argv[1:4]
env = {
    "HOME": os.environ["HOME"],
    "PATH": path,
    "SUPERORCHESTRATOR_CONFIG_DIR": os.environ["SUPERORCHESTRATOR_CONFIG_DIR"],
    "SUPERORCHESTRATOR_SOCKET": os.environ["SUPERORCHESTRATOR_SOCKET"],
    "SUPERORCHESTRATOR_SKIP_SKILL_REFRESH": "1",
}
cfg = {"mcpServers": {"superorchestrator": {"command": binary, "args": ["mcp"], "env": env}}}
json.dump(cfg, open(out, "w"))
PY

harnesses=()
import_account() { # import_account HARNESS NAME HOME_DIR
  local out
  if out=$(so account import "$2" --harness "$1" --home "$3" 2>&1); then
    local signed
    signed=$(so --json account list 2>/dev/null | python3 -I -c '
import json, sys
n = sys.argv[1]
print(next((a["signed_in"] for a in json.load(sys.stdin) if a["name"] == n), ""))' "$2")
    if [ "$signed" = yes ]; then
      record "import $2" PASS "$1 home $3"
      return 0
    fi
    record "import $2" FAIL "not signed in (sign in with the harness first)"
  else
    record "import $2" FAIL "$out"
  fi
  return 1
}

if [ -n "$codex_home" ] && import_account codex smoke-codex "$codex_home"; then harnesses+=(codex); fi
if [ -n "$codex_home2" ] && ! import_account codex smoke-codex2 "$codex_home2"; then codex_home2=; fi
if [ -n "$opencode_home" ] && import_account opencode smoke-opencode "$opencode_home"; then harnesses+=(opencode); fi
if [ -n "$claude_home" ] && import_account claude smoke-claude "$claude_home"; then harnesses+=(claude); fi
if [ -n "$agy_home" ] && import_account agy smoke-agy "$agy_home"; then harnesses+=(agy); fi

# Runs `claude -p` as the orchestrator with only the superorchestrator tools.
orchestrate() { # orchestrate LOGNAME PROMPT
  (cd "$tmp/work" && bounded "$timeout_s" claude -p "$2" \
    --setting-sources project \
    --mcp-config "$tmp/mcp.json" --strict-mcp-config \
    --allowedTools 'mcp__superorchestrator__*' \
    --output-format json >"$tmp/logs/$1.json" 2>"$tmp/logs/$1.err")
}

delegate_prompt() { # delegate_prompt HARNESS ACCOUNT WAIT PROMPT
  local acct=${2:+ account \"$2\",}
  printf 'Call the superorchestrator spawn_agent tool exactly once with harness "%s",%s isolation "none", cwd "%s", wait %s and prompt "%s". Then reply with the run id and its final state. Call no other tool except get_run.' \
    "$1" "$acct" "$tmp/work" "$3" "$4"
}

account_of() { echo "smoke-$1"; }

for h in ${harnesses[@]+"${harnesses[@]}"}; do
  acct=$(account_of "$h")
  run_ids "$h" "$acct" "" >"$tmp/before"
  prompt=$(delegate_prompt "$h" "$acct" true "Reply with the single word PONG and do nothing else.")
  if ! orchestrate "delegate-$h" "$prompt"; then
    record "delegate $h" FAIL "claude -p failed; see $tmp/logs/delegate-$h.err"
    keep=1
    continue
  fi
  id=$(new_run_id "$tmp/before" "$h" "$acct" succeeded)
  if [ -n "$id" ]; then
    record "delegate $h" PASS "run ${id:0:8} on $acct succeeded"
  else
    state=$(run_ids "$h" "$acct" "" | grep -vxFf "$tmp/before" | tail -n 1)
    record "delegate $h" FAIL "no succeeded run on $acct (new run: ${state:-none}); see $tmp/logs"
    keep=1
  fi
done

# A codex run that is still going, so you can join it.
join_run=
if [[ " ${harnesses[*]-} " == *" codex "* ]]; then
  run_ids codex smoke-codex "" >"$tmp/before"
  prompt=$(delegate_prompt codex smoke-codex false "Run the shell command sleep 90, then reply with the single word DONE.")
  if orchestrate join-codex "$prompt"; then
    sleep 5
    join_run=$(new_run_id "$tmp/before" codex smoke-codex "")
  fi
  if [ -n "$join_run" ] && join_cmd=$(so join "$join_run" 2>"$tmp/logs/join.err") && [ -n "$join_cmd" ]; then
    record "join codex" PASS "run ${join_run:0:8}"
    echo "smoke: join command for the live codex run:"
    echo "  $join_cmd"
    echo "smoke: run it in another terminal with these set:"
    echo "  export SUPERORCHESTRATOR_CONFIG_DIR=$SUPERORCHESTRATOR_CONFIG_DIR"
    echo "  export SUPERORCHESTRATOR_SOCKET=$SUPERORCHESTRATOR_SOCKET"
    if [ -t 0 ]; then
      read -r -p "smoke: press Enter when you are done joining (the run is cancelled next) " _
    fi
    so runs cancel "$join_run" >/dev/null 2>&1
  else
    record "join codex" FAIL "no join command; see $tmp/logs/join.err"
    keep=1
  fi
fi

# Flag the first codex account, so the router has to use the second.
if [ -n "$codex_home2" ] && [[ " ${harnesses[*]-} " == *" codex "* ]]; then
  if so account flag smoke-codex --hours 1 >/dev/null 2>&1; then
    run_ids codex "" "" >"$tmp/before"
    prompt=$(delegate_prompt codex "" true "Reply with the single word PONG and do nothing else.")
    if orchestrate failover "$prompt"; then
      id=$(new_run_id "$tmp/before" codex smoke-codex2 succeeded)
      if [ -n "$id" ]; then
        record "failover codex" PASS "run ${id:0:8} went to smoke-codex2"
      else
        record "failover codex" FAIL "no succeeded run on smoke-codex2; see $tmp/logs/failover.json"
        keep=1
      fi
    else
      record "failover codex" FAIL "claude -p failed; see $tmp/logs/failover.err"
      keep=1
    fi
    so account flag smoke-codex --clear >/dev/null 2>&1
  else
    record "failover codex" FAIL "could not flag smoke-codex"
  fi
else
  record "failover codex" SKIP "needs --codex-home and --codex-home2"
fi

if [ -n "${OPENROUTER_API_KEY:-}" ]; then
  if bounded 600 "$bin" route eval --require-live >"$tmp/logs/route-eval.log" 2>&1; then
    record "route eval" PASS "$(tail -n 1 "$tmp/logs/route-eval.log")"
  else
    record "route eval" FAIL "see $tmp/logs/route-eval.log"
    keep=1
  fi
else
  record "route eval" SKIP "OPENROUTER_API_KEY is not set"
fi

echo
printf '%-24s %-6s %s\n' STEP RESULT NOTE
for i in "${!names[@]}"; do
  printf '%-24s %-6s %s\n' "${names[$i]}" "${results[$i]}" "${notes[$i]}"
done
echo
if [ "$failures" -gt 0 ]; then
  echo "smoke: $failures step(s) failed"
  exit 1
fi
echo "smoke: all steps passed"
