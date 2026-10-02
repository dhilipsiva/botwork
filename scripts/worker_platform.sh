#!/bin/bash
# Print the facts docs/worker-platforms.md records for a host the worker suites
# ran on: kernel, distribution, libc, util-linux, namespace quotas, descriptor
# limit, and the proc and temporary-directory mounts. Read-only.
echo "kernel: $(uname -srm)"
echo "distribution: $(sed -n 's/^PRETTY_NAME="\{0,1\}\([^"]*\)"\{0,1\}$/\1/p' /etc/os-release 2>/dev/null)"
echo "libc: $(ldd --version 2>&1 | head -n 1)"
echo "util-linux: $(unshare --version 2>&1 | head -n 1)"
for quota in user pid mnt; do
    echo "max_${quota}_namespaces: $(cat /proc/sys/user/max_${quota}_namespaces 2>/dev/null || echo unavailable)"
done
echo "apparmor_restrict_unprivileged_userns: $(sysctl -n kernel.apparmor_restrict_unprivileged_userns 2>/dev/null || echo absent)"
echo "descriptor limit: $(ulimit -n)"
echo "proc mount: $(findmnt -no FSTYPE,OPTIONS /proc 2>/dev/null || echo unknown)"
echo "temporary directory: $(findmnt -no FSTYPE --target "${TMPDIR:-/tmp}" 2>/dev/null || echo unknown)"
