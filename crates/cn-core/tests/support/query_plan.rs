use std::collections::BTreeMap;

use serde_json::Value;

/// 実行計画（`EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`）の走査の節点で、表ごとに読んだ行の数と
/// 触ったページの数を足す。選んだ行を位置で消す Tid Scan は、その行だけを読む（行の数に出る）。ページの
/// 数は、選ぶ段（InitPlan）で触った他の表の分を含んで計画の形で変わるので足さない。
pub fn add_reads(plan: &Value, totals: &mut BTreeMap<String, (f64, f64)>) {
    if let Some(relation) = plan["Relation Name"].as_str() {
        let rows: f64 = [
            "Actual Rows",
            "Rows Removed by Filter",
            "Rows Removed by Index Recheck",
        ]
        .iter()
        .filter_map(|key| plan[*key].as_f64())
        .sum();
        let pages: f64 = ["Shared Hit Blocks", "Shared Read Blocks"]
            .iter()
            .filter(|_| plan["Node Type"] != "Tid Scan")
            .filter_map(|key| plan[*key].as_f64())
            .sum();
        let total = totals.entry(relation.to_string()).or_default();
        total.0 += rows * plan["Actual Loops"].as_f64().unwrap_or(1.0);
        total.1 += pages;
    }
    for child in plan["Plans"].as_array().into_iter().flatten() {
        add_reads(child, totals);
    }
}
