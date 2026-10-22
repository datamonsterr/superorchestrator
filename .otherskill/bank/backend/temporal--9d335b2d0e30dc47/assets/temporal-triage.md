# Temporal triage template

## Context

- UI URL: http://temporal/namespaces/default/workflows/bumba-repo-0e6476cd-6df7-4ee3-8184-95029cd50c88/019b7788-e535-752c-8a4a-2ee44de7065e/history
- Namespace: default
- Task queue: bumba
- Workflow type: enrichRepository
- Workflow ID: bumba-repo-0e6476cd-6df7-4ee3-8184-95029cd50c88
- Run ID: 019b7788-e535-752c-8a4a-2ee44de7065e

## Failure snapshot

- Failure summary: Workflow did not replay all history entries
- Last event ID: 31
- Worker image tag: 91fcf5a0

Common cause: workflow code changed in a way that alters the command stream for an in-flight execution (e.g. reordering
`activities.schedule(...)` without patch/getVersion gating).

## Diagnostics

- Describe output: `temporal workflow describe --workflow-id bumba-repo-0e6476cd-6df7-4ee3-8184-95029cd50c88 --run-id 019b7788-e535-752c-8a4a-2ee44de7065e`
- History JSON: `/tmp/workflow-history.json`
- Worker logs: `kubectl logs -n jangar deploy/bumba --tail=200`

## Mitigation

- Identify a safe reset event (often the last `WorkflowTaskCompleted` before the divergent scheduled command), then reset.
- See: `docs/temporal-nondeterminism.md`

- Re-run `enrichRepository` using the repo script.
- Confirm child `enrichFile` workflows complete.
