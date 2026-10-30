//! feature `vp8` のとき、`KUKURI_LIBVPX_DIR`（`sdk/unity/build-libvpx.ps1` の出力。`include` と `lib`）の libvpx と、
//! それを呼ぶ C の関数（`src/vp8/shim.c`）を link する（ADR 0070 §8.3、#1982）。

fn main() {
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
