//! capability 別のリスクと推奨対応（#359）。
//!
//! この内容は「個人・小規模運営を discourage する」ためのものではない。各 capability の性質を
//! 理解し、限定された責任範囲で現実的に運用するための実践的なガイドとして提供する。
//!
//! `capability.rs` の表示用 note と型付き `policy_descriptor()` を補完する形で、
//! user expectation / authority scope / responsibility boundary / risks /
//! recommended practices / small-scale tips / how to reduce を定義する。

use crate::capability::Capability;

/// capability を運用する際のリスクと推奨対応。
pub struct CapabilityRiskPractices {
    /// この capability が user に生じさせる期待。
    pub user_expectation: &'static str,
    /// authority scope（この capability で node が責任を主張する範囲）。
    pub authority_scope: &'static str,
    /// responsibility boundary（引き受けない範囲）。
    pub responsibility_boundary: &'static str,
    /// 想定されるリスク（法務 / 運用 / プライバシー / safety）。
    pub risks: &'static [&'static str],
    /// 推奨対応。
    pub recommended_practices: &'static [&'static str],
    /// 個人・小規模運営のための実践 tips。
    pub small_scale_tips: &'static str,
    /// scope を狭める / 無効化する方法。
    pub how_to_reduce: &'static str,
}

impl Capability {
    /// 文書生成用の capability 別リスク・推奨対応（#359）。
    pub fn risk_practices(self) -> CapabilityRiskPractices {
        match self {
            Capability::AuthConsent => CapabilityRiskPractices {
                user_expectation: "補助機能を使う際に認証・同意が必要であり、同意状態が記録されること。",
                authority_scope: "本ノードの補助機能に対する認証・同意の管理のみ。",
                responsibility_boundary: "user identity そのものの所有・認証の canonical source ではない（鍵が canonical）。",
                risks: &[
                    "公開鍵・同意レコードの取り扱い（個人情報該当性は運用次第）。",
                    "認証ログ・IP の保持期間が長すぎるとプライバシー負荷になる。",
                ],
                recommended_practices: &[
                    "認証チャレンジは短期 TTL で失効させる。",
                    "同意レコードは撤回可能にし、撤回時に確実に反映する。",
                    "接続ログ保持期間をポリシーに明記し最小化する。",
                ],
                small_scale_tips: "既定の短期 TTL と最小ログで十分。独自の identity DB を作らない。",
                how_to_reduce: "baseline 機能のため無効化はできないが、ログ保持期間を最小化することで負荷を下げられる。",
            },
            Capability::BootstrapAssist => CapabilityRiskPractices {
                user_expectation: "新規 client が最初の peer に到達できること。",
                authority_scope: "本ノードが提供する seed peer 情報の範囲のみ。",
                responsibility_boundary: "到達後の P2P 通信内容・相手 peer の振る舞いには責任を持たない。",
                risks: &[
                    "登録された peer 接続ヒントの一時的な取り扱い。",
                    "悪意ある peer 情報の混入（登録元の検証が弱い場合）。",
                ],
                recommended_practices: &[
                    "ピア登録は短期 TTL の ephemeral state とし長期保存しない。",
                    "登録は認証済み client に限定する。",
                ],
                small_scale_tips: "ephemeral state のみ扱うため運用負荷は低い。TTL を既定のまま使う。",
                how_to_reduce: "`features.bootstrap_assist: false` で無効化できる（onboarding 補助が外れる点に注意）。",
            },
            Capability::TopicRendezvous => CapabilityRiskPractices {
                user_expectation: "同じ topic の相手と接続を成立できること。",
                authority_scope: "topic presence の一時的な突き合わせのみ。",
                responsibility_boundary: "topic 内の投稿内容・モデレーションには責任を持たない（rendezvous は presence のみ）。",
                risks: &["どの topic に接続中かという presence 情報の一時的な扱い。"],
                recommended_practices: &[
                    "presence は TTL 付き ephemeral state とし短期失効させる。",
                    "presence の長期ログを残さない。",
                ],
                small_scale_tips: "KV の TTL に任せれば運用は軽い。presence を分析目的に転用しない。",
                how_to_reduce: "`features.topic_rendezvous: false` で無効化できる。",
            },
            Capability::IrohRelay => CapabilityRiskPractices {
                user_expectation: "Direct P2P が成立しないときも接続が成立すること。",
                authority_scope: "暗号化済みパケットの中継・hole punching 補助の範囲のみ。",
                responsibility_boundary: "中継するトラフィックの内容は復号できず、内容に責任を持たない。",
                risks: &[
                    "帯域・転送量の負荷（高トラフィック capability）。",
                    "電気通信事業の届出該当性の検討が必要になり得る（暗号化済み traffic relay fallback を伴う場合）。",
                    "relay 経由の通信量増加に伴うコスト。",
                ],
                recommended_practices: &[
                    "relay は dedicated mode で運用し、帯域・転送量を監視する。",
                    "暗号化済み traffic fallback が起こり得る旨を外部送信表示・利用規約に明記する（生成文書が自動で含める）。",
                    "転送量上限・レート制限を設ける。",
                ],
                small_scale_tips: "個人運営では転送量課金に注意。必要なら relay を無効化し signaling 補助のみに寄せる構成も検討する。",
                how_to_reduce: "`features.iroh_relay: false` で無効化できる。`traffic_relay_fallback` も併せて見直す。",
            },
            Capability::TrafficRelayFallback => CapabilityRiskPractices {
                user_expectation: "直接接続も relay hole punching も失敗したときの最終手段で接続できること。",
                authority_scope: "暗号化済みトラフィックの fallback 中継の範囲のみ。",
                responsibility_boundary: "中継内容は復号できず、内容には責任を持たない。",
                risks: &[
                    "最も帯域負荷が高くなり得る経路。",
                    "暗号化済みとはいえ通信の中継を行うため、電気通信事業の届出該当性の検討が必要になり得る。",
                ],
                recommended_practices: &[
                    "転送量を監視し上限・レート制限を設ける。",
                    "fallback が起こり得る旨を生成文書（外部送信表示 / 電気通信届出補助）で開示する。",
                ],
                small_scale_tips: "コストが読みにくいため、個人運営では上限設定を必須にする。",
                how_to_reduce: "`features.traffic_relay_fallback: false` で無効化できる。",
            },
            Capability::BlobCache => CapabilityRiskPractices {
                user_expectation: "メディア・添付が高速に取得できること。",
                authority_scope: "本ノードが cache した blob の範囲のみ。",
                responsibility_boundary: "blob 本体の恒久保存・truth source ではない（原本は P2P / author 側）。",
                risks: &[
                    "違法・有害コンテンツ（CSAM 含む）を一時的に保持・配信してしまうリスク。",
                    "ストレージ容量・転送量の負荷。",
                ],
                recommended_practices: &[
                    "index / 配信前に safety scan を行う（#353 の fail-closed 方針）。",
                    "cache は短期で失効させ、blob 本体を恒久保存しない。",
                    "scan 失敗時は配信しない（fail-closed）。",
                ],
                small_scale_tips: "CSAM リスクを避けるため、safety provider を用意できないうちは blob cache を有効化しない選択も現実的。",
                how_to_reduce: "`features.blob_cache: false` で無効化できる。原本配信に任せる。",
            },
            Capability::PrivateMessageStorage => CapabilityRiskPractices {
                user_expectation: "オフライン時の DM が後で受け取れること。",
                authority_scope: "本ノードが保管する暗号化メッセージの範囲のみ。",
                responsibility_boundary: "メッセージ内容は復号できず、会話の当事者・内容に責任を持たない。",
                risks: &[
                    "暗号化済みとはいえメッセージを保管するため、保持期間・削除要求の取り扱いが論点になる。",
                    "ストレージ負荷。",
                ],
                recommended_practices: &[
                    "保管は暗号化済みのまま行い、復号鍵を持たない。",
                    "保持期間を明示し、配送後・期限後に削除する。",
                ],
                small_scale_tips: "保管を持たない（store-and-forward を無効化する）構成が最も負荷・リスクが低い。",
                how_to_reduce: "`features.private_message_storage: false` で無効化できる。",
            },
            Capability::Analytics => CapabilityRiskPractices {
                user_expectation: "（有効時）利用統計が収集され得ること。",
                authority_scope: "本ノードが収集する利用統計の範囲のみ。",
                responsibility_boundary: "個々の user の identity / 投稿内容の所有者ではない。",
                risks: &[
                    "外部 analytics provider への送信が発生し、プライバシー開示が必要になる。",
                    "収集範囲が広いとプライバシー負荷が増す。",
                ],
                recommended_practices: &[
                    "外部送信先と収集項目を privacy policy / 外部送信表示に明記する（生成文書が自動反映）。",
                    "収集を最小化し、可能なら無効のまま運用する。",
                ],
                small_scale_tips: "既定の無効のままで全く問題ない。必要になってから最小範囲で有効化する。",
                how_to_reduce: "`features.analytics: false`（既定）で無効。無効時は外部送信表示にも現れない。",
            },
            Capability::CrashReport => CapabilityRiskPractices {
                user_expectation: "（有効時）クラッシュ情報が送信され得ること。",
                authority_scope: "本ノードが収集するクラッシュ情報の範囲のみ。",
                responsibility_boundary: "user identity / 投稿内容の所有者ではない。",
                risks: &[
                    "外部 crash provider への送信が発生する。",
                    "クラッシュデータに意図せず個人情報が含まれ得る。",
                ],
                recommended_practices: &[
                    "送信先と項目を privacy policy / 外部送信表示に明記する。",
                    "PII を含めないようスクラブする。",
                ],
                small_scale_tips: "既定の無効のままで問題ない。",
                how_to_reduce: "`features.crash_report: false`（既定）で無効。",
            },
            Capability::CloudflareProxy => CapabilityRiskPractices {
                user_expectation: "（有効時）CDN / WAF 経由で接続が保護・高速化されること。",
                authority_scope: "本ノードのエッジ保護・配信の範囲のみ。",
                responsibility_boundary: "Cloudflare の処理に対する責任を kukuri network 全体に拡張しない。",
                risks: &["Cloudflare への外部送信（IP・リクエスト情報）が発生し、開示が必要。"],
                recommended_practices: &[
                    "Cloudflare 経由の外部送信を外部送信表示に明記する（生成文書が自動反映）。",
                ],
                small_scale_tips: "DDoS 対策として有用。使う場合は外部送信表示の自動生成に任せる。",
                how_to_reduce: "`features.cloudflare_proxy: false` で無効化できる。",
            },
            Capability::PushNotification => CapabilityRiskPractices {
                user_expectation: "（有効時）通知が push されること。",
                authority_scope: "本ノードが扱う通知配信の範囲のみ。",
                responsibility_boundary: "通知の元になる投稿内容の truth source ではない。",
                risks: &["push provider への外部送信（device token 等）が発生する。"],
                recommended_practices: &[
                    "device token の取り扱い・保持期間を明示する。",
                    "外部 provider への送信を開示する。",
                ],
                small_scale_tips: "ローカル通知で足りる範囲なら push provider を持たない選択もできる。",
                how_to_reduce: "`features.push_notification: false` で無効化できる。",
            },
            Capability::CommunityIndex => CapabilityRiskPractices {
                user_expectation: "本ノードの検索・発見結果が提供されること。",
                authority_scope: "本ノードが index した対象の範囲のみ（communities_indexed_by_this_node）。",
                responsibility_boundary: "kukuri network 全体の content truth source ではない。他ノードの index に責任を持たない。",
                risks: &[
                    "違法・有害コンテンツ（CSAM 含む）を index・配信してしまうリスク。",
                    "index 対象の選定・除外に伴う運用・法務負荷。",
                ],
                recommended_practices: &[
                    "index 前に safety scan を行い、scan 前 / scan 失敗 / critical verdict を index しない（#353 fail-closed）。",
                    "index 除外を signed moderation event として説明・監査可能にする。",
                ],
                small_scale_tips: "safety provider を用意できるまで有効化しない判断も妥当。",
                how_to_reduce: "`features.community_index: false`（既定）で無効化できる。",
            },
            Capability::Moderation => CapabilityRiskPractices {
                user_expectation: "本ノードの moderation 判断・ラベルが提供されること。",
                authority_scope: "本ノードが発行した moderation event の範囲のみ（issuer node の authority scope 内）。",
                responsibility_boundary: "network-wide moderation authority ではない。moderation event は optional trust input（#362）。",
                risks: &[
                    "誤検知・過剰除外による表現への影響。",
                    "CSAM 等 critical safety の取り扱いに伴う法務・心理的負荷。",
                ],
                recommended_practices: &[
                    "known CSAM / suspected unknown CSAM / 一般モデレーションを分離する（#353）。",
                    "moderation event を署名し、visibility（local/subscribed/public）を適切に設定する（#362）。",
                    "suspected unknown は local visibility を基本とする。",
                ],
                small_scale_tips: "人力レビュー依存を避け、provider / mock 経由の自動判定を前提にする（#353）。",
                how_to_reduce: "`features.moderation: false`（既定）で無効化できる。",
            },
            Capability::CommunityLocalTrust => CapabilityRiskPractices {
                user_expectation: "本ノードの trust / risk signal が参照できること。",
                authority_scope: "本ノードが発行した trust signal の範囲のみ。",
                responsibility_boundary: "network-wide な信頼権威ではない。trust signal は optional trust input（#362）。",
                risks: &["断定ラベル化による誤った信頼判断の誘発。"],
                recommended_practices: &[
                    "断定ラベルではなく根拠つき risk signal（basis / confidence / severity）として扱う。",
                    "visibility を適切に設定し、誤検知を public に拡散しない。",
                ],
                small_scale_tips: "local-first で運用し、subscribed/public への昇格は慎重に行う。",
                how_to_reduce: "`features.community_local_trust: false`（既定）で無効化できる。",
            },
            Capability::ReportEndpoint => CapabilityRiskPractices {
                user_expectation: "本ノードに対して通報を送れること。",
                authority_scope: "本ノードが関与した対象への通報受付の範囲のみ（中央通報窓口ではない）。",
                responsibility_boundary: "kukuri network 全体・他ノードが関与した対象の通報窓口ではない（#310）。",
                risks: &[
                    "通報内容・通報者連絡先の取り扱い。",
                    "通報を受け付ける以上、最低限のトリアージ運用が必要になる。",
                ],
                recommended_practices: &[
                    "reporter の identity / social graph を保持せず、明示入力された連絡先のみ任意保存する（#370 実装）。",
                    "受信通報を `cn-cli reports` で確認し、保持期間ポリシーに従う。",
                    "critical safety 区分は evidence を再配布せず内容で説明する。",
                ],
                small_scale_tips: "完全な ticketing system は不要。`cn-cli reports list/show` で確認できれば十分。",
                how_to_reduce: "`features.report_endpoint: false` で無効化できる。無効時は abuse contact が窓口になる。",
            },
            Capability::RightsRequestEndpoint => CapabilityRiskPractices {
                user_expectation: "申請前に本ノードが実行できる措置と authority 外の範囲を確認し、申出後の状態を追跡できること。",
                authority_scope: "本ノードの索引・検索・発見・推薦・moderation・blob cache に適用可能な node-local 措置のみ。",
                responsibility_boundary: "他ノード、第三者端末、投稿正本、Direct P2P、暗号化 relay packet、既取得データには強制力を持たない。",
                risks: &[
                    "申出人の個人情報、代理権、権利主張、証拠参照を取り扱う。",
                    "対応範囲の説明が不十分だと、network-wide な削除が可能だという誤解を招く。",
                ],
                recommended_practices: &[
                    "版付き scope と明示同意を受付前に検証し、client の申告とは独立に対象 scope を判定する。",
                    "証拠は URL・hash・識別子だけを受け取り、ファイルや対象内容を複製しない。",
                    "状態遷移と送信防止を append-only event と operator audit に記録する。",
                ],
                small_scale_tips: "初回応答の現実的な運用目標を設定し、専用 CLI と公開 status で小さく運用できる。",
                how_to_reduce: "`features.rights_request_endpoint: false`（既定）で無効化できる。無効時は専用 URL を公開しない。",
            },
            Capability::TesterFeedback => CapabilityRiskPractices {
                user_expectation: "本ノードへテスターとして利用経験のフィードバックを送れること。",
                authority_scope: "本ノードが受信・保存したフィードバックの範囲のみ（中央収集窓口ではない）。",
                responsibility_boundary: "フィードバック内容への対応・修正を保証しない。通報・権利侵害申出の窓口ではない。",
                risks: &[
                    "自由記述に意図せず個人情報・機微情報が書き込まれ得る。",
                    "蓄積した本文の保持期間・閲覧範囲の管理が必要になる。",
                ],
                recommended_practices: &[
                    "送信者の identity / social graph を保持しない（#802 実装）。",
                    "保持期間（tester_feedback_days）を設定し、期限後に削除する。",
                    "閲覧は admin dashboard / `cn-cli tester-feedback` に限定する。",
                ],
                small_scale_tips: "テスト運用時のみ有効化し、収集が不要になったら無効へ戻す運用で十分。",
                how_to_reduce: "`features.tester_feedback: false`（既定）で無効化できる。",
            },
            Capability::DomeHosting => CapabilityRiskPractices {
                user_expectation: "owner不在時もlease期限内はDome sessionが継続し、Nodeがphysics authorityになること。",
                authority_scope: "owner署名Hosting Leaseが指すDome、epoch、manifest、期限の範囲のみ。",
                responsibility_boundary: "Domeのcanonical ownerではなく、他Domeやlease外のworld definitionを変更しない。",
                risks: &[
                    "physics sessionのCPU・memory・帯域負荷。",
                    "participant inputと一時的な行動状態を処理するプライバシー負荷。",
                ],
                recommended_practices: &[
                    "Domeごとにactive assignmentを一件へ制限する。",
                    "stale epochとsplit-brainをfail closedで拒否する。",
                    "raw inputを永続化せず、lease expiryとcloseでsessionを停止する。",
                ],
                small_scale_tips: "既定無効のまま始め、同時host数とlease期間を小さく制限して有効化する。",
                how_to_reduce: "`features.dome_hosting: false`（既定）で無効化できる。",
            },
            Capability::PublicBlobSearch => CapabilityRiskPractices {
                user_expectation: "手元・既知の相手に無い公開 blob の保持端末を、Web 等の client が見つけられること。",
                authority_scope: "認証・同意済みの client が依頼した公開 blob の hash の検索だけ。",
                responsibility_boundary: "blob を取得・保存・中継しない。候補は保持・権限の証明ではなく、取得の成否を保証しない。",
                risks: &[
                    "依頼された hash から導いた infohash と本ノードの IP アドレスが DHT の参加者から観測され得る。",
                    "DHT と補助 index への照会の負荷。",
                ],
                recommended_practices: &[
                    "検索の同時数を全体 32 件・端末ごと 8 件、1 件を 10 秒までに制限する（実装の既定）。",
                    "受け取った hash と見つけた端末の情報を保存しない（log にも出さない）。",
                ],
                small_scale_tips: "kukuri の補助 index の一覧の鍵が入った配布物で、必要になってから有効化する。",
                how_to_reduce: "`features.public_blob_search: false`（既定）で無効化できる。",
            },
        }
    }
}
