fn main() {
    // Windows の main thread の stack reserve は既定 1 MiB で、Linux / macOS の 8 MiB より小さい。
    // WebView2 の IPC callback は main thread で動き、Tauri は command の future を値渡しで
    // `tokio::spawn` まで運ぶので、future の大きい command で stack overflow して終了する（#1526）。
    // bin だけ 8 MiB に揃える。future の大きさの上限は
    // `crates/desktop-runtime/tests/command_future_sizes.rs` で固定する。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
    tauri_build::build();
}
