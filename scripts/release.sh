#!/usr/bin/env bash
# 发版：版本号以 Cargo.toml 的 workspace 版本为准，发布渠道是 GitHub Release。
#
#   scripts/release.sh            完整发布
#   scripts/release.sh --dry-run  只做发布前检查，不构建、不发布
#
# 流程：检查（main、工作区干净、与远端一致、CHANGELOG 有本版本）→ 测试 + clippy
#       → 本机构建 universal DMG（Developer ID 签名 + 公证）与 macOS 命令行包（Apple 芯片 / Intel）
#       → Docker 交叉编译 Linux（x86_64 / arm64）命令行包（scripts/build-cross.sh）
#       → 建 GitHub Release 草稿，传齐全部附件 → 推送 vX.Y.Z 标签
#       → 附件核对无误后发布（用户的「检查更新」此时才看到新版本）
#
# 全程在本机完成，不使用 CI。Apple 凭据只在本机钥匙串里（见 scripts/build-mac-release.sh），不进仓库。
# 需要：gh（已登录且有仓库写权限）、cargo、create-dmg、公证 profile、Docker（交叉编译 Linux）。

set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

VERSION=$(awk -F'"' '/^\[workspace.package\]/{s=1} s && /^version/{print $2; exit}' Cargo.toml)
[[ -n "$VERSION" ]] || { echo "✘ 无法从 Cargo.toml 读取版本号" >&2; exit 1; }
TAG="v$VERSION"
DMG="build/mac/requestbaby-$VERSION.dmg"
# 命令行包的目标平台（每个 2 个附件：tar.gz + .sha256），与 core::update::target() 的取值一致；加上 DMG 的 2 个附件
CLI_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
CROSS_PACKAGES=2   # Linux x86_64 / Linux arm64，见 scripts/build-cross.sh
EXPECTED_ASSETS=$(( (${#CLI_TARGETS[@]} + 1 + CROSS_PACKAGES) * 2 ))
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
# 本版本的更新说明：CHANGELOG.md 里 "## <版本>" 到下一个 "## " 之间
NOTES=$(awk -v v="$VERSION" '/^## /{f=($2==v)} f && !/^## /' CHANGELOG.md | sed '/./,$!d')

fail() { echo "✘ $*" >&2; exit 1; }

echo "==> 发布前检查：$TAG"
command -v gh >/dev/null || fail "缺少 gh（brew install gh）"
gh auth status >/dev/null 2>&1 || fail "gh 未登录：gh auth login"
[[ "$(git branch --show-current)" == "main" ]] || fail "只能在 main 分支发布"
[[ -z "$(git status --porcelain)" ]] || fail "工作区有未提交的改动：先提交"
git fetch -q origin main --tags
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || fail "本地 main 与 origin/main 不一致：先 git push / git pull"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && fail "标签 $TAG 已存在：先在 Cargo.toml 提升版本号"
[[ -n "$NOTES" ]] || fail "CHANGELOG.md 里没有「## ${VERSION}」一节（或为空）"
gh release view "$TAG" >/dev/null 2>&1 && fail "GitHub 上已有 $TAG 的 Release"

echo "    版本 $VERSION · 提交 $(git rev-parse --short HEAD)"
echo "    更新说明："
printf '%s\n' "$NOTES" | sed 's/^/      /'
if [[ "$DRY_RUN" == "1" ]]; then
  echo "✓ 检查通过（--dry-run：未构建、未发布）"
  exit 0
fi

echo "==> 测试与 clippy"
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings

echo "==> 构建 DMG（签名 + 公证，几分钟）"
scripts/build-mac-release.sh
[[ -f "$DMG" ]] || fail "没有生成 $DMG"
( cd build/mac && shasum -a 256 "$(basename "$DMG")" > "$(basename "$DMG").sha256" )

echo "==> 打包命令行（macOS）"
ASSETS=("$DMG" "$DMG.sha256")
for t in "${CLI_TARGETS[@]}"; do
  BIN="$TARGET_DIR/$t/release/requestbaby"
  [[ -x "$BIN" ]] || fail "没有 $BIN"
  PKG="build/mac/requestbaby-$t.tar.gz"
  tar -czf "$PKG" -C "$(dirname "$BIN")" requestbaby
  ( cd build/mac && shasum -a 256 "$(basename "$PKG")" > "$(basename "$PKG").sha256" )
  ASSETS+=("$PKG" "$PKG.sha256")
done

echo "==> 交叉编译 Linux 命令行（Docker）"
rm -rf build/cross
scripts/build-cross.sh HEAD build/cross
for f in build/cross/requestbaby-*; do ASSETS+=("$f"); done

echo "==> 创建 Release 草稿并上传全部附件"
NOTES_FILE=$(mktemp)
printf '%s\n' "$NOTES" > "$NOTES_FILE"
gh release create "$TAG" "${ASSETS[@]}" --draft --title "$TAG" --notes-file "$NOTES_FILE"
rm -f "$NOTES_FILE"

echo "==> 推送标签 ${TAG}"
git tag -a "$TAG" -m "requestbaby $VERSION"
git push origin "$TAG"

N=$(gh release view "$TAG" --json assets --jq '.assets | length')
if [[ "$N" -ne "$EXPECTED_ASSETS" ]]; then
  echo "✘ 附件数量不对（现有 $N 个，应有 $EXPECTED_ASSETS 个）。Release 仍是草稿，未发布。" >&2
  echo "  核对：gh release view $TAG；无误后发布：gh release edit $TAG --draft=false --latest" >&2
  exit 1
fi

gh release edit "$TAG" --draft=false --latest
echo "✓ 已发布 ${TAG}：$(gh release view "$TAG" --json url --jq .url)"
