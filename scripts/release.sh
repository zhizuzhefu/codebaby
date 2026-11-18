#!/usr/bin/env bash
# 发版：版本号以 Cargo.toml 的 workspace 版本为准，发布渠道是 GitHub Release。
#
#   scripts/release.sh            完整发布
#   scripts/release.sh --dry-run  只做发布前检查，不构建、不发布
#
# 流程：检查（main、工作区干净、与远端一致、CHANGELOG 有本版本）→ 测试 + clippy
#       → 本机构建 universal DMG（Developer ID 签名 + 公证）→ 建 GitHub Release 草稿并传 DMG
#       → 推送 vX.Y.Z 标签，CI 构建各平台命令行并传到同一个草稿
#       → 附件到齐后发布（用户的「检查更新」此时才看到新版本）
#
# Apple 凭据只在本机钥匙串里（见 scripts/build-mac-release.sh），不进 CI、不进仓库。
# 需要：gh（已登录且有仓库写权限）、cargo、create-dmg、公证 profile。

set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

VERSION=$(awk -F'"' '/^\[workspace.package\]/{s=1} s && /^version/{print $2; exit}' Cargo.toml)
[[ -n "$VERSION" ]] || { echo "✘ 无法从 Cargo.toml 读取版本号" >&2; exit 1; }
TAG="v$VERSION"
DMG="build/mac/requestbaby-$VERSION.dmg"
# CI 要构建的命令行目标（3 个目标 × 附件各 2 个：tar.gz + .sha256）；与 .github/workflows/ci.yml 的 release 矩阵一致
CLI_ASSETS=6
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
[[ -n "$NOTES" ]] || fail "CHANGELOG.md 里没有「## $VERSION」一节（或为空）"
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

echo "==> 创建 Release 草稿并上传 DMG"
NOTES_FILE=$(mktemp)
printf '%s\n' "$NOTES" > "$NOTES_FILE"
gh release create "$TAG" "$DMG" "$DMG.sha256" --draft --title "$TAG" --notes-file "$NOTES_FILE"
rm -f "$NOTES_FILE"

echo "==> 推送标签 $TAG（CI 开始构建命令行）"
git tag -a "$TAG" -m "requestbaby $VERSION"
git push origin "$TAG"

echo "==> 等待 CI 把命令行安装包传到草稿（最多 30 分钟）"
for _ in $(seq 1 180); do
  N=$(gh release view "$TAG" --json assets --jq '.assets | length' 2>/dev/null || echo 0)
  [[ "$N" -ge $((CLI_ASSETS + 2)) ]] && break
  sleep 10
done
N=$(gh release view "$TAG" --json assets --jq '.assets | length')
if [[ "$N" -lt $((CLI_ASSETS + 2)) ]]; then
  echo "✘ CI 还没传完（现有 $N 个附件，应有 $((CLI_ASSETS + 2)) 个）。Release 仍是草稿，未发布。" >&2
  echo "  查看 CI：gh run list --workflow ci.yml；传齐后发布：gh release edit $TAG --draft=false" >&2
  exit 1
fi

gh release edit "$TAG" --draft=false --latest
echo "✓ 已发布 $TAG：$(gh release view "$TAG" --json url --jq .url)"
