#!/usr/bin/env bash
# Mac 用の NekoReq.app をビルドする（ホストに Rust は不要）。
#
#   1. Docker コンテナ内で画面をビルドし、cargo-zigbuild で macOS 向けにクロスコンパイル
#   2. ホストで .app バンドルを組み立て、アドホック署名（codesign -s -）する
#   3. --dmg を付けると .dmg も作る
#
# 使い方:
#   scripts/build-mac.sh                # このMacのCPU向け（Apple Silicon なら arm64）
#   scripts/build-mac.sh --universal    # Apple Silicon + Intel のユニバーサル
#   scripts/build-mac.sh --dmg          # .dmg も作る
#   scripts/build-mac.sh -f compose.yaml -f .compose-m7.yaml
#                                       # 使う compose ファイル（複数可）。環境変数 NEKOREQ_COMPOSE_FILES でも指定できる
#                                       # （空白区切り）。既定は compose.yaml
#
# 成果物: dist-mac/NekoReq.app（と dist-mac/NekoReq_<version>_<arch>.dmg）
#
# これはローカル確認用。配布用の .dmg / .msi と updater 用の署名つき更新ファイルは
# GitHub Actions（.github/workflows/release.yml）が作る（docs/07-build-and-release.md）。
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

UNIVERSAL=0
MAKE_DMG=0
COMPOSE_FILES=(${NEKOREQ_COMPOSE_FILES:-compose.yaml})
while [[ $# -gt 0 ]]; do
  case "$1" in
    --universal) UNIVERSAL=1 ;;
    --dmg) MAKE_DMG=1 ;;
    -f|--compose-file)
      [[ $# -ge 2 ]] || { echo "$1 にはファイル名が必要です" >&2; exit 2; }
      # 最初の -f で環境変数・既定の指定を置き換え、以降は追加する
      if [[ "${F_GIVEN:-0}" == 0 ]]; then COMPOSE_FILES=(); F_GIVEN=1; fi
      COMPOSE_FILES+=("$2"); shift ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "不明なオプション: $1" >&2; exit 2 ;;
  esac
  shift
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "このスクリプトはMacで実行してください" >&2
  exit 1
fi

COMPOSE=(docker compose)
for f in "${COMPOSE_FILES[@]}"; do COMPOSE+=(-f "$f"); done
SERVICE=workspace
dc_run() { "${COMPOSE[@]}" run --rm -T "$@"; }

if [[ "$UNIVERSAL" == 1 ]]; then
  TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
  ARCH_LABEL=universal
elif [[ "$(uname -m)" == "arm64" ]]; then
  TARGETS=(aarch64-apple-darwin)
  ARCH_LABEL=aarch64
else
  TARGETS=(x86_64-apple-darwin)
  ARCH_LABEL=x64
fi

CONF=src-tauri/tauri.conf.json
APP_NAME="$(plutil -extract productName raw -o - "$CONF")"
BIN_NAME="nekoreq"   # src-tauri/Cargo.toml の [[bin]]
VERSION="$(plutil -extract version raw -o - "$CONF")"
IDENTIFIER="$(plutil -extract identifier raw -o - "$CONF")"
MIN_OS="$(plutil -extract bundle.macOS.minimumSystemVersion raw -o - "$CONF")"
OUT="$ROOT/dist-mac"

echo "==> Docker イメージを準備"
"${COMPOSE[@]}" build "$SERVICE"

echo "==> macOS SDK を Docker ボリュームへ用意"
if ! dc_run "$SERVICE" test -e /opt/macos-sdk/MacOSX.sdk/SDKSettings.json; then
  SDK_DIR="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"
  if [[ -z "$SDK_DIR" || ! -d "$SDK_DIR" ]]; then
    SDK_DIR="/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk"
  fi
  if [[ ! -d "$SDK_DIR" ]]; then
    echo "macOS SDK が見つかりません。'xcode-select --install' で Command Line Tools を入れてください" >&2
    exit 1
  fi
  SDK_REAL="$(cd "$SDK_DIR" && pwd -P)"
  SDK_BASENAME="$(basename "$SDK_REAL")"
  echo "    $SDK_REAL をコピーします"
  tar -C "$(dirname "$SDK_REAL")" -cf - "$SDK_BASENAME" 2>/dev/null \
    | dc_run "$SERVICE" bash -c \
        "tar -xf - -C /opt/macos-sdk 2>/dev/null; ln -sfn '$SDK_BASENAME' /opt/macos-sdk/MacOSX.sdk"
fi
# zig のリンカ向けに .tbd を補正（何度実行しても同じ結果）
dc_run "$SERVICE" bash scripts/patch-sdk-tbd.sh

echo "==> 画面と Rust をビルド（${TARGETS[*]}、最低 macOS ${MIN_OS}）"
dc_run -e TARGETS="${TARGETS[*]}" -e MACOSX_DEPLOYMENT_TARGET="$MIN_OS" "$SERVICE" \
  bash -euo pipefail -c '
    npm ci --no-audit --no-fund
    npm run build
    mkdir -p /work/dist-mac/bin
    for t in $TARGETS; do
      cargo zigbuild --release --target "$t" -p nekoreq --features tauri/custom-protocol
      cp "$CARGO_TARGET_DIR/$t/release/nekoreq" "/work/dist-mac/bin/nekoreq-$t"
    done
  '

echo "==> .app バンドルを作成"
APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

if [[ "${#TARGETS[@]}" -gt 1 ]]; then
  lipo -create -output "$APP/Contents/MacOS/$BIN_NAME" \
    "$OUT/bin/$BIN_NAME-aarch64-apple-darwin" "$OUT/bin/$BIN_NAME-x86_64-apple-darwin"
else
  cp "$OUT/bin/$BIN_NAME-${TARGETS[0]}" "$APP/Contents/MacOS/$BIN_NAME"
fi
chmod +x "$APP/Contents/MacOS/$BIN_NAME"
cp src-tauri/icons/icon.icns "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>ja</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleExecutable</key><string>$BIN_NAME</string>
  <key>CFBundleIdentifier</key><string>$IDENTIFIER</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>$MIN_OS</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSRequiresAquaSystemAppearance</key><false/>
</dict>
</plist>
PLIST

echo "==> アドホック署名（Apple Silicon で起動するために必要。配布用のコード署名ではない）"
codesign --force --deep --sign - "$APP"
codesign --verify --verbose=1 "$APP"

if [[ "$MAKE_DMG" == 1 ]]; then
  echo "==> .dmg を作成"
  DMG="$OUT/${APP_NAME}_${VERSION}_${ARCH_LABEL}.dmg"
  STAGE="$(mktemp -d)"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$DMG"
  hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
  rm -rf "$STAGE"
  echo "    $DMG"
fi

echo
echo "完了: $APP"
echo "起動: open \"$APP\""
