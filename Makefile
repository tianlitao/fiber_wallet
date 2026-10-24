.PHONY: check fmt test lint recovery-smoke

check: fmt test lint

fmt:
	cd core && cargo fmt --all -- --check

test:
	cd core && cargo test

lint:
	cd core && cargo clippy --all-targets -- -D warnings

recovery-smoke:
	./scripts/recovery_smoke.sh
