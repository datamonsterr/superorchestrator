# PR process

## Commit conventions

`fix(bumba): stabilize workflows`
`chore(deploy): roll bumba and jangar`

## PR creation

```bash
cp .github/PULL_REQUEST_TEMPLATE.md /tmp/pr.md
$EDITOR /tmp/pr.md
gh pr create --body-file /tmp/pr.md
```

## Merge

Use squash merge, and avoid deleting the branch via CLI:

```bash
gh pr merge 2202 --squash -R proompteng/lab
```
