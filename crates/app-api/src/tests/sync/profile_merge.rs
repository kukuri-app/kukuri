//! #1221 R5-C: 手元と remote の複数の source を合わせたプロフィールのページが、行を飛ばさず重複もしないこと。
//!
//! 行の集合を乱数で複数の source へ(重なりを許して)分け、各 source が cursor から乱数の件数だけ読む操作列で、
//! ページを最後までたどる。どの分け方・読む件数・ページの大きさでも、全行が新しい順に 1 回ずつ出ることを確かめる。

use crate::service::profile_timeline_support::{ProfileSourceRows, assemble_profile_page};
use anyhow::Result;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

type Position = (i64, String);

/// 1 つの source が cursor より古い行を新しい順に `take` 件読む。残りがあれば最後に読んだ位置を境界にする。
fn read_source(
    rows: &[Position],
    cursor: Option<&Position>,
    take: usize,
) -> ProfileSourceRows<Position> {
    let mut older = rows
        .iter()
        .filter(|row| cursor.is_none_or(|cursor| *row < cursor))
        .cloned()
        .collect::<Vec<_>>();
    older.sort_by(|left, right| right.cmp(left));
    let more = older.len() > take;
    older.truncate(take);
    ProfileSourceRows {
        frontier: more.then(|| older.last().cloned()).flatten(),
        rows: older.into_iter().map(|row| (row.clone(), row)).collect(),
    }
}

#[tokio::test]
async fn merged_profile_pages_return_every_row_once_in_order() -> Result<()> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..300 {
        let total = 1 + rng.below(60) as usize;
        // 同じ時刻の行も作り、object id で順が決まる場合を含める。
        let all = (0..total)
            .map(|index| (rng.below(20) as i64, format!("object-{index:03}")))
            .collect::<Vec<_>>();
        let source_count = 1 + rng.below(4) as usize;
        let mut sources = vec![Vec::new(); source_count];
        for row in &all {
            let first = rng.below(source_count as u64) as usize;
            sources[first].push(row.clone());
            for (index, source) in sources.iter_mut().enumerate() {
                if index != first && rng.below(3) == 0 {
                    source.push(row.clone());
                }
            }
        }
        let limit = 1 + rng.below(8) as usize;
        let mut cursor: Option<Position> = None;
        let mut seen = Vec::new();
        for _ in 0..=total * 2 + 2 {
            let pages = sources
                .iter()
                .map(|rows| read_source(rows, cursor.as_ref(), 1 + rng.below(6) as usize))
                .collect::<Vec<_>>();
            let (items, next) =
                assemble_profile_page(pages, limit, |row: Position| async move { Ok(Some(row)) })
                    .await?;
            assert!(items.len() <= limit);
            seen.extend(items);
            match next {
                Some(next) => {
                    assert!(
                        cursor.as_ref().is_none_or(|cursor| next < *cursor),
                        "the cursor moves to older rows"
                    );
                    cursor = Some(next);
                }
                None => break,
            }
        }
        let mut expected = all.clone();
        expected.sort_by(|left, right| right.cmp(left));
        assert_eq!(seen, expected, "sources: {sources:?}, limit: {limit}");
    }
    Ok(())
}
