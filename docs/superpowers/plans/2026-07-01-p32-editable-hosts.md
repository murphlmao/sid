# Editable Host List (P3.2) — Implementation Plan (Plan 3A)

> Execution guide. Runs **in parallel** with Plan 3B (`2026-07-01-p33-ssh-adapter-port.md`);
> the two touch disjoint crates except the workspace `Cargo.toml`. For agentic workers:
> execute task-by-task with a review between tasks; steps are checkboxes.

**Goal:** The SSH tab's host list becomes fully editable — add/edit/delete hosts with a
`save to: workspace | global` dialog (persisted, configurable `default_scope`), auth
fields (agent / key / password) whose secrets land in the OS keyring, and promote/demote
buttons per row — exercising the store's whole write side from the GUI.

**Architecture:** Extend `sid-store`'s facade with the missing write ops (delete, settings)
and a versioned `Host` schema bump (auth method); lift the POC's `KeyringStore` into
`sid-secrets`; build the GPUI form on a ported single-line text-input element (gpui 0.2
ships none). Store logic is unit-tested; rendering is observation-gated.

**Tech:** Rust 2024 · gpui 0.2.2 · sid-store (redb/postcard/TOML) · keyring-core 1 +
zbus-secret-service-keyring-store 1 (Linux).

## Global Constraints
- **Attributive — never override.** No write path may silently clobber a same-identity
  record. Add-mode writes into a layer that already holds the alias are rejected with a
  visible error; only explicit *edit* upserts. New write paths get a guard + a test.
- **Secrets are never committed**: TOML/redb carry only an opaque `secret_ref`; bytes go
  through `sid_secrets::SecretStore`. GPUI is named **only** in `crates/sid`.
- **postcard is positional** — never `skip_serializing_if` on stored entities; schema
  changes go through the codec **version byte** (`codec.rs`) with an explicit migration.
- Identity-level prefs (here: `default_scope`) are **always global**, never layered.
- Pragmatic testing: targeted tests per task; UI tasks gated by observation; **hard gate
  at A7**. Commits: no Claude trailer; push gate-green to `main`.

## Salvage references (from `~/vcs/sid-poc`, read-only)
- Keyring impl (lift, adapt to new trait): `crates/sid-secrets/src/keyring_store.rs` —
  `KeyringStore`, durability probe, `FakeKeyring`. Dep pins + load-bearing feature notes:
  POC root `Cargo.toml:110-132` (`keyring-core = "1"`,
  `zbus-secret-service-keyring-store = { version = "1", features = ["rt-async-io-crypto-rust"] }`
  — the zbus store **must be registered as default store at startup or every op fails**).
- Opaque id minting (crib if suitable): `crates/sid-core/src/id.rs`.
- Text input (port, trim): `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-0.2.2/examples/input.rs`
  (~746 lines; `EntityInputHandler` impl with IME, cursor, selection).

## Tasks

### A1 — `Store::delete_host` (write-side completion)
- **Files:** `crates/sid-store/src/store.rs`; tests in `crates/sid-store/tests/store.rs`.
- **New:** `pub fn delete_host(&self, alias: &str, scope: &Scope) -> Result<bool>` —
  Global → `self.global.remove_host(alias)`; Workspace → `self.workspace_store(id)?.remove_host(alias)`.
  Removes **exactly the named layer's record** (deleting a workspace copy un-shadows the
  global copy in the collapsed view — that is attributive behavior, not loss).
- **Tests:** delete from workspace leaves global intact (and vice versa); composed read
  reflects it; missing alias → `Ok(false)`; unregistered workspace → error.
- **Deliverable:** the facade's CRUD is symmetric (`write`/`read`/`delete` + move ops).

### A2 — `Settings` + persisted `default_scope`
- **Files:** `crates/sid-store/src/{entities,global,store}.rs`; tests in `tests/global.rs`.
- **New:** `#[derive(..., Default)] pub struct Settings { #[serde(default)] pub default_scope: DefaultScope }`;
  `pub enum DefaultScope { #[default] Ask, Workspace, Global }`. New redb table
  `SETTINGS: TableDefinition<&str, &[u8]> = "settings"` (single key `"settings"`, versioned
  postcard V1, opened in `GlobalStore::open` alongside the others at `global.rs:43-46`).
  `GlobalStore::{get_settings() -> Result<Settings>` (missing → `Settings::default()`),
  `set_settings(&Settings)}`; facade passthroughs `Store::settings()` / `Store::set_settings()`.
- **Tests:** missing key → default (`Ask`); round-trip; persists across reopen.
- **Deliverable:** the save-to dialog has a durable home for its preselection.

### A3 — `Host` schema v2: `AuthMethod` *(critical path — migration)*
- **Files:** `crates/sid-store/src/{entities,global}.rs`; tests in `tests/roundtrip.rs`, `tests/global.rs`.
- **New:** on `Host`: `#[serde(default)] pub auth: AuthMethod` with
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
  pub enum AuthMethod {
      #[default]
      Agent,
      Password,              // password bytes in keyring under secret_ref
      Key { path: String },  // key path committed; passphrase (if any) in keyring under secret_ref
  }
  ```
  redb migration: `HOST_VERSION: u8 = 2`. Host reads branch on the codec version byte:
  `1` → decode private `HostV1` (the current 5-field shape) → `From<HostV1> for Host`
  (`auth: Agent`); `2` → decode `Host`. Host writes encode with version 2; other entities
  stay `V1` (`global.rs:28`). TOML needs no migration (`#[serde(default)]` is honored by
  self-describing formats; postcard is why redb needs the branch).
- **Tests:** craft a v1 payload via `encode_versioned(1, &HostV1 {..})`, decode → `auth == Agent`;
  v2 round-trip for all three variants; TOML round-trip for all three variants; TOML file
  *without* an `auth` key parses; a seeded v1 redb store reopens and lists cleanly.
- **Deliverable:** stored hosts carry an auth method; existing stores migrate on read.

### A4 — `KeyringStore` lift into `sid-secrets`
- **Files:** `crates/sid-secrets/src/{lib,keyring}.rs`, `crates/sid-secrets/Cargo.toml`;
  workspace `Cargo.toml` (deps above); bootstrap wiring in `crates/sid/src/app.rs` (`open_store`
  area, `app.rs:363-371`).
- **Lift:** POC `keyring_store.rs` — the zbus-secret-service store registration, the
  **durability probe** (put/get/delete a canary at startup), and `FakeKeyring` — adapted to
  implement the *new* `SecretStore` trait (`sid-secrets/src/lib.rs:40-49`).
- **New:** `pub fn open_default_secrets() -> (Box<dyn SecretStore>, Option<String>)` —
  keyring if the probe passes, else `MemorySecretStore` + a warning string the app surfaces
  in its error line. Mint ids as `ssh-<alias>-<unix_nanos>` (opaque; uniqueness across
  same-alias dup records in different layers).
- **Tests:** `FakeKeyring` put/get/delete/list; probe-failure path returns the memory
  fallback + warning. (Real zbus store is exercised by observation on this machine.)
- **Deliverable:** `secret_ref` stops being inert — there is a durable backend behind it.

### A5 — single-line `TextInput` element *(the P3.2 risk — spike first)*
- **Files:** `crates/sid/src/ui/mod.rs`, `crates/sid/src/ui/text_input.rs`; keybindings in
  `crates/sid/src/main.rs`.
- **Port:** gpui's `examples/input.rs` trimmed to a reusable single-line input entity:
  `EntityInputHandler` (IME-correct), focus handle, cursor + selection, mouse
  click/drag-to-position, Backspace/Delete/arrows/Home/End, paste; a `masked: bool` mode
  rendering bullets for secret fields. Strip the example's window scaffolding and demo `main`.
- **Gate by observation** (rendering-spike rule): a scratch overlay with one input — type,
  select, IME compose, paste, mask — before any form work builds on it.
- **Deliverable:** a reusable, focused text input the form (and later tabs) consume.

### A6 — host form modal + save-to dialog *(critical path — write flow)*
- **Files:** `crates/sid/src/ui/host_form.rs`; wiring in `crates/sid/src/app.rs`
  (state fields at `app.rs:69-77`, "+ Add host" button in the ssh_tab header at
  `app.rs:248-261`, modal overlay in `render` at `app.rs:327-344` via `deferred`/`anchored`).
- **New:** `HostForm` entity — fields alias/user/host/port (`TextInput`s), an auth-method
  segmented selector (`agent | key | password`) with conditional inputs (key path +
  optional passphrase, or password, masked), and the **`save to:`** selector
  (`workspace (.sid/ · travels with git) | global (everywhere · never lost)`), preselected
  from `Settings::default_scope` (`Ask` → no preselection; the dialog itself always shows).
  Workspace option enabled only when a workspace scope is active.
  - **Validation before write:** alias/user/host non-empty; port parses 1–65535; key path
    non-empty when `Key`.
  - **Add-mode guard (attributive):** if the target layer already holds the alias
    (`global().get_host` / workspace load), reject with "alias exists in ⌂ global — edit it
    instead"; only edit-mode upserts. Edit mode prefills and locks the alias field (no rename).
  - **Secrets:** on save, password/passphrase bytes → `SecretStore::put` under a minted id →
    `host.secret_ref`; on edit that clears/changes auth, delete the old id. Then
    `write_host` → `refresh()` → `cx.notify()`; store errors land in the form's error line.
- **Tests:** validation + add-guard logic as plain functions (no gpui); secret-lifecycle
  (mint on save, delete on replace) against `FakeKeyring`. Rendering by observation.
- **Deliverable:** add + edit work end-to-end into either layer, secrets in the keyring.

### A7 — row actions: edit · delete · promote · demote
- **Files:** `crates/sid/src/app.rs` (`host_row` at `app.rs:274-314`, next to the origin badge).
- **New:** per-row buttons — ✎ edit (opens A6 prefilled), ✕ delete (two-click confirm on
  the row; calls `delete_host` for the row's **origin** layer and deletes its `secret_ref`
  from the keyring), ⤒ promote (workspace-origin rows), ⤓ demote (global-origin rows,
  workspace scope only). Promote/demote call the existing guarded store ops;
  `StoreError::Conflict` is surfaced verbatim in the header error line — the demo seed's
  duplicate `vps-1` (`app.rs:406`) makes this the *first* click's behavior, not an edge case.
- **Tests:** store behaviors already covered (Plan 2); button→store-call routing logic
  factored testable where trivial; the rest by observation (see A8 checklist).
- **Deliverable:** every host row is fully manageable in place.

### A8 — Gate (end of P3.2)
- `cargo test`, `clippy`, `fmt` across the workspace (check exit codes, not `| tail`).
- Adversarial review focused on: A3 migration correctness, secret lifecycle (assert no
  secret bytes ever appear in `.sid/config.toml` or the redb file — grep-style test),
  delete paths, add-mode guard.
- Observation checklist: add (both layers, all three auth methods) · edit · delete ·
  promote/demote incl. `vps-1` conflict · `default_scope` preselection honored · scope
  switch refreshes · error surfaces visible.
- **Deliverable:** P3.2 shippable; HANDOFF.md updated (P3.2 ✅, pointer to Plan 3C next).
