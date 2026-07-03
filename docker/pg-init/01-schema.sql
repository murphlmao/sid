-- FK-rich fixture schema for sid-db's Postgres integration tests
-- (crates/sid-db/tests/postgres_integration.rs). Exercises:
--   - a single-column FK (orders.customer_id -> customers.id)
--   - a composite FK (orders.(warehouse_id,bin_id) -> bins.(warehouse_id,bin_id))
--   - a schema-qualified table in a non-public schema (billing.invoices),
--     referencing a public-schema table, so schema_graph's pg_catalog query
--     is exercised across namespaces, not just within `public`.
--
-- Loaded automatically by the official postgres:16 image's
-- /docker-entrypoint-initdb.d convention (docker-compose.test.yml mounts this
-- directory read-only). Runs once, against a fresh data volume.

CREATE TABLE public.customers (
    id   SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE public.warehouses (
    id   SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);

-- Composite primary key (warehouse_id, bin_id) + single-column FK to warehouses.
CREATE TABLE public.bins (
    warehouse_id INTEGER NOT NULL REFERENCES public.warehouses (id),
    bin_id       INTEGER NOT NULL,
    label        TEXT NOT NULL,
    PRIMARY KEY (warehouse_id, bin_id)
);

-- orders carries both FK shapes: a single-column FK to customers, and a
-- composite FK to bins (column order deliberately non-alphabetical to prove
-- the WITH ORDINALITY assembly preserves declared key order).
CREATE TABLE public.orders (
    id           SERIAL PRIMARY KEY,
    customer_id  INTEGER NOT NULL REFERENCES public.customers (id),
    warehouse_id INTEGER NOT NULL,
    bin_id       INTEGER NOT NULL,
    CONSTRAINT orders_bin_fk FOREIGN KEY (warehouse_id, bin_id)
        REFERENCES public.bins (warehouse_id, bin_id)
);

CREATE SCHEMA billing;

-- Schema-qualified table (non-public namespace) referencing a public-schema
-- table — exercises the ns.nspname <> refns.nspname join path.
CREATE TABLE billing.invoices (
    id       SERIAL PRIMARY KEY,
    order_id INTEGER NOT NULL REFERENCES public.orders (id),
    amount   NUMERIC(10, 2) NOT NULL
);

INSERT INTO public.customers (name) VALUES ('Ada Lovelace'), ('Grace Hopper');
INSERT INTO public.warehouses (name) VALUES ('North'), ('South');
INSERT INTO public.bins (warehouse_id, bin_id, label) VALUES
    (1, 1, 'N-1'),
    (1, 2, 'N-2'),
    (2, 1, 'S-1');
INSERT INTO public.orders (customer_id, warehouse_id, bin_id) VALUES
    (1, 1, 1),
    (2, 1, 2),
    (1, 2, 1);
INSERT INTO billing.invoices (order_id, amount) VALUES
    (1, 19.99),
    (2, 42.00);
