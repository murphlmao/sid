-- Representative analytics query (repeated ~10x for bench size).
WITH ranked_users AS (
    SELECT u.id, u.email, u.created_at,
           COUNT(o.id) AS order_count,
           SUM(o.total_amount) AS lifetime_value,
           ROW_NUMBER() OVER (PARTITION BY u.country ORDER BY SUM(o.total_amount) DESC) AS rk
    FROM users u
    LEFT JOIN orders o ON o.user_id = u.id AND o.status = 'completed'
    WHERE u.created_at > '2020-01-01' AND u.deleted_at IS NULL
    GROUP BY u.id, u.email, u.created_at, u.country
)
SELECT r.id, r.email, r.lifetime_value, r.order_count
FROM ranked_users r
WHERE r.rk <= 10 AND r.lifetime_value > 1000
ORDER BY r.lifetime_value DESC
LIMIT 100;

/* Another big block. Various stuff. */
SELECT a.foo, b.bar, c.baz FROM aa a INNER JOIN bb b ON a.id = b.aa_id
LEFT JOIN cc c ON b.id = c.bb_id WHERE a.flag = TRUE AND b.value BETWEEN 10 AND 99;

-- Yet another query with comments and strings.
INSERT INTO audit (msg, ts) VALUES ('user ''42'' logged in at boot', NOW())
ON CONFLICT (id) DO UPDATE SET msg = EXCLUDED.msg;

WITH ranked_users AS (
    SELECT u.id, u.email, u.created_at,
           COUNT(o.id) AS order_count,
           SUM(o.total_amount) AS lifetime_value,
           ROW_NUMBER() OVER (PARTITION BY u.country ORDER BY SUM(o.total_amount) DESC) AS rk
    FROM users u
    LEFT JOIN orders o ON o.user_id = u.id AND o.status = 'completed'
    WHERE u.created_at > '2020-01-01' AND u.deleted_at IS NULL
    GROUP BY u.id, u.email, u.created_at, u.country
)
SELECT r.id, r.email, r.lifetime_value, r.order_count
FROM ranked_users r
WHERE r.rk <= 10 AND r.lifetime_value > 1000
ORDER BY r.lifetime_value DESC
LIMIT 100;

WITH ranked_users AS (
    SELECT u.id, u.email, u.created_at,
           COUNT(o.id) AS order_count,
           SUM(o.total_amount) AS lifetime_value,
           ROW_NUMBER() OVER (PARTITION BY u.country ORDER BY SUM(o.total_amount) DESC) AS rk
    FROM users u
    LEFT JOIN orders o ON o.user_id = u.id AND o.status = 'completed'
    WHERE u.created_at > '2020-01-01' AND u.deleted_at IS NULL
    GROUP BY u.id, u.email, u.created_at, u.country
)
SELECT r.id, r.email, r.lifetime_value, r.order_count
FROM ranked_users r
WHERE r.rk <= 10 AND r.lifetime_value > 1000
ORDER BY r.lifetime_value DESC
LIMIT 100;

WITH ranked_users AS (
    SELECT u.id, u.email, u.created_at,
           COUNT(o.id) AS order_count,
           SUM(o.total_amount) AS lifetime_value,
           ROW_NUMBER() OVER (PARTITION BY u.country ORDER BY SUM(o.total_amount) DESC) AS rk
    FROM users u
    LEFT JOIN orders o ON o.user_id = u.id AND o.status = 'completed'
    WHERE u.created_at > '2020-01-01' AND u.deleted_at IS NULL
    GROUP BY u.id, u.email, u.created_at, u.country
)
SELECT r.id, r.email, r.lifetime_value, r.order_count
FROM ranked_users r
WHERE r.rk <= 10 AND r.lifetime_value > 1000
ORDER BY r.lifetime_value DESC
LIMIT 100;
