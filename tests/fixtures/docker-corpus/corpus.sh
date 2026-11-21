#!/bin/sh
set -eu

. /opt/docker-corpus/lib.sh
node=/opt/node/bin/node

start_daemon wipe
assert_data_root

pull_pids=''
for image in registry:2 alpine:3.24 docker:29.9.3-cli testcontainers/ryuk:0.11.0; do
  docker pull -q "$image" >/dev/null 2>&1 &
  pull_pids="$pull_pids $!"
done
for pid in $pull_pids; do
  wait "$pid" || true
done

note 'item 1: shell docker run with local registry push and pull'
docker pull -q registry:2 >/dev/null
docker pull -q alpine:3.24 >/dev/null
docker rm -f sr-corpus-registry >/dev/null 2>&1 || true
docker run -d --name sr-corpus-registry -p 127.0.0.1:15000:5000 registry:2 >/dev/null
docker tag alpine:3.24 127.0.0.1:15000/sr-corpus/alpine:roundtrip
pushed=0
i=0
while [ "$i" -lt 40 ]; do
  if docker push 127.0.0.1:15000/sr-corpus/alpine:roundtrip > /work/push.log 2>&1; then
    pushed=1
    break
  fi
  i=$((i + 1))
  sleep 1
done
[ "$pushed" -eq 1 ] || fail "local registry push failed: $(tail -5 /work/push.log)"
docker rmi 127.0.0.1:15000/sr-corpus/alpine:roundtrip >/dev/null
docker pull 127.0.0.1:15000/sr-corpus/alpine:roundtrip > /work/pull.log 2>&1 || \
  fail "local registry pull failed: $(tail -5 /work/pull.log)"
out=$(docker run --rm 127.0.0.1:15000/sr-corpus/alpine:roundtrip echo REGISTRY_ROUNDTRIP_OK)
assert_eq registry-roundtrip REGISTRY_ROUNDTRIP_OK "$out"
docker rm -f sr-corpus-registry >/dev/null
note 'item 1: pushed to local registry:2, removed local tag, pulled and ran the image'

note 'item 2: BuildKit and Buildx with cache'
mkdir -p /work/build
cat > /work/build/Dockerfile <<'EOF'
FROM alpine:3.24
RUN echo corpus-build-v1 > /marker
CMD ["/bin/cat", "/marker"]
EOF
DOCKER_BUILDKIT=1 docker buildx build --builder default --progress=plain \
  -t sr-corpus/build:v1 /work/build > /work/build1.log 2>&1 || \
  fail "first buildx build failed: $(tail -10 /work/build1.log)"
out=$(docker run --rm sr-corpus/build:v1)
assert_eq build-output corpus-build-v1 "$out"
DOCKER_BUILDKIT=1 docker buildx build --builder default --progress=plain \
  -t sr-corpus/build:v1 /work/build > /work/build2.log 2>&1 || \
  fail "cached buildx build failed: $(tail -10 /work/build2.log)"
grep -q CACHED /work/build2.log || \
  fail "no BuildKit cache hit on the second build: $(tail -20 /work/build2.log)"
note "item 2: BuildKit built an image and the second buildx build reused cache ($(docker buildx version | head -1))"

note 'item 6: Compose'
docker pull -q docker:29.9.3-cli >/dev/null
toolbox=$(docker create docker:29.9.3-cli true)
mkdir -p /usr/local/lib/docker/cli-plugins
docker cp "$toolbox:/usr/local/libexec/docker/cli-plugins/docker-compose" \
  /usr/local/lib/docker/cli-plugins/docker-compose >/dev/null
chmod 0755 /usr/local/lib/docker/cli-plugins/docker-compose
docker rm -f "$toolbox" >/dev/null
compose_version=$(docker compose version)
case "$compose_version" in *'Docker Compose version v'*) ;; *) fail "compose plugin unavailable: $compose_version" ;; esac
mkdir -p /work/compose
cat > /work/compose/compose.yaml <<'EOF'
services:
  alpha:
    image: alpine:3.24
    command: sleep 3600
  beta:
    image: alpine:3.24
    command: sleep 3600
EOF
( cd /work/compose && docker compose up -d ) > /work/compose-up.log 2>&1 || \
  fail "compose up failed: $(tail -10 /work/compose-up.log)"
count=$(docker ps -aq --filter label=com.docker.compose.project | wc -l | tr -d ' ')
[ "$count" -eq 2 ] || fail "compose-service-count: expected 2, got $count; $(cat /work/compose-up.log); $(docker ps -aq --format '{{.Names}} {{.Status}}')"
( cd /work/compose && docker compose down ) > /work/compose-down.log 2>&1 || \
  fail "compose down failed: $(tail -10 /work/compose-down.log)"
count=$(docker ps -aq --filter label=com.docker.compose.project | wc -l | tr -d ' ')
assert_eq compose-cleanup-count 0 "$count"
note "item 6: $compose_version started two services and down removed both containers"

note 'item 6: Testcontainers reaper lifecycle'
docker pull -q testcontainers/ryuk:0.11.0 >/dev/null
docker rm -f sr-corpus-ryuk sr-corpus-canary >/dev/null 2>&1 || true
docker run -d --name sr-corpus-ryuk \
  -v /var/run/docker.sock:/var/run/docker.sock -p 127.0.0.1:15001:8080 \
  testcontainers/ryuk:0.11.0 >/dev/null
docker run -d --name sr-corpus-canary -l org.testcontainers=true \
  alpine:3.24 sleep 3600 >/dev/null
"$node" /opt/docker-corpus/ryuk-client.js 15001
reaped=0
i=0
while [ "$i" -lt 45 ]; do
  if [ -z "$(docker ps -q --filter name=sr-corpus-canary)" ]; then reaped=1; break; fi
  i=$((i + 1))
  sleep 1
done
[ "$reaped" -eq 1 ] || fail "Testcontainers reaper did not remove the canary: $(docker logs sr-corpus-ryuk 2>&1 | tail -15)"
docker rm -f sr-corpus-ryuk >/dev/null
note 'item 6: Testcontainers ryuk reaped the labeled canary container after client disconnect'

note 'item 7: privileged docker:dind service with an inner daemon'
dind_build=$(mktemp -d /work/dind-build.XXXXXX)
for binary in dockerd docker containerd containerd-shim-runc-v2 ctr runc docker-init; do
  cp "/usr/local/bin/$binary" "$dind_build/$binary" || \
    fail "missing pinned binary for the local dind image: $binary"
done
cat > "$dind_build/Dockerfile" <<'EOF'
FROM alpine:3.24
COPY dockerd docker containerd containerd-shim-runc-v2 ctr runc docker-init /usr/local/bin/
RUN mkdir -p /var/lib/docker /run/containerd
ENTRYPOINT ["dockerd"]
CMD ["--iptables=false","--ip6tables=false","--userland-proxy=false","--bridge=none","--storage-driver=vfs"]
EOF
DOCKER_BUILDKIT=1 docker buildx build --builder default -q -t sr-corpus/dind "$dind_build" \
  > /work/dind-build.log 2>&1 || \
  fail "local dind image build failed: $(tail -10 /work/dind-build.log)"
docker rm -f sr-corpus-dind >/dev/null 2>&1 || true
docker run -d --privileged --name sr-corpus-dind sr-corpus/dind \
  > /work/dind-create.log 2>&1 || \
  fail "dind container did not start: $(tail -10 /work/dind-create.log)"
ready=0
i=0
while [ "$i" -lt 60 ]; do
  if docker exec sr-corpus-dind docker info >/dev/null 2>&1; then ready=1; break; fi
  i=$((i + 1))
  sleep 1
done
[ "$ready" -eq 1 ] || fail "inner dind daemon never became ready: $(docker logs sr-corpus-dind 2>&1 | tail -20)"
inner=$(docker exec sr-corpus-dind docker info --format '{{.ServerVersion}}')
[ -n "$inner" ] || fail 'inner daemon reported no version'
docker save alpine:3.24 -o /work/alpine.tar
docker cp /work/alpine.tar sr-corpus-dind:/alpine.tar >/dev/null
docker exec sr-corpus-dind docker load -i /alpine.tar >/dev/null
out=$(docker exec sr-corpus-dind docker run --rm alpine:3.24 echo DIND_INNER_OK)
assert_eq dind-inner-container DIND_INNER_OK "$out"
docker rm -f sr-corpus-dind >/dev/null
note "item 7: inner daemon $inner started and created a container"

note 'item 8: cancellation'
cat > /work/cancel-job.sh <<'EOF'
#!/bin/sh
trap 'docker rm -f sr-corpus-cancel >/dev/null 2>&1 || true; exit 0' TERM INT HUP
docker run -d --name sr-corpus-cancel alpine:3.24 sleep 3600 >/dev/null
printf '%s\n' started
while :; do sleep 1; done
EOF
chmod +x /work/cancel-job.sh
/work/cancel-job.sh > /work/cancel-job.log 2>&1 &
job=$!
wait_for 'cancellation job container' docker inspect sr-corpus-cancel
kill -TERM "$job"
wait "$job" || true
i=0
while [ "$i" -lt 15 ]; do
  [ -z "$(docker ps -aq --filter name=sr-corpus-cancel)" ] && break
  i=$((i + 1))
  sleep 1
done
[ -z "$(docker ps -aq --filter name=sr-corpus-cancel)" ] || \
  fail 'cancellation leaked a container'
note 'item 8: cancellation removed the in-flight container'

note 'item 8: daemon failure and restart'
docker run -d --name sr-corpus-persist alpine:3.24 sleep 3600 >/dev/null
dpid=$(cat /work/dockerd.pid)
kill -9 "$dpid"
i=0
while [ "$i" -lt 30 ]; do
  docker ps >/dev/null 2>&1 || break
  i=$((i + 1))
  sleep 1
done
if docker ps >/dev/null 2>&1; then fail 'daemon still reachable after kill -9'; fi
start_daemon keep
docker inspect sr-corpus-persist >/dev/null || \
  fail 'container state was lost across the daemon restart'
docker rm -f sr-corpus-persist >/dev/null
note 'item 8: killed daemon failed commands, restarted daemon preserved container state'

note 'item 8: disk full'
avail=$(df -Pk /var/lib/docker | awk 'NR == 2 {print $4}')
case "$avail" in ''|*[!0-9]*) fail "cannot read data-root free space: $avail" ;; esac
[ "$avail" -gt 100000 ] || fail "unexpected data-root free space: $avail KiB"
filler_size=$((avail - 4096))
fallocate -l "$((filler_size * 1024))" /var/lib/docker/filler || \
  fail 'fallocate could not fill the data root'
set +e
docker pull hello-world > /work/diskfull.log 2>&1
pull_code=$?
set -e
[ "$pull_code" -ne 0 ] || fail 'pull succeeded with a full data root'
grep -qiE 'no space left on device|insufficient disk' /work/diskfull.log || \
  fail "disk-full error had no cause: $(tail -5 /work/diskfull.log)"
rm -f /var/lib/docker/filler
note "item 8: disk-full pull failed with: $(grep -iE 'no space left on device|insufficient disk' /work/diskfull.log | head -1)"

remaining=$(docker ps -aq --filter name=sr-corpus- | wc -l | tr -d ' ')
[ "$remaining" -eq 0 ] || fail "corpus left $remaining containers"
note 'corpus: local items complete'
