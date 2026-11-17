#!/usr/bin/env bash
# コンテナ内で実行する。macOS 26 以降の SDK は .tbd に arm64e-macos しか書かれていないものがあり、
# zig のリンカが arm64 向けのシンボルを見つけられない。arm64-macos を併記して回避する（コピーしたSDKのみ変更）。
set -euo pipefail
SDK="${1:-/opt/macos-sdk/MacOSX.sdk}"
SDK="$(readlink -f "$SDK")"
count=0
while IFS= read -r -d '' f; do
  if grep -q 'arm64e-macos' "$f" && ! grep -q 'arm64-macos' "$f"; then
    perl -pi -e 's/\barm64e-macos\b/arm64-macos, arm64e-macos/g' "$f"
    count=$((count + 1))
  fi
done < <(find "$SDK" -name '*.tbd' -print0)
echo "patched $count .tbd files in $SDK"
