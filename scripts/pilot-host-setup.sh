#!/bin/sh
set -eu
bundle=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
fail() { printf 'pilot-host-setup: %s\n' "$*" >&2; exit 1; }
[ "$(id -u)" -eq 0 ] || fail 'must run as root'
for tool in curl python3 tar dpkg-deb apt-get ldconfig sha256sum mke2fs zstd; do
  command -v "$tool" >/dev/null 2>&1 || fail "missing tool: $tool"
done
arch=$(python3 - "$bundle/bin/smolrunners" <<'PY'
import struct
import sys
with open(sys.argv[1], 'rb') as binary:
    header = binary.read(20)
if header[:5] != b'\x7fELF\x02' or header[5] != 1:
    raise SystemExit('pilot-host-setup: unsupported executable format')
arch = {62: 'x86_64', 183: 'aarch64'}.get(struct.unpack('<H', header[18:20])[0])
if not arch:
    raise SystemExit('pilot-host-setup: unsupported executable architecture')
print(arch)
PY
)
case $(uname -m) in arm64|aarch64) host_arch=aarch64 ;; amd64|x86_64) host_arch=x86_64 ;; *) fail 'unsupported host architecture' ;; esac
[ "$host_arch" = "$arch" ] || fail "bundle architecture $arch does not match host $host_arch"
[ -c /dev/kvm ] || fail '/dev/kvm is missing'
[ -f /sys/fs/cgroup/cgroup.controllers ] || fail 'cgroup v2 is required'
case $arch in
  aarch64) runtime=smolvm-linux-arm64; inputs='runner-linux-arm64 docker-static-aarch64 buildx-linux-arm64 compose-linux-arm64 node-linux-arm64' ;;
  x86_64) runtime=smolvm-linux-x86_64; inputs='runner-linux-x64 docker-static-x86_64 buildx-linux-amd64 compose-linux-x64 node-linux-x64' ;;
esac
mkdir -p "$bundle/.work/tmp"
TMPDIR="$bundle/.work/tmp"; export TMPDIR
for input in $runtime $inputs; do
  sh "$bundle/bootstrap/buildlib-run" -- "$bundle/scripts/fetch-inputs.py" --name "$input" --verify-only >/dev/null || fail "missing or invalid bundled input: $input"
done
install -d -m 0755 /opt/smolrunners/bin /var/lib/smolrunners/images/debian13
for name in smolrunners smolrunners-launcher smolrunners-collector smolrunners-cache smolrunners-adapter; do
  install -m 0755 "$bundle/bin/$name" "/opt/smolrunners/bin/$name"
done
sh "$bundle/scripts/install-runtime" "$runtime" /opt/smolrunners/runtime
ARCH="$arch" SMOLRUNNERS_COLLECTOR="$bundle/bin/smolrunners-collector" sh "$bundle/images/debian13/build.sh" /var/lib/smolrunners/images/debian13 "$bundle/.work/cache"
install -d -o root -g root -m 0711 /var/lib/smolrunners/state
printf 'next: /opt/smolrunners/bin/smolrunners serve --pilot /etc/smolrunners/pilot.toml\n'
