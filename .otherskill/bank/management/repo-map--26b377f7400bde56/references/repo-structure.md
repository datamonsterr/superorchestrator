# Repo structure

## Top-level

- apps: frontend apps
- packages: shared libraries and Convex backend
- services: backend services
- argocd, kubernetes, tofu, ansible: infra and GitOps
- scripts, packages/scripts: helper scripts
- skills: agent skills

## Search examples

```bash
rg -n "OPENAI_API_BASE_URL" services
rg --files -g "*openwebui*"
```
