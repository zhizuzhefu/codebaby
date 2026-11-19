#!/usr/bin/env bash
# 在 Docker 里交叉编译 Linux 命令行，产物是各平台安装包（带 .sha256）。
#
#   scripts/build-cross.sh [REF] [OUT_DIR]
#
#   REF      要构建的 git 引用，默认 HEAD（发布时传标签，如 v0.3.0，保证与标签源码一致）
#   OUT_DIR  产物目录，默认 build/cross
#
# 产物：
#   requestbaby-x86_64-unknown-linux-musl.tar.gz    Linux x86_64，静态链接
#   requestbaby-aarch64-unknown-linux-musl.tar.gz   Linux arm64，静态链接
#
# 需要 Docker（Docker Desktop 运行中）。Linux x86_64 在 Apple 芯片上走 amd64 模拟，比较慢。

set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

REF="${1:-HEAD}"
OUT="${2:-build/cross}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

command -v docker >/dev/null || { echo "✘ 缺少 docker" >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "✘ Docker 没有运行" >&2; exit 1; }

# 挂载进容器的目录放在 /tmp 下（Docker Desktop 默认共享，仓库所在路径不一定共享）
SRC="$(mktemp -d /tmp/requestbaby-src.XXXXXX)"
STAGE="$(mktemp -d /tmp/requestbaby-out.XXXXXX)"
trap 'rm -rf "$SRC" "$STAGE"' EXIT
git archive "$REF" | tar -x -C "$SRC"
echo "==> 构建 ${REF}（$(git rev-parse --short "$REF")）"

# 目标 | 容器平台
TARGETS=(
  "x86_64-unknown-linux-musl|linux/amd64"
  "aarch64-unknown-linux-musl|linux/arm64"
)

for spec in "${TARGETS[@]}"; do
  IFS='|' read -r target platform <<<"$spec"
  arch="${platform#linux/}"
  image="requestbaby-cross:$arch"
  docker image inspect "$image" >/dev/null 2>&1 \
    || { echo "==> 构建镜像 $image"; docker build --platform "$platform" -t "$image" scripts/cross; }

  echo "==> cargo build --release ($target)"
  docker run --rm --platform "$platform" \
    -v "$SRC:/src" -v "requestbaby-cargo-$arch:/usr/local/cargo/registry" -v "requestbaby-target-$target:/target" \
    -v "$STAGE:/out" -e CARGO_TARGET_DIR=/target -e TARGET="$target" \
    -w /src "$image" bash -euo pipefail -c '
      rustup target add "$TARGET" >/dev/null
      export CC_$(echo "$TARGET" | tr - _)=musl-gcc
      cargo build --release --locked -p requestbaby --target "$TARGET"
      cd "/target/$TARGET/release"
      tar -czf "/out/requestbaby-$TARGET.tar.gz" requestbaby
    '
done

( cd "$STAGE" && for f in requestbaby-*-linux-musl.tar.gz; do shasum -a 256 "$f" > "$f.sha256"; done )
cp "$STAGE"/requestbaby-* "$OUT"/
echo "✓ 产物在 $OUT"
ls -la "$OUT"
