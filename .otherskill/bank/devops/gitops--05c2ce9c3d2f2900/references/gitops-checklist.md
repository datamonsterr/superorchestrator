# GitOps Checklist

## Common directories

- `argocd/`: Argo CD applications and overlays (primary GitOps source)
- `kubernetes/`: raw K8s manifests
- `tofu/`: Terraform/OpenTofu
- `ansible/`: Ansible playbooks

## Validation commands

- `scripts/argo-lint.sh`
- `scripts/kubeconform.sh argocd`
- `bun run lint:argocd`
- `bun run tf:plan` (apply only if asked)
- `bun run ansible`

## Helm + kustomize

- `mise exec helm@3 -- kustomize build --enable-helm <path>`

## kubectl usage

- Always set namespace: `kubectl <cmd> -n <ns>`
- Prefer GitOps edits; direct apply only if explicitly requested.
