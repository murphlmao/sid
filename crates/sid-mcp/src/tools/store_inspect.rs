//! `store_inspect` — read-only JSON dump of a sid store with origin tags.
//!
//! Read-only by construction: `GlobalStore::open` creates tables (a write), so the
//! redb file is never opened in place. Instead we take a shared look at it — try an
//! exclusive advisory lock (redb's own lock; failing means a running sid holds it),
//! copy the bytes to a temp dir while holding it, and open the *copy*. Secret values
//! are never read: entities carry only an opaque `secret_ref`, and connection strings
//! are never emitted at all - only parsed non-secret facts (see [`super::dsn`]).

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sid_store::{GlobalStore, WorkspaceStore};

/// Locate `store.redb` under a data dir (`<d>/sid/store.redb` or `<d>/store.redb`).
pub fn locate_store(data_dir: &Path) -> Option<PathBuf> {
    [data_dir.join("sid/store.redb"), data_dir.join("store.redb")]
        .into_iter()
        .find(|p| p.is_file())
}

/// Default XDG data dir.
pub fn default_data_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(x);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".local/share"))
        .unwrap_or_else(|| PathBuf::from(".local/share"))
}

/// Copy `src` to a temp dir under an exclusive advisory lock; fail clearly if locked.
fn snapshot(src: &Path) -> Result<(tempfile::TempDir, PathBuf), String> {
    let f = File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
    match f.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(format!(
                "{} is locked by a running sid (redb is single-process). Close sid, or point data_dir at a copy.",
                src.display()
            ));
        }
        Err(TryLockError::Error(e)) => return Err(format!("lock {}: {e}", src.display())),
    }
    let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let dst = dir.path().join("store.redb");
    std::fs::copy(src, &dst).map_err(|e| format!("snapshot copy: {e}"))?;
    drop(f); // releases the lock
    Ok((dir, dst))
}

fn tag<T: serde::Serialize>(origin: &str, dup: bool, item: &T) -> Value {
    json!({ "origin": origin, "duplicate": dup, "item": item })
}

fn host_json(h: &sid_store::Host) -> Value {
    // `Host` carries only `secret_ref`, never the secret; serialised as-is.
    serde_json::to_value(h).unwrap_or(Value::Null)
}

fn conn_json(c: &sid_store::DbConnection) -> Value {
    // The connection string is never emitted, redacted or not: only parsed,
    // non-secret facts, plus the opaque `secret_ref`.
    let kind = serde_json::to_value(c.kind).unwrap_or(Value::Null);
    let file_kind = matches!(
        kind.as_str().map(str::to_ascii_lowercase).as_deref(),
        Some("sqlite" | "redb")
    );
    let f = super::dsn::dsn_facts(&c.dsn, file_kind);
    json!({
        "id": c.id,
        "name": c.name,
        "kind": kind,
        "folder": c.folder,
        "secret_ref": c.secret_ref,
        "scheme": f.scheme,
        "host": f.host,
        "port": f.port,
        "database": f.database,
        "user": f.user,
        "file": f.file,
        "has_inline_password": f.has_inline_password,
        "dsn_unparsed": f.unparsed,
    })
}

/// Build the dump.
pub fn inspect(data_dir: &Path, workspace_root: Option<&Path>) -> Result<Value, String> {
    let store_path = locate_store(data_dir).ok_or_else(|| {
        format!(
            "no store.redb under {} (looked for sid/store.redb and store.redb)",
            data_dir.display()
        )
    })?;
    let (_guard, copy) = snapshot(&store_path)?;
    let g = GlobalStore::open(&copy).map_err(|e| format!("open snapshot: {e}"))?;
    let gh = g.list_hosts().map_err(|e| e.to_string())?;
    let gc = g.list_connections().map_err(|e| e.to_string())?;
    let workspaces = g.list_workspaces().map_err(|e| e.to_string())?;
    let settings = g.get_settings().map_err(|e| e.to_string())?;

    let mut hosts = Vec::new();
    let mut conns = Vec::new();
    let mut ws_info = Value::Null;
    let mut warnings: Vec<String> = Vec::new();

    let (mut wh, mut wc) = (Vec::new(), Vec::new());
    if let Some(root) = workspace_root {
        let ws = WorkspaceStore::new(root);
        if !ws.config_path().is_file() {
            warnings.push(format!(
                "{} not found; global layer only",
                ws.config_path().display()
            ));
        } else {
            let cfg = ws.load().map_err(|e| format!("workspace config: {e}"))?;
            warnings.extend(
                cfg.duplicates()
                    .into_iter()
                    .map(|d| format!("workspace duplicate: {d}")),
            );
            wh = cfg.ssh.host;
            wc = cfg.db.connection;
        }
        ws_info = json!({ "root": root.display().to_string(), "config": ws.config_path().display().to_string() });
    }
    let wlabel = workspace_root.map(|r| format!("workspace:{}", r.display()));
    // Attributive: both layers are listed; `duplicate` marks same-identity pairs.
    for h in &gh {
        hosts.push(tag(
            "global",
            wh.iter().any(|w| w.alias == h.alias),
            &host_json(h),
        ));
    }
    for h in &wh {
        let o = wlabel.as_deref().unwrap_or("workspace");
        hosts.push(tag(o, gh.iter().any(|g| g.alias == h.alias), &host_json(h)));
    }
    for c in &gc {
        conns.push(tag(
            "global",
            wc.iter().any(|w| w.id == c.id),
            &conn_json(c),
        ));
    }
    for c in &wc {
        let o = wlabel.as_deref().unwrap_or("workspace");
        conns.push(tag(o, gc.iter().any(|g| g.id == c.id), &conn_json(c)));
    }
    Ok(json!({
        "store": store_path.display().to_string(),
        "note": "secret values are never read; entities show only secret_ref",
        "workspace": ws_info,
        "hosts": hosts,
        "db_connections": conns,
        "workspaces": serde_json::to_value(&workspaces).unwrap_or(Value::Null),
        "settings": serde_json::to_value(&settings).unwrap_or(Value::Null),
        "warnings": warnings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_store::{AuthMethod, DbConnection, Host};

    fn host(alias: &str, secret: Option<&str>) -> Host {
        Host {
            alias: alias.into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: secret.map(String::from),
            auth: AuthMethod::Agent,
            folder: None,
        }
    }

    /// (stored DSN, password fragments that must never reach the output)
    const PW_CASES: &[(&str, &[&str])] = &[
        (
            "postgres://bob:p@ss:w0rd@db:5433/app",
            &["p@ss", "ss:w0rd", "w0rd"],
        ),
        ("postgres://bob:s3cr%40t%21@db/app", &["s3cr", "%40t"]),
        (
            "host=db user=bob password='a b c' dbname=app",
            &["a b", "b c", "'a"],
        ),
        ("host=db password=it\\'s\\ fine user=bob", &["fine", "it's"]),
        (
            "postgres://db/app?sslmode=require&password=hunter2",
            &["hunter2"],
        ),
    ];

    #[test]
    fn no_password_fragment_ever_reaches_the_output() {
        for (dsn, fragments) in PW_CASES {
            let c = DbConnection {
                id: "c".into(),
                dsn: (*dsn).into(),
                secret_ref: Some("keyring:ref".into()),
                kind: Default::default(),
                name: "c".into(),
                folder: None,
            };
            let out = conn_json(&c).to_string();
            for frag in *fragments {
                assert!(!out.contains(frag), "{dsn}: leaked {frag:?} in {out}");
            }
            assert!(!out.contains("\"dsn\""), "no dsn field: {out}");
            assert!(out.contains("keyring:ref"));
            assert!(out.contains("\"has_inline_password\":true"), "{dsn}: {out}");
        }
    }

    #[test]
    fn dumps_both_layers_with_origin_and_never_secrets() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sid")).unwrap();
        {
            let g = GlobalStore::open(&d.path().join("sid/store.redb")).unwrap();
            g.upsert_host(&host("shared", Some("keyring:abc"))).unwrap();
            g.upsert_host(&host("only-global", None)).unwrap();
            g.upsert_connection(&DbConnection {
                id: "c1".into(),
                dsn: "postgres://u:topsecret@h/db".into(),
                secret_ref: Some("keyring:pw".into()),
                kind: Default::default(),
                name: "c1".into(),
                folder: None,
            })
            .unwrap();
        }
        let ws = tempfile::tempdir().unwrap();
        WorkspaceStore::new(ws.path())
            .upsert_host(&host("shared", None))
            .unwrap();

        let v = inspect(d.path(), Some(ws.path())).unwrap();
        let text = v.to_string();
        assert!(!text.contains("topsecret") && !text.contains("postgres://"));
        assert!(text.contains("keyring:abc"), "secret_ref is shown");
        let hosts = v["hosts"].as_array().unwrap();
        assert_eq!(hosts.len(), 3);
        let shared: Vec<_> = hosts
            .iter()
            .filter(|h| h["item"]["alias"] == "shared")
            .collect();
        assert_eq!(shared.len(), 2, "attributive: both copies listed");
        assert!(shared.iter().all(|h| h["duplicate"] == true));
        assert!(shared.iter().any(|h| h["origin"] == "global"));
        assert!(
            shared
                .iter()
                .any(|h| h["origin"].as_str().unwrap().starts_with("workspace:"))
        );
        let only = hosts
            .iter()
            .find(|h| h["item"]["alias"] == "only-global")
            .unwrap();
        assert_eq!(only["duplicate"], false);
    }

    #[test]
    fn missing_store_and_locked_store_are_clear_errors() {
        let d = tempfile::tempdir().unwrap();
        assert!(
            inspect(d.path(), None)
                .unwrap_err()
                .contains("no store.redb")
        );

        let p = d.path().join("store.redb");
        drop(GlobalStore::open(&p).unwrap());
        let holder = File::open(&p).unwrap();
        holder.lock().unwrap();
        let e = inspect(d.path(), None).unwrap_err();
        assert!(e.contains("locked by a running sid"), "{e}");
        drop(holder);
        assert!(inspect(d.path(), None).is_ok());
    }

    #[test]
    fn original_file_is_untouched() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("store.redb");
        drop(GlobalStore::open(&p).unwrap());
        let before = std::fs::read(&p).unwrap();
        inspect(d.path(), None).unwrap();
        assert_eq!(before, std::fs::read(&p).unwrap());
    }
}
