//! CSV export for query results. Uses the `csv` crate with default quoting.

use std::io::Write;

use sid_core::adapters::db_client::QueryPage;

/// Write a `QueryPage` to `out` as CSV: one header row, then one row per record.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::db_client::{Column, ColumnType, QueryPage};
/// use sid_widgets::csv_export::write_page_csv;
/// let p = QueryPage {
///     columns: vec![Column { name: "id".into(), ty: ColumnType::Integer }],
///     rows: vec![],
///     next_cursor: None,
///     duration_ms: 0,
/// };
/// let mut buf = Vec::new();
/// write_page_csv(&p, &mut buf).unwrap();
/// assert!(String::from_utf8(buf).unwrap().contains("id"));
/// ```
pub fn write_page_csv<W: Write>(page: &QueryPage, out: W) -> std::io::Result<()> {
    let mut w = csv::Writer::from_writer(out);
    let headers: Vec<&str> = page.columns.iter().map(|c| c.name.as_str()).collect();
    w.write_record(&headers).map_err(map_csv_err)?;
    for row in &page.rows {
        w.write_record(row.values.iter().map(|s| s.as_str()))
            .map_err(map_csv_err)?;
    }
    w.flush()
}

fn map_csv_err(e: csv::Error) -> std::io::Error {
    std::io::Error::other(e)
}
