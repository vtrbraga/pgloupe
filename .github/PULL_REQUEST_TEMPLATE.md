## What and why

<!-- What changes, and why it is needed. Link the issue if there is one. -->

## How it was checked

<!-- Tests added, commands run. -->

- [ ] `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test` pass
- [ ] New behaviour has a test that fails without it
- [ ] No real log, query text or customer data in fixtures, messages or screenshots
- [ ] `CHANGELOG.md` has an entry under *Unreleased* for anything a user would notice
- [ ] no changelog needed (nothing user-visible changed)
- [ ] If a command or flag changed: `cargo xtask generate` was run (man pages, completions, command reference)
- [ ] If behaviour or output changed: the page in `docs/` was updated
- [ ] The title is a Conventional Commit subject (`fix(decode): ...`, at most 50 characters)
