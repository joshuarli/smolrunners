PYTHON ?= sh bootstrap/buildlib-run --
SHELL := /bin/sh

.PHONY: setup fetch check check-fetch test-fast test-vm test-vm-all test-faults web-test bench \
	live-pilot live-scaleset-probe live-corpus live-faults live-alpine

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
ifeq ($(T),connected_job)
	TMPDIR=$(CURDIR)/.work/tmp sh tests/vm/connected_job.sh
else
	sh scripts/linux-host.sh up
	sh scripts/linux-host.sh sync
	sh scripts/linux-host.sh exec 'sh tests/vm/$(T).sh'
endif

PARKED := alpine_runner
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

live-pilot live-scaleset-probe live-corpus live-faults live-alpine:
	@echo "$@: needs a GitHub App key file; set SMOLRUNNERS_GITHUB_APP_FILE to a key file outside the repository (O1)" >&2; exit 2
