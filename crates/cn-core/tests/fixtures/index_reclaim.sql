-- #1736 の再現データ。index_retention.rs の専用 TestDatabase にだけ作る。
-- fixture の投入時だけ trigger と FK を止める。回収時には通常どおり動かす。

-- 受入下限 floor = 1790000000（unix 秒）。期限切れの行は floor より古く（floor - 1000000 + g）、
-- 期限内の行は floor 以降（floor + 10 × g）。どの表も作成時刻の古い順に入れる（trigger と FK を止める）。
-- 期限切れの索引行は期限内の判定（1〜1500 番）を参照し、期限切れの判定はどの索引行からも参照されない
-- （回収は索引行を判定より先に消すので、判定の回収が上限まで消す回には期限切れの索引行が残っていない）。
CREATE OR REPLACE FUNCTION public.probe_seed(kind text, from_g int, to_g int)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    f constant bigint := 1790000000;
    t constant text := left(kind, 1);
BEGIN
    PERFORM set_config('session_replication_role', 'replica', true);
    CREATE TEMP TABLE IF NOT EXISTS probe_g (g int, ts bigint) ON COMMIT DROP;
    TRUNCATE probe_g;
    INSERT INTO probe_g
    SELECT g, CASE WHEN kind = 'live' THEN f + 10 * g ELSE f - 1000000 + g END
    FROM generate_series(from_g, to_g) g;

    INSERT INTO cn_index.relation_actions
        (kind, source_id, actor_pubkey, target_pubkey, scope_id, anchor_object_id, created_at)
    SELECT k,
           CASE WHEN k = 'follow' THEN actor || ':' || target ELSE md5('s' || t || g) || md5('u' || t || g) END,
           actor, target,
           CASE WHEN k = 'follow' THEN NULL ELSE 'kukuri:topic:' || md5('topic' || (g % 20)) END,
           CASE WHEN k = 'follow' THEN NULL ELSE md5('o' || t || g) || md5('p' || t || g) END,
           ts
    FROM (SELECT g, ts, (ARRAY['reply', 'repost', 'reaction', 'follow'])[1 + g % 4] AS k,
                 md5('a' || (g % 3000)) || md5('b' || (g % 3000)) AS actor,
                 md5('c' || t || g) || md5('d' || t || g) AS target
          FROM probe_g) s
    ORDER BY g;

    INSERT INTO cn_index.known_post_withdrawals (scope_kind, scope_id, object_id, created_at)
    SELECT 'public_topic', 'kukuri:topic:' || md5('topic' || (g % 20)),
           md5('w' || t || g) || md5('x' || t || g), ts
    FROM probe_g ORDER BY g;

    INSERT INTO cn_safety.scan_verdicts
        (id, subject_kind, subject_id, action, critical, reason_code, confidence, provider, policy_version,
         scanned_at, updated_at, source_fingerprint, scan_config_fingerprint, derived_tags, advisory_labels)
    SELECT md5('v' || t || g)::uuid::text, 'post', md5('i' || t || g) || md5('j' || t || g),
           'allow', false, 'clean', (g % 100)::smallint, 'vlm-moderation', 'v3',
           to_char(to_timestamp(ts) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), to_timestamp(ts),
           md5('f' || t || g) || md5('h' || t || g),
           'vlm-moderation|TextClassification,ImageClassification',
           '["outdoor", "cat"]'::jsonb, '[]'::jsonb
    FROM probe_g ORDER BY g;

    INSERT INTO cn_index.index_entries
        (scope_kind, scope_id, object_id, author_pubkey, created_at, source_replica_id,
         verdict_id, verdict_action, critical, indexed_at)
    SELECT 'public_topic', 'kukuri:topic:' || md5('topic' || (g % 20)),
           md5('i' || t || g) || md5('j' || t || g),
           md5('a' || (g % 3000)) || md5('b' || (g % 3000)), ts,
           'topic:' || md5('r' || (g % 20)) || md5('q' || (g % 20)),
           md5('vl' || CASE WHEN kind = 'live' THEN g ELSE 1 + g % 1500 END)::uuid::text,
           'allow', false, to_timestamp(ts)
    FROM probe_g ORDER BY g;

    INSERT INTO cn_safety.content_scan_cache (cache_key, scan_results, completed_at)
    SELECT md5('k' || t || g) || md5('m' || t || g),
           jsonb_build_array(jsonb_build_object(
               'provider', 'vlm-moderation', 'capability', 'image_classification',
               'outcome', 'completed', 'known_hash_match', false, 'labels', '[]'::jsonb,
               'derived_tags', '["outdoor", "cat", "sky"]'::jsonb)),
           to_timestamp(ts)
    FROM probe_g ORDER BY g;
END;
$$;

-- 解除した scope の回収の状態: 索引の真実源を作り直し、解除した scope の行 retired 件を、他の 20 個の scope の行
-- others 件の間に、作成時刻の順で均等に混ぜて入れる（解除した topic の投稿は、他の topic の投稿と同じ期間に入る）。
CREATE OR REPLACE FUNCTION public.probe_seed_scope(retired int, others int)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    f constant bigint := 1790000000;
    total constant bigint := retired + others;
BEGIN
    PERFORM set_config('session_replication_role', 'replica', true);
    TRUNCATE cn_index.index_entries;
    INSERT INTO cn_index.index_entries
        (scope_kind, scope_id, object_id, author_pubkey, created_at, source_replica_id,
         verdict_id, verdict_action, critical, indexed_at)
    SELECT 'public_topic',
           CASE WHEN mine THEN 'kukuri:topic:' || md5('retired') ELSE 'kukuri:topic:' || md5('topic' || (g % 20)) END,
           md5('is' || g) || md5('js' || g),
           md5('a' || (g % 3000)) || md5('b' || (g % 3000)), f + 10 * g,
           'topic:' || md5('r' || (g % 20)) || md5('q' || (g % 20)),
           md5('vl' || (1 + g % 1500))::uuid::text, 'allow', false, to_timestamp(f + 10 * g)
    FROM (SELECT g, (g * retired / total) > ((g - 1) * retired / total) AS mine
          FROM generate_series(1::bigint, total) g) s
    ORDER BY g;
END;
$$;
