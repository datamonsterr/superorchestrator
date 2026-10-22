# Ansible runbook

## Inventory summary

- Masters: `kube-master-00`, `kube-master-01`, `kube-master-02`
- Workers: `kube-worker-00` through `kube-worker-29`
- Proxy: `nuc`
- Docker host: `docker-host`
- Groups: `kube_masters`, `kube_workers`, `k3s_cluster`, `proxy`, `docker_hosts`

## Preflight checks

```bash
ansible -i ansible/inventory/hosts.ini k3s_cluster -m ping -u kalmyk
```

If host keys block connection:

```bash
ANSIBLE_HOST_KEY_CHECKING=False ansible -i ansible/inventory/hosts.ini k3s_cluster -m ping -u kalmyk   --ssh-extra-args '-o StrictHostKeyChecking=accept-new'
```

## Running playbooks

Cluster-wide change:

```bash
ansible-playbook -i ansible/inventory/hosts.ini ansible/playbooks/install_nfs_client.yml -u kalmyk -b
```

Scoped change:

```bash
ansible-playbook -i ansible/inventory/hosts.ini ansible/playbooks/install_tailscale.yml -u kalmyk -b   --limit kube-worker-00
```

Dry run with diff:

```bash
ansible-playbook -i ansible/inventory/hosts.ini ansible/playbooks/install_tailscale.yml -u kalmyk -b   --check --diff
```

## Troubleshooting

- Unreachable hosts: verify SSH access and DNS/IP.
- sudo failures: confirm `kalmyk` has sudo on the host.
- Package locks: retry or clear locks on the host.
- Long-running tasks: add `-vvv` for detailed output.

## Post-run validation

- Check service status with systemctl.
- Check logs with journalctl.
- For k3s changes, verify nodes with kubectl.
