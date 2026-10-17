// 公開スレッド（公開 channel。#1840、ADR 0068）の ID は `pc-<作成時刻のミリ秒>-<作成者の公開鍵の先頭 16 桁>`。
// private channel の ID（`channel-…`）とは形が重ならないので、channel の種類は ID の形で見分ける
// （native の `ChannelId::is_public_channel` と同じ判定）。
const PUBLIC_CHANNEL_ID = /^pc-[1-9][0-9]*-[0-9a-f]{16}$/;

export function isPublicChannelId(channelId: string | null | undefined): channelId is string {
  return typeof channelId === 'string' && PUBLIC_CHANNEL_ID.test(channelId);
}
