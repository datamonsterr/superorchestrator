# kubectl runbook

## Basic checks

```bash
kubectl get pods -n jangar
kubectl get svc -n jangar
kubectl get endpoints -n jangar
```

## Logs

```bash
kubectl logs -n jangar deploy/bumba --tail=200
kubectl logs -n jangar deploy/jangar --tail=200
```

## Rollouts

```bash
kubectl rollout status -n jangar deployment/bumba
kubectl rollout status -n jangar deployment/jangar
```

## CNPG

```bash
kubectl cnpg psql -n jangar jangar-db -- -c 'select count(*) from memories.entries;'
```

## OpenWebUI access

```bash
kubectl -n jangar port-forward svc/open-webui 8080:80
```
