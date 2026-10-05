//! 受信した Dome host の heartbeat の台帳(ADR 0038・0045、#1553)。owner の端末の host の最新の 1 件を、context と
//! instance id で引く。最後の heartbeat から `HEARTBEAT_RETAIN_MILLIS` で捨て、context ごとに部屋の一覧の窓
//! (`LIVE_GAME_LIST_LIMIT`)の件数まで置く(満ちた context では新しい host を置かない)。

use super::*;
use kukuri_core::{
    DOME_HOSTING_HEARTBEAT_GRACE_MILLIS, SignedDomeHostHeartbeatV1, SpatialContextV1,
};

/// grace の判定(closed)の後も同じ時間だけ残し、入室中の画面へ closed を届けてから捨てる。
const HEARTBEAT_RETAIN_MILLIS: i64 = 2 * DOME_HOSTING_HEARTBEAT_GRACE_MILLIS;

/// (context の canonical id, instance id)
type Key = (String, String);

#[derive(Default)]
pub(crate) struct ReceivedDomeHeartbeats {
    latest: BTreeMap<Key, (i64, SignedDomeHostHeartbeatV1)>,
    /// (捨てる時刻, key)。先頭から捨てる。
    expiries: BTreeSet<(i64, Key)>,
}

impl ReceivedDomeHeartbeats {
    /// context と合わせて instance id を導ける host(owner の端末の hosting)の、前より新しい heartbeat を置く。
    /// 捨てる時刻は署名時刻(受信より未来なら受信時刻)から数える。
    pub(crate) fn record(
        &mut self,
        context: &SpatialContextV1,
        instance_id: &str,
        signed: SignedDomeHostHeartbeatV1,
        now: i64,
    ) -> bool {
        self.prune(now);
        let key = (context.canonical_id(), instance_id.to_string());
        let accepted = match self.latest.get(&key) {
            Some((_, current)) => {
                (signed.heartbeat.sequence, signed.heartbeat.sent_at)
                    > (current.heartbeat.sequence, current.heartbeat.sent_at)
            }
            None => self.hosts_in(&key.0).count() < LIVE_GAME_LIST_LIMIT,
        };
        let expires_at = signed
            .heartbeat
            .sent_at
            .min(now)
            .saturating_add(HEARTBEAT_RETAIN_MILLIS);
        if !accepted
            || expires_at <= now
            || dome_instance_id(context, &signed.heartbeat.host_pubkey) != instance_id
        {
            return false;
        }
        self.remove_key(&key);
        self.expiries.insert((expires_at, key.clone()));
        self.latest.insert(key, (expires_at, signed));
        true
    }

    pub(crate) fn latest(
        &mut self,
        context: &SpatialContextV1,
        instance_id: &str,
        now: i64,
    ) -> Option<SignedDomeHostHeartbeatV1> {
        self.prune(now);
        self.latest
            .get(&(context.canonical_id(), instance_id.to_string()))
            .map(|(_, signed)| signed.clone())
    }

    /// context の host。
    pub(crate) fn hosts(&mut self, context: &SpatialContextV1, now: i64) -> Vec<Pubkey> {
        self.prune(now);
        self.hosts_in(&context.canonical_id()).cloned().collect()
    }

    pub(crate) fn remove(&mut self, context: &SpatialContextV1, instance_id: &str) {
        self.remove_key(&(context.canonical_id(), instance_id.to_string()));
    }

    fn hosts_in<'a>(&'a self, context_id: &'a str) -> impl Iterator<Item = &'a Pubkey> {
        self.latest
            .range((context_id.to_string(), String::new())..)
            .take_while(move |((entry_context, _), _)| entry_context == context_id)
            .map(|(_, (_, signed))| &signed.heartbeat.host_pubkey)
    }

    fn remove_key(&mut self, key: &Key) {
        if let Some((expires_at, _)) = self.latest.remove(key) {
            self.expiries.remove(&(expires_at, key.clone()));
        }
    }

    fn prune(&mut self, now: i64) {
        while self
            .expiries
            .first()
            .is_some_and(|(expires_at, _)| *expires_at <= now)
        {
            let (_, key) = self.expiries.pop_first().expect("checked above");
            self.latest.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kukuri_core::{DomeHostHeartbeatV1, KukuriKeys, generate_keys, sign_envelope_json};

    const AT: i64 = 1_000_000;

    fn topic(topic: &str) -> SpatialContextV1 {
        SpatialContextV1::Topic {
            topic_id: TopicId::new(topic),
        }
    }

    /// context の Dome の host(instance id は context と host から導く)。
    fn host(context: &SpatialContextV1) -> (KukuriKeys, String) {
        let keys = generate_keys();
        let instance_id = dome_instance_id(context, &keys.public_key());
        (keys, instance_id)
    }

    fn signed(
        keys: &KukuriKeys,
        instance_id: &str,
        sequence: u64,
        sent_at: i64,
    ) -> SignedDomeHostHeartbeatV1 {
        let heartbeat = DomeHostHeartbeatV1 {
            instance_id: instance_id.into(),
            instance_generation: 1,
            lease_epoch: 1,
            session_id: "session".into(),
            host_pubkey: keys.public_key(),
            participants: 0,
            sleeping: true,
            sequence,
            sent_at,
        };
        let envelope =
            sign_envelope_json(keys, "dome-host-heartbeat", vec![], &heartbeat).expect("sign");
        SignedDomeHostHeartbeatV1 {
            heartbeat,
            envelope,
        }
    }

    #[test]
    fn a_heartbeat_leaves_the_ledger_thirty_seconds_after_it_was_signed() {
        let mut ledger = ReceivedDomeHeartbeats::default();
        let context = topic("retention");
        let (keys, id) = host(&context);
        assert!(ledger.record(&context, &id, signed(&keys, &id, 1, AT), AT));
        assert_eq!(ledger.hosts(&context, AT + 29_999), vec![keys.public_key()]);
        assert!(ledger.latest(&context, &id, AT + 29_999).is_some());
        assert!(ledger.hosts(&context, AT + 30_000).is_empty());
        assert!(ledger.latest.is_empty() && ledger.expiries.is_empty());
        // 署名時刻から 30 秒を過ぎたものは置かない。未来の署名時刻は受信時刻から数える。
        assert!(!ledger.record(&context, &id, signed(&keys, &id, 2, AT), AT + 30_000));
        assert!(ledger.record(&context, &id, signed(&keys, &id, 3, AT + 60_000), AT));
        assert!(ledger.latest(&context, &id, AT + 29_999).is_some());
        assert!(ledger.latest(&context, &id, AT + 30_000).is_none());
    }

    #[test]
    fn only_a_newer_heartbeat_replaces_the_latest_and_extends_its_retention() {
        let mut ledger = ReceivedDomeHeartbeats::default();
        let context = topic("replace");
        let (keys, id) = host(&context);
        let sequence = |ledger: &mut ReceivedDomeHeartbeats, now| {
            ledger
                .latest(&context, &id, now)
                .map(|signed| signed.heartbeat.sequence)
        };
        assert!(ledger.record(&context, &id, signed(&keys, &id, 2, AT), AT));
        assert!(!ledger.record(&context, &id, signed(&keys, &id, 1, AT + 5_000), AT + 5_000));
        assert!(!ledger.record(&context, &id, signed(&keys, &id, 2, AT), AT + 5_000));
        assert!(ledger.record(&context, &id, signed(&keys, &id, 2, AT + 5_000), AT + 5_000));
        assert!(ledger.record(
            &context,
            &id,
            signed(&keys, &id, 3, AT + 10_000),
            AT + 10_000
        ));
        assert_eq!(sequence(&mut ledger, AT + 39_999), Some(3));
        assert_eq!(ledger.expiries.len(), 1);
        ledger.remove(&context, &id);
        assert!(ledger.latest.is_empty() && ledger.expiries.is_empty());
    }

    #[test]
    fn each_context_keeps_hosts_up_to_the_list_window() {
        let mut ledger = ReceivedDomeHeartbeats::default();
        let (full, other) = (topic("full"), topic("other"));
        let hosts = (0..LIVE_GAME_LIST_LIMIT)
            .map(|_| host(&full))
            .collect::<Vec<_>>();
        for (keys, id) in &hosts {
            assert!(ledger.record(&full, id, signed(keys, id, 1, AT), AT));
        }
        let (late, late_id) = host(&full);
        assert!(
            !ledger.record(&full, &late_id, signed(&late, &late_id, 1, AT), AT),
            "a full context takes no new host"
        );
        let (first, first_id) = &hosts[0];
        assert!(
            ledger.record(&full, first_id, signed(first, first_id, 2, AT + 1), AT + 1),
            "a host in the ledger keeps updating"
        );
        let (outside, outside_id) = host(&other);
        assert!(ledger.record(
            &other,
            &outside_id,
            signed(&outside, &outside_id, 1, AT),
            AT
        ));
        assert_eq!(ledger.hosts(&full, AT + 1).len(), LIVE_GAME_LIST_LIMIT);
        assert_eq!(ledger.hosts(&other, AT + 1), vec![outside.public_key()]);
        // 期限で空いた分には、新しい host が入る。
        assert!(ledger.record(
            &full,
            &late_id,
            signed(&late, &late_id, 1, AT + 30_000),
            AT + 30_000
        ));
        assert_eq!(
            ledger
                .hosts(&full, AT + 30_000)
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([first.public_key(), late.public_key()])
        );
    }

    #[test]
    fn a_heartbeat_that_does_not_derive_its_instance_in_the_context_is_ignored() {
        let mut ledger = ReceivedDomeHeartbeats::default();
        let (context, other) = (topic("derive"), topic("elsewhere"));
        let (keys, id) = host(&context);
        // 別の context の hint で届いた。
        assert!(!ledger.record(&other, &id, signed(&keys, &id, 1, AT), AT));
        // 別の host が送った。
        let (stranger, _) = host(&context);
        assert!(!ledger.record(&context, &id, signed(&stranger, &id, 1, AT), AT));
        assert!(ledger.latest.is_empty() && ledger.expiries.is_empty());
    }
}
