#!/usr/bin/env bash
set -euo pipefail

# Jev Skill Selector Runner
# Discovers the jev_skill_selector CLI via PATH, environment variable, or known paths.

# 1. Global binary in PATH (via bun link or global install)
if command -v jev-skill-selector >/dev/null 2>&1; then
  exec jev-skill-selector "$@"
fi

# 2. Environment variable override
if [ -n "${JEV_PATH:-}" ] && [ -f "$JEV_PATH/index.ts" ]; then
  exec bun run "$JEV_PATH/index.ts" "$@"
fi

# 3. Known clone directory candidates
CANDIDATES=(
  "/home/dat/dev/jev_skill_selector"
  "$HOME/dev/jev_skill_selector"
  "$HOME/dev/jev_auto_select_skills"
  "$HOME/.agents/jev_skill_selector"
  "$HOME/jev_skill_selector"
)

for cand in "${CANDIDATES[@]}"; do
  if [ -f "$cand/index.ts" ]; then
    exec bun run "$cand/index.ts" "$@"
  fi
done

echo "Error: jev-skill-selector was not found in PATH or standard directories." >&2
echo "Please run 'bun link' inside your cloned jev_skill_selector repository," >&2
echo "or set 'export JEV_PATH=/path/to/jev_skill_selector'." >&2
exit 1
