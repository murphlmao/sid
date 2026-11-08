# sid-store (layered, attributive store)

Global redb + per-workspace committed `.sid/config.toml`, composed attributively.

## Surface
- `Store` (`store.rs`) facade: scoped reads/writes, promote/demote, `read_hosts`, `settings()`.
- `GlobalStore` (`global.rs`, redb + postcard), `WorkspaceStore`/`WorkspaceConfig` (`workspace.rs`, TOML).
- `compose` + `ViewFilters` (`composer.rs`); `Scope`, `Attributed`, `WorkspaceId` (`scope.rs`).
- Entities (`entities.rs`): `Host`, `DbConnection`, `Settings`, `KeyBinding`, ... plus frozen
  `*V1/V2/...` shapes. `codec::{encode_versioned, decode_versioned}`; `StoreError`.

## Invariants
- Composition is a union tagged by origin; never shadow/override. Dedupe is a view filter.
- Secrets never stored here: only an opaque `SecretId` (`secret_ref`). The one related table is
  `secret_bindings` (global redb only, `SecretBinding`, `SECRET_BINDING_VERSION`): which layer and
  endpoint fingerprint a keyring secret was saved for. The policy lives in `crates/sid/src/secret_binding.rs`.
- postcard is positional: new fields `#[serde(default)]` only, never `skip_serializing_if`;
  bump `*_VERSION` in `global.rs`, keep every old `decode_*` arm, add a migration test.
- New write paths get a `StoreError::Conflict` guard + test (promote/demote model it).
- Workspace config is TOML and diffable; never put it in redb.

## Test
`nice -n 10 cargo test -p sid-store -j 3` — `tests/`: `compose.rs`, `compose_proptest.rs`,
`global.rs`, `roundtrip.rs`, `secret_boundary.rs`, `store.rs`, `workspace.rs`, `keybindings.rs`.
Use real redb in `tempfile` dirs, not mocks. Skill: `sid-store-migration`.

## Landmines
- redb is single-writer: a second open of the same file in one process fails/blocks.
- Entities migrated from old versions fill added fields with defaults — assert that in tests.
