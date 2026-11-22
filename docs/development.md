# Development

## Tools

Setup installs nothing system-wide. The Python interpreter and `laputa-buildlib` wheel come from
the vendored `bootstrap/buildlib-run` shim; all other tools are expected on the host.

| Tool | Version | Consumer | Slice |
| --- | --- | --- | --- |
| Rust + Cargo | 1.99.1 | all Rust crates | P0-ENV |
| Go | 1.27.3 | `adapter/` | P0-ENV |
| Python (shim) | 3.15.1 | `scripts/fetch-inputs.py`, `scripts/install-runtime.py` | P0-ENV |
| laputa-buildlib (shim) | 0.2.0 | pinned-input reader and fetcher | P0-ENV |
| smolvm darwin runtime | 1.36.0 | nested Linux host | P0-ENV |
| smolvm linux runtime | 1.36.0 | inner job guests | P0-ENV |
| Docker static bundle | 29.9.3 | image recipes | I1 |
| Buildx | 0.39.1 | image recipes | I1 |
| Node | 22.23.5 | `web/` and runner host steps | W1, L2 |
| .NET SDK | 10.0.404 | musl runner spike | L2 |
| `gh` | any | read-only pin resolution | P0-ENV |

## Commands

```sh
make setup          # idempotent host checks and scratch dirs
make fetch          # download and verify every pinned input through buildlib
make check          # fetch-script boundary tests, cargo build, go build
make test-fast      # cargo test --workspace and go test ./... (no VM)
make test-vm T=smoke  # sync the tree into sr-dev and run tests/vm/smoke.sh
```

Scratch and downloaded inputs live under `.work/`, which is ignored through `.git/info/exclude`.
`TMPDIR` for all repository commands is `.work/tmp`.

Guest binaries are build artifacts, not source. They are ignored by git; `make test-vm` runs
`scripts/build-vm-binaries.sh` to cross-compile them from the current checkout into
`tests/vm/bin/`, `tests/cli/bin/` and `crates/smolrunners/src/host/cache/` before every VM gate.
A fresh checkout therefore has no binary fixtures until a VM target (or the script directly) is
run. Do not commit those files: they dominated the repository size and made pushes impractical.

## Adding an input

Add one entry to `sources.lock.json` with `name`, `version`, `url`, `sha256`, `size`, `target`,
`license` and `purpose`, then add its evidence row to `docs/upstream-evidence.md`. No code may
download an HTTPS archive outside `scripts/fetch-inputs.py`.
