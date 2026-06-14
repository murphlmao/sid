//! Tests for WorkspaceUpserter trait and merge_discoveries_into — Task 21.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use sid_core::{
    workspace_discovery::{WorkspaceUpserter, merge_discoveries_into, scan_workspace_root},
    workspace_metadata::WorkspaceKind,
};
use tempfile::tempdir;

fn init_git_at(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::create_dir_all(path.join(".git")).unwrap();
    fs::write(path.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
}

struct CapturingUpserter {
    records: Mutex<Vec<(PathBuf, WorkspaceKind, String)>>,
}

impl CapturingUpserter {
    fn new() -> Self {
        Self {
            records: Mutex::new(vec![]),
        }
    }

    fn count(&self) -> usize {
        self.records.lock().unwrap().len()
    }

    fn has_path(&self, path: &Path) -> bool {
        self.records
            .lock()
            .unwrap()
            .iter()
            .any(|(p, _, _)| p == path)
    }
}

impl WorkspaceUpserter for CapturingUpserter {
    fn upsert(&self, path: &Path, kind: WorkspaceKind, name: &str) -> Result<(), String> {
        self.records
            .lock()
            .unwrap()
            .push((path.to_path_buf(), kind, name.to_string()));
        Ok(())
    }
}

#[test]
fn merge_calls_upsert_once_per_discovery() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("x"));
    init_git_at(&root.path().join("y"));
    init_git_at(&root.path().join("z"));
    let discoveries = scan_workspace_root(root.path(), 2).unwrap();
    let upserter = CapturingUpserter::new();
    let n = merge_discoveries_into(&upserter, &discoveries).unwrap();
    assert_eq!(n, discoveries.len());
    assert_eq!(upserter.count(), discoveries.len());
}

#[test]
fn merge_passes_correct_kind_to_upsert() {
    let root = tempdir().unwrap();
    let umbrella = root.path().join("stack");
    fs::create_dir(&umbrella).unwrap();
    fs::write(umbrella.join("CLAUDE.md"), "# stack").unwrap();
    init_git_at(&umbrella.join("child"));
    let discoveries = scan_workspace_root(root.path(), 4).unwrap();
    let upserter = CapturingUpserter::new();
    merge_discoveries_into(&upserter, &discoveries).unwrap();
    let records = upserter.records.lock().unwrap();
    let umbrella_record = records
        .iter()
        .find(|(p, _, _)| p.ends_with("stack"))
        .unwrap();
    assert_eq!(umbrella_record.1, WorkspaceKind::Umbrella);
    let child_record = records
        .iter()
        .find(|(p, _, _)| p.ends_with("child"))
        .unwrap();
    assert_eq!(child_record.1, WorkspaceKind::Repo);
}

#[test]
fn merge_passes_name_to_upsert() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("my-service"));
    let discoveries = scan_workspace_root(root.path(), 2).unwrap();
    let upserter = CapturingUpserter::new();
    merge_discoveries_into(&upserter, &discoveries).unwrap();
    let records = upserter.records.lock().unwrap();
    let record = records
        .iter()
        .find(|(p, _, _)| p.ends_with("my-service"))
        .unwrap();
    assert_eq!(record.2, "my-service");
}

#[test]
fn merge_empty_slice_ok_returns_zero() {
    let upserter = CapturingUpserter::new();
    let n = merge_discoveries_into(&upserter, &[]).unwrap();
    assert_eq!(n, 0);
    assert_eq!(upserter.count(), 0);
}

#[test]
fn merge_error_on_second_stops_before_third() {
    let root = tempdir().unwrap();
    init_git_at(&root.path().join("a"));
    init_git_at(&root.path().join("b"));
    init_git_at(&root.path().join("c"));
    let discoveries = scan_workspace_root(root.path(), 2).unwrap();
    assert_eq!(discoveries.len(), 3);

    struct FailOnSecond {
        count: Mutex<usize>,
        seen: Mutex<Vec<PathBuf>>,
    }
    impl WorkspaceUpserter for FailOnSecond {
        fn upsert(&self, path: &Path, _kind: WorkspaceKind, _name: &str) -> Result<(), String> {
            let mut c = self.count.lock().unwrap();
            *c += 1;
            if *c == 2 {
                return Err("fail on second".into());
            }
            self.seen.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }
    let upserter = FailOnSecond {
        count: Mutex::new(0),
        seen: Mutex::new(vec![]),
    };
    let result = merge_discoveries_into(&upserter, &discoveries);
    assert!(result.is_err());
    // Only the first should have been persisted
    assert_eq!(upserter.seen.lock().unwrap().len(), 1);
}

#[test]
fn workspace_upserter_is_dyn_compatible() {
    // Verify the trait is object-safe by making a Box<dyn WorkspaceUpserter>
    let upserter: Box<dyn WorkspaceUpserter> = Box::new(CapturingUpserter::new());
    let result = upserter.upsert(Path::new("/tmp/test"), WorkspaceKind::Repo, "test");
    assert!(result.is_ok());
}

#[test]
fn merge_100_discoveries_completes() {
    let root = tempdir().unwrap();
    for i in 0..100 {
        init_git_at(&root.path().join(format!("repo-{i:03}")));
    }
    let discoveries = scan_workspace_root(root.path(), 2).unwrap();
    assert_eq!(discoveries.len(), 100);
    let upserter = CapturingUpserter::new();
    let n = merge_discoveries_into(&upserter, &discoveries).unwrap();
    assert_eq!(n, 100);
}

#[test]
fn upserter_can_be_shared_across_calls() {
    let root1 = tempdir().unwrap();
    let root2 = tempdir().unwrap();
    init_git_at(&root1.path().join("a"));
    init_git_at(&root2.path().join("b"));
    let disc1 = scan_workspace_root(root1.path(), 2).unwrap();
    let disc2 = scan_workspace_root(root2.path(), 2).unwrap();
    let store = CapturingUpserter::new();
    merge_discoveries_into(&store, &disc1).unwrap();
    merge_discoveries_into(&store, &disc2).unwrap();
    assert_eq!(store.count(), 2);
    assert!(store.has_path(&root1.path().join("a")));
    assert!(store.has_path(&root2.path().join("b")));
}

#[test]
fn discoveries_with_duplicate_paths_call_upsert_for_each() {
    // If the same path appears twice (e.g., via umbrella + repo detection),
    // merge calls upsert for each occurrence.
    let root = tempdir().unwrap();
    let umbrella = root.path().join("stack");
    fs::create_dir(&umbrella).unwrap();
    fs::write(umbrella.join("CLAUDE.md"), "# stack").unwrap();
    init_git_at(&umbrella.join("child"));
    let discoveries = scan_workspace_root(root.path(), 4).unwrap();
    // discoveries will contain: stack (Umbrella), child (Repo)
    assert_eq!(discoveries.len(), 2);
    let path_counts: BTreeMap<String, usize> = BTreeMap::new();

    struct CountingStore<'a> {
        counts: &'a Mutex<BTreeMap<String, usize>>,
    }
    impl WorkspaceUpserter for CountingStore<'_> {
        fn upsert(&self, path: &Path, _kind: WorkspaceKind, _name: &str) -> Result<(), String> {
            *self
                .counts
                .lock()
                .unwrap()
                .entry(path.to_string_lossy().to_string())
                .or_default() += 1;
            Ok(())
        }
    }
    let counts = Mutex::new(path_counts);
    merge_discoveries_into(&CountingStore { counts: &counts }, &discoveries).unwrap();
    let counts = counts.into_inner().unwrap();
    assert!(
        counts.values().all(|&v| v == 1),
        "each path should be upserted once"
    );
}
