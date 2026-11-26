#!/usr/bin/env bash
# 把签名的 codebaby.app 安装到 /Applications（替换旧版本前先校验签名与身份）。
#
# 用法：
#   scripts/install-mac.sh                       # 安装 build/mac/codebaby.app
#   scripts/install-mac.sh path/to/codebaby-x.y.z.dmg   # 从 DMG 安装

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE="${1:-$REPO_ROOT/build/mac/codebaby.app}"
BUNDLE_ID="${CODEBABY_BUNDLE_ID:-com.zhizuzhefu.app.codebaby}"
INSTALL_PATH="/Applications/codebaby.app"
EXECUTABLE="Contents/MacOS/codebaby-desktop"

TMP="$(mktemp -d /tmp/codebaby-install.XXXXXX)"
MOUNTED=""
cleanup() {
  [[ -n "$MOUNTED" ]] && hdiutil detach "$MOUNTED" >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

case "$SOURCE" in
  *.dmg)
    MOUNTED="$TMP/mount"
    mkdir -p "$MOUNTED"
    hdiutil attach -readonly -nobrowse -mountpoint "$MOUNTED" "$SOURCE" >/dev/null
    APP="$MOUNTED/codebaby.app"
    ;;
  *.app) APP="$SOURCE" ;;
  *) echo "✘ 需要 .app 或 .dmg：$SOURCE" >&2; exit 2 ;;
esac
[[ -d "$APP" ]] || { echo "✘ 找不到 ${APP}（先运行 scripts/build-mac-release.sh）" >&2; exit 3; }

# 安装前校验：签名完整、身份正确、Developer ID 签发
codesign --verify --strict "$APP"
ACTUAL_ID=$(defaults read "$APP/Contents/Info" CFBundleIdentifier)
[[ "$ACTUAL_ID" == "$BUNDLE_ID" ]] || { echo "✘ Bundle ID 不符：$ACTUAL_ID" >&2; exit 4; }
# awk 不能提前 exit：管道被关闭后 codesign 收到 SIGPIPE（141），pipefail 下整个脚本中止
AUTHORITY=$(codesign -dvv "$APP" 2>&1 | awk -F= '/^Authority=/ && !n++ {print $2}')
[[ "$AUTHORITY" == Developer\ ID\ Application:* ]] || { echo "✘ 不是 Developer ID 签名：$AUTHORITY" >&2; exit 5; }
if spctl --assess --type execute "$APP" 2>/dev/null; then
  GATEKEEPER="已公证，Gatekeeper 通过"
else
  GATEKEEPER="未公证（仅限本机使用）"
fi

# 替换正在运行的旧版本前先退出它
if [[ -d "$INSTALL_PATH" ]]; then
  OLD_ID=$(defaults read "$INSTALL_PATH/Contents/Info" CFBundleIdentifier 2>/dev/null || true)
  [[ "$OLD_ID" == "$BUNDLE_ID" ]] || { echo "✘ $INSTALL_PATH 是其他应用（${OLD_ID}），不替换" >&2; exit 6; }
  pkill -f "^$INSTALL_PATH/$EXECUTABLE" 2>/dev/null || true
  for _ in {1..50}; do
    pgrep -f "^$INSTALL_PATH/$EXECUTABLE" >/dev/null || break
    sleep 0.1
  done
fi

# 先完整复制到同卷临时位置再原子替换，避免中途失败留下半个 app
STAGING="/Applications/.codebaby.app.installing"
rm -rf "$STAGING"
ditto "$APP" "$STAGING"
codesign --verify --strict "$STAGING"
rm -rf "$INSTALL_PATH"
mv "$STAGING" "$INSTALL_PATH"

VERSION=$(defaults read "$INSTALL_PATH/Contents/Info" CFBundleShortVersionString)
BUILD=$(defaults read "$INSTALL_PATH/Contents/Info" CFBundleVersion)
echo "✓ 已安装 codebaby $VERSION ($BUILD) → $INSTALL_PATH · $GATEKEEPER"
# 内置 CLI：/usr/local/bin/codebaby 链接到它（链接已存在则随 App 一起更新，无需重复操作）
CLI_IN_APP="$INSTALL_PATH/Contents/MacOS/codebaby"
if [[ -x "$CLI_IN_APP" ]]; then
  if [[ "$(readlink /usr/local/bin/codebaby 2>/dev/null || true)" == "$CLI_IN_APP" ]]; then
    echo "  命令行工具：/usr/local/bin/codebaby → App 内置（已随 App 更新）"
  else
    echo "  命令行工具：App 菜单 codebaby → 安装命令行工具…（或 ln -sf \"$CLI_IN_APP\" /usr/local/bin/codebaby）"
  fi
  RESOLVED=$(command -v codebaby 2>/dev/null || true)
  if [[ -n "$RESOLVED" && "$RESOLVED" != "/usr/local/bin/codebaby" ]]; then
    echo "  ⚠ 当前终端的 codebaby 是 ${RESOLVED}（$("$RESOLVED" --version 2>/dev/null || echo 未知版本)），会遮蔽 App 内置版本"
  fi
fi
echo "  命令行打开项目：codebaby open"
