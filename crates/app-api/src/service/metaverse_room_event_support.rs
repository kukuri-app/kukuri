use super::*;

const METAVERSE_ROOM_EVENT_BUFFER_LIMIT: usize = 512;

pub(crate) async fn push_metaverse_room_event_buffer(
    buffers: &Arc<Mutex<HashMap<String, VecDeque<MetaverseRoomEventView>>>>,
    view: MetaverseRoomEventView,
) {
    let key = metaverse_room_event_buffer_key(
        view.content.topic_id.as_str(),
        view.content.room_id.as_str(),
    );
    let mut guard = buffers.lock().await;
    let queue = guard.entry(key).or_default();
    if queue
        .iter()
        .any(|existing| existing.envelope_id == view.envelope_id)
    {
        return;
    }
    if queue.iter().any(|existing| {
        existing.envelope.pubkey == view.envelope.pubkey
            && existing.content.peer_id == view.content.peer_id
            && existing.content.session_id == view.content.session_id
            && existing.content.seq >= view.content.seq
    }) {
        return;
    }
    let now = Utc::now().timestamp_millis();
    queue.retain(|existing| metaverse_room_event_is_live(&existing.content, now));
    queue.push_back(view);
    while queue.len() > METAVERSE_ROOM_EVENT_BUFFER_LIMIT {
        queue.pop_front();
    }
}

pub(crate) fn metaverse_room_event_buffer_key(topic_id: &str, room_id: &str) -> String {
    format!("{topic_id}::{room_id}")
}

pub(crate) fn parse_metaverse_room_event_envelope(
    envelope: KukuriEnvelope,
    received_at: i64,
    source_peer: String,
) -> Result<Option<MetaverseRoomEventView>> {
    if envelope.kind != "metaverse-room-event" {
        return Ok(None);
    }
    envelope.verify()?;
    let content: MetaverseRoomEventEnvelopeContentV1 =
        serde_json::from_str(envelope.content.as_str())
            .context("failed to decode metaverse room event content")?;
    validate_metaverse_room_event_content(&content)?;
    Ok(Some(MetaverseRoomEventView {
        envelope_id: envelope.id.0.clone(),
        content,
        envelope,
        received_at,
        source_peer,
    }))
}
