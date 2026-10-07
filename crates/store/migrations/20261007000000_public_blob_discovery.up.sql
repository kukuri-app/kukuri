-- #1632 / ADR 0063: 検証済みの公開記録が参照する blob の索引と、Mainline への告知の予定。
-- 索引は公開記録を書く transaction で記録ごとに置き換える。source_kind は post・link_preview・profile・reaction。
CREATE TABLE public_blob_refs (
  source_kind TEXT NOT NULL,
  source_id TEXT NOT NULL,
  blob_hash TEXT NOT NULL,
  PRIMARY KEY (source_kind, source_id, blob_hash)
) WITHOUT ROWID;
CREATE INDEX idx_public_blob_refs_hash ON public_blob_refs (blob_hash);

-- 公開参照と返せる保持の両方がある hash だけを、512 件まで持つ。own は本人が書いた blob か。
CREATE TABLE public_blob_announcements (
  blob_hash TEXT PRIMARY KEY NOT NULL,
  own INTEGER NOT NULL,
  next_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX idx_public_blob_announcements_next ON public_blob_announcements (next_at);

-- 導入前の公開記録を、種類ごとに rowid の順で 1 回 128 件ずつ索引へ取り込む位置。取り込み終えた種類の行は消す。
-- 導入後に書く行は書込みの transaction で索引へ載るので、位置を巻き戻さない。
CREATE TABLE public_blob_ref_backfill (
  kind TEXT PRIMARY KEY NOT NULL,
  cursor INTEGER NOT NULL
);
INSERT INTO public_blob_ref_backfill (kind, cursor) VALUES ('post', 0), ('profile', 0), ('reaction', 0);
