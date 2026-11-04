//! feature `vp8` のとき、`KUKURI_LIBVPX_DIR`（`sdk/unity/build-libvpx.ps1` の出力。`include` と `lib`）の libvpx と、
//! それを呼ぶ C の関数（`src/vp8/shim.c`）を link する（ADR 0070 §8.3、#1982）。

fn main() {
    // 配布物の照合の印の中身。版は UPM package の版、source は build-native.ps1 が渡す commit（`src/lib.rs` の `BUILD_ID`）。
    let package = "../../sdk/unity/com.kukuri.gamesdk/package.json";
    println!("cargo:rerun-if-changed={package}");
    println!("cargo:rerun-if-env-changed=KUKURI_SOURCE_COMMIT");
    let package: serde_json::Value =
        serde_json::from_slice(&std::fs::read(package).expect("the UPM package.json"))
            .expect("package.json is JSON");
    println!(
        "cargo:rustc-env=KUKURI_SDK_BUILD_ID={}:{}",
        package["version"].as_str().expect("the package version"),
        std::env::var("KUKURI_SOURCE_COMMIT").unwrap_or_else(|_| "unknown".into())
    );
    println!("cargo:rerun-if-env-changed=KUKURI_LIBVPX_DIR");
    println!("cargo:rerun-if-changed=src/vp8/shim.c");
    if std::env::var_os("CARGO_FEATURE_VP8").is_none() {
        return;
    }
    let dir = std::path::PathBuf::from(
        std::env::var_os("KUKURI_LIBVPX_DIR")
            .expect("the vp8 feature needs KUKURI_LIBVPX_DIR (run sdk/unity/build-libvpx.ps1)"),
    );
    cc::Build::new()
        .file("src/vp8/shim.c")
        .include(dir.join("include"))
        .compile("kukuri_vp8_shim");
    println!(
        "cargo:rustc-link-search=native={}",
        dir.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=vpx");
}
