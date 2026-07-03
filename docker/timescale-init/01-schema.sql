-- TimescaleDB fixture for sid-db's integration tests
-- (crates/sid-db/tests/postgres_timescale.rs). A regular table plus a
-- hypertable with its own primary key and a foreign key to the regular
-- table — exercises BUG 3: TimescaleDB's internal `_timescaledb_internal`/
-- `_timescaledb_catalog`/`_timescaledb_config`/`timescaledb_information`/
-- `timescaledb_experimental` schemas (and the per-chunk FK/PK constraints
-- Timescale creates under `_timescaledb_internal`) must never leak into
-- `schema_introspect`/`schema_graph`.
--
-- Loaded automatically by the `timescale/timescaledb:latest-pg16` image's
-- /docker-entrypoint-initdb.d convention (docker-compose.test.yml's
-- `timescale` service mounts this directory read-only).

CREATE EXTENSION IF NOT EXISTS timescaledb;

CREATE TABLE public.devices (
    id   SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);

-- A hypertable's PK/UNIQUE constraint must include the partitioning column
-- (`time`) — Timescale enforces this, hence the composite PK here rather
-- than a plain surrogate `id`.
CREATE TABLE public.metrics (
    time      TIMESTAMPTZ NOT NULL,
    device_id INTEGER NOT NULL REFERENCES public.devices (id),
    value     DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (time, device_id)
);

-- Must run before any data is inserted (create_hypertable requires an empty
-- table) — hence devices/metrics rows are inserted after this call.
SELECT create_hypertable('public.metrics', 'time');

INSERT INTO public.devices (name) VALUES ('sensor-1'), ('sensor-2');
INSERT INTO public.metrics (time, device_id, value) VALUES
    (now(), 1, 12.5),
    (now(), 2, 42.0),
    (now() - INTERVAL '1 hour', 1, 11.0);
