import type { DesktopApi, LiveVideoSource } from '@/lib/api';

import { captureErrorKind, captureVideo, encodeVideo } from './liveMedia';

type LiveBroadcastApi = Pick<DesktopApi, 'setLiveBroadcast' | 'pushLiveVideo' | 'stopLiveBroadcast'>;

export type LiveBroadcastTarget = { topic: string; sessionId: string; title: string };

/// 配信の状態。`notice` は `live:` の文言の key(取り込みの失敗の理由・mic を開けない)。
export type LiveBroadcastState = LiveBroadcastTarget & {
  source: LiveVideoSource;
  microphone: boolean;
  phase: 'starting' | 'live' | 'failed';
  notice: string | null;
};

/**
 * この端末の配信(#1034、#1839 AC-3)。取り込み・圧縮・送信を、画面の表示の寿命から離して 1 つだけ持つ。開始は利用者の
 * 操作だけで、止めるのは停止・Live の終了・shell を離れるとき。同時に配信するのは 1 つの Live だけ。
 */
export class LiveBroadcastController {
  /// 自分の映像の表示で、`MediaStreamTrackProcessor` の無い WebKitGTK では frame の取り出しにも使う(bar に置く)。
  readonly preview = document.createElement('video');
  private state: LiveBroadcastState | null = null;
  private listeners = new Set<() => void>();
  private encoding: { stop(): void } | null = null;
  /// 止める・始め直すたびに進め、古い開始の続きを捨てる。
  private generation = 0;

  constructor(private readonly api: LiveBroadcastApi) {
    this.preview.muted = true;
    this.preview.playsInline = true;
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  getState = () => this.state;

  /// 配信を始める。別の Live を配信している間は始めない。
  start(target: LiveBroadcastTarget, source: LiveVideoSource) {
    if (this.state && this.state.sessionId !== target.sessionId) return;
    this.update({ ...target, source, microphone: true, phase: 'starting', notice: null });
    void this.run(source, true);
  }

  /// 配信中の source の切替(mic は閉じない)。
  setSource(source: LiveVideoSource) {
    if (this.state?.phase === 'live') void this.run(source, this.state.microphone);
  }

  async toggleMicrophone() {
    const current = this.state;
    if (current?.phase !== 'live') return;
    const generation = this.generation;
    const next = !current.microphone;
    this.update({ ...current, microphone: next });
    const status = await this.api.setLiveBroadcast({ ...this.request(current), source: current.source, microphone: next });
    if (generation !== this.generation || !this.state) return;
    this.update({ ...this.state, microphone: status.microphone, notice: next && !status.microphone ? 'media.microphoneUnavailable' : null });
  }

  /// 配信を止め、bar を閉じる。
  stop() {
    this.generation += 1;
    this.stopEncoding();
    if (this.state) void this.api.stopLiveBroadcast(this.request(this.state)).catch(() => undefined);
    this.update(null);
  }

  /// その Live の配信なら止める(Live の終了)。
  stopIfSession(sessionId: string) {
    if (this.state?.sessionId === sessionId) this.stop();
  }

  // mic は取り込みが始まってから開く(取り込みが拒否されたら mic も開かない)。配信中の source の切替では mic を閉じない。
  private async run(source: LiveVideoSource, microphone: boolean) {
    const current = this.state!;
    const generation = ++this.generation;
    const keepMicrophone = current.phase === 'live' && microphone;
    const request = this.request(current);
    this.stopEncoding();
    this.update({ ...current, source, phase: 'starting', notice: null });
    try {
      const status = await this.api.setLiveBroadcast({ ...request, source, microphone: keepMicrophone });
      if (generation !== this.generation) return;
      const stream = await captureVideo(source, status.limits);
      if (generation !== this.generation) {
        stream.getTracks().forEach((track) => track.stop());
        return;
      }
      this.encoding = encodeVideo({
        stream,
        limits: status.limits,
        preview: this.preview,
        push: (keyframe, ageUs, data) => this.api.pushLiveVideo(request.session_id, keyframe, ageUs, data),
        onEnded: () => this.stop(),
      });
      let opened = keepMicrophone;
      if (microphone && !keepMicrophone) {
        opened = (await this.api.setLiveBroadcast({ ...request, source, microphone: true })).microphone;
        if (generation !== this.generation) return;
      }
      this.update({ ...this.state!, phase: 'live', microphone: opened, notice: microphone && !opened ? 'media.microphoneUnavailable' : null });
    } catch (error) {
      if (generation !== this.generation) return;
      this.stopEncoding();
      await this.api.stopLiveBroadcast(request).catch(() => undefined);
      if (generation !== this.generation) return;
      this.update({ ...this.state!, phase: 'failed', notice: `media.errors.${captureErrorKind(error, source)}` });
    }
  }

  private request(target: LiveBroadcastTarget) {
    return { topic: target.topic, session_id: target.sessionId };
  }

  private stopEncoding() {
    this.encoding?.stop();
    this.encoding = null;
  }

  private update(next: LiveBroadcastState | null) {
    this.state = next;
    this.listeners.forEach((listener) => listener());
  }
}

const controllers = new WeakMap<object, LiveBroadcastController>();

/// `api` ごとに 1 つの配信の controller。
export function liveBroadcastFor(api: LiveBroadcastApi) {
  let controller = controllers.get(api);
  if (!controller) controllers.set(api, (controller = new LiveBroadcastController(api)));
  return controller;
}
