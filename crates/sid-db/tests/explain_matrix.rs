//! The EXPLAIN capability matrix, across every engine `sid-db` ships.
//!
//! `DbClient::explain_support` is a *classification*, not I/O: it answers "can this
//! engine show me a plan, and if not, what do I tell the user" without a connection,
//! because the Database tab has to decide whether to enable the affordance before
//! anything is open. That makes it worth a table-driven test rather than three
//! scattered unit tests — the property that matters is that the three engines
//! *disagree in the documented way*, and a per-crate test can never see that.
//!
//! # The safety rule this file pins
//!
//! **Plain `EXPLAIN` only — never `EXPLAIN ANALYZE`.** Postgres's `ANALYZE` option
//! *executes* the statement to collect real timings, so "show me the plan for this
//! `DELETE`" would delete the rows. sid's affordance is read-only by construction: no
//! engine may declare a keyword containing `ANALYZE`, and
//! [`explain_keyword_never_executes_the_statement`] fails the build if one does.

use sid_core::db::{DbClient, DbError, DbKind, ExplainSupport, OpenParams, SqliteMode};

/// Every engine's factory, paired with the support it must declare.
fn engines() -> Vec<(&'static str, std::sync::Arc<dyn DbClient>)> {
    vec![
        ("postgres", sid_db::PostgresClient::factory()),
        ("sqlite", sid_db::SqliteClient::factory()),
        ("redb", sid_db::RedbBrowseClient::factory()),
    ]
}

#[test]
fn each_engine_declares_the_documented_support() {
    // The matrix itself. Postgres and SQLite both have a plan syntax and spell it
    // differently; the redb browse engine has no planner at all — it lists sid's own
    // store tables — and must say so in words a user can act on.
    for (name, client) in engines() {
        let support = client.explain_support();
        match name {
            "postgres" => assert_eq!(support, ExplainSupport::Supported { keyword: "EXPLAIN" }),
            "sqlite" => assert_eq!(
                support,
                ExplainSupport::Supported {
                    keyword: "EXPLAIN QUERY PLAN"
                }
            ),
            "redb" => {
                let reason = support.reason().unwrap_or_else(|| {
                    panic!("redb must explain why it cannot explain, got {support:?}")
                });
                assert!(
                    !reason.is_empty(),
                    "an empty reason is not a reason the UI can show"
                );
                assert!(!support.is_supported());
            }
            other => panic!("unregistered engine {other}"),
        }
    }
}

#[test]
fn explain_keyword_never_executes_the_statement() {
    // See the module docs: `EXPLAIN ANALYZE` runs the statement for real. A plan
    // affordance that can delete rows is not a plan affordance.
    for (name, client) in engines() {
        if let Some(keyword) = client.explain_support().keyword() {
            let upper = keyword.to_ascii_uppercase();
            assert!(
                !upper.contains("ANALYZE"),
                "{name}: {keyword:?} would execute the statement"
            );
            assert!(
                upper.starts_with("EXPLAIN"),
                "{name}: {keyword:?} is not a plan keyword"
            );
        }
    }
}

#[test]
fn support_and_keyword_and_reason_agree() {
    // The three accessors are one fact seen from three sides: exactly one of
    // `keyword`/`reason` is present, and `is_supported` says which.
    for (name, client) in engines() {
        let support = client.explain_support();
        assert_eq!(
            support.keyword().is_some(),
            support.is_supported(),
            "{name}: keyword disagrees with is_supported"
        );
        assert_eq!(
            support.reason().is_some(),
            !support.is_supported(),
            "{name}: reason disagrees with is_supported"
        );
    }
}

#[tokio::test]
async fn an_engine_that_cannot_explain_fails_with_its_own_reason() {
    // The error a disabled affordance would produce if something reached it anyway
    // has to be the same sentence the disabled control shows — otherwise the UI and
    // the backend tell the user two different stories.
    let client = sid_db::RedbBrowseClient::factory();
    let reason = client
        .explain_support()
        .reason()
        .expect("redb cannot explain")
        .to_string();
    match client.explain("select 1").await {
        Err(DbError::Invalid(message)) => assert!(
            message.contains(&reason),
            "{message:?} does not carry the declared reason {reason:?}"
        ),
        other => panic!("expected DbError::Invalid, got {other:?}"),
    }
}

#[tokio::test]
async fn sqlite_explains_a_real_statement_into_plan_rows() {
    // The only engine that can be exercised end-to-end with no server. SQLite's
    // `EXPLAIN QUERY PLAN` returns one row per plan step; the `detail` column is the
    // human-readable part and must name the table being scanned.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("explain.sqlite");
    let client = sid_db::SqliteClient::factory()
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: path.to_string_lossy().into_owned(),
            password: None,
            sqlite_mode: Some(SqliteMode::CreateNew),
        })
        .await
        .expect("create");
    client
        .execute("CREATE TABLE widgets (id INTEGER PRIMARY KEY, label TEXT)")
        .await
        .expect("create table");

    let plan = client
        .explain("SELECT * FROM widgets")
        .await
        .expect("explain");
    assert!(!plan.rows.is_empty(), "a plan with no steps is not a plan");
    let text = plan
        .rows
        .iter()
        .flat_map(|r| r.values.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        text.to_ascii_uppercase().contains("WIDGETS"),
        "the plan never mentions the table it scans: {text:?}"
    );
}

#[tokio::test]
async fn sqlite_explaining_a_broken_statement_reports_the_syntax_error() {
    // An EXPLAIN of nonsense must fail like a query of nonsense — surfaced, not
    // swallowed into an empty plan that reads as "this query is free".
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bad.sqlite");
    let client = sid_db::SqliteClient::factory()
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: path.to_string_lossy().into_owned(),
            password: None,
            sqlite_mode: Some(SqliteMode::CreateNew),
        })
        .await
        .expect("create");
    assert!(client.explain("SELEKT nope FROM nothing").await.is_err());
}

#[tokio::test]
async fn sqlite_explain_tolerates_a_trailing_semicolon() {
    // The editor's own seed SQL ends in `;`, and every query the user pastes probably
    // does too. `EXPLAIN QUERY PLAN select 1;` is fine, but the same trailing-trivia
    // hazard `query_paged` documents (a `; --` paste artifact) applies here, so the
    // same lexer-backed strip runs first.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("semi.sqlite");
    let client = sid_db::SqliteClient::factory()
        .open(OpenParams {
            kind: DbKind::Sqlite,
            dsn: path.to_string_lossy().into_owned(),
            password: None,
            sqlite_mode: Some(SqliteMode::CreateNew),
        })
        .await
        .expect("create");
    client
        .execute("CREATE TABLE t (a INTEGER)")
        .await
        .expect("create table");
    client
        .explain("SELECT * FROM t; -- trailing comment")
        .await
        .expect("a trailing semicolon and comment must not break EXPLAIN");
}
