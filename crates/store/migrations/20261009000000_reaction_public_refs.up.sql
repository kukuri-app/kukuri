-- #1232 AC-3 / ADR 0063 §1: 自作の custom reaction の asset の画像（envelope の行）と、保存済みの custom reaction の画像
-- （bookmark の行）を公開参照にする。source_kind は reaction_asset・reaction_bookmark。導入後の行は書込みの transaction で
-- 索引へ載る。導入前の行は、告知の task が種類ごとに rowid の順で 1 回 128 件ずつ取り込む（envelope の行は種類で絞らずに
-- rowid の窓で読み、custom reaction の asset だけを索引へ置く）。
INSERT INTO public_blob_ref_backfill (kind, cursor) VALUES ('reaction_asset', 0), ('reaction_bookmark', 0);
