.PHONY: test-unit

test-unit:
	cargo fmt --all -- --check
	cargo test --locked
	node --test tests/*.test.mjs
