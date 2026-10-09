//! 既存投影の写しの進行位置。Tantivy の commit payload と一緒に確定する。

use kukuri_cn_core::IndexScopeKind;
use serde::{Deserialize, Serialize};

use crate::IndexedEntry;

pub const BACKFILL_PAGE: usize = 128;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackfillCursor {
    #[default]
    Start,
    Before(i64),
    Time(i64),
    After {
        time: i64,
        kind: IndexScopeKind,
        scope: String,
        object: String,
    },
    Complete,
}

impl BackfillCursor {
    pub fn after(entry: &IndexedEntry) -> Self {
        Self::After {
            time: entry.created_at,
            kind: entry.scope_kind,
            scope: entry.scope_id.clone(),
            object: entry.object_id.clone(),
        }
    }
}

pub struct BackfillPage {
    pub entries: Vec<IndexedEntry>,
    pub cursor: BackfillCursor,
}
