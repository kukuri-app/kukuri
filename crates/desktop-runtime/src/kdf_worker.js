// passphrase の鍵の導出（argon2id）だけを行う Dedicated Worker（src/kdf.rs、ADR 0056 §1 の例外）。
// main thread から同じ web-runtime の WebAssembly.Module を受け取り、glue を使わずに instantiate する。導出は JS の
// import を呼ばないので、import はすべて「呼ばれたら投げる関数」で埋める。結果は鍵（Uint8Array）か error の文字列。
self.onmessage = async ({ data: [module, passphrase, salt, mCostKib, tCost, pCost] }) => {
  try {
    const imports = {};
    for (const { module: from, name } of WebAssembly.Module.imports(module)) {
      imports[from] ??= {};
      imports[from][name] = () => {
        throw new Error(`${from}.${name} is not available in the key derivation worker`);
      };
    }
    const { exports } = await WebAssembly.instantiate(module, imports);
    const length = passphrase.length + salt.length;
    const buffer = exports.kukuri_kdf_alloc(length + 32);
    new Uint8Array(exports.memory.buffer, buffer, passphrase.length).set(passphrase);
    new Uint8Array(exports.memory.buffer, buffer + passphrase.length, salt.length).set(salt);
    if (exports.kukuri_kdf_derive(buffer, passphrase.length, salt.length, mCostKib, tCost, pCost) !== 0) {
      throw new Error("failed to derive account key export key");
    }
    // 導出で memory が伸びると前の view は使えないので、作り直す。
    self.postMessage(new Uint8Array(exports.memory.buffer, buffer + length, 32).slice());
  } catch (error) {
    self.postMessage(String(error));
  }
};
