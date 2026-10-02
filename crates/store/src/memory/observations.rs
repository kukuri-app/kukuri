use super::*;
use crate::traits::{CONTENT_OBSERVATION_RETENTION_MS, MAX_CONTENT_OBSERVATIONS};

#[async_trait]
impl ContentObservationStore for MemoryStore {
    async fn put_content_observation(&self, row: ContentObservationRow) -> Result<bool> {
        let subject_exists = match row.subject_kind.as_str() {
            "post" => self
                .object_projection_rows
                .read()
                .await
                .contains_key(&EnvelopeId::from(row.subject_id.as_str())),
            "profile" => self
                .profiles
                .read()
                .await
                .contains_key(row.subject_id.as_str()),
            _ => false,
        };
        if !subject_exists {
            return Ok(false);
        }

        let key = (
            row.subject_kind.clone(),
            row.subject_id.clone(),
            row.node_base_url.clone(),
            row.capability.clone(),
        );
        let cutoff = row
            .observed_at
            .saturating_sub(CONTENT_OBSERVATION_RETENTION_MS);
        let mut observations = self.content_observation_rows.write().await;
        observations
            .entry(key)
            .and_modify(|current| {
                current.observed_at = current.observed_at.max(row.observed_at);
            })
            .or_insert(row);
        observations.retain(|_, observation| observation.observed_at >= cutoff);
        if observations.len() > MAX_CONTENT_OBSERVATIONS {
            let mut oldest = observations
                .iter()
                .map(|(key, observation)| (key.clone(), observation.observed_at))
                .collect::<Vec<_>>();
            oldest.sort_by_key(|(_, observed_at)| *observed_at);
            for (key, _) in oldest
                .into_iter()
                .take(observations.len() - MAX_CONTENT_OBSERVATIONS)
            {
                observations.remove(&key);
            }
        }
        Ok(true)
    }

    async fn list_content_observations_at(
        &self,
        subject_kind: &str,
        subject_id: &str,
        now_millis: i64,
    ) -> Result<Vec<ContentObservationRow>> {
        let cutoff = now_millis.saturating_sub(CONTENT_OBSERVATION_RETENTION_MS);
        let mut observations = self.content_observation_rows.write().await;
        observations.retain(|_, observation| observation.observed_at >= cutoff);
        let mut rows = observations
            .values()
            .filter(|row| row.subject_kind == subject_kind && row.subject_id == subject_id)
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            right
                .observed_at
                .cmp(&left.observed_at)
                .then_with(|| left.node_base_url.cmp(&right.node_base_url))
                .then_with(|| left.capability.cmp(&right.capability))
        });
        Ok(rows)
    }
}
