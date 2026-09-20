-- #1258 / ADR 0053: 投稿の行に、著者が署名つきで申告した docs author の id を持たせる。取り下げの確認は、この docs author と
-- key の組で 1 件読む。NULL の行は旧 record(tag の無い投稿、またはこの列より前に反映した行)で、反映し直さずにそのまま使う。
ALTER TABLE object_index_cache ADD COLUMN source_docs_author TEXT;
