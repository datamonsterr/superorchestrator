# Deployment runbook

## GitOps rules

- Update `argocd/` manifests and commit changes.
- Avoid manual kubectl changes unless explicitly asked.
- Use Helm v3 for kustomize when required.

## Bumba

```bash
bun run packages/scripts/src/bumba/deploy-service.ts
kubectl rollout status deployment/bumba -n jangar
```

## Jangar

```bash
bun run packages/scripts/src/jangar/deploy-service.ts
kubectl rollout status deployment/jangar -n jangar
```

If manual rendering is required:

```bash
mise exec helm@3 -- kustomize build --enable-helm argocd/applications/jangar | kubectl apply -n jangar -f -
```

## Verify

- `kubectl get pods -n jangar`
- Check logs for new pods
- Confirm UI endpoints and health checks
