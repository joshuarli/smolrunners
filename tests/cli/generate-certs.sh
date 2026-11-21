#!/bin/sh
set -eu

out=${1:?usage: generate-certs.sh OUTPUT_DIR}
mkdir -p "$out"
cd "$out"

openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
  -keyout ca.key -out ca.crt -subj '/CN=smolrunners-test-ca' \
  -addext 'basicConstraints=critical,CA:TRUE' \
  -addext 'keyUsage=critical,keyCertSign,cRLSign' >/dev/null 2>&1

issue() {
  name=$1
  common_name=$2
  usage=$3
  san=$4
  openssl req -new -newkey rsa:2048 -nodes -keyout "$name.key" -out "$name.csr" \
    -subj "/CN=$common_name" >/dev/null 2>&1
  cat > "$name.ext" <<EOF
basicConstraints=CA:FALSE
keyUsage=digitalSignature
extendedKeyUsage=$usage
subjectAltName=DNS:$san
EOF
  openssl x509 -req -in "$name.csr" -CA ca.crt -CAkey ca.key -CAcreateserial \
    -out "$name.crt" -days 1 -extfile "$name.ext" >/dev/null 2>&1
}

issue server server serverAuth localhost
issue operator operator:admin clientAuth operator
issue host-a host:sr-host-a clientAuth host-a
issue host-b host:sr-host-b clientAuth host-b
