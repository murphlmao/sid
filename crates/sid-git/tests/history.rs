//! Paginated decorated log + read-only worktree listing.

mod common;

use common::*;
use sid_core::git::{LogQuery, RefKind};

fn linear(n: usize) -> (tempfile::TempDir, git2::Repository) {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());
    for i in 0..n {
        write(dir.path(), "a.txt", &format!("{i}\n"));
        commit_all(&repo, &format!("c{i}"));
    }
    (dir, repo)
}

#[test]
fn pages_do_not_overlap_and_has_more_is_accurate() {
    let (dir, _repo) = linear(7);
    let p = open(dir.path());
    let q = |skip, limit| LogQuery {
        skip,
        limit,
        ..LogQuery::default()
    };

    let p1 = p.log_page(&q(0, 3)).unwrap();
    let p2 = p.log_page(&q(3, 3)).unwrap();
    let p3 = p.log_page(&q(6, 3)).unwrap();
    let summaries = |pg: &sid_core::git::LogPage| {
        pg.entries
            .iter()
            .map(|e| e.commit.summary.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(summaries(&p1), ["c6", "c5", "c4"]);
    assert_eq!(summaries(&p2), ["c3", "c2", "c1"]);
    assert_eq!(summaries(&p3), ["c0"]);
    assert!(p1.has_more && p2.has_more && !p3.has_more);
    // Exactly-full last page: no phantom "more".
    assert!(!p.log_page(&q(4, 3)).unwrap().has_more);
    assert!(p.log_page(&q(0, 0)).unwrap().entries.is_empty());
    assert!(p.log_page(&q(99, 5)).unwrap().entries.is_empty());
    assert_eq!(p1.entries[0].parents.len(), 1);
    assert!(p3.entries[0].parents.is_empty());
}

#[test]
fn unborn_head_is_an_empty_page() {
    let dir = tempfile::tempdir().unwrap();
    let _r = init_repo(dir.path());
    let p = open(dir.path());
    assert!(p.log_page(&LogQuery::default()).unwrap().entries.is_empty());
}

#[test]
fn decorations_name_head_branches_remotes_and_tags() {
    let (dir, repo) = linear(2);
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("topic", &head, false).unwrap();
    repo.tag_lightweight("v1", head.as_object(), false).unwrap();
    let sig = git2::Signature::now("T", "t@x").unwrap();
    repo.tag("v1-annotated", head.as_object(), &sig, "release", false)
        .unwrap();
    repo.reference("refs/remotes/origin/main", head.id(), true, "t")
        .unwrap();
    let older = head.parent(0).unwrap();
    repo.tag_lightweight("old", older.as_object(), false)
        .unwrap();

    let p = open(dir.path());
    let page = p.log_page(&LogQuery::default()).unwrap();
    let top: Vec<_> = page.entries[0]
        .refs
        .iter()
        .map(|r| (r.name.as_str(), r.kind))
        .collect();
    assert_eq!(
        top,
        [
            ("HEAD", RefKind::Head),
            ("main", RefKind::Branch),
            ("topic", RefKind::Branch),
            ("origin/main", RefKind::RemoteBranch),
            ("v1", RefKind::Tag),
            ("v1-annotated", RefKind::Tag),
        ]
    );
    let second: Vec<_> = page.entries[1]
        .refs
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(second, ["old"]);
}

#[test]
fn first_parent_hides_merged_in_history() {
    let (dir, repo) = linear(1);
    let base = repo.head().unwrap().peel_to_commit().unwrap();
    // side branch: two commits
    repo.branch("side", &base, false).unwrap();
    repo.set_head("refs/heads/side").unwrap();
    write(dir.path(), "s.txt", "1");
    commit_all(&repo, "side1");
    write(dir.path(), "s.txt", "2");
    let side_tip = commit_all(&repo, "side2");
    // main: one commit, then a real merge commit
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    write(dir.path(), "m.txt", "m");
    let main_tip = commit_all(&repo, "main1");
    let sig = git2::Signature::now("T", "t@x").unwrap();
    let side_c = repo.find_commit(side_tip).unwrap();
    let main_c = repo.find_commit(main_tip).unwrap();
    let mut idx = repo.merge_commits(&main_c, &side_c, None).unwrap();
    let tree = repo.find_tree(idx.write_tree_to(&repo).unwrap()).unwrap();
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "merge",
        &tree,
        &[&main_c, &side_c],
    )
    .unwrap();

    let p = open(dir.path());
    let all = p
        .log_page(&LogQuery {
            limit: 50,
            ..LogQuery::default()
        })
        .unwrap();
    let fp = p
        .log_page(&LogQuery {
            limit: 50,
            first_parent: true,
            ..LogQuery::default()
        })
        .unwrap();
    let names = |pg: &sid_core::git::LogPage| {
        pg.entries
            .iter()
            .map(|e| e.commit.summary.clone())
            .collect::<Vec<_>>()
    };
    assert!(names(&all).contains(&"side2".to_string()));
    assert_eq!(names(&fp), ["merge", "main1", "c0"]);
    assert_eq!(fp.entries[0].parents.len(), 2);
}

#[test]
fn log_from_an_explicit_rev() {
    let (dir, repo) = linear(3);
    let first = repo.revparse_single("HEAD~2").unwrap().id().to_string();
    let p = open(dir.path());
    let page = p
        .log_page(&LogQuery {
            rev: Some(first),
            ..LogQuery::default()
        })
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert!(
        p.log_page(&LogQuery {
            rev: Some("nope".into()),
            ..LogQuery::default()
        })
        .is_err()
    );
}

#[test]
fn worktrees_list_main_and_linked() {
    let (dir, repo) = linear(1);
    let p = open(dir.path());
    let list = p.worktrees().unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].is_main && list[0].branch.as_deref() == Some("main"));

    let base = repo.head().unwrap().peel_to_commit().unwrap();
    let br = repo.branch("wt-branch", &base, false).unwrap();
    let wt_parent = tempfile::tempdir().unwrap();
    let wt_path = wt_parent.path().join("linked");
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(br.get()));
    repo.worktree("linked", &wt_path, Some(&opts)).unwrap();

    let list = p.worktrees().unwrap();
    assert_eq!(list.len(), 2);
    let l = list.iter().find(|w| !w.is_main).unwrap();
    assert_eq!(l.name.as_deref(), Some("linked"));
    assert_eq!(l.branch.as_deref(), Some("wt-branch"));
    assert!(!l.locked && !l.prunable);
    assert!(l.path.ends_with("linked") || l.path.ends_with("linked/"));

    // Listing from inside the linked worktree sees the same set.
    let from_linked = open(&wt_path).worktrees().unwrap();
    assert_eq!(from_linked.len(), 2);

    // Remove the checkout dir: the entry becomes prunable.
    std::fs::remove_dir_all(&wt_path).unwrap();
    let l = p
        .worktrees()
        .unwrap()
        .into_iter()
        .find(|w| !w.is_main)
        .unwrap();
    assert!(l.prunable);
}
