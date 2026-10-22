# Repo notes

## Area

services/bumba workflows

## Files

- services/bumba/src/workflows/index.ts
- services/bumba/src/activities/index.ts
- packages/scripts/src/bumba/enrich-repository.ts

## Commands run

- rg -n "enrichRepository" services/bumba
- bun run packages/scripts/src/bumba/enrich-repository.ts --path-prefix services/bumba --max-files 50 --wait
