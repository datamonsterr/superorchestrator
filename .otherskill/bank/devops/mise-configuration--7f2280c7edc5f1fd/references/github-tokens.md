# GitHub Token Multi-Account Patterns

**Parent Skill**: [mise-configuration](../SKILL.md)

## Overview

Per-directory GH_TOKEN configuration for multi-account GitHub setups using mise `[env]`.

## Problem

GitHub's `gh` CLI has no native per-directory authentication. When working across multiple GitHub accounts (personal, organization, client), the wrong account may be used for operations like `semantic-release`.

## Solution

Use mise `[env]` to set `GH_TOKEN` and `GITHUB_TOKEN` per directory, overriding gh CLI's stored credentials.

## Token File Storage

```bash
# Create secure directory
mkdir -p ~/.claude/.secrets
chmod 700 ~/.claude/.secrets

# Store tokens (one per account)
gh auth login --hostname github.com  # Login as account
gh auth token > ~/.claude/.secrets/gh-token-accountname
chmod 600 ~/.claude/.secrets/gh-token-*
```

**Token file naming convention**: `gh-token-<accountname>`

## mise [env] Templates

### Same-Directory Token (using config_root)

```toml
# ~/.claude/.mise.toml
[env]
GH_TOKEN = "{{ read_file(path=config_root ~ '/.secrets/gh-token-terrylica') | trim }}"
GITHUB_TOKEN = "{{ read_file(path=config_root ~ '/.secrets/gh-token-terrylica') | trim }}"
GH_ACCOUNT = "terrylica"  # Human reference only
```

### Cross-Directory Token (using env.HOME)

```toml
# ~/eon/.mise.toml
[env]
GH_TOKEN = "{{ read_file(path=env.HOME ~ '/.claude/.secrets/gh-token-terrylica') | trim }}"
GITHUB_TOKEN = "{{ read_file(path=env.HOME ~ '/.claude/.secrets/gh-token-terrylica') | trim }}"
GH_ACCOUNT = "terrylica"
```

**When to use each**:

- `config_root`: Token file is relative to the `.mise.toml` location
- `env.HOME`: Token file is in a shared location (recommended)

## GH_TOKEN vs GITHUB_TOKEN

| Variable       | Usage Context                                 | Example                     |
| -------------- | --------------------------------------------- | --------------------------- |
| `GH_TOKEN`     | mise [env], Doppler, verification tasks       | `.mise.toml`, shell scripts |
| `GITHUB_TOKEN` | npm scripts, GitHub Actions, semantic-release | `package.json`, workflows   |

**Rule**: Always set BOTH variables pointing to the same token file.

## Directory-Account Mapping

| Directory              | GitHub Account | Token File           |
| ---------------------- | -------------- | -------------------- |
| `~/.claude/`           | terrylica      | `gh-token-terrylica` |
| `~/eon/`               | terrylica      | `gh-token-terrylica` |
| `~/raw-data-services/` | terrylica      | `gh-token-terrylica` |
| `~/own/`               | tainora        | `gh-token-tainora`   |
| `~/scripts/`           | tainora        | `gh-token-tainora`   |
| `~/459ecs/`            | 459ecs         | `gh-token-459ecs`    |

## Account Alignment Verification

```bash
/usr/bin/env bash << 'VALIDATE_EOF'
# Verify all directories use correct account
for dir in ~/.claude ~/eon ~/own ~/scripts ~/459ecs; do
  cd "$dir" && eval "$(mise hook-env -s bash)" && echo "$dir → $(gh api user --jq '.login')"
done
VALIDATE_EOF
```

## SSH ControlMaster Warning

> **CRITICAL**: If using multi-account SSH, ensure `ControlMaster no` is set for GitHub hosts in `~/.ssh/config`. Cached connections can authenticate with the wrong account.

```ssh-config
Match host github.com,ssh.github.com exec "pwd | grep -q '/.claude'"
    IdentityFile ~/.ssh/id_ed25519_terrylica
    IdentitiesOnly yes
    ControlMaster no  # ← CRITICAL
```

See [SSH ControlMaster Cache](../../semantic-release/references/troubleshooting.md#ssh-controlmaster-cache) for troubleshooting.

## Troubleshooting

### "Repository not found" Error

1. Check account alignment:

   ```bash
   ssh -T git@github.com  # SSH account
   gh api user --jq '.login'  # gh CLI account (should match)
   ```

2. If mismatch, verify mise config:

   ```bash
   mise env | grep GH_TOKEN
   cat ~/.claude/.secrets/gh-token-accountname  # First 10 chars
   ```

3. Clear SSH ControlMaster cache:

   ```bash
   ssh -O exit git@github.com 2>/dev/null || pkill -f 'ssh.*github.com'
   ```

### Token Not Loading

1. Verify mise trusted:

   ```bash
   mise trust
   ```

2. Check token file exists:

   ```bash
   ls -la ~/.claude/.secrets/gh-token-*
   ```

3. Test token directly:

   ```bash

   ```

/usr/bin/env bash << 'GITHUB_TOKENS_SCRIPT_EOF'
GH_TOKEN=$(cat ~/.claude/.secrets/gh-token-accountname) gh api user --jq '.login'

GITHUB_TOKENS_SCRIPT_EOF

````

### Token Expired

GitHub tokens expire. Refresh with:

```bash
gh auth login --hostname github.com
gh auth token > ~/.claude/.secrets/gh-token-accountname
````

## 1Password Integration

For automatic token rotation:

```toml
[env]
GH_TOKEN = "{{ cache(key='gh_token', duration='1h', run='op read op://Engineering/GitHub Token/credential') }}"
GITHUB_TOKEN = "{{ cache(key='gh_token', duration='1h', run='op read op://Engineering/GitHub Token/credential') }}"
```

## References

- [mise env documentation](https://mise.jdx.dev/environments/)
