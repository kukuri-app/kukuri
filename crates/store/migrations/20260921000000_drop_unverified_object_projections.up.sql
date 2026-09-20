-- #1248: 投稿の行(object_index_cache)は、projection_version 3 から、署名つき envelope と読んだ replica の
-- topic / channel を確かめた投稿だけから作る。それより前の行は、docs の `state` の申告値をそのまま写した行で、
-- 他人の著者や private channel を騙る行を含みうる。検証なしで表示し続けないよう、旧い行を消す。
-- projection は docs から作り直せる手元の cache で、正しい投稿は手元の docs から反映し直される
-- (docs は読まない。bookmark・通知・reaction の表は触らない)。
DELETE FROM object_index_cache WHERE projection_version < 3;
