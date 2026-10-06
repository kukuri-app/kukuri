// 実ブラウザの E2E の、ページの script より先に動く script（#1220 AC-5b）。fixture（harness の web_e2e_fixture）が、配信の
// `index.html` の head の先頭に同じ origin の script として足す。driver の機能（BiDi の addInitScript）に頼らないので、Safari
// でも、driver が開いた別の tab でも動く。配信の artifact は変えない。
(() => {
  localStorage.setItem('kukuri:media-debug', '1');
  window.__kukuriMediaEvents = [];
  for (const level of ['info', 'warn']) {
    const log = console[level];
    console[level] = (...args) => {
      if (String(args[0]).startsWith('[kukuri.media]')) {
        window.__kukuriMediaEvents.push(args);
        if (window.__kukuriMediaEvents.length > 32) window.__kukuriMediaEvents.shift();
      }
      log.apply(console, args);
    };
  }
  window.__kukuriReceivedBytes = 0;
  // 共有リンク（clipboard へ書く値）を控える。headless では clipboard へ書けないことがある。
  window.__kukuriCopied = [];
  navigator.clipboard.writeText = async (text) => {
    window.__kukuriCopied.push(text);
  };

  // CSP の違反を控える。配信の `_headers` の CSP の下で動くことを確かめる（#1220 AC-6）。
  window.__kukuriCspViolations = [];
  document.addEventListener('securitypolicyviolation', (event) =>
    window.__kukuriCspViolations.push(`${event.effectiveDirective} ${event.blockedURI}`)
  );

  // 画面が作った blob の URL から、その Blob を引けるようにする。配信の CSP の `connect-src` は `blob:` を許さないので、表示
  // した画像の中身は `fetch` でなく Blob から読む。
  window.__kukuriObjectUrls = new Map();
  const create = URL.createObjectURL;
  const revoke = URL.revokeObjectURL;
  URL.createObjectURL = (object) => {
    const url = create.call(URL, object);
    window.__kukuriObjectUrls.set(url, object);
    return url;
  };
  URL.revokeObjectURL = (url) => {
    window.__kukuriObjectUrls.delete(url);
    revoke.call(URL, url);
  };

  // WebRTC の session（RTCPeerConnection）を、runtime が DataChannel を作る時点で作った順に控え、WebRTC の経路だけを失わせる
  // 口を持つ（#1220 AC-4）。constructor は差し替えない（下の ICE の候補の除去が prototype の getter を差し替える）。
  // `window.__kukuriCut = { after }` を置くと、1 本の DataChannel で `after` bytes を受け取った時点で、開いている DataChannel を
  // すべて閉じ、閉じた時刻を `done` に残す（runtime は close の event で session を閉じる。RTCPeerConnection を外から閉じても
  // event は出ない）。1 本だけ閉じると、他の相手との session が残り、転送が relay を通らずに完了しうる（#1549）。
  window.__kukuriPeers = [];
  const channels = [];
  const createDataChannel = RTCPeerConnection.prototype.createDataChannel;
  RTCPeerConnection.prototype.createDataChannel = function (...args) {
    window.__kukuriPeers.push(this);
    return createDataChannel.apply(this, args);
  };
  const onmessage = Object.getOwnPropertyDescriptor(RTCDataChannel.prototype, 'onmessage');
  Object.defineProperty(RTCDataChannel.prototype, 'onmessage', {
    configurable: true,
    get() {
      return onmessage.get.call(this);
    },
    set(handler) {
      if (handler) channels.push(this);
      let received = 0;
      onmessage.set.call(this, handler && ((event) => {
        window.__kukuriReceivedBytes += event.data.byteLength;
        const cut = window.__kukuriCut;
        if (cut && !cut.done && (received += event.data.byteLength) >= cut.after) {
          cut.done = Date.now();
          for (const channel of channels) channel.close();
        }
        handler(event);
      }));
    },
  });

  // relay fallback の端（driver が置く cookie `kukuri-e2e-without-ice`）と、WebRTC の経路だけの喪失を保つ間（`__kukuriCut` で
  // DataChannel を閉じてから `released` まで）: 送る SDP と受け取る SDP から ICE の候補を除き、ICE を成立させない。
  const always = document.cookie.split('; ').includes('kukuri-e2e-without-ice=1');
  const blocked = () => always || (window.__kukuriCut?.done && !window.__kukuriCut.released);
  const strip = (description) =>
    description && blocked()
      ? {
          type: description.type,
          sdp: description.sdp.split(/\r?\n/).filter((line) => !line.startsWith('a=candidate:')).join('\r\n'),
        }
      : description;
  const prototype = RTCPeerConnection.prototype;
  const local = Object.getOwnPropertyDescriptor(prototype, 'localDescription');
  Object.defineProperty(prototype, 'localDescription', { get() { return strip(local.get.call(this)); } });
  const setRemote = prototype.setRemoteDescription;
  prototype.setRemoteDescription = function (description) { return setRemote.call(this, strip(description)); };
})();
