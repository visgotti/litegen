-- Give keys made by keys their maker's owner (security follow-up, 2026-11-11).
--
-- Since 2026-11-06 an API key acts for the person who made it
-- (`owner_user_id`), so it stops working when they leave its org, and a key
-- made by another key takes that key's owner. Before that change keys made by
-- keys were stored with no owner, so they outlived the person behind them.
--
-- The audit log names the key behind each creation: a `key.create` row has
-- the making key as `actor_key_id` and the new key as `target_id`. Each key
-- with no owner takes the owner of the nearest key up its chain that has one.
-- Keys made with the master key (no `actor_key_id`) keep no owner: they belong
-- to the org. So does a key whose chain ends at a key that is gone or that
-- belongs to the org. An owner already set is never changed. The depth bound
-- ends a cycle, which only a hand-edited log could hold.
--
-- Audit rows are written in the background, so a key whose `key.create` row
-- was never written stays ownerless (docs/RUNBOOK.md, "A key stopped working
-- after someone left").
WITH RECURSIVE made_by(key_id, maker_id) AS (
    SELECT target_id, actor_key_id
    FROM audit_log
    WHERE action = 'key.create' AND target_type = 'api_key' AND actor_key_id IS NOT NULL
),
chain(key_id, maker_id, depth) AS (
    SELECT m.key_id, m.maker_id, 1
    FROM made_by m
    JOIN api_keys k ON k.id = m.key_id
    WHERE k.owner_user_id IS NULL
    UNION ALL
    SELECT c.key_id, m.maker_id, c.depth + 1
    FROM chain c
    JOIN api_keys maker ON maker.id = c.maker_id AND maker.owner_user_id IS NULL
    JOIN made_by m ON m.key_id = c.maker_id
    WHERE c.depth < 64
),
owners(key_id, owner_user_id, depth) AS (
    SELECT c.key_id, maker.owner_user_id, c.depth
    FROM chain c
    JOIN api_keys maker ON maker.id = c.maker_id
    WHERE maker.owner_user_id IS NOT NULL
)
UPDATE api_keys
SET owner_user_id = (
    SELECT o.owner_user_id FROM owners o WHERE o.key_id = api_keys.id ORDER BY o.depth LIMIT 1
)
WHERE owner_user_id IS NULL
  AND id IN (SELECT key_id FROM owners);
