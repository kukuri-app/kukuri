use crate::service::*;

impl AppService {
    /// 購読している(lease のある)非公開チャンネルについて、通信層が実際に購読する
    /// `hint/private/<channel_id>` と現在世代の秘密からランデブー鍵を派生する。
    /// 秘密そのものを実行層の境界へ渡さず、不正な秘密は公開用の派生へ落とさない。
    pub async fn private_channel_rendezvous_keys(&self) -> BTreeMap<String, String> {
        let states = self
            .joined_private_channels
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut keys = BTreeMap::new();
        for state in states {
            let rendezvous_topic = kukuri_core::wire::hint_topic_id(&private_channel_hint_topic(
                state.channel_id.as_str(),
            ));
            match kukuri_core::private_topic_rendezvous_key_hex_secret(
                state.current_epoch_secret_hex.as_str(),
                &rendezvous_topic,
            ) {
                Ok(key) => {
                    keys.insert(rendezvous_topic.as_str().to_string(), key);
                }
                Err(error) => {
                    warn!(
                        channel_id = state.channel_id.as_str(),
                        error = %error,
                        "非公開チャンネルのランデブー鍵を派生できなかったため除外しました"
                    );
                }
            }
        }
        keys
    }
}
