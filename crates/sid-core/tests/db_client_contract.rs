//! Verifies the DbClient trait is dyn-compatible and a mock impl can satisfy
//! every method.

use std::sync::Arc;

use sid_core::adapters::db_client::{
    Column, ColumnType, DbClient, DbError, DbKind, ExecResult, OpenParams, PageCursor, QueryPage,
    Row, SchemaInfo,
};

struct MockDb;

#[async_trait::async_trait]
impl DbClient for MockDb {
    async fn open(&self, _p: OpenParams) -> Result<Arc<dyn DbClient>, DbError> {
        Ok(Arc::new(MockDb))
    }
    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }
    async fn execute(&self, _sql: &str) -> Result<ExecResult, DbError> {
        Ok(ExecResult {
            rows_affected: 0,
            duration_ms: 0,
        })
    }
    async fn query_paged(
        &self,
        _sql: &str,
        _cursor: Option<PageCursor>,
        _page_size: u32,
    ) -> Result<QueryPage, DbError> {
        Ok(QueryPage {
            columns: vec![],
            rows: vec![],
            next_cursor: None,
            duration_ms: 0,
        })
    }
    async fn schema_introspect(&self) -> Result<SchemaInfo, DbError> {
        Ok(SchemaInfo { tables: vec![] })
    }
    async fn cancel(&self) -> Result<(), DbError> {
        Ok(())
    }
    fn kind(&self) -> DbKind {
        DbKind::Sqlite
    }
}

#[tokio::test]
async fn dyn_dispatch_works() {
    let c: Arc<dyn DbClient> = Arc::new(MockDb);
    assert_eq!(c.execute("SELECT 1").await.unwrap().rows_affected, 0);
    let p = c.query_paged("SELECT 1", None, 50).await.unwrap();
    assert!(p.columns.is_empty());
}

#[test]
fn send_sync_bounds() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn DbClient>>();
}

#[test]
fn dbkind_variants() {
    let _ = DbKind::Postgres;
    let _ = DbKind::Sqlite;
}

#[test]
fn column_type_variants_exist() {
    let _ = ColumnType::Text;
    let _ = ColumnType::Integer;
    let _ = ColumnType::Float;
    let _ = ColumnType::Bool;
    let _ = ColumnType::Bytes;
    let _ = ColumnType::Null;
    let _ = ColumnType::Other("uuid".into());
}

#[test]
fn row_construction() {
    let r = Row {
        values: vec!["a".into(), "1".into()],
    };
    assert_eq!(r.values.len(), 2);
}

#[test]
fn page_cursor_construction() {
    let c = PageCursor { offset: 100 };
    assert_eq!(c.offset, 100);
}

#[test]
fn column_construction() {
    let c = Column {
        name: "id".into(),
        ty: ColumnType::Integer,
    };
    assert_eq!(c.name, "id");
}
