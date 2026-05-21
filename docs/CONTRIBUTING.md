# Contributing

`sid` is a personal tool I work on for myself, but contributions are
welcome — fixes especially, features after a quick chat. The rules
below keep the code base shaped the way it is.

## Before you contribute

- **Read [`CLAUDE.md`](../CLAUDE.md).** It is the binding rules document
  for this project: testing rigour, adapter pattern enforcement,
  adversarial coverage. The rules apply to humans and AI assistants
  alike.
- **Read [ARCHITECTURE.md](ARCHITECTURE.md).** The crate boundaries are
  intentional. Any contribution that crosses them needs a discussion
  first.
- **Tests land in the same commit as code.** A commit that adds a
  feature without tests will be returned for the missing tests, even
  if the feature is correct.
- **`cargo clippy --all-targets --all-features -- -D warnings` must be
  green** before you push. So must `cargo fmt --check` and
  `cargo test --workspace --all-features`.

## Filing an issue

Open an issue at <https://github.com/murphlmao/sid/issues> with:

- **`sid` version** — `sid --version` (or the commit hash you built).
- **OS and terminal** — distro + version, terminal emulator + version.
  `$COLORTERM` value if you have a rendering issue.
- **What you ran** — the exact CLI invocation.
- **What happened** — error message verbatim, screenshot if it's
  visual, copy of the relevant `~/.local/state/sid/crash-*.log` if it
  panicked.
- **What you expected.**
- **Reproducible steps.** If a fresh DB is needed,
  `sid --db /tmp/repro.redb` keeps your real one untouched.

For security issues (anything that could leak data or escalate
privileges), don't open a public issue — email
murphyjmalcolm@gmail.com directly.

## Submitting a PR

1. **Branch from `main`.** Feature branches are preferred for anything
   non-trivial.
2. **One logical change per PR.** Multiple unrelated fixes belong in
   multiple PRs. Reviewers should be able to read the whole diff
   without losing the thread.
3. **Conventional commit prefixes.** See
   [DEVELOPMENT.md → Conventional commits](DEVELOPMENT.md#conventional-commits)
   for the grammar. A commit body that explains *why* matters more
   than one that lists *what*.
4. **No `Co-Authored-By: Claude` trailer.** Murphy has opted out — if
   you used an AI assistant, the work and the byline are yours.
5. **Tests in the same commit.** See
   [TESTING.md → What to test for a new feature](TESTING.md#what-to-test-for-a-new-feature)
   for the checklist.
6. **Doc test on every new `pub` item.** This is non-negotiable.
7. **CI must be green.** PRs run `cargo fmt --check`,
   `cargo clippy --all-targets --all-features -- -D warnings`,
   `cargo test --workspace --all-features`, and `cargo deny check`.
   The status checks must all pass before merge.
8. **Adapter pattern stays intact.** If your change ends up making a
   widget crate depend on `git2`, `russh`, `redb`, or any external
   library directly, that is a structural bug. The fix is usually a
   new method on the relevant trait in `sid-core::adapters`.

## Code style

```sh
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
```

The `rustfmt.toml` has a few nightly-only options (`imports_granularity`,
`group_imports`). On stable they are silently ignored — your changes
will still format correctly for the project. Use nightly rustfmt locally
if you want them honoured (`cargo +nightly fmt`).

Other style notes:

- **No emojis in code.** The cosmos theme uses `✦` / `·` / `★` glyphs
  intentionally; that is the only place emoji-adjacent characters
  belong.
- **No `unwrap()` in production code.** Use `?`, `expect("msg")` with a
  reason, or pattern matching. `unwrap()` is fine in tests.
- **`Result<T, SidError>` for all crate-public functions that can fail.**
  `anyhow::Result` is reserved for binary-crate top-level glue.
- **Prefer borrowing over cloning.** If your change adds a `.clone()` to
  a hot path, the commit body needs to justify it.

## Review and merge

Murphy reviews. Typical turnaround is a day or two. Expect feedback
that is sometimes more about shape than substance — the adapter pattern
and testing rigour are the project's identity.

Once reviewed and CI is green, merge is fast-forward or squash
depending on the PR shape. Force-pushes to PR branches during review
are fine; the reviewer's comments stay attached via GitHub's commit-SHA
references.

Thanks for contributing.
