#!/usr/bin/env python3
"""PreToolUse guard: subagents in this project use only Haiku 5.5 or Sonnet 5.5 at medium effort."""
import json
import sys

ALLOWED_MODELS = {"haiku", "sonnet", "claude-haiku-5-5", "claude-sonnet-5-5"}
ALLOWED_EFFORT = {"medium"}
# Built-in agents that can edit code with the session model. Use the project agents instead.
BLOCKED_TYPES = {"general-purpose"}


def deny(reason: str) -> None:
    print(json.dumps({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
    }}))
    sys.exit(0)


try:
    tool_input = json.load(sys.stdin).get("tool_input") or {}
except (ValueError, AttributeError):
    sys.exit(0)

model = tool_input.get("model")
effort = tool_input.get("effort")
agent_type = tool_input.get("subagent_type") or "general-purpose"

if model is not None and model not in ALLOWED_MODELS:
    deny(f"Model '{model}' is not allowed in this project. Use implementer-haiku or implementer-sonnet, "
         "which pin claude-haiku-5-5 / claude-sonnet-5-5 at medium effort, and omit the model parameter.")
if effort is not None and effort not in ALLOWED_EFFORT:
    deny(f"Effort '{effort}' is not allowed in this project. Subagents run at medium effort; omit the effort parameter.")
if agent_type in BLOCKED_TYPES:
    deny("general-purpose subagents are disabled in this project. Use implementer-haiku, implementer-sonnet, "
         "spec-reviewer or code-quality-reviewer (subagent-driven-development skill), or Explore for read-only search.")
sys.exit(0)
