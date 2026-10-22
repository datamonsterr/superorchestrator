# Temporal CLI quick reference

## Connection

```bash
export TEMPORAL_ADDRESS=temporal-grpc.ide-newton.ts.net:7233
export TEMPORAL_NAMESPACE=default
```

## List

```bash
temporal --namespace "$TEMPORAL_NAMESPACE" workflow list --limit 20
```

## Filter

```bash
temporal --namespace "$TEMPORAL_NAMESPACE" workflow list   --query 'WorkflowType="enrichFile" and ExecutionStatus="Running"'
```

## Describe / show / result

```bash
temporal --namespace "$TEMPORAL_NAMESPACE" workflow describe   --workflow-id bumba-repo-0e6476cd-6df7-4ee3-8184-95029cd50c88   --run-id 019b7788-e535-752c-8a4a-2ee44de7065e

temporal --namespace "$TEMPORAL_NAMESPACE" workflow show   --workflow-id bumba-repo-0e6476cd-6df7-4ee3-8184-95029cd50c88   --run-id 019b7788-e535-752c-8a4a-2ee44de7065e   --output json > /tmp/workflow-history.json

temporal --namespace "$TEMPORAL_NAMESPACE" workflow result   --workflow-id bumba-repo-0e6476cd-6df7-4ee3-8184-95029cd50c88   --run-id 019b7788-e535-752c-8a4a-2ee44de7065e
```

## Reset

```bash
temporal --namespace "$TEMPORAL_NAMESPACE" workflow reset   --workflow-id bumba-repo-0e6476cd-6df7-4ee3-8184-95029cd50c88   --run-id 019b7788-e535-752c-8a4a-2ee44de7065e   --event-id 31   --reset-type FirstWorkflowTask   --reason "reset to known-good event"
```

## Cancel / terminate

```bash
temporal --namespace "$TEMPORAL_NAMESPACE" workflow cancel --workflow-id bumba-repo-0e6476cd-6df7-4ee3-8184-95029cd50c88

temporal --namespace "$TEMPORAL_NAMESPACE" workflow terminate   --workflow-id bumba-repo-0e6476cd-6df7-4ee3-8184-95029cd50c88   --reason "manual cleanup"
```

## Task queues

```bash
temporal --namespace "$TEMPORAL_NAMESPACE" task-queue describe --task-queue bumba
```

## Batch operations

```bash
temporal --namespace "$TEMPORAL_NAMESPACE" batch start   --query 'WorkflowType="enrichFile" and ExecutionStatus="Failed"'   --reason "cleanup failed enrichFile runs"   --terminate
```

## Schedules

```bash
temporal --namespace "$TEMPORAL_NAMESPACE" schedule list
```
