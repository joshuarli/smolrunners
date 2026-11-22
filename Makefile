PYTHON ?= sh bootstrap/buildlib-run --
SHELL := /bin/sh

.PHONY: setup fetch check check-fetch test-fast test-vm test-vm-all test-faults web-test bench \
	live-pilot live-scaleset-probe live-corpus live-faults live-alpine pilot-bundle

setup:
	sh scripts/setup-dev

fetch:
	$(PYTHON) scripts/fetch-inputs.py

check-fetch:
	$(PYTHON) -m unittest discover -s tests -p 'test_*.py' -v

check: check-fetch
	cargo build --workspace
	cd adapter && go build ./...

test-fast:
	cargo test --workspace
	cd adapter && go test ./...

vm-binaries:
	sh scripts/build-vm-binaries.sh

test-vm: vm-binaries
	@test -n "$(T)" || { echo "usage: make test-vm T=<name>" >&2; exit 2; }
	sh scripts/linux-host.sh up
	sh scripts/linux-host.sh sync
	sh scripts/linux-host.sh exec 'sh tests/vm/$(T).sh'

PARKED :=
HELPERS := telemetry_spool

test-vm-all:
	@for f in tests/vm/*.sh; do \
	  name=$$(basename $$f .sh); \
	  case " $(PARKED) " in *" $$name "*) echo "skipping parked $$name"; continue ;; esac; \
	  case " $(HELPERS) " in *" $$name "*) echo "skipping helper $$name"; continue ;; esac; \
	  $(MAKE) test-vm T=$$name || exit 1; \
	done

test-faults:
	sh tests/faults/run.sh

web-test:
	cd web && npm ci && npm test && npm run build

bench:
	@echo "bench: not implemented until X1-bench" >&2; exit 2

SMOLRUNNERS_BIN ?= /opt/smolrunners/bin/smolrunners
ARCH ?= x86_64

pilot-bundle:
	ARCH=$(ARCH) TMPDIR=$(CURDIR)/.work/tmp sh scripts/pilot-bundle.sh

live-pilot:
	@test -n "$(SMOLRUNNERS_PILOT_CONFIG)" || { echo "live-pilot: set SMOLRUNNERS_PILOT_CONFIG to a pilot TOML whose adapter.github_config names a GitHub App key file outside the repository; see docs/pilot.md" >&2; exit 2; }
	@test -r "$(SMOLRUNNERS_PILOT_CONFIG)" || { echo "live-pilot: cannot read $(SMOLRUNNERS_PILOT_CONFIG)" >&2; exit 2; }
	$(SMOLRUNNERS_BIN) serve --pilot "$(SMOLRUNNERS_PILOT_CONFIG)"

live-scaleset-probe live-corpus live-faults live-alpine:
	@echo "$@: needs a GitHub App key file; set SMOLRUNNERS_GITHUB_APP_FILE to a key file outside the repository (O1)" >&2; exit 2
