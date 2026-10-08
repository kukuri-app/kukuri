ARG RUST_VERSION=1.98.1
FROM rust:${RUST_VERSION}-bookworm AS chef

# #1413: 依存 crate の build を application の build と別の layer にする（cargo-chef）。manifest と
# lockfile が変わらなければ、source を変えても依存の layer を build cache から再利用する。
RUN cargo install cargo-chef --locked --version 0.1.78
WORKDIR /workspace

FROM chef AS planner

# source の変更ごとに作り直すので、toolchain の component を入れる rust-toolchain.toml は置かない。
COPY Cargo.toml Cargo.lock ./
COPY xtask ./xtask
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

# #1122: 4 つの image が同じ builder stage を共有し、共通の依存 crate を 1 回だけ
# compile する。package ごとに cargo を分けて呼ぶのは、まとめて指定すると feature が
# 統合されて binary が変わりうるため。依存の cook も同じ単位で行う。
ARG TARGET_PACKAGES
ARG CARGO_BUILD_JOBS=2

RUN test -n "$TARGET_PACKAGES"
COPY rust-toolchain.toml ./
COPY --from=planner /workspace/recipe.json recipe.json
RUN set -eu; for package in $TARGET_PACKAGES; do cargo chef cook --release --recipe-path recipe.json -p "$package"; done

COPY Cargo.toml Cargo.lock ./
COPY xtask ./xtask
COPY crates ./crates
RUN set -eu; for package in $TARGET_PACKAGES; do cargo build --release -p "$package"; done

# #1657: kukuri が運用する署名つき一覧の publisher。fork の同じ固定 revision を使う。
RUN case " $TARGET_PACKAGES " in *" kukuri-cn-addr-index "*) \
    cargo install --git https://github.com/KingYoSun/iroh-content-discovery \
      --rev f6e2864a1d2228a19aa1cb88239ba87eb0068d2e --locked \
      --root /tmp/index-list --target-dir /workspace/target/index-list \
      --features cli --bin iroh-index-list iroh-mainline-endpoint-discovery \
    && cp /tmp/index-list/bin/iroh-index-list /workspace/target/release/ ;; esac

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

# #1483: relay の image に、relay とは別の process として動かす STUN の binary（cn-stun）を同梱する。
# #1632: 公開 blob の発見の補助 index（cn-addr-index）も同じ形で同梱する（空白区切りで複数）。
ARG EXTRA_BIN
RUN --mount=type=bind,from=builder,source=/workspace/target/release,target=/release \
    for bin in $EXTRA_BIN; do cp "/release/$bin" /usr/local/bin/; done

ENTRYPOINT ["/usr/local/bin/app"]
