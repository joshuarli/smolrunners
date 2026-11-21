# smolrunners repository instructions

Follow the owner's working contract and `smolrunners-campaign.md`. Scratch goes in `.work/tmp` with
`TMPDIR` set to it. Never use `/tmp` or the system `$TMPDIR`. Never push. No formatter is configured.

## Commands

| Command | Purpose |
| --- | --- |
| `make setup` | Idempotent host checks; installs nothing system-wide |
| `make fetch` | Download and verify every pinned input through the buildlib shim |
| `make check` | Fetch-script boundary tests, `cargo build --workspace`, `go build ./...` |
| `make test-fast` | `cargo test --workspace` and `go test ./...`, no VM |
| `make test-vm T=<name>` | Start `sr-dev`, sync the tree, run `tests/vm/<name>.sh` in the guest |
| `make test-vm-all` | Every `tests/vm/*.sh` |
| `make live-*` | Run on the owner's host; without credentials exits 2 naming the input |

Pinned inputs are declared in `sources.lock.json` and justified in `docs/upstream-evidence.md`.
OCI images are pinned by digest in `images/*/image.toml`. Download only through
`scripts/fetch-inputs.py`.

## Verification table (warm, this M5 Mac, 2026-11-21)

| Check | Warm cost | Gate |
| --- | --- | --- |
| `buildlib-run` shim | 4.3 s | — |
| `fetch_input` of the darwin bundle, verified | 1.8 s | runs from cache when verified |
| `make check` (fetch tests + builds) | 0.41 s | 10 min |
| `make test-vm T=smoke` (two inner guests) | 20.8 s | 10 min |
| `make test-vm T=image_docker` (local Docker corpus) | 735 s | 10 min; exceeded in the nested host after all eight local items passed — nested-host timings are functional evidence, not gates |
| `make test-fast` | measured at P1 | 120 s |
| `cargo build` after a one-line change | measured at P1 | 60 s |
| inner guest `machine run … true` | 3.7–4.1 s | 15 s |
| a single unit test | measured at P1 | 5 s |

Numbers from the nested host are functional evidence and never performance gates.
