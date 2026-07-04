//! `WorkspaceStore` — the per-workspace layer: a committed `.sid/config.toml`.
//!
//! This is the git-centric half: one human-readable, diffable TOML file per workspace,
//! living in the repo so it travels with a clone. It holds only config — secrets are
//! referenced by an opaque `secret_ref` and never written here. A **missing file is an
//! empty layer, not an error** (a fresh workspace simply has nothing yet).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::entities::{DbConnection, Host, Identity, QuickAction};
use crate::error::{Result, StoreError};

/// The committed workspace config document. Field names map directly to TOML keys:
/// `[[ssh.host]]`, `[[db.connection]]`, `[[quick_action]]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    /// Schema version of this file.
    pub version: u32,
    /// `[ssh]` section (holds `[[ssh.host]]`).
    pub ssh: SshSection,
    /// `[db]` section (holds `[[db.connection]]`).
    pub db: DbSection,
    /// Top-level `[[quick_action]]` array.
    pub quick_action: Vec<QuickAction>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            version: 1,
            ssh: SshSection::default(),
            db: DbSection::default(),
            quick_action: Vec::new(),
        }
    }
}

impl WorkspaceConfig {
    /// Diagnostic: identities that appear more than once in this loaded layer.
    ///
    /// TOML parses two `[[ssh.host]]` entries with the same `alias` (or two
    /// `[[db.connection]]` entries with the same `id`) just fine — a normal git-merge
    /// artifact (both sides add a host, merge keeps both blocks). [`WorkspaceStore::load`]
    /// deliberately does NOT silently dedupe on read (that would be a silent data
    /// decision on someone else's merge); every by-identity mutator instead treats a
    /// duplicate-identity match losslessly-by-intent (see [`upsert_by_identity`]'s doc
    /// comment). Callers of `load()` should surface this list as a warning so a human
    /// resolves the merge deliberately — e.g. via the SSH tab's status/error line — rather
    /// than an in-flight duplicate quietly waiting to be collapsed by the next unrelated
    /// edit.
    ///
    /// Each entry is human-readable: `"<kind> '<identity>' appears Nx"`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::workspace::WorkspaceConfig;
    /// let cfg = WorkspaceConfig::default();
    /// assert!(cfg.duplicates().is_empty());
    /// ```
    pub fn duplicates(&self) -> Vec<String> {
        let mut out = duplicate_identities("ssh host", &self.ssh.host);
        out.extend(duplicate_identities("db connection", &self.db.connection));
        out
    }
}

/// Identities appearing more than once in `items`, in first-seen order, formatted for
/// [`WorkspaceConfig::duplicates`].
fn duplicate_identities<T: Identity>(kind: &str, items: &[T]) -> Vec<String> {
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut order: Vec<&str> = Vec::new();
    for item in items {
        let id = item.identity();
        let entry = counts.entry(id).or_insert(0);
        if *entry == 0 {
            order.push(id);
        }
        *entry += 1;
    }
    order
        .into_iter()
        .filter_map(|id| {
            let n = counts[id];
            (n > 1).then(|| format!("{kind} '{id}' appears {n}x"))
        })
        .collect()
}

/// The `[ssh]` table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SshSection {
    /// `[[ssh.host]]` entries.
    #[serde(default)]
    pub host: Vec<Host>,
}

/// The `[db]` table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbSection {
    /// `[[db.connection]]` entries.
    #[serde(default)]
    pub connection: Vec<DbConnection>,
}

/// Reads/writes a single workspace's `.sid/config.toml`.
pub struct WorkspaceStore {
    root: PathBuf,
}

impl WorkspaceStore {
    /// Bind to a workspace root directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to the committed config file (`<root>/.sid/config.toml`).
    pub fn config_path(&self) -> PathBuf {
        self.root.join(".sid").join("config.toml")
    }

    /// Load the config. A missing file (or missing `.sid/`) yields the empty default —
    /// this is not an error.
    pub fn load(&self) -> Result<WorkspaceConfig> {
        let path = self.config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| StoreError::Decode {
                version: 0,
                msg: format!("toml {}: {e}", path.display()),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(WorkspaceConfig::default()),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    /// Save the config, creating `.sid/` if needed.
    pub fn save(&self, cfg: &WorkspaceConfig) -> Result<()> {
        let dir = self.root.join(".sid");
        std::fs::create_dir_all(&dir)?;
        let text =
            toml::to_string_pretty(cfg).map_err(|e| StoreError::Encode(format!("toml: {e}")))?;
        // Atomic write: a crash or I/O error must never truncate the committed file.
        let tmp = dir.join("config.toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, dir.join("config.toml"))?;
        Ok(())
    }

    // ---- convenience mutators (load → modify → save), deduping by identity within this layer ----

    /// Insert or replace a host by alias.
    pub fn upsert_host(&self, h: &Host) -> Result<()> {
        let mut cfg = self.load()?;
        upsert_by_identity(&mut cfg.ssh.host, h.clone());
        self.save(&cfg)
    }

    /// Remove a host by alias. Returns whether one was present.
    ///
    /// `retain` already drops EVERY entry matching `alias`, not just the first — so a
    /// duplicate-alias layer (see [`WorkspaceConfig::duplicates`]) is removed in full
    /// here, never leaving a stray duplicate behind.
    pub fn remove_host(&self, alias: &str) -> Result<bool> {
        let mut cfg = self.load()?;
        let before = cfg.ssh.host.len();
        cfg.ssh.host.retain(|h| h.identity() != alias);
        let changed = cfg.ssh.host.len() != before;
        if changed {
            self.save(&cfg)?;
        }
        Ok(changed)
    }

    /// Insert or replace a connection by id.
    pub fn upsert_connection(&self, c: &DbConnection) -> Result<()> {
        let mut cfg = self.load()?;
        upsert_by_identity(&mut cfg.db.connection, c.clone());
        self.save(&cfg)
    }

    /// Remove a connection by id. Returns whether one was present.
    ///
    /// Same "removes every match, not just the first" behavior as
    /// [`WorkspaceStore::remove_host`] — see its doc comment.
    pub fn remove_connection(&self, id: &str) -> Result<bool> {
        let mut cfg = self.load()?;
        let before = cfg.db.connection.len();
        cfg.db.connection.retain(|c| c.identity() != id);
        let changed = cfg.db.connection.len() != before;
        if changed {
            self.save(&cfg)?;
        }
        Ok(changed)
    }
}

/// Replace the element(s) sharing `item`'s [`Identity`] with the single new `item`, or
/// push `item` if none matched.
///
/// The common case (0 or 1 existing match) replaces in place / appends, exactly as
/// before. The duplicate-identity case (see [`WorkspaceConfig::duplicates`] — TOML
/// happily parses two `[[ssh.host]]` blocks with the same alias, e.g. after a git
/// merge) used to silently replace only the FIRST match via `.find()`, leaving the
/// other duplicate(s) behind untouched and undiscoverable through this API. An
/// explicit upsert is instead treated as the user's intent to resolve that ambiguity:
/// every entry sharing the identity is removed and the single new value takes their
/// place. This is a deliberate, lossy collapse — the same "an explicit edit wins"
/// intent [`WorkspaceStore::remove_host`]/[`WorkspaceStore::remove_connection`] already
/// apply by removing every match, not just the first.
fn upsert_by_identity<T: Identity>(items: &mut Vec<T>, item: T) {
    let matches: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, x)| x.identity() == item.identity())
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [] => items.push(item),
        [only] => items[*only] = item,
        _ => {
            let id = item.identity().to_string();
            items.retain(|x| x.identity() != id);
            items.push(item);
        }
    }
}
