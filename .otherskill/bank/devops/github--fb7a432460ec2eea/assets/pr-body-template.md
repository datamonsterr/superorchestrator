# PR Summary

## What changed

- Standardized tuned model aliases for self-hosted Ollama
- Updated deployment manifests for bumba and jangar
- Added documentation and runbooks for the new defaults

## Why

- Ensure OpenWebUI only shows tuned models
- Reduce confusion and keep model selection stable

## Testing

- bun test services/bumba/src/activities/index.test.ts
- kubectl rollout status deployment/bumba -n jangar
- kubectl rollout status deployment/jangar -n jangar

## Risk

- Low. Changes are config and docs only.
