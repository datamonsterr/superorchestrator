---
name: railway-preview-cleanup
description: Reclaim Railway preview capacity when environmentCreate fails with quota, resource, or trial-expired errors. Use when Railway preview deploy fails, CI reports 'used all available resources', 'creating environments too quickly', trial expired, or user says clean old envs, free Railway capacity, retry pr preview.
---

# Railway Preview Cleanup

Free ephemeral `pr-*` envs, retry target PR deploy, notify affected PRs.

## Quick start

```bash
export PATH="$HOME/.npm-global/bin:$PATH" GH_CONFIG_DIR="$HOME/.config/gh"
export RAILWAY_PROJECT_ID=c49458fb-d741-468f-a3ed-d35b96788f68
python3 scripts/railway_pr.py cleanup --pr <OLD_PR>
python3 scripts/railway_pr.py setup --pr <TARGET_PR> --head-sha <SHA>
```

Never delete `dev`, `staging`, `production` (non-ephemeral guard in script).

## Workflow

### 1. Diagnose exact error

Run raw mutation, do not trust wrapper stderr alone:

```bash
railway api 'mutation createEnv($input: EnvironmentCreateInput!) { environmentCreate(input: $input) { id name } }' --variables '{"input":{"projectId":"c49458fb-d741-468f-a3ed-d35b96788f68","name":"pr-<N>","sourceEnvironmentId":"e0d4551b-ff56-4f0f-9dd0-ef3c74af31cb","ephemeral":true}}'
```

Classify:

- `used all available resources` -> quota/capacity, proceed to step 2.
- `creating environments too quickly / 1 environment per 30 seconds` -> wait 60-90s, retry once with no intervening creates.
- `trial has expired / select a plan` -> stop, no CLI fix. Tell user to renew plan.
- `NOT_AUTHENTICATED` -> `railway whoami`, `railway login`.

Auth notes: CLI lives at `$HOME/.npm-global/bin/railway`. `gh` needs `GH_CONFIG_DIR=$HOME/.config/gh` because `XDG_CONFIG_HOME` points to omnigent xdg path.

### 2. List and pick victims

```bash
railway api 'query getEnvs($projectId: String!) { project(id: $projectId) { environments { edges { node { id name isEphemeral } } } } }' --variables '{"projectId":"c49458fb-d741-468f-a3ed-d35b96788f68"}'
```

Delete order:

1. `pr-*` for closed/merged PRs first (`gh pr view <N> --json state`).
2. Then oldest open-PR previews if still blocked, ask user before deleting active PR envs.
3. Never touch non-ephemeral envs.

```bash
python3 scripts/railway_pr.py cleanup --pr <OLD_PR>
```

Space deletes 30s apart. Wait 60s after last delete before create.

### 3. Retry target

```bash
gh pr view <TARGET> --json headRefOid -q .headRefOid
python3 scripts/railway_pr.py setup --pr <TARGET> --head-sha <SHA>
```

If retry still `resources` after 2 victims removed, stop and escalate to plan/quota, do not loop deletes.

### 4. Comment on removed PRs

For each deleted `pr-<OLD>` still open, post recreate note:

```bash
gh pr comment <OLD> --body "🧹 Railway preview \`pr-<OLD>\` removed to free capacity for \`pr-<TARGET>\` (Railway environment limit). To retest here: rerun the failed Deploy Railway preview job or run \`python3 scripts/railway_pr.py setup --pr <OLD> --head-sha <SHA>\`. 🤖"
```

Closed PRs: no comment needed.

## Report

State: deleted envs with IDs, retry result with full GraphQL message, comments posted, next action (rerun CI vs renew plan).
