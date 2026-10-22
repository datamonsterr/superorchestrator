#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: run-playbook.sh ansible/playbooks/install_nfs_client.yml [extra ansible-playbook args]" >&2
  exit 1
fi

PLAYBOOK="$1"
shift

INVENTORY=${ANSIBLE_INVENTORY:-ansible/inventory/hosts.ini}
USER=${ANSIBLE_USER:-kalmyk}
BECOME=${ANSIBLE_BECOME:-true}
LIMIT=${ANSIBLE_LIMIT:-}
TAGS=${ANSIBLE_TAGS:-}

ARGS=("-i" "$INVENTORY" "$PLAYBOOK" "-u" "$USER")
if [[ "$BECOME" == "true" ]]; then
  ARGS+=("-b")
fi
if [[ -n "$LIMIT" ]]; then
  ARGS+=("--limit" "$LIMIT")
fi
if [[ -n "$TAGS" ]]; then
  ARGS+=("--tags" "$TAGS")
fi

if [[ "${ANSIBLE_HOST_KEY_CHECKING:-}" == "false" ]]; then
  ARGS+=("--ssh-extra-args" "-o StrictHostKeyChecking=accept-new")
fi

exec ansible-playbook "${ARGS[@]}" "$@"
