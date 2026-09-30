# 共用 crate の wasm32 の build と browser 試験を、clang の無い host（Windows）で行う image。
# 使い方は docs/runbooks/dev.md の「Web（wasm32）の build と browser 試験」。
# wasm-bindgen-cli の版は Cargo.lock の wasm-bindgen と揃える。
FROM rust:1.98.1-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends clang lld llvm chromium chromium-driver && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown
RUN cargo install --locked wasm-bindgen-cli --version 0.2.129
ENV CC_wasm32_unknown_unknown=clang AR_wasm32_unknown_unknown=llvm-ar CHROMEDRIVER=/usr/bin/chromedriver
