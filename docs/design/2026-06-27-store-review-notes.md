# sid-store — adversarial review notes (2026-06-27)

A 4-lens adversarial review (composition / secret-boundary / data-integrity / API-robustness)
ran against the finished `sid-store`. Nine findings; disposition below.

## Fixed (with tests)

- **HIGH — `promote`/`demote` silently overwrote a same-identity record in the destination
  layer.** Cross-layer duplicates are a *supported* state, so promoting/demoting a host whose
  alias already existed in the destination destroyed the destination's copy (and for demote,
  the *global* value survived — the opposite of "workspace wins"). Now both operations return
  `StoreError::Conflict` and change nothing; the caller (UI) resolves the conflict.
- **MEDIUM — `WorkspaceStore::save` used a non-atomic truncating write.** A crash or I/O error
  mid-write could truncate `config.toml`, making the *entire* workspace layer unreadable. Now
  writes to `config.toml.tmp` then `rename`s over the target (atomic on the same filesystem).
- **LOW — `hide_global` at `Global` scope yielded an empty view.** It's a workspace-mode
  filter; now a no-op when no workspace is focused.

## Deferred (conscious decisions, not current-code bugs)

- **MEDIUM — flat secret keyspace.** `secret_ref` is a human-authored string in a shared,
  machine-wide keyring namespace, so two repos that pick the same natural ref (e.g.
  `ssh.prod.key`) alias to the same secret. The store correctly keeps only the ref in config;
  the collision is a **naming-policy** decision for when real secrets get wired. Options:
  namespace the keyring key by `WorkspaceId`, or auto-generate unique refs (UUID) on creation.
  Decide during the SSH slice's secret wiring; the `SecretStore` trait is the seam.
- **LOW — `decode_versioned` doesn't reject unknown versions**, and `list()` aborts on the
  first undecodable value. Fine while every entity is v1; revisit using the POC's per-entity
  `match version { .. }` migration pattern when a v2 entity appears.
- **LOW — `WorkspaceId::from_root` uses `to_string_lossy`**, so pathological non-UTF8 Linux
  paths could collide. Acceptable; revisit only if it ever matters.
- **LOW — `MemorySecretStore` panics on mutex poisoning.** Acceptable for an in-memory
  test/fallback impl (poisoning already implies a panicked thread); the real keyring impl
  won't hold a process-wide `Mutex` this way.
