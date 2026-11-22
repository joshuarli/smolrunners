#!/bin/sh
set -eu

export PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin
manifest=/opt/smolrunners/manifest.json
fail() { printf 'verify: %s\n' "$*" >&2; exit 1; }
pin() { sed -n "s/.*\"$1\": *\"\\([^\"]*\\)\".*/\\1/p" "$manifest" | head -1; }

[ -f "$manifest" ] || fail "missing $manifest"
[ -n "$(pin base_digest)" ] || fail 'manifest lacks base_digest'

[ -f /etc/alpine-release ] || fail '/etc/alpine-release missing; not an Alpine rootfs'
release=$(cat /etc/alpine-release)
case "$release" in
  3.24.*) ;;
  *) fail "unexpected Alpine release: $release" ;;
esac
[ -x /sbin/apk ] || fail 'apk missing; not an Alpine rootfs'
[ ! -e /etc/debian_version ] || fail 'Debian marker present under an Alpine image'
[ ! -x /usr/bin/dpkg ] || fail 'dpkg present under an Alpine image'
if command -v apt-get >/dev/null 2>&1; then fail 'apt-get present under an Alpine image'; fi
grep -q '^ID=alpine$' /etc/os-release || fail 'os-release does not identify Alpine'

expect() {
  label=$1
  expected=$2
  actual=$3
  [ -n "$expected" ] || fail "manifest lacks $label"
  [ "$actual" = "$expected" ] || fail "$label: expected $expected, got $actual"
  printf 'verify: %s=%s ok\n' "$label" "$actual"
}

expect docker "$(pin docker)" \
  "$(/usr/local/bin/docker --version | sed -E 's/^Docker version ([0-9.]+).*/\1/')"
expect containerd "$(pin containerd)" \
  "$(/usr/local/bin/containerd --version | awk '{print $3}')"
expect runc "$(pin runc)" \
  "$(/usr/local/bin/runc --version | head -1 | awk '{print $3}')"
expect buildx "$(pin buildx)" \
  "$(/usr/local/lib/docker/cli-plugins/docker-buildx version | awk '{print $2}')"
expect compose "$(pin compose)" \
  "$(/usr/local/lib/docker/cli-plugins/docker-compose version --short | head -1)"
[ -x /usr/local/bin/dockerd ] || fail 'dockerd not installed'
[ -x /opt/smolrunners/bin/smolrunners-collector ] || fail 'collector not installed'
/opt/smolrunners/bin/smolrunners-collector --help | grep -q 'usage: smolrunners-collector' \
  || fail 'collector does not run on Alpine'

[ -x /usr/bin/node ] || fail 'musl node is missing'
node_version=$(/usr/bin/node --version 2>/dev/null) || fail 'musl node does not run'
case "$node_version" in
  v24.*) ;;
  *) fail "unexpected musl node version: $node_version" ;;
esac
ldd /usr/bin/node 2>/dev/null | grep -q 'ld-musl' || fail 'node is not musl linked'
[ -e /usr/lib/libstdc++.so.6 ] || fail 'libstdc++ runtime is missing'
printf 'verify: musl-node=%s ok\n' "$node_version"

printf 'verify: alpine-release=%s\n' "$release"
printf 'verify: ok\n'
