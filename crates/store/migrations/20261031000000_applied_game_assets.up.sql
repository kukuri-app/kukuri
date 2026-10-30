-- #1980: この端末で適用したゲームのアセットの release（発行者・application ID・asset ID ごとに 1 行。Local Only）。
-- 適用は revision が大きいときだけで、古い release で戻さない。行と同じ transaction で、その版の blob と記録に保護参照を付け替える。
CREATE TABLE applied_game_assets (
    publisher_pubkey TEXT NOT NULL,
    application_id TEXT NOT NULL,
    asset_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    release_json TEXT NOT NULL,
    file_path TEXT NOT NULL,
    applied_at INTEGER NOT NULL,
    PRIMARY KEY (publisher_pubkey, application_id, asset_id)
);
