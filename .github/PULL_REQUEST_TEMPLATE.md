## What and why

<!-- What changes, and why it is needed. Link the issue if there is one. -->

## How it was checked

<!-- Tests added, commands run. -->

- [ ] `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test` pass
- [ ] New behaviour has a test that fails without it
- [ ] No real log, query text or customer data in fixtures or messages
