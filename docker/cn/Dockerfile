ARG RUST_VERSION=1.92.0
FROM rust:${RUST_VERSION}-bookworm AS builder

WORKDIR /workspace

# #1122: 4 つの image が同じ builder stage を共有し、共通の依存 crate を 1 回だけ
# compile する。package ごとに cargo を分けて呼ぶのは、まとめて指定すると feature が
# 統合されて binary が変わりうるため。
ARG TARGET_PACKAGES
ARG CARGO_BUILD_JOBS=2

COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY xtask ./xtask
COPY crates ./crates

RUN test -n "$TARGET_PACKAGES"
RUN set -eu; for package in $TARGET_PACKAGES; do cargo build --release -p "$package"; done

FROM debian:bookworm-slim AS runtime

ARG TARGET_BIN

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && if [ "$TARGET_BIN" = "cn-indexer" ] || [ "$TARGET_BIN" = "cn-cli" ]; then \
         apt-get install -y --no-install-recommends ffmpeg \
         && mkdir -p /usr/local/share/kukuri \
         && ffmpeg -version > /usr/local/share/kukuri/decoder-build-id \
         && ffprobe -version >> /usr/local/share/kukuri/decoder-build-id; \
       fi \
    && rm -rf /var/lib/apt/lists/*

RUN test -n "$TARGET_BIN"
COPY --from=builder /workspace/target/release/${TARGET_BIN} /usr/local/bin/app

ENTRYPOINT ["/usr/local/bin/app"]
