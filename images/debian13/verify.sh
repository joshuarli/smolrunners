#!/bin/sh
set -eu

manifest=/opt/smolrunners/manifest.json
export HOME=/tmp
fail() { printf 'verify: %s\n' "$*" >&2; exit 1; }
pin() { sed -n "s/.*\"$1\": *\"\\([^\"]*\\)\".*/\\1/p" "$manifest" | head -1; }

[ -f "$manifest" ] || fail "missing $manifest"
[ -n "$(pin base_digest)" ] || fail 'manifest lacks base_digest'
[ -n "$(pin apt_snapshot)" ] || fail 'manifest lacks apt_snapshot'

expect() {
  label=$1
  expected=$2
  actual=$3
  [ -n "$expected" ] || fail "manifest lacks $label"
  [ "$actual" = "$expected" ] || fail "$label: expected $expected, got $actual"
  printf 'verify: %s=%s ok\n' "$label" "$actual"
}

cd /opt/runner
expect runner "$(pin runner)" "$(bin/Runner.Listener --version 2>&1 | head -1)"
expect node20 "$(pin node20)" "$(externals/node20/bin/node --version)"
expect node24 "$(pin node24)" "$(externals/node24/bin/node --version)"
expect docker "$(pin docker)" \
  "$(/usr/local/bin/docker --version | sed -E 's/^Docker version ([0-9.]+).*/\1/')"
expect containerd "$(pin containerd)" \
  "$(/usr/local/bin/containerd --version | awk '{print $3}')"
expect runc "$(pin runc)" \
  "$(/usr/local/bin/runc --version | head -1 | awk '{print $3}')"
expect buildx "$(pin buildx)" \
  "$(/usr/local/lib/docker/cli-plugins/docker-buildx version | awk '{print $2}')"
[ -x /usr/local/bin/dockerd ] || fail 'dockerd not installed'
[ -x /opt/node/bin/node ] || fail 'host-step node not installed'

printf 'verify: ok\n'
