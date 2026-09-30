use kukuri_core::{EnvelopeId, KukuriEnvelope};

use crate::models::{DirectMessageMessageRow, ObjectProjectionRow, Page, TimelineCursor};

pub(crate) fn apply_desc_cursor(
    items: Vec<KukuriEnvelope>,
    cursor: Option<TimelineCursor>,
    limit: usize,
) -> Page<KukuriEnvelope> {
    let mut filtered = items
        .into_iter()
        .filter(|envelope| {
            cursor.as_ref().is_none_or(|cursor| {
                envelope.created_at < cursor.created_at
                    || (envelope.created_at == cursor.created_at && envelope.id < cursor.object_id)
            })
        })
        .take(limit)
        .collect::<Vec<_>>();
    let next_cursor = if filtered.len() == limit {
        filtered.last().map(|envelope| TimelineCursor {
            created_at: envelope.created_at,
            object_id: envelope.id.clone(),
        })
    } else {
        None
    };
    Page {
        items: std::mem::take(&mut filtered),
        next_cursor,
    }
}

pub(crate) fn apply_asc_cursor(
    items: Vec<KukuriEnvelope>,
    cursor: Option<TimelineCursor>,
    limit: usize,
) -> Page<KukuriEnvelope> {
    let mut filtered = items
        .into_iter()
        .filter(|envelope| {
            cursor.as_ref().is_none_or(|cursor| {
                envelope.created_at > cursor.created_at
                    || (envelope.created_at == cursor.created_at && envelope.id > cursor.object_id)
            })
        })
        .take(limit)
        .collect::<Vec<_>>();
    let next_cursor = if filtered.len() == limit {
        filtered.last().map(|envelope| TimelineCursor {
            created_at: envelope.created_at,
            object_id: envelope.id.clone(),
        })
    } else {
        None
    };
    Page {
        items: std::mem::take(&mut filtered),
        next_cursor,
    }
}

pub(crate) fn apply_desc_projection_cursor(
    items: Vec<ObjectProjectionRow>,
    cursor: Option<TimelineCursor>,
    limit: usize,
) -> Page<ObjectProjectionRow> {
    let mut filtered = items
        .into_iter()
        .filter(|row| {
            cursor.as_ref().is_none_or(|cursor| {
                row.created_at < cursor.created_at
                    || (row.created_at == cursor.created_at && row.object_id < cursor.object_id)
            })
        })
        .take(limit)
        .collect::<Vec<_>>();
    let next_cursor = if filtered.len() == limit {
        filtered.last().map(|row| TimelineCursor {
            created_at: row.created_at,
            object_id: row.object_id.clone(),
        })
    } else {
        None
    };
    Page {
        items: std::mem::take(&mut filtered),
        next_cursor,
    }
}

pub(crate) fn apply_desc_direct_message_cursor(
    items: Vec<DirectMessageMessageRow>,
    cursor: Option<TimelineCursor>,
    limit: usize,
) -> Page<DirectMessageMessageRow> {
    let mut filtered = items
        .into_iter()
        .filter(|row| {
            cursor.as_ref().is_none_or(|cursor| {
                row.created_at < cursor.created_at
                    || (row.created_at == cursor.created_at
                        && row.message_id.as_str() < cursor.object_id.as_str())
            })
        })
        .take(limit)
        .collect::<Vec<_>>();
    let next_cursor = if filtered.len() == limit {
        filtered.last().map(|row| TimelineCursor {
            created_at: row.created_at,
            object_id: EnvelopeId::from(row.message_id.clone()),
        })
    } else {
        None
    };
    Page {
        items: std::mem::take(&mut filtered),
        next_cursor,
    }
}

pub(crate) fn apply_asc_projection_cursor(
    items: Vec<ObjectProjectionRow>,
    cursor: Option<TimelineCursor>,
    limit: usize,
) -> Page<ObjectProjectionRow> {
    let mut filtered = items
        .into_iter()
        .filter(|row| {
            cursor.as_ref().is_none_or(|cursor| {
                row.created_at > cursor.created_at
                    || (row.created_at == cursor.created_at && row.object_id > cursor.object_id)
            })
        })
        .take(limit)
        .collect::<Vec<_>>();
    let next_cursor = if filtered.len() == limit {
        filtered.last().map(|row| TimelineCursor {
            created_at: row.created_at,
            object_id: row.object_id.clone(),
        })
    } else {
        None
    };
    Page {
        items: std::mem::take(&mut filtered),
        next_cursor,
    }
}
