#!/usr/bin/env bash
# 发版：版本号以 Cargo.toml 的 workspace 版本为准，发布渠道是 GitHub Release。
#
#   scripts/release.sh                          完整发布：版本号 z 自动加一（0.3.0 → 0.3.1）
#   scripts/release.sh --bump minor|major       升 x.y（0.3.z → 0.4.0 / 1.0.0）：需要人工确认，见下
#   scripts/release.sh --confirm-version X.Y.Z  非交互环境下给出确认（必须等于将发布的版本号）
#   scripts/release.sh --dry-run                只做发布前检查，不构建、不发布、不改版本号（会列出将发布的版本与将清理的旧版本）
#   scripts/release.sh --keep-old               发布后保留旧版本（缺省清理）
#
# 版本号策略：从不复用版本号。已发布过的版本之上，z（补丁号）由脚本自动加一，提交并推送后再发布；
# x.y 变化（手动改了 Cargo.toml 或用 --bump）必须人工确认：交互终端里输入新版本号，或 --confirm-version。
#
# 流程：检查（main、工作区干净、与远端一致、标签未被占用）→ 测试 + clippy
#       → 本机构建 universal DMG（Developer ID 签名 + 公证）与 macOS 命令行包（Apple 芯片 / Intel）
#       → 建 GitHub Release 草稿，传齐全部附件 → 推送 vX.Y.Z 标签
#       → 附件核对无误后发布（用户的「检查更新」此时才看到新版本）
#       → 清理旧版本：比这次版本旧的 Release 连同它们的标签一并删除，Release 页只留最新版
#
# 全程在本机完成，不使用 CI。Apple 凭据只在本机钥匙串里（见 scripts/build-mac-release.sh），不进仓库。
# 需要：gh（已登录且有仓库写权限）、cargo、create-dmg、公证 profile。只发布 macOS 版（桌面端 + 命令行）。

set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

DRY_RUN=0
KEEP_OLD=0
BUMP=""
CONFIRM=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --keep-old) KEEP_OLD=1 ;;
    --bump) BUMP="${2:-}"; shift; [[ "$BUMP" == "minor" || "$BUMP" == "major" ]] || { echo "✘ --bump 只能是 minor 或 major" >&2; exit 1; } ;;
    --confirm-version) CONFIRM="${2:-}"; shift ;;
    *) echo "✘ 不认识的参数：$1（可用 --dry-run / --keep-old / --bump minor|major / --confirm-version X.Y.Z）" >&2; exit 1 ;;
  esac
  shift
done

CURRENT=$(awk -F'"' '/^\[workspace.package\]/{s=1} s && /^version/{print $2; exit}' Cargo.toml)
[[ "$CURRENT" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "✘ 无法从 Cargo.toml 读取版本号（得到「$CURRENT」）" >&2; exit 1; }
VERSION="$CURRENT"   # 下面按策略可能被改成新版本
TAG="v$VERSION"
DMG="build/mac/requestbaby-$VERSION.dmg"
# 命令行包的目标平台（每个 2 个附件：tar.gz + .sha256），与 core::update::target() 的取值一致；加上 DMG 的 2 个附件
CLI_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
EXPECTED_ASSETS=$(( (${#CLI_TARGETS[@]} + 1) * 2 ))
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

fail() { echo "✘ $*" >&2; exit 1; }

# 版本号小工具：xy 取 x.y；bump_patch 的 z 加一
xy() { echo "$1" | cut -d. -f1,2; }
bump_patch() { IFS=. read -r a b c <<<"$1"; echo "$a.$b.$((c + 1))"; }
bump_minor() { IFS=. read -r a b _ <<<"$1"; echo "$a.$((b + 1)).0"; }
bump_major() { IFS=. read -r a _ _ <<<"$1"; echo "$((a + 1)).0.0"; }
vmax() { printf '%s\n' "$@" | sort -V | tail -1; }

# 已发布过的最高版本（GitHub 上的 Release + 本地 / 远端标签，只认 vX.Y.Z）
latest_released() {
  { gh release list --limit 200 --json tagName --jq '.[].tagName'; git tag -l 'v*'; } 2>/dev/null \
    | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sed 's/^v//' | sort -V | tail -1
}

# 比这次版本旧的 Release 的标签（只认 vX.Y.Z；草稿也算；不含这次的标签）
old_tags() {
  gh release list --limit 200 --json tagName --jq '.[].tagName' | while read -r t; do
    [[ "$t" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ && "$t" != "$TAG" ]] || continue
    [[ "$(printf '%s\n%s\n' "${t#v}" "$VERSION" | sort -V | head -1)" == "${t#v}" ]] && echo "$t"
  done
}

echo "==> 发布前检查"
command -v gh >/dev/null || fail "缺少 gh（brew install gh）"
gh auth status >/dev/null 2>&1 || fail "gh 未登录：gh auth login"
[[ "$(git branch --show-current)" == "main" ]] || fail "只能在 main 分支发布"
[[ -z "$(git status --porcelain)" ]] || fail "工作区有未提交的改动：先提交"
git fetch -q origin main --tags
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || fail "本地 main 与 origin/main 不一致：先 git push / git pull"

# ---- 版本号策略：不复用版本号；z 自动加一；x.y 变化要人工确认
LATEST="$(latest_released)"
if [[ -n "$BUMP" ]]; then
  BASE="$(vmax "$CURRENT" "${LATEST:-$CURRENT}")"
  if [[ "$BUMP" == "minor" ]]; then VERSION="$(bump_minor "$BASE")"; else VERSION="$(bump_major "$BASE")"; fi
elif [[ -n "$LATEST" ]] && [[ "$(vmax "$CURRENT" "$LATEST")" == "$LATEST" ]]; then
  # 当前版本已经发布过（或比已发布的还旧）：在已发布的最高版本上 z 加一
  VERSION="$(bump_patch "$LATEST")"
fi
TAG="v$VERSION"
DMG="build/mac/requestbaby-$VERSION.dmg"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null && fail "标签 $TAG 已存在（不复用版本号）"
gh release view "$TAG" >/dev/null 2>&1 && fail "GitHub 上已有 $TAG 的 Release（不复用版本号）"

NEEDS_CONFIRM=0
if [[ -n "$LATEST" && "$(xy "$VERSION")" != "$(xy "$LATEST")" ]]; then NEEDS_CONFIRM=1; fi
echo "    已发布的最高版本：${LATEST:-（无）} · Cargo.toml：$CURRENT · 本次发布：$VERSION"
if [[ "$NEEDS_CONFIRM" == "1" ]]; then
  echo "    ! x.y 变化（${LATEST} → ${VERSION}）：需要人工确认"
  if [[ "$DRY_RUN" != "1" && "$CONFIRM" != "$VERSION" ]]; then
    if [[ -t 0 ]]; then
      read -r -p "    输入 ${VERSION} 确认发布，其他任意输入取消：" answer
      [[ "$answer" == "$VERSION" ]] || fail "未确认，已取消（没有改动任何东西）"
    else
      fail "x.y 升级需要人工确认：在终端里运行，或加 --confirm-version $VERSION"
    fi
  fi
elif [[ -n "$CONFIRM" && "$CONFIRM" != "$VERSION" ]]; then
  fail "--confirm-version $CONFIRM 与将发布的版本 $VERSION 不一致"
fi

echo "    版本 $VERSION · 提交 $(git rev-parse --short HEAD)"
if [[ "$DRY_RUN" == "1" ]]; then
  [[ "$VERSION" != "$CURRENT" ]] && echo "    发布时会先把 Cargo.toml 的版本号改成 $VERSION，提交并推送"
  OLD="$(old_tags | paste -sd' ' -)"
  if [[ "$KEEP_OLD" == "1" ]]; then echo "    发布后保留旧版本（--keep-old）"; else echo "    发布后将清理旧版本：${OLD:-（没有）}"; fi
  echo "✓ 检查通过（--dry-run：未构建、未发布）"
  exit 0
fi

# 版本号变了：先写进 Cargo.toml（和 Cargo.lock），提交并推送，后面的构建与发布都基于这个提交
if [[ "$VERSION" != "$CURRENT" ]]; then
  echo "==> 版本号 $CURRENT → $VERSION"
  awk -v v="$VERSION" '/^\[workspace.package\]/{s=1} s && /^version/{print "version = \"" v "\""; s=0; next} {print}' Cargo.toml > Cargo.toml.new && mv Cargo.toml.new Cargo.toml
  cargo update --workspace --offline >/dev/null 2>&1 || cargo metadata --format-version 1 >/dev/null
  git add Cargo.toml Cargo.lock
  git commit -q -m "chore(release): v$VERSION"
  git push -q origin main
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

echo "==> 创建 Release 草稿并上传全部附件"
# 更新说明由 GitHub 按提交自动生成（发布前可在草稿里改）
gh release create "$TAG" "${ASSETS[@]}" --draft --title "$TAG" --generate-notes

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

# 新版本已经发布成功，才清理旧版本：Release 连同它的标签一起删，历史上只留最新版
if [[ "$KEEP_OLD" == "1" ]]; then
  echo "    保留旧版本（--keep-old）"
else
  echo "==> 清理旧版本"
  for t in $(old_tags); do
    gh release delete "$t" --cleanup-tag --yes && echo "    已删除 $t（Release 与标签）"
  done
fi
