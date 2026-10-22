# OtherSkill project bank

`.otherskill/bank/` holds this repository's OtherSkill skills bank. `otherskill.config.json`
points at it with the relative `bankPath` `.otherskill/bank`, so a clone routes against these
skills without any machine-specific paths. The bank sits outside every native skills directory,
so agents only see skills that OtherSkill selects for a prompt.

Hooks are portable (`portableHooks: true`): they call
`$HOME/.local/share/otherskill/bin/otherskill`, so each machine needs OtherSkill installed there,
with a build that supports portable hooks.

- Add a skill: `otherskill add <dir|github:owner/repo> --cwd <repo root>`
- List skills: `otherskill list` (run inside the repo)
- Check setup: `otherskill doctor`
- Regenerate hooks: `otherskill setup --non-interactive --agents claude,antigravity,opencode --scope project --portable`
