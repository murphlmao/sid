-- Rich-type rendering fixture for sid-db's Postgres integration tests
-- (crates/sid-db/tests/postgres_rich_types.rs). Exercises BUG 1's fix in
-- `render_pg_value` (crates/sid-db/src/postgres.rs):
--   - uuid/numeric/timestamptz/jsonb/text[]/bool/text real values must render
--     as their real text, never the string "NULL".
--   - genuine SQL NULLs (the nullable columns, in the second row) must still
--     render "NULL" — and be distinguishable from a present-but-undecodable
--     value.
--   - `duration` (INTERVAL) has no dedicated `render_pg_value` arm and isn't
--     decodable via the String fallback either — a present INTERVAL value
--     must render the `⟨type?⟩` marker, never "NULL" (this is the exact
--     silent-data-corruption bug the probe found).
--
-- Loaded automatically alongside 01-schema.sql by the official postgres:16
-- image's /docker-entrypoint-initdb.d convention (numbered to run second).

CREATE TABLE public.rich_types (
    id         UUID NOT NULL PRIMARY KEY,
    amount     NUMERIC(10, 2) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    payload    JSONB,
    tags       TEXT[],
    active     BOOLEAN NOT NULL,
    nickname   TEXT,
    duration   INTERVAL NOT NULL
);

-- Row 1: every column populated with a real, non-null value.
-- Row 2: the NULL-heavy row — but id/amount/created_at/active/duration are
-- NOT NULL and hold real (if boundary-ish: all-zeroes uuid, 0, epoch, false)
-- values. Only payload/tags/nickname (the nullable columns) are genuinely
-- NULL here.
INSERT INTO public.rich_types
    (id, amount, created_at, payload, tags, active, nickname, duration)
VALUES
    (
        '11111111-1111-1111-1111-111111111111',
        123.45,
        '2026-01-15 10:30:00+00',
        '{"k": "v"}'::jsonb,
        ARRAY['a', 'b', 'c'],
        true,
        'row-one',
        INTERVAL '1 day'
    ),
    (
        '00000000-0000-0000-0000-000000000000',
        0,
        'epoch',
        NULL,
        NULL,
        false,
        NULL,
        INTERVAL '0'
    );
