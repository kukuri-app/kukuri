-- #1888 / ADR 0068: 公開スレッドのタイトルの変更と削除。行は metadata の版と削除を持ち、版の大きい記録だけを反映する。
ALTER TABLE public_channels ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE public_channels ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
