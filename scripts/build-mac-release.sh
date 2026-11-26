#!/usr/bin/env bash
# 构建可安装到 /Applications 的 codebaby.app：
#   Rust release 构建（默认 universal：arm64 + x86_64）→ 组装 .app → Developer ID 签名
#   （hardened runtime + 时间戳）→ 公证 + staple → DMG（签名 + 公证 + staple）
#
# 用法：
#   scripts/build-mac-release.sh                  # 完整发布
#   SKIP_NOTARIZE=1 scripts/build-mac-release.sh  # 只签名不公证（本机自用，Gatekeeper 外发会拦）
#   SKIP_DMG=1 ...                                # 只出 .app
#   MAC_ARCH=arm64 ...                            # 只构建 Apple Silicon（更快）
#   DMG_HEADLESS=1 ...                            # 无 Finder 布局（CI / 无图形会话）
#
# 凭证（全部来自钥匙串，脚本不读写任何密钥文件）：
#   签名：钥匙串中的 "Developer ID Application" 身份；有多个时用 DEVELOPER_ID_IDENTITY 指定
#   公证：notarytool 钥匙串 profile，CODEBABY_NOTARY_PROFILE（缺省 zhizuzhefu-notary）
#         与其他 Mac 产品共用 iCloud 钥匙串里的 zhizuzhefu-notary
#
# 产物：
#   build/mac/codebaby.app
#   build/mac/codebaby-<version>.dmg

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

APP_NAME="codebaby"
EXECUTABLE="codebaby-desktop"
# 内置命令行工具：与 App 同版本同构建，App 菜单「安装命令行工具」把它链接到 /usr/local/bin
CLI="codebaby"
BUNDLE_ID="${CODEBABY_BUNDLE_ID:-com.zhizuzhefu.app.codebaby}"
MIN_MACOS="12.0"
ARCH="${MAC_ARCH:-universal}"
NOTARY_PROFILE="${CODEBABY_NOTARY_PROFILE:-zhizuzhefu-notary}"

VERSION=$(awk -F'"' '/^\[workspace.package\]/{s=1} s && /^version/{print $2; exit}' Cargo.toml)
BUILD_NUMBER=$(git rev-list --count HEAD)
OUT_DIR="$REPO_ROOT/build/mac"
APP_PATH="$OUT_DIR/$APP_NAME.app"
DMG_PATH="$OUT_DIR/$APP_NAME-$VERSION.dmg"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

[[ -n "$VERSION" ]] || { echo "✘ 无法从 Cargo.toml 读取 workspace 版本号" >&2; exit 1; }

# ------------------------------------------------------------------ 签名身份
if [[ -z "${DEVELOPER_ID_IDENTITY:-}" ]]; then
  ALL=$(security find-identity -v -p codesigning \
    | grep -oE '"Developer ID Application: [^"]+"' \
    | sed 's/^"//;s/"$//' | sort -u || true)
  COUNT=$(printf "%s\n" "$ALL" | grep -c . || true)
  if [[ "$COUNT" -eq 0 ]]; then
    echo "✘ 钥匙串中没有 'Developer ID Application' 身份。" >&2
    echo "  Xcode → Settings → Accounts → Manage Certificates → '+' → Developer ID Application" >&2
    exit 1
  elif [[ "$COUNT" -gt 1 ]]; then
    echo "✘ 有多个 Developer ID Application 身份，请用 DEVELOPER_ID_IDENTITY 指定：" >&2
    printf '%s\n' "$ALL" | sed 's/^/    /' >&2
    exit 1
  fi
  DEVELOPER_ID_IDENTITY="$ALL"
fi
TEAM_ID=$(echo "$DEVELOPER_ID_IDENTITY" | grep -oE '\([A-Z0-9]+\)$' | tr -d '()')

# 公证凭证提前检查：避免构建几分钟后才发现
if [[ "${SKIP_NOTARIZE:-0}" != "1" ]] && \
   ! xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1; then
  echo "✘ 找不到公证凭证 profile '$NOTARY_PROFILE'。" >&2
  echo "  一次性创建：xcrun notarytool store-credentials $NOTARY_PROFILE" >&2
  echo "  或复用已有 profile：CODEBABY_NOTARY_PROFILE=<名称> $0" >&2
  echo "  或先只签名不公证：SKIP_NOTARIZE=1 $0" >&2
  exit 1
fi

echo "==> codebaby $VERSION (build $BUILD_NUMBER) · $ARCH"
echo "    bundle:   $BUNDLE_ID"
echo "    identity: $DEVELOPER_ID_IDENTITY"

# ------------------------------------------------------------------ 编译
case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  arm64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "✘ MAC_ARCH 只能是 universal / arm64 / x86_64" >&2; exit 2 ;;
esac
INSTALLED_TARGETS=$(rustup target list --installed)
for t in "${TARGETS[@]}"; do
  grep -qx "$t" <<<"$INSTALLED_TARGETS" || rustup target add "$t"
done

export MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS"
BINARIES=()
CLI_BINARIES=()
for t in "${TARGETS[@]}"; do
  echo "==> cargo build --release ($t)"
  nice -n 10 cargo build --release --locked -p codebaby-desktop -p codebaby --target "$t"
  BINARIES+=("$TARGET_DIR/$t/release/$EXECUTABLE")
  CLI_BINARIES+=("$TARGET_DIR/$t/release/$CLI")
done

# ------------------------------------------------------------------ 组装 .app
rm -rf "$APP_PATH"
mkdir -p "$APP_PATH/Contents/MacOS" "$APP_PATH/Contents/Resources"
if [[ "${#BINARIES[@]}" -gt 1 ]]; then
  lipo -create -output "$APP_PATH/Contents/MacOS/$EXECUTABLE" "${BINARIES[@]}"
  lipo -create -output "$APP_PATH/Contents/MacOS/$CLI" "${CLI_BINARIES[@]}"
else
  cp "${BINARIES[0]}" "$APP_PATH/Contents/MacOS/$EXECUTABLE"
  cp "${CLI_BINARIES[0]}" "$APP_PATH/Contents/MacOS/$CLI"
fi

ICONSET="$OUT_DIR/AppIcon.iconset"
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
for px in 16 32 128 256 512; do
  rsvg-convert -w "$px" -h "$px" assets/icon.svg -o "$ICONSET/icon_${px}x${px}.png"
  rsvg-convert -w "$((px * 2))" -h "$((px * 2))" assets/icon.svg -o "$ICONSET/icon_${px}x${px}@2x.png"
done
iconutil -c icns "$ICONSET" -o "$APP_PATH/Contents/Resources/AppIcon.icns"
rm -rf "$ICONSET"

cat > "$APP_PATH/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>zh_CN</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleExecutable</key>
    <string>$EXECUTABLE</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$BUILD_NUMBER</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>LSMinimumSystemVersion</key>
    <string>$MIN_MACOS</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
EOF
plutil -lint "$APP_PATH/Contents/Info.plist" >/dev/null

# ------------------------------------------------------------------ 签名
echo "==> codesign（Developer ID · hardened runtime · timestamp）"
# 嵌套的 CLI 先单独签名（不用 --deep），再签整个 App
codesign --force --timestamp --options runtime --identifier "$BUNDLE_ID.cli" --sign "$DEVELOPER_ID_IDENTITY" "$APP_PATH/Contents/MacOS/$CLI"
codesign --force --timestamp --options runtime --sign "$DEVELOPER_ID_IDENTITY" "$APP_PATH"
codesign --verify --strict --verbose=2 "$APP_PATH"
CODESIGN_INFO=$(codesign --display --verbose=2 "$APP_PATH" 2>&1 || true)
grep -q 'flags=.*runtime' <<<"$CODESIGN_INFO" || { echo "✘ 缺少 hardened runtime 标记" >&2; exit 1; }
grep -q "TeamIdentifier=$TEAM_ID" <<<"$CODESIGN_INFO" || { echo "✘ 签名团队不是 $TEAM_ID" >&2; exit 1; }
# hardened runtime 拒绝加载非本团队签名的动态库：确认只依赖系统库
# （universal 二进制每个架构一节，节标题不缩进；只检查缩进的依赖条目）
for bin in "$EXECUTABLE" "$CLI"; do
  if otool -L "$APP_PATH/Contents/MacOS/$bin" | grep -E '^\s' | grep -vE '^\s+(/System/Library/|/usr/lib/)'; then
    echo "✘ $bin 依赖了系统以外的动态库（见上），签名后会启动失败" >&2
    exit 1
  fi
done
codesign --verify --strict --verbose=2 "$APP_PATH/Contents/MacOS/$CLI"
"$APP_PATH/Contents/MacOS/$CLI" --version >/dev/null || { echo "✘ 内置 CLI 无法运行" >&2; exit 1; }

notarize() {
  local target="$1" upload="$1" tmp=""
  if [[ "$target" == *.app ]]; then
    tmp="$(mktemp -d /tmp/codebaby-notary.XXXXXX)"
    upload="$tmp/$APP_NAME.zip"
    ditto -c -k --keepParent "$target" "$upload"
  fi
  echo "==> 公证 $(basename "$target")（通常 1–5 分钟）"
  xcrun notarytool submit "$upload" --keychain-profile "$NOTARY_PROFILE" --wait
  [[ -n "$tmp" ]] && rm -rf "$tmp"
  xcrun stapler staple "$target"
  xcrun stapler validate "$target"
}

if [[ "${SKIP_NOTARIZE:-0}" != "1" ]]; then
  notarize "$APP_PATH"
  spctl --assess --type execute --verbose=2 "$APP_PATH"
fi

# ------------------------------------------------------------------ DMG
if [[ "${SKIP_DMG:-0}" != "1" ]]; then
  command -v create-dmg >/dev/null 2>&1 || { echo "✘ 缺少 create-dmg：brew install create-dmg" >&2; exit 1; }
  rm -f "$DMG_PATH"
  DMG_OPTIONS=()
  [[ "${DMG_HEADLESS:-0}" == "1" ]] && DMG_OPTIONS+=(--skip-jenkins)
  echo "==> create-dmg"
  create-dmg \
    ${DMG_OPTIONS[@]+"${DMG_OPTIONS[@]}"} \
    --volname "$APP_NAME" \
    --window-pos 200 120 \
    --window-size 600 360 \
    --icon-size 96 \
    --icon "$APP_NAME.app" 165 180 \
    --hide-extension "$APP_NAME.app" \
    --app-drop-link 430 180 \
    --no-internet-enable \
    "$DMG_PATH" \
    "$APP_PATH" >/dev/null
  codesign --force --timestamp --sign "$DEVELOPER_ID_IDENTITY" "$DMG_PATH"
  if [[ "${SKIP_NOTARIZE:-0}" != "1" ]]; then
    notarize "$DMG_PATH"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG_PATH"
  fi
fi

echo ""
echo "✓ $APP_PATH"
[[ "${SKIP_DMG:-0}" != "1" ]] && echo "✓ $DMG_PATH"
[[ "${SKIP_NOTARIZE:-0}" == "1" ]] && echo "  （未公证：仅限本机使用）"
echo "  安装到 /Applications：scripts/install-mac.sh"
