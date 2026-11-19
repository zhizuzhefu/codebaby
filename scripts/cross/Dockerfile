# 交叉编译命令行用的构建镜像（由 scripts/build-cross.sh 构建，不随发布分发）。
# 工具链版本由仓库的 rust-toolchain.toml 决定，rustup 会在首次构建时安装。
FROM rust:bookworm
RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      musl-tools cmake clang pkg-config \
 && rm -rf /var/lib/apt/lists/*
