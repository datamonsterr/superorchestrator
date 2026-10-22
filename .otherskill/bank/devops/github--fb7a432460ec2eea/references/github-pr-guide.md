# GitHub PR guide

## Commit and PR titles

Use Conventional Commits:

```
fix(bumba): stabilize workflows
```

PR title should match the commit style.

## Create PR

```bash
cp .github/PULL_REQUEST_TEMPLATE.md /tmp/pr.md
$EDITOR /tmp/pr.md
gh pr create --body-file /tmp/pr.md
```

## Check status

```bash
gh pr checks 2259
gh pr view 2259 --web
gh run view 123456789 --log
```

## Merge

Use squash merge without deleting the branch:

```bash
gh pr merge 2202 --squash -R proompteng/lab
```
