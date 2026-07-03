//! One-shot demo-data seeding for the bundled demo SQLite connection.
//!
//! `crates/sid`'s first-run seed calls [`seed_demo_sqlite`] so the Database tab has an
//! explorable schema out of the box — tables for the schema tree, foreign keys for the
//! relationships diagram, and rows to `SELECT` — instead of an empty file that renders a
//! blank tab. `rusqlite` stays behind this crate (the adapter rule); the frontend only
//! names this helper.
//!
//! Idempotent: if the demo tables already exist the call is a no-op, so it is safe to run
//! on every launch even though the seed gate only fires on a fresh store.

use std::path::Path;

use rusqlite::Connection;

/// A small, FK-rich Northwind-style fixture: `customers`, `products`, a self-referencing
/// `employees`, `orders` (FK → customers, employees), and `order_items` (composite PK,
/// FK → orders, products). Chosen so the schema tree, the Access-style relationships
/// diagram (single + composite + self-referencing FKs), and a first `SELECT` are all
/// immediately non-empty.
///
/// Opens (creating if absent) the SQLite file at `path` and seeds it in one transaction.
/// A no-op if the fixture is already present.
///
/// # Examples
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// let db = dir.path().join("demo.db");
/// sid_db::demo::seed_demo_sqlite(&db).unwrap();
/// // idempotent — a second call is a clean no-op
/// sid_db::demo::seed_demo_sqlite(&db).unwrap();
/// let conn = rusqlite::Connection::open(&db).unwrap();
/// let customers: i64 = conn
///     .query_row("SELECT count(*) FROM customers", [], |r| r.get(0))
///     .unwrap();
/// assert_eq!(customers, 3);
/// ```
pub fn seed_demo_sqlite(path: &Path) -> Result<(), rusqlite::Error> {
    let conn = Connection::open(path)?;
    // Already seeded? (any prior run, or a store the user has since populated) → no-op.
    let seeded: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'customers'",
        [],
        |r| r.get(0),
    )?;
    if seeded > 0 {
        return Ok(());
    }
    conn.execute_batch(
        "BEGIN;
         CREATE TABLE customers (
             id      INTEGER PRIMARY KEY,
             company TEXT NOT NULL,
             contact TEXT,
             city    TEXT
         );
         CREATE TABLE products (
             id         INTEGER PRIMARY KEY,
             name       TEXT NOT NULL,
             unit_price REAL NOT NULL,
             in_stock   INTEGER NOT NULL
         );
         CREATE TABLE employees (
             id         INTEGER PRIMARY KEY,
             last_name  TEXT NOT NULL,
             first_name TEXT NOT NULL,
             title      TEXT,
             reports_to INTEGER REFERENCES employees(id)
         );
         CREATE TABLE orders (
             id          INTEGER PRIMARY KEY,
             customer_id INTEGER NOT NULL REFERENCES customers(id),
             employee_id INTEGER REFERENCES employees(id),
             order_date  TEXT NOT NULL,
             ship_city   TEXT,
             freight     REAL
         );
         CREATE TABLE order_items (
             order_id   INTEGER NOT NULL REFERENCES orders(id),
             product_id INTEGER NOT NULL REFERENCES products(id),
             quantity   INTEGER NOT NULL,
             discount   REAL NOT NULL DEFAULT 0,
             PRIMARY KEY (order_id, product_id)
         );
         INSERT INTO customers (id, company, contact, city) VALUES
             (1, 'ACME Corp', 'R. Runner', 'Phoenix'),
             (2, 'Globex', 'H. Simpson', 'Springfield'),
             (3, 'Initech', 'P. Gibbons', 'Austin');
         INSERT INTO products (id, name, unit_price, in_stock) VALUES
             (1, 'Widget', 9.99, 140),
             (2, 'Gizmo', 24.50, 12),
             (3, 'Sprocket', 3.25, 0);
         INSERT INTO employees (id, last_name, first_name, title, reports_to) VALUES
             (1, 'Fuller', 'Andrew', 'VP Sales', NULL),
             (2, 'Davolio', 'Nancy', 'Sales Rep', 1),
             (3, 'Leverling', 'Janet', 'Sales Rep', 1);
         INSERT INTO orders (id, customer_id, employee_id, order_date, ship_city, freight) VALUES
             (1, 1, 2, '2026-06-30', 'Phoenix', 12.50),
             (2, 2, 3, '2026-07-01', 'Springfield', 7.10),
             (3, 1, 2, '2026-07-02', 'Phoenix', 22.00);
         INSERT INTO order_items (order_id, product_id, quantity, discount) VALUES
             (1, 1, 3, 0.0),
             (1, 2, 1, 0.10),
             (2, 3, 10, 0.05),
             (3, 1, 5, 0.0);
         COMMIT;",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_the_fixture_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("demo.db");
        seed_demo_sqlite(&db).unwrap();
        let conn = Connection::open(&db).unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 5, "all five demo tables created");
        let orders: i64 = conn
            .query_row("SELECT count(*) FROM orders", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orders, 3);

        // Second run is a no-op — no duplicate rows, no error.
        seed_demo_sqlite(&db).unwrap();
        let customers: i64 = conn
            .query_row("SELECT count(*) FROM customers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(customers, 3, "idempotent — not double-inserted");
    }
}
