# Deploy checklist

- [ ] Build and push image via repo script
- [ ] Confirm manifest tag updated in `argocd/`
- [ ] Confirm rollout annotation updated
- [ ] Apply manifests or confirm Argo CD sync
- [ ] Wait for rollout to finish
- [ ] Check logs and health endpoints
- [ ] Note rollback plan if needed
