# kubectl triage checklist

- [ ] Identify namespace and context
- [ ] Check pods: `kubectl get pods -n jangar`
- [ ] Check services: `kubectl get svc -n jangar`
- [ ] Check logs: `kubectl logs -n jangar deploy/bumba --tail=200`
- [ ] Check rollout: `kubectl rollout status -n jangar deployment/bumba`
- [ ] Verify endpoints and readiness
