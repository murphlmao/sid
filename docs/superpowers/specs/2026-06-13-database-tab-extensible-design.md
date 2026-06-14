# Database Tab — SQLite file ops, redb config viewer, engine extensibility — Design

**Date:** 2026-06-13
**Owner asks:** (1) SQLite — open an existing file & connect, and create a new file; (2) view/connect to sid's own config store; (3) **architecture must scale to "a shit ton" of supported databases.**

## Current architecture (confirmed; recon verifying details)
- `sid_core::adapters::db_client`: `trait DbClient`, `enum DbKind { Postgres, Sqlite }`, `OpenParams { kind, dsn, password }`. `dsn` is a Postgres URL or a SQLite path (`:memory:`/file).
- Concrete impls in `sid-db-clients` (`PostgresClient`, `SqliteClient`); the binary holds them as **separate** `SidApp.postgres` / `SidApp.sqlite` fields → NOT extensible as-is.
- DB connection password keyed `db.connection.{id}.password` in the secret store.

## Decisions (owner)
- SQLite: open existing + create new.
- redb config viewer: yes (read-only) inside the DB tab.
- **Engine extensibility is a first-class requirement.**

## Design

### 1. Engine extensibility (the load-bearing part)
Replace per-engine `SidApp` fields with a **registry**:
- `DbKind` becomes the open extension point; adding an engine = (a) a `DbClient` impl in `sid-db-clients`, (b) a `DbKind` variant, (c) a **connection-param descriptor**, (d) one registry registration. No UI rewrites.
- `trait DbClient` gains (or a sibling trait provides) a `descriptor() -> ConnectionDescriptor` declaring the fields the connect form needs: ordered `Vec<ConnField { key, label, kind: Text|Port|Path|Password|Choice|Bool, required, default }>` and how to assemble the `dsn`/`OpenParams` from collected values. The connection **form is generated from the descriptor** — no hardcoded Postgres layout.
- `DbClientRegistry`: `DbKind -> Arc<dyn DbClient>` + descriptor lookup. Built once in the binary; the widget asks the registry "what engines exist? what fields does engine X need?" via a trait, never naming concrete crates (adapter rule).
- Acceptance: adding e.g. MySQL later touches only `sid-db-clients` (+ the `DbKind` enum + one registry line). The Database widget code does not change.

### 2. SQLite open + create
- SQLite descriptor: a `Path` field (the .sqlite file) + a `Choice` open-mode `{ open_existing, create_new }`.
  - `open_existing`: error if the file does not exist.
  - `create_new`: error if it exists; otherwise create (SQLite creates on first open; ensure parent dir exists; for create, touch/initialize).
- `OpenParams { kind: Sqlite, dsn: <path>, password: None }`. (Validate path; expand `~`.)

### 3. redb config viewer (read-only)
- sid's config is **redb** (KV), not SQL — it cannot be a `DbClient`. Model it as a separate read-only capability:
  - New `trait ConfigStoreReader` in `sid-core` (e.g. `tables() -> Vec<String>`, `scan(table) -> Vec<(KeyRepr, ValueRepr)>`), impl in **`sid-store`** (the only crate allowed to touch redb). Values decoded best-effort (utf8 / hex / "versioned-postcard (N bytes)").
  - The DB tab gets a **browse view** (tree: tables → entries) distinct from the SQL query pane. A special connection entry "sid config (redb, read-only)" is **auto-added** to the connection list on startup and opens the browse view.
  - Strictly read-only; no writes to the live config store from the DB tab.

### 4. Connection list / secrets
- New connections persist via the existing connection store; passwords (for engines that need them) keyed `db.connection.{id}.password`. SQLite/redb need no password.

## Tests (scoped: `cargo test -p sid-db-clients`, `-p sid-store config_reader`, `-p sid-widgets database`, `-p sid`)
- SQLite: open-existing missing-file error; create-new existing-file error; create-new makes a usable DB; round-trip a trivial query.
- Descriptor-driven form: Postgres descriptor yields its fields; SQLite descriptor yields path+mode; a dummy 3rd engine proves the form adapts with zero widget changes (extensibility regression guard).
- Registry: lookup by `DbKind`; unknown kind handled.
- `ConfigStoreReader` (sid-store, critical path): lists real tables; scans a seeded table; value decode for utf8/binary; read-only (no mutation method exists).
- redb viewer auto-added to the connection list; opens browse view; never writes.

## Recon-confirmed implementation detail (2026-06-13)
Architecture assessment confirms the above and pins specifics:

- **Current blockers (file:line):** per-engine fields `SidApp.postgres`/`.sqlite` (`wire.rs:240-244`); hardcoded factory match in `spawn_test_connection` (`wire.rs:7145`); form `make_sections`/`build_dsn` hardcoded to Postgres+SQLite (`wire.rs:6790-7000`); kind label match in `database.rs:837`. `DbClient` (`db_client.rs:260-305`) has `kind()` but **no descriptor method**. `DbConnection` (`sid-store/src/lib.rs:687`) + secret key `db.connection.{id}.password` are already engine-agnostic — keep.
- **Phase 1 (blocker, do first): registry + descriptor.** Add `ConnField`/`ConnFieldKind`/`trait DbClientDescriptor { connection_fields(), label(), assemble_params(values)->OpenParams }` to `sid-core/db_client.rs`; add `fn descriptor(&self)->&'static dyn DbClientDescriptor` to `DbClient`. Add `PostgresDescriptor`/`SqliteDescriptor` in `sid-db-clients`. Replace the two `SidApp` fields with `db_registry: DbClientRegistry` (`DbKind -> (Arc<dyn DbClient>, &'static dyn DbClientDescriptor)`). Rewrite the connection form to render from `descriptor.connection_fields()` and submit via `assemble_params`. **Regression guard:** register a dummy 3rd engine in a test and assert the form adapts with zero widget changes.
- **Phase 2: SQLite modes.** Add `OpenParams.sqlite_mode: Option<SqliteMode { OpenExisting, CreateNew }>` (clean, not DSN-encoded). `SqliteClient::open` (`sid-db-clients/src/sqlite.rs:48`): OpenExisting errors if missing; CreateNew errors if exists, else `create_dir_all(parent)` then open. SQLite descriptor exposes a `path` Path field + a `mode` Choice (default open_existing).
- **Phase 3: redb viewer.** `trait ConfigStoreReader { list_tables(), scan_table(name)->Vec<(String,String)> }` + `ConfigError` in `sid-core`; impl on `RedbStore` in `sid-store/src/config_reader.rs` (redb tables are in `sid-store/src/schema.rs`; values rendered best-effort utf8/hex/"versioned-postcard: N bytes"). Add a `DbKind::ConfigReader` (or `Redb`) variant + auto-insert a read-only "sid config (redb)" connection; `database.rs` gains a `RightPane::ConfigBrowse` tree view (distinct from the SQL results grid). No write path.
- **Phase order:** 1 → (2 ∥ 3). Phase 1 unblocks everything; 2 and 3 are independent after it.

## Out of scope (now)
- Actually implementing many engines (MySQL/etc.) — only the architecture + SQLite + redb viewer. Adding engines is follow-up, made cheap by §1.
- Writing to the redb config store from the DB tab.
