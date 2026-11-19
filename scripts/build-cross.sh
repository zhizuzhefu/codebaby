#!/usr/bin/env bash
# 在 Docker 里交叉编译 Linux / Windows 命令行，产物是各平台安装包（带 .sha256）。
#
#   scripts/build-cross.sh [REF] [OUT_DIR]
#
#   REF      要构建的 git 引用，默认 HEAD（发布时传标签，如 v0.3.0，保证与标签源码一致）
#   OUT_DIR  产物目录，默认 build/cross
#
# 产物：
#   requestbaby-x86_64-unknown-linux-musl.tar.gz    Linux x86_64，静态链接
#   requestbaby-aarch64-unknown-linux-musl.tar.gz   Linux arm64，静态链接
#   requestbaby-x86_64-pc-windows-gnu.zip           Windows x86_64（requestbaby.exe）
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

SRC="$(mktemp -d /tmp/requestbaby-src.XXXXXX)"
trap 'rm -rf "$SRC"' EXIT
git archive "$REF" | tar -x -C "$SRC"
echo "==> 构建 $REF（$(git rev-parse --short "$REF")）"

# 目标 | 容器平台 | 打包格式
TARGETS=(
  "x86_64-unknown-linux-musl|linux/amd64|tar"
  "aarch64-unknown-linux-musl|linux/arm64|tar"
  "x86_64-pc-windows-gnu|linux/arm64|zip"
)

for spec in "${TARGETS[@]}"; do
  IFS='|' read -r target platform pack <<<"$spec"
  arch="${platform#linux/}"
  image="requestbaby-cross:$arch"
  docker image inspect "$image" >/dev/null 2>&1 \
    || { echo "==> 构建镜像 $image"; docker build --platform "$platform" -t "$image" scripts/cross; }

  echo "==> cargo build --release ($target)"
  docker run --rm --platform "$platform" \
    -v "$SRC:/src" -v "requestbaby-cargo-$arch:/usr/local/cargo/registry" -v "requestbaby-target-$target:/target" \
    -v "$OUT:/out" -e CARGO_TARGET_DIR=/target -e TARGET="$target" -e PACK="$pack" \
    -w /src "$image" bash -euo pipefail -c '
      rustup target add "$TARGET" >/dev/null
      case "$TARGET" in
        x86_64-pc-windows-gnu)
          export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
          export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar ;;
        *-linux-musl)
          export CC_$(echo "$TARGET" | tr - _)=musl-gcc ;;
      esac
      cargo build --release --locked -p requestbaby --target "$TARGET"
      bin=/target/$TARGET/release/requestbaby; [ "$PACK" = zip ] && bin=$bin.exe
      cd "$(dirname "$bin")"
      if [ "$PACK" = zip ]; then zip -q -j "/out/requestbaby-$TARGET.zip" "$bin"; else tar -czf "/out/requestbaby-$TARGET.tar.gz" "$(basename "$bin")"; fi
    '
done

( cd "$OUT" && for f in requestbaby-*-linux-musl.tar.gz requestbaby-*-windows-gnu.zip; do shasum -a 256 "$f" > "$f.sha256"; done )
echo "✓ 产物在 $OUT"
ls -la "$OUT"
