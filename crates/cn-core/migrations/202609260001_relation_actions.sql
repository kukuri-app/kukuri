-- #1221 R5-E: 関係解析を投稿の共起の全件集計から、2 者間のアクションの差分へ移す（2026-09-26 ユーザー決定）。
--
-- アクション（返信・repost・引用・リアクション・フォロー）を 1 行ずつ保存し、行の追加・削除を trigger で
-- ペアの計数と author の参加数へ差分反映する。解析（cn-cli relation analyze）は印の付いたペアと author だけを
-- 古い順に上限つきで処理する。public topic 由来だけを扱い、private channel は入れない。

-- 2 者間のアクション。source_id は由来（返信・repost・引用は行為の投稿、リアクションは reaction id、
-- フォローは '<actor>:<target>'）。anchor_object_id の索引行が消えると、その行も消える（フォローは NULL）。
CREATE TABLE cn_index.relation_actions (
    kind TEXT NOT NULL CHECK (kind IN ('reply', 'repost', 'reaction', 'follow')),
    source_id TEXT NOT NULL,
    actor_pubkey TEXT NOT NULL,
    target_pubkey TEXT NOT NULL CHECK (target_pubkey <> actor_pubkey),
    scope_id TEXT,
    anchor_object_id TEXT,
    PRIMARY KEY (kind, source_id),
    CHECK ((kind = 'follow') = (scope_id IS NULL AND anchor_object_id IS NULL))
);

CREATE INDEX relation_actions_anchor ON cn_index.relation_actions (scope_id, anchor_object_id);

CREATE SEQUENCE cn_index.relation_dirty_seq;

-- 正規化したペア（author_a < author_b）ごとの方向別の件数と、アクションのあった public topic の数。
-- dirty_seq が付いたペアだけを解析が読む。
CREATE TABLE cn_index.relation_pairs (
    author_a TEXT NOT NULL,
    author_b TEXT NOT NULL,
    a_to_b BIGINT NOT NULL DEFAULT 0,
    b_to_a BIGINT NOT NULL DEFAULT 0,
    shared_topics BIGINT NOT NULL DEFAULT 0,
    dirty_seq BIGINT,
    PRIMARY KEY (author_a, author_b),
    CHECK (author_a < author_b)
);

CREATE INDEX relation_pairs_dirty ON cn_index.relation_pairs (dirty_seq)
    WHERE dirty_seq IS NOT NULL;

CREATE TABLE cn_index.relation_pair_topics (
    author_a TEXT NOT NULL,
    author_b TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    actions BIGINT NOT NULL,
    PRIMARY KEY (author_a, author_b, scope_id)
);

CREATE FUNCTION cn_index.relation_action_changed() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    action cn_index.relation_actions%ROWTYPE;
    delta BIGINT;
    a TEXT;
    b TEXT;
    forward BOOLEAN;
    remaining BIGINT;
BEGIN
    IF TG_OP = 'INSERT' THEN
        action := NEW;
        delta := 1;
    ELSE
        action := OLD;
        delta := -1;
    END IF;
    forward := action.actor_pubkey < action.target_pubkey;
    a := LEAST(action.actor_pubkey, action.target_pubkey);
    b := GREATEST(action.actor_pubkey, action.target_pubkey);
    INSERT INTO cn_index.relation_pairs AS p (author_a, author_b, a_to_b, b_to_a, dirty_seq)
    VALUES (
        a,
        b,
        CASE WHEN forward THEN delta ELSE 0 END,
        CASE WHEN forward THEN 0 ELSE delta END,
        nextval('cn_index.relation_dirty_seq')
    )
    ON CONFLICT (author_a, author_b) DO UPDATE SET
        a_to_b = p.a_to_b + EXCLUDED.a_to_b,
        b_to_a = p.b_to_a + EXCLUDED.b_to_a,
        dirty_seq = EXCLUDED.dirty_seq;
    IF action.scope_id IS NOT NULL THEN
        INSERT INTO cn_index.relation_pair_topics AS t (author_a, author_b, scope_id, actions)
        VALUES (a, b, action.scope_id, delta)
        ON CONFLICT (author_a, author_b, scope_id) DO UPDATE SET actions = t.actions + EXCLUDED.actions
        RETURNING actions INTO remaining;
        IF delta = 1 AND remaining = 1 THEN
            UPDATE cn_index.relation_pairs SET shared_topics = shared_topics + 1
            WHERE author_a = a AND author_b = b;
        ELSIF delta = -1 AND remaining = 0 THEN
            UPDATE cn_index.relation_pairs SET shared_topics = shared_topics - 1
            WHERE author_a = a AND author_b = b;
            DELETE FROM cn_index.relation_pair_topics
            WHERE author_a = a AND author_b = b AND scope_id = action.scope_id;
        END IF;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER relation_action_changed
    AFTER INSERT OR DELETE ON cn_index.relation_actions
    FOR EACH ROW EXECUTE FUNCTION cn_index.relation_action_changed();

-- author ごとの public topic の索引件数（dominant topic = cluster 帰属の入力）。0 になった行は消す。
CREATE TABLE cn_index.relation_participation (
    author_pubkey TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    entries BIGINT NOT NULL CHECK (entries > 0),
    PRIMARY KEY (author_pubkey, scope_id)
);

CREATE INDEX relation_participation_dominant
    ON cn_index.relation_participation (author_pubkey, entries DESC, scope_id);

CREATE TABLE cn_index.relation_dirty_authors (
    author_pubkey TEXT PRIMARY KEY,
    dirty_seq BIGINT NOT NULL
);

CREATE INDEX relation_dirty_authors_seq ON cn_index.relation_dirty_authors (dirty_seq);

CREATE FUNCTION cn_index.relation_participation_add(author TEXT, scope TEXT, delta BIGINT)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF delta > 0 THEN
        INSERT INTO cn_index.relation_participation AS p (author_pubkey, scope_id, entries)
        VALUES (author, scope, delta)
        ON CONFLICT (author_pubkey, scope_id) DO UPDATE SET entries = p.entries + EXCLUDED.entries;
    ELSE
        UPDATE cn_index.relation_participation SET entries = entries + delta
        WHERE author_pubkey = author AND scope_id = scope AND entries + delta > 0;
        IF NOT FOUND THEN
            DELETE FROM cn_index.relation_participation
            WHERE author_pubkey = author AND scope_id = scope;
        END IF;
    END IF;
    INSERT INTO cn_index.relation_dirty_authors AS d (author_pubkey, dirty_seq)
    VALUES (author, nextval('cn_index.relation_dirty_seq'))
    ON CONFLICT (author_pubkey) DO UPDATE SET dirty_seq = EXCLUDED.dirty_seq;
END;
$$;

-- 索引行の追加・削除・著者の変更を参加数へ反映し、削除された投稿を起点にしたアクションを消す。
CREATE FUNCTION cn_index.relation_index_entry_changed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('DELETE', 'UPDATE') AND OLD.scope_kind = 'public_topic' THEN
        PERFORM cn_index.relation_participation_add(OLD.author_pubkey, OLD.scope_id, -1);
        IF TG_OP = 'DELETE' THEN
            DELETE FROM cn_index.relation_actions
            WHERE scope_id = OLD.scope_id AND anchor_object_id = OLD.object_id;
        END IF;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') AND NEW.scope_kind = 'public_topic' THEN
        PERFORM cn_index.relation_participation_add(NEW.author_pubkey, NEW.scope_id, 1);
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER relation_index_entry_inserted_or_deleted
    AFTER INSERT OR DELETE ON cn_index.index_entries
    FOR EACH ROW EXECUTE FUNCTION cn_index.relation_index_entry_changed();

CREATE TRIGGER relation_index_entry_author_changed
    AFTER UPDATE OF author_pubkey ON cn_index.index_entries
    FOR EACH ROW WHEN (OLD.author_pubkey IS DISTINCT FROM NEW.author_pubkey)
    EXECUTE FUNCTION cn_index.relation_index_entry_changed();

-- 移行時の 1 回だけ、既存の索引から参加数を作り、全 author の cluster を解析の対象にする。
INSERT INTO cn_index.relation_participation (author_pubkey, scope_id, entries)
SELECT author_pubkey, scope_id, COUNT(*)
FROM cn_index.index_entries
WHERE scope_kind = 'public_topic'
GROUP BY author_pubkey, scope_id;

INSERT INTO cn_index.relation_dirty_authors (author_pubkey, dirty_seq)
SELECT author_pubkey, nextval('cn_index.relation_dirty_seq')
FROM (SELECT DISTINCT author_pubkey FROM cn_index.relation_participation) AS authors;
