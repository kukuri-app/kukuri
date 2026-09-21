# ADR 0053: アカウントの署名鍵から docs author を導出し、著者の record を docs author と key の組で読む

## Status

Accepted

## Context

docs（iroh-docs）の entry は（namespace、docs author、key）の組で、docs author の署名を持つ。public topic の replica は topic id を知る誰もが書けるので、
同じ key には、その replica に書ける者の数だけ record が並びうる。key 指定の読み出しは、それらを docs author の昇順で返す。

kukuri は、record の正しさを docs の層ではなく署名つき envelope で判定する（ADR 0011 §14.1、ADR 0052 §2）。検証に通る record は、どの docs author が書いていても正しい。
足りなかったのは、検証に通る 1 件を、列を走査せずに見つける手段である。

- 書き込みは端末ごとの既定の docs author（乱数の鍵）で行い、アカウントの署名鍵（secp256k1）との対応はどこにも無かった。`DocRecord` なども docs author を持たなかった。
- そのため読む側は、key の列を先頭から上限つき（8 件）で調べるしかなかった（Issue #1248・#1250）。docs author は誰でもいくつでも作れるので、9 件以上の不正な record を
  小さい docs author id で積まれると、著者の正しい取り下げは反映されず（本文が残る）、著者の正しい投稿は反映されない（投稿が出ない）（Issue #1258）。
- iroh-docs は、同期で受け取る entry について namespace・entry の署名・timestamp だけを検査する。アプリ側の判定を挟む口は無く、他の docs author の entry は消せない。
  検証に通らない record の伝播を、client が止める手段は無い。

前提として、同期は best effort であり（ADR 0052 §1）、取り下げがネットワーク全体へ伝播することは保証しない。ADR 0032 の取り下げは協力的な client への要請である。
本 ADR が扱うのは、record を手元に持っている viewer が、第三者の低コストな操作で、その record を読めなくされることである。

## Decision

### 1. docs author の導出

- docs author の秘密鍵（ed25519 の 32 byte）を、アカウントの署名鍵の秘密鍵（32 byte）から決定的に導出する。
  `blake3::derive_key("kukuri.app 2026-09-21 docs author v1", <アカウントの秘密鍵 32 byte>)` の出力 32 byte を、iroh-docs の `Author::from_bytes` へ渡す。
- 導出は一方向で、docs author の秘密鍵からアカウントの秘密鍵は求められない。用途の文字列（context）で、他の用途の鍵導出と分離する。
- 導出の単位はアカウントで 1 つとする。replica ごとには分けない。docs author の id を見られるのは、その replica を同期できる者（= 同じ replica の envelope の
  `pubkey` を見られる者）に限られ、分けても隠せる情報が増えない。分けると、読む側が別の replica（epoch をまたぐ取り下げ、replica の時間分割）での著者の docs author を知る手段が無くなる。
- 同じアカウント鍵を持つ端末は、同じ docs author になる（複数端末でのアカウント鍵の共有は Issue #1211・#1218・#1219 が所有する）。
- runtime は起動時に、導出した author を iroh-docs へ import し、既定の docs author にする。アカウントの切り替えと復元は runtime を作り直すので、同じ経路を通る。
  書き込みごとに docs author を選ぶ口は作らない。
- それまで使っていた端末ごとの docs author の鍵は、保存場所に残す（消すと、その名義の entry の tombstone を書けなくなる）。その名義の entry は旧 record として扱う。
- 既定の docs author を切り替えた後に同じ key を書き直すと、その key に新旧 2 つの名義の entry が並ぶ。key だけを指定して先頭を読む読み手（profile、session の state など、
  投稿と取り下げ以外の key）が旧い値を読まないよう、書き込みのたびに、旧い名義が同じ key に持つ entry を消す（旧い名義ごとに key を 1 つ調べるだけで、件数に依存しない）。
  prefix の削除は、旧い名義でも行う（旧い名義で書いた entry は、その名義でしか消せない）。
- 導出した秘密鍵は、iroh-docs の保存場所（端末内）以外に保存しない。docs・gossip・log・エラー文言へ出さない。
  端末バックアップ（ADR 0048）は iroh-docs の保存場所ごと暗号化して運ぶので、導出した鍵も暗号化された backup に含まれる（同じ backup がアカウントの秘密鍵も含む。平文では出ない）。
  復元先は、アカウントの鍵から導出し直して設定するので、backup の中の鍵に依存しない。

### 2. 著者が自分の docs author を示す方法

- 投稿（post・comment・repost）の envelope に、tag `["docs_author", <docs author の id。64 桁の hex>]` を入れる。tag は署名の対象であり、
  「この `pubkey` の著者は、この docs author で書く」という著者自身の申告になる。
- gossip の hint の `HintObjectRef` に、任意の field `docs_author` を入れる（署名なしの手がかり）。public topic の hint だけに入れる。
  private channel の hint の topic は epoch の秘密に依存せず、現在の epoch を同期できない者にも届きうるので、docs author の id を載せない。
  channel の参加者は、docs の event と索引の entry から同じ手がかりを得る。
- docs の層の情報として、`DocRecord`・`DocKeyEntry`・`DocEvent` に、その entry を書いた docs author の id を持たせる（値を持たない実装のために任意）。
  索引の entry（`indexes/timeline/…`・`indexes/thread/…`）は、書いた docs author 自身が手がかりになるので、値は変えない。
- docs author の id は、`pubkey` と同じ範囲へ公開される識別子である（その replica を同期できる者と、public topic の hint を受け取る者）。新しい送信先は増えない。

### 3. 読む順序

投稿の envelope（`objects/<object id>/envelope`）:

1. docs author の手がかり（docs の event、hint、索引の entry の docs author）があれば、「docs author と key の組」を指定して 1 件読む。
2. 読んだ envelope を、これまでと同じ検証（`VerifiedPost::verify`: 署名、object id、投稿の kind、replica の scope）に通す。通ればそれを使う。
3. 手がかりが無い、組の record が無い、検証に通らない場合は、key だけを指定した上限つき（8 件）の読み出しへ落とす（旧 record と、手がかりが偽の場合）。

手がかりは読む record を選ぶことだけに使う。手がかりが偽でも、検証に通らない値が反映されることは無い。
手がかりを持たない入口（利用者の操作からの key 指定の反映、返信先の preview、repost 元の解決、hint の `ThreadUpdated`）は、投稿の envelope を 3 の読み出しで読む
（旧 record と同じ best effort）。その投稿が反映済みなら、取り下げは行の列の docs author で読める。

取り下げ（`withdrawals/<object id>/state`）:

1. 対象の投稿の docs author（検証済みの envelope の tag。反映済みの行には列として保存する）があれば、「docs author と key の組」で 1 件読む。
   取り下げを書いた docs author の手がかり（docs の event、hint）があれば、その組でも 1 件読む。tag の無い旧い投稿の取り下げでも、手がかりが届いていれば読める。
   手がかりは署名の無い入力で、読む record を選ぶことだけに使う。取り下げの検証に使う対象の envelope も、対象の投稿の docs author が分かっていれば、その組で 1 件読む。
2. 読んだ取り下げを、これまでと同じ検証（`verify_post_withdrawal`: 署名、対象の object id、対象の著者との一致）に通す。通れば反映する。
3. tag が無い（旧 record）、組の record が無い、検証に通らない場合は、key だけを指定した上限つき（8 件）の読み出しへ落とす。

docs author が合うことは、検証の代わりにしない。他人の docs author の名義の entry は誰にも書けない（entry の署名を、同期の時点で全 peer が検査する）ので、
同じ key に不正な record を何件積まれても、1 の読み出しには入らない。1 件の反映で読む docs の record 数は定数で、その key に積まれた record 数に依存しない。

- 「docs author と key の組」の読み出しは `DocsSync` の独立した method とし、trait の既定実装はエラーを返す（key だけの読み出しへ黙って落ちる実装を作らない）。
  docs author を持たない実装は「docs author を照会できない」と答え、その場合は tag も手がかりも付けない。
- docs と projection の読み書きの失敗は、これまでと同じくエラーとして返す。

### 4. 旧 record と相互運用

- 旧 client が書いた投稿（tag なし）と、その取り下げは、上限つきの読み出しのままとする（best effort）。不正な record を 9 件以上積まれた旧 record は救済しない
  （2026-09-21 決定。利用者が 10 名程度で、投稿も少ない）。
- 新しい client が既定の docs author を切り替える前に書いた自分の投稿も、旧 record である。
- 旧 client と CN indexer は、未知の tag と hint の未知の field を無視し、key だけを指定してこれまでどおり読める（hint は JSON で、未知の field を拒否しない。
  envelope の tag の集合を照合する読み手は無い）。docs の key と値の形は変えない。
- projection の投稿の行に docs author の列を足す。列が空の行は旧 record として扱い、反映し直さない。

### 5. 採らない方式

- 上限に達して取り下げが見つからないときは伏せる: 不正な record を積むだけで他人の投稿を伏せられる。
- 上限を上げる、走査を受け入れて背景で読み進める: 読む量と取得する本体の量を、攻撃者が決められる（`AGENTS.md` の「設計原則: 件数に依存しない処理」）。
- 検証に通らない record を伝播させない: iroh-docs の層に手段が無い（Context）。
- 端末ごとの docs author を署名つきで公開する: 投稿した端末と取り下げる端末が違うと成り立たない。
- 著者しか書けない replica にも置く、docs とは別の経路で envelope を運ぶ: 開く replica の数や復旧の経路という別の問題を増やす。

### 6. author replica の読み出し（#1239 で追加）

author replica（`author::<pubkey>`）の profile・follow・block・custom reaction の asset も、誰でも書ける replica に置かれる。
同じ key に他の名義の record を積まれると、先頭の 1 件や上限つきの読み出しでは、著者の record を隠せる。投稿と同じ方法で、名義を決定的に選んで読む。

- profile（`identity-profile`）・follow の edge（`follow-edge`）・block の edge（`block-edge`）・custom reaction の asset（`custom-reaction-asset`）の envelope にも、
  §2 と同じ `docs_author` の tag を入れる。
- 読む側は、著者の docs author の id を、署名を検証した著者の envelope の tag から覚え、端末内に保存する（`author_docs_authors`。著者ごとに 1 行）。
  自分の docs author は手元の値を使う。profile を先に読み、そこで覚えた docs author を、同じ回の follow・block の読み出しから使う。
- 著者の docs author が分かっているとき:
  - profile・follow・block の record と、それが指す `envelopes/<id>` は、docs author と key の組で 1 件読む。
  - follow・block の窓（上限つきの key の一覧）は、docs author を指定した一覧（`query_replica_keys_by_author`）で作る。他の名義の key で窓を埋められない。
  - follow の通知の起点と自分の custom reaction の asset の一覧も、docs author を指定して読む。プロフィールのタイムラインの行（`profile/posts/`・
    `profile/reposts/`）も、組で先に読む。
- 組の record が無い・検証に通らないとき（tag の無い旧 record、端末ごとの旧名義）と、docs author が分からないときは、§3 と同じく key だけを指定した上限つき（8 件）の読み出しに落とし、
  検証に通ったものから最も新しい envelope を選ぶ（best effort）。旧名義の edge は、docs author を指定した窓には入らない（docs の event と key 指定の読み出しでは入る）。
- 自分の replica の edge も、他人の replica と同じく、購読の開始時の上限つきの窓（docs author を指定した一覧）と docs の event で反映する。
  窓を超える edge と旧名義でしか読めない edge を、後からすべて読むことはしない（自分の replica の件数に比例する読み出しになり、端末間のアカウント同期が
  実装されるまで利用者はそれを必要としない。#1239）。旧名義の edge を自分の docs author へ書き直すこともしない。自分の docs author へ移るのは、
  利用者がその edge を書いた（follow・unfollow・block・unblock した）とき。
- 旧版の client が、新版と同じアカウントで旧名義に後から書いた edge は、組の record が先に使われるので読まれない（旧版との混在期間だけの best effort）。

## Consequences

- 同じアカウントの書き込みは、端末が違っても同じ docs author になる。docs の層から端末を区別できなくなる。
- 同じ（docs author、key）への書き込みは、iroh-docs の規則で新しい timestamp が勝つ。投稿の envelope は不変で、取り下げは同じ対象への上書きなので、端末をまたいでも矛盾しない。
- 索引や replica へ偽の entry を大量に積まれる問題と、replica の肥大は、本 ADR では解決しない（replica の時間分割は Issue #1243、spam の検知と trust の判定は CN の責務）。
- reaction・live session・game room の state doc は、本 ADR の対象外とする。同じ方法を使えるかは、それぞれの Issue で決める。
- 導出の context を変えると、全アカウントの docs author の id が変わり、それまでの投稿の tag と合わなくなる。context と導出の手順は固定値の test で保護する。

## Data classification

- 新しく docs と gossip へ置く情報: docs author の id（公開の識別子）。Canonical Source は投稿の envelope の tag（§6 の後は、profile・follow・block・custom reaction の asset の envelope の tag も）。公開範囲は `pubkey` と同じ。
- 端末内に置く情報（§6）: 著者ごとの docs author の id（`author_docs_authors`）。著者の envelope の tag から得た公開の識別子の写しで、Canonical Source は envelope の tag。
- 端末内だけに置く情報: 導出した docs author の秘密鍵（iroh-docs の保存場所）。暗号化された端末バックアップには保存場所ごと含まれるが、復元先はアカウントの秘密鍵から導出し直す。
- 外部送信の一覧（`docs/legal/app-data-flow-inventory.md`）の「公開鍵・endpoint ID・接続情報」の行に、docs author の id を加える。

## References

- Issue #1258（本 ADR の実装）、#1250、#1248、#1239、#1243、#1211・#1218・#1219
- ADR 0011 §14.1・§16、ADR 0032、ADR 0048、ADR 0052 §1〜§3
