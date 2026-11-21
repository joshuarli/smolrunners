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

test-vm:
	@test -n "$(T)" || { echo "usage: make test-vm T=<name>" >&2; exit 2; }
	sh scripts/linux-host.sh up
	sh scripts/linux-host.sh sync
	sh scripts/linux-host.sh exec 'sh tests/vm/$(T).sh'

test-vm-all:
	@for f in tests/vm/*.sh; do \
	  $(MAKE) test-vm T=$$(basename $$f .sh) || exit 1; \
	done

test-faults:
	@echo "test-faults: not implemented until F1" >&2; exit 2

web-test:
	cd web && npm ci && npm test && npm run build

bench:
	@echo "bench: not implemented until X1-bench" >&2; exit 2

live-pilot live-scaleset-probe live-corpus live-faults live-alpine:
	@echo "$@: needs a GitHub App key file; set SMOLRUNNERS_GITHUB_APP_FILE to a key file outside the repository (O1)" >&2; exit 2
