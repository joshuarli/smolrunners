#!/bin/sh
set -eu

. /opt/docker-corpus/lib.sh
tag=${1:?usage: service.sh TAG}
node=/opt/node/bin/node

start_daemon wipe
assert_data_root
docker pull -q alpine:3.24 >/dev/null
marker="PORT_GUEST_$tag"
docker rm -f sr-corpus-web >/dev/null 2>&1 || true
docker run -d --name sr-corpus-web -p 18080:80 alpine:3.24 \
  /bin/sh -c "while :; do printf 'HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\n$marker' | nc -l -p 80 -w 2; done" \
  >/dev/null
wait_for "guest $tag service container" docker inspect sr-corpus-web
running=$(docker inspect --format '{{.State.Running}}' sr-corpus-web)
[ "$running" = true ] || fail "guest $tag service exited: $(docker logs sr-corpus-web 2>&1 | tail -10)"
out=$("$node" /opt/docker-corpus/http-get.js http://127.0.0.1:18080/ "$marker")
guest_ip=$("$node" -e 'const os=require("os"); for (const addrs of Object.values(os.networkInterfaces())) for (const a of addrs) if (a.family === "IPv4" && !a.internal) { console.log(a.address); process.exit(0); }')
[ -n "$guest_ip" ] || fail "guest $tag has no IPv4 address"
printf 'guest=%s port=18080 ip=%s response=%s\n' "$tag" "$guest_ip" "$out"
