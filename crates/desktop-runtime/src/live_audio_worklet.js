// Web の live の mic（src/media_devices.rs の Web の実装、#1879）。入力の最初の channel を 20 ms ずつ main thread へ送る。
// 入力が無い間（mic を止めた間）は何も送らない。出力は無音のまま。
class KukuriLiveCapture extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buffer = new Float32Array(Math.round(sampleRate / 50));
    this.filled = 0;
  }

  process(inputs) {
    const channel = inputs[0]?.[0];
    if (!channel) return true;
    for (let at = 0; at < channel.length; ) {
      const count = Math.min(channel.length - at, this.buffer.length - this.filled);
      this.buffer.set(channel.subarray(at, at + count), this.filled);
      this.filled += count;
      at += count;
      if (this.filled === this.buffer.length) {
        this.port.postMessage(this.buffer, [this.buffer.buffer]);
        this.buffer = new Float32Array(this.buffer.length);
        this.filled = 0;
      }
    }
    return true;
  }
}

registerProcessor('kukuri-live-capture', KukuriLiveCapture);
