# ADR 0068: 公開 channel（公開スレッド）

## Status

Accepted（2026-10-17、#1840）。#1840 の AC-1 で識別・署名・置き場所・投稿の経路を実装した。一覧（AC-2）、画面（AC-3）、配信中の Live（AC-4）は同じ Issue の後続の PR でこの ADR に追記する。

## Context

kukuri の channel は、招待や相互フォローで参加する private channel だけだった。`ChannelRef` は topic の公開タイムライン（ID なし）と private channel（ID あり）の 2 種類で、多くの処理が「channel の ID があれば private」と決めていた。

#1840 では、topic の中に誰でも読み書きできるタイトル付きの channel を作り、掲示板のスレッドとして使う。判断の経緯と案の比較は [#1840 のコメント](https://github.com/kukuri-app/kukuri/issues/1840#issuecomment-6230392310) にある。

## Decision

### 1. 識別子と metadata

- 公開 channel の ID は `pc-<作成時刻のミリ秒>-<作成者の公開鍵の先頭 16 桁>` とする（Live の session ID と同じ 64 bit の結び付け）。
  - 作成時刻から metadata を置いた bucket が決まるので、ID だけで metadata を 1 件読める。
  - private channel の ID（`channel-<ミリ秒>-<8 文字>`）とは形が重ならない。private channel の参加（招待・grant・share の取込み）は、この形の ID を拒否する。
  - 公開 channel かどうかは ID の形で見分ける（`ChannelId::is_public_channel`）。「ID があるか」では決めない。
- metadata は作成者が署名した envelope（種類 `public-channel`）で、中身は ID・所属 topic・タイトル（1〜200 文字、前後に空白なし）・作成時の最初の投稿の ID（無くてもよい）。
  - 署名の時刻は ID の作成時刻（秒）にする。読む側は署名、ID と署名者・時刻の結び付き、上限を確かめる。
  - どの欄も後から変えない。タイトルの変更と channel の削除は無い。
- 作成の順は、ID を決める → 最初の投稿を channel の投稿として保存する → その ID を入れた metadata を署名して保存する。投稿は channel の ID だけを参照するので、互いの hash を待つ循環は起きない。
- ID から読むときは、metadata と、作成時の最初の投稿を作成日と翌日の bucket から 1 件ずつ読む。作成から日を跨いだ channel でも、ID から最初の投稿の返信ツリーを開ける。

### 2. 置き場所

公開 channel の内容は、すべて所属する公開 topic の時間 bucket（[ADR 0054](0054-time-bucketed-docs-replicas.md) §1）に置く。専用の replica は作らない。

| record | key | bucket |
| --- | --- | --- |
| metadata | `channels/list/<channel ID>` | ID の作成時刻 |
| 投稿・返信 | `objects/<id>/{state,envelope}`（既存と同じ） | 署名した作成時刻 |
| channel の時刻順の索引 | `channels/<channel ID>/timeline/<並びの key>/<object id>` | 同上 |
| 返信の索引 | `indexes/thread/<root>/…`（既存と同じ） | 同上 |
| 添付の manifest・リアクション・取り下げ・リンクプレビュー | 既存と同じ | 既存と同じ |

- 投稿の envelope は既存の項目で表す（`channel_id` に公開 channel の ID、`visibility` に Public）。envelope の形は変えない。
- 公開 channel の投稿は、topic のタイムラインの索引（`indexes/timeline/`）にもプロフィールの索引にも載せない。topic のタイムラインとプロフィールの投稿一覧には出ない（2026-10-17 ユーザー判断）。
- Community Node は topic の bucket の「channel_id あり」の投稿と `channels/` の key を読まないので、公開 channel の投稿は CN の検索に出ない（同日、受け入れ済み）。

### 3. 受信側の確認

- 公開 topic の replica は、topic の公開タイムラインの投稿と、公開 channel の ID の形で公開範囲が Public の投稿・リアクション・session を受け入れる。それ以外の channel の申告（private の形の ID、公開範囲が Public でないもの）は従来どおり拒否する。
- private channel の replica は、公開 channel の ID の形の申告を受け入れない。
- 旧版の端末は従来の確認で公開 channel の投稿を拒否するので、旧版では公開 channel も投稿も見えない（同日、受け入れ済み）。

### 4. hint・購読・読み先

- channel の中の投稿・リアクション・取り下げの hint は、channel ごとの hint の topic（`hint/public-channel/<channel ID>`）に送る。rendezvous には公開の鍵を使う。この topic は topic の一覧・診断・CN の公開 topic の索引に出さない（`wire::is_non_public_topic`）。
- 公開 channel の列は `Topic(topic)` と `PublicChannel(topic, channel)` の 2 つの lease を取る（[ADR 0055](0055-demand-owned-network-work.md) §1.1）。`PublicChannel` の task は channel の hint だけを購読し、topic の gossip の停止に従う。topic の lease だけの端末は channel の投稿を受け取らない。
- channel のページと対象の取得は、channel の hint の近隣を先に、足りない分を topic の近隣で埋めて有界に読む（最大 4）。他の参加者が検証して保持した内容の提供（ADR 0054 §4、#1395）は topic の bucket に働くので、提供側の変更は要らない。保持する側は、書き手と同じ関数で §2 の索引を導く。

### 5. API

- `ChannelRef` と `TimelineScope` に公開 channel の種類（`public_channel`）を足す。既存の `public` と、private channel を指す `private_channel`・`channel` の意味は変えない。
- 公開 channel への返信は親の channel に置く。リアクション・取り下げ・bookmark は対象の channel と同じ種類を求める。
- 公開 channel の投稿は repost できない（既存の channel の投稿と同じ。同日のユーザー判断）。
- Live・ゲーム・metaverse（Dome）は公開 channel を受け付けない。Live は #1840 AC-4 で扱う。

## 互換

| 対象 | 変更 | 旧版・既存データへの影響 |
| --- | --- | --- |
| 投稿の envelope | 形は変えない。「channel_id あり + Public」の組を新たに使う | 旧版は受け付けず、表示しない |
| channel の metadata | 新しい種類の署名つき envelope | 旧版は読まない |
| docs の key | topic の bucket に `channels/list/…` と `channels/<ID>/timeline/…` を足す | 旧版と CN は読まない |
| hint | channel ごとの hint の topic を新設する | 旧版は購読しない |
| API・CLI | `ChannelRef`・`TimelineScope` に種類を足し、作成と取得の command を足す | 既存の値と出力の意味は変えない |
| backup・account 同期 | 変えない（公開 channel には参加の状態が無い） | — |

## Feature Data Classification

- Feature 名: 公開 channel（公開スレッド）
- Durable / Transient: Durable（metadata と投稿）。hint は Transient
- Canonical Source: 作成者が署名した metadata の envelope と、投稿者が署名した投稿の envelope
- Replicated?: はい（公開 topic の bucket を有界に読み、検証した record を保持して提供する）
- Rebuildable From: 公開 topic の bucket の署名つき record
- Public Replica / Private Replica / Local Only: Public Replica（所属 topic の時間 bucket）
- Gossip Hint 必要有無: あり（channel ごとの hint の topic）
- Blob 必要有無: 投稿の本文と添付（既存の投稿と同じ）
- SQLite projection 必要有無: 投稿は既存の projection（channel の値は公開 channel の ID）
- 必須 contract: 公開 channel の ID の形と結び付き、metadata の署名、topic の bucket での受け入れと拒否（`crates/core/src/tests/public_channels.rs`、`hydration_integrity_contract.rs`）
- 必須 scenario: 招待の無い端末が ID から読み、投稿・返信・リアクションし、作成者に同じ channel の文脈で届く（`crates/app-api/src/tests/public_channels.rs`）

## References

- [#1840](https://github.com/kukuri-app/kukuri/issues/1840)
- [ADR 0018](0018-channel-first-sidebar-and-unified-epoch-lifecycle.md)（private channel）
- [ADR 0054](0054-time-bucketed-docs-replicas.md)、[ADR 0055](0055-demand-owned-network-work.md)
