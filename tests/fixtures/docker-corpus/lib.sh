#!/bin/sh

fail() { printf 'docker-corpus: %s\n' "$*" >&2; exit 1; }
note() { printf 'docker-corpus: %s\n' "$*"; }
assert_eq() {
  label=$1
  expected=$2
  actual=$3
  [ "$actual" = "$expected" ] || fail "$label: expected '$expected', got '$actual'"
}
wait_for() {
  label=$1
  shift
  attempts=${WAIT_ATTEMPTS:-60}
  i=0
  while [ "$i" -lt "$attempts" ]; do
    if "$@" >/dev/null 2>&1; then return 0; fi
    i=$((i + 1))
    sleep 1
  done
  fail "timeout waiting for $label"
}
daemon_env() {
  export HOME=/tmp
  export DOCKER_CONFIG=/tmp/.docker
  export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
  mkdir -p /var/run /tmp/.docker /work/docker-exec /run/containerd /work/containerd
}
start_containerd() {
  containerd config default > /work/containerd/config.toml
  sed -i 's|root = "/var/lib/containerd"|root = "/var/lib/docker/containerd"|' \
    /work/containerd/config.toml
  containerd --config /work/containerd/config.toml \
    --address /run/containerd/containerd.sock > /work/containerd.log 2>&1 &
  wait_for containerd ctr --address /run/containerd/containerd.sock version
}
start_daemon() {
  daemon_env
  if [ "${1:-wipe}" = wipe ]; then
    find /var/lib/docker -mindepth 1 -maxdepth 1 -exec rm -rf {} + 2>/dev/null || true
    find /work/docker-exec -mindepth 1 -maxdepth 1 -exec rm -rf {} + 2>/dev/null || true
  fi
  if ! ctr --address /run/containerd/containerd.sock version >/dev/null 2>&1; then
    start_containerd
  fi
  dockerd --data-root=/var/lib/docker --exec-root=/work/docker-exec \
    --host=unix:///var/run/docker.sock --pidfile=/work/dockerd.pid \
    --containerd=/run/containerd/containerd.sock \
    --exec-opt native.cgroupdriver=cgroupfs --storage-driver=vfs \
    --iptables=false --ip6tables=false --insecure-registry=127.0.0.0/8 \
    > /work/dockerd.log 2>&1 &
  wait_for dockerd docker info
  kill -0 "$(cat /work/dockerd.pid)" >/dev/null 2>&1 || \
    fail "dockerd exited during startup: $(tail -10 /work/dockerd.log)"
}
stop_daemon() {
  [ -f /work/dockerd.pid ] || return 0
  pid=$(cat /work/dockerd.pid)
  kill "$pid" 2>/dev/null || true
  i=0
  while [ "$i" -lt 15 ]; do
    kill -0 "$pid" 2>/dev/null || return 0
    i=$((i + 1))
    sleep 1
  done
  kill -9 "$pid" 2>/dev/null || true
}
assert_data_root() {
  awk '$2 == "/var/lib/docker" && $3 == "ext4" { found = 1 } END { exit !found }' /proc/mounts \
    || fail "docker data root is not ext4: $(awk '$2 == "/var/lib/docker"' /proc/mounts)"
  driver=$(docker info --format '{{.Driver}}')
  assert_eq selected-image-store vfs "$driver"
  [ -d /var/lib/docker/vfs ] || fail "vfs image store absent: $(ls /var/lib/docker)"
  note "data root /var/lib/docker is ext4; selected image store is $driver"
}
