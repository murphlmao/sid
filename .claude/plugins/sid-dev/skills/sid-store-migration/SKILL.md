---
name: sid-store-migration
description: Add or change a stored field on a sid-store entity (Host, DbConnection, Settings, ...) safely - postcard is positional, so it needs a frozen old shape, a version bump, a decode arm and a migration test. Use for any change to a struct that is persisted in redb or the workspace TOML.
---

# sid-store-migration

redb values are `postcard` with a leading version byte (`codec::encode_versioned`), and
postcard is **positional**: adding/removing/reordering fields silently corrupts old data
unless the version is bumped and the old shape kept. Never use `skip_serializing_if`.

## Steps (files under `crates/sid-store/src/`)

1. **Freeze the current shape** in `entities.rs` as `<Entity>V<N>` (N = current version;
   see `HostV1/HostV2`, `DbConnectionV1/V2`, `SettingsV1..V4` for the pattern): copy the
   struct, derive `Deserialize`, keep it forever.
2. **Change the live struct**: add the field with `#[serde(default)]` (or
   `#[serde(default = "fn")]` for non-zero defaults). Append new fields at the END.
3. **Add `impl From<<Entity>V<N>> for <Entity>`** filling the new field with its default.
4. **Bump** `<ENTITY>_VERSION` in `global.rs` (`HOST_VERSION`, `CONNECTION_VERSION`,
   `SETTINGS_VERSION`) and add a `N =>` arm to `decode_host` / `decode_connection` /
   `decode_settings` converting via `From` (chain through intermediate versions). Keep
   every older arm; unknown versions stay `StoreError::UnsupportedVersion`.
5. **Re-export** new `V<N>` types from `lib.rs` if tests need them.
6. **Workspace TOML** (`workspace.rs`): if the entity lives in `.sid/config.toml`, the field
   needs `#[serde(default)]` there too so older committed files still parse.
7. **Migration test** (red first): in `crates/sid-store/tests/global.rs` or `roundtrip.rs`,
   encode an old-version value with `encode_versioned(<old version>, &old_value)`, write it
   to the table (see existing tests for how raw bytes are inserted), read it back via the
   public API, and assert the old fields survive and the new field is its default. Also
   assert a round-trip at the new version.
8. **Secrets**: a secret value never becomes a field - store an opaque `SecretId`
   (`secret_ref`). `tests/secret_boundary.rs` must stay green.
9. New **write paths** (promote/rename/import) need a `StoreError::Conflict` guard + test.
10. `nice -n 10 cargo test -p sid-store -j 3`, then the full gate (`sid-gate`).
    Callers in `crates/sid` that build the entity literally need the new field.
