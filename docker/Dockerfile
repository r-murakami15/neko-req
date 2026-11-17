# NekoReq 開発環境（Rust + Node + Mac向けクロスビルド）
# ホストにRustを入れずに、このコンテナ内で fmt / clippy / test と
# macOS(aarch64 / x86_64)向けのクロスビルドを行う。
FROM rust:1.90-bookworm

ARG NODE_MAJOR=20
ARG ZIG_VERSION=0.13.0
ARG CARGO_ZIGBUILD_VERSION=0.20.1

ENV DEBIAN_FRONTEND=noninteractive

# Linux向けのTauri依存（cargo test / clippy をコンテナ内で動かすため）
RUN apt-get update && apt-get install -y --no-install-recommends \
      libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
      libssl-dev libxdo-dev libdbus-1-dev pkg-config build-essential \
      curl ca-certificates xz-utils file jq \
    && rm -rf /var/lib/apt/lists/*

# Node.js（フロントエンドのビルドと型チェック）
RUN curl -fsSL https://deb.nodesource.com/setup_${NODE_MAJOR}.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

# Zig（macOS向けリンカとして使う）
RUN arch="$(uname -m)" \
    && curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-linux-${arch}-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz \
    && mkdir -p /opt/zig && tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1 \
    && ln -s /opt/zig/zig /usr/local/bin/zig && rm /tmp/zig.tar.xz

RUN rustup component add rustfmt clippy \
    && rustup target add aarch64-apple-darwin x86_64-apple-darwin \
    && cargo install --locked cargo-zigbuild --version ${CARGO_ZIGBUILD_VERSION}

# macOS SDK は名前付きボリュームに置く（scripts/build-mac.sh 参照）
ENV SDKROOT=/opt/macos-sdk/MacOSX.sdk \
    CARGO_TARGET_DIR=/cargo-target \
    MACOSX_DEPLOYMENT_TARGET=13.0

WORKDIR /work
