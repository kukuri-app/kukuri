-- #1252: reaction の行(reaction_cache)と、live session・game room の行(live_session_cache・game_room_cache)は、
-- projection_version 2 から、署名つき envelope(または署名された manifest)と、読んだ replica の topic / channel を確かめた
-- record だけから作る。それより前の行は、docs の `state` の申告値をそのまま写した行で、他人の作者・owner や
-- private channel を騙る行を含みうる。検証なしで表示し続けないよう、旧い行を消す。
-- projection は docs から作り直せる手元の cache で、正しい record は手元の docs から反映し直される
-- (docs は読まない。他の表は触らない)。
DELETE FROM reaction_cache WHERE projection_version < 2;
DELETE FROM live_session_cache WHERE projection_version < 2;
DELETE FROM game_room_cache WHERE projection_version < 2;
