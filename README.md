<h1 align="center">
  <br>
  <a href="https://github.com/r-murakami15/neko-req"><img src="src-tauri/icons/icon.png" alt="NekoReq" width="160"></a>
  <br>
  NekoReq
  <br>
</h1>

<h4 align="center">OpenAPI の yaml を正本にして API を呼び出す、Mac と Windows のためのデスクトップ API クライアント。<br><a href="https://v2.tauri.app/" target="_blank">Tauri 2</a> と Rust と React で作っています。</h4>

<p align="center">
  <a href="https://github.com/r-murakami15/neko-req/releases/latest">
    <img src="https://img.shields.io/github/v/release/r-murakami15/neko-req?label=release&color=6e56cf" alt="最新リリース">
  </a>
  <a href="https://github.com/r-murakami15/neko-req/actions/workflows/ci.yml">
    <img src="https://github.com/r-murakami15/neko-req/actions/workflows/ci.yml/badge.svg" alt="CI">
  </a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey" alt="対応 OS">
  <img src="https://img.shields.io/badge/OpenAPI-3.0%20%7C%203.1%20%7C%203.2-6BA539?logo=openapiinitiative&logoColor=white" alt="OpenAPI 3.0 / 3.1 / 3.2">
  <img src="https://img.shields.io/badge/Rust-1.90-000000?logo=rust" alt="Rust 1.90">
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT ライセンス">
</p>

<p align="center">
  <a href="#主な機能">主な機能</a> •
  <a href="#使い方">使い方</a> •
  <a href="#ダウンロード">ダウンロード</a> •
  <a href="#開発">開発</a> •
  <a href="#構成">構成</a> •
  <a href="#クレジット">クレジット</a> •
  <a href="#ライセンス">ライセンス</a>
</p>

---

**NekoReq は yaml を読むだけ。** 設計の変更は yaml で行い、アプリが持つのは「呼び出すための値」（入力値・環境変数・履歴）だけです。データは自分の PC の SQLite 1 ファイルに、シークレットは OS のキーチェーンにだけ置き、自分が送るリクエストと更新の確認のほかには外へ通信しません。

## 主な機能

* **OpenAPI の取り込み**
  - ファイルのパス・URL・フォルダ・複数ファイル・zip から読み込む
  - 分割された yaml の `$ref` を解決する（OpenAPI 3.0 / 3.1 / 3.2）
* **yaml との同期**
  - yaml が変わるとサイドバーに印が付き、差分を見てから反映できる
  - yaml から消えた API は「削除された API」として残る
* **設計タブ**
  - パラメータ・スキーマ・説明（Markdown）を読み取り専用で表示
  - 行番号からいつものエディタで yaml を開ける
* **リクエストの送信**
  - 送信・中止、DNS から受信までのタイミングを表示
  - ボディ: JSON（構文チェック・整形・スキーマから例を挿入）、フォーム、multipart、テキスト、バイナリ
  - 認証: yaml の `security` に従う既定、API キー（ヘッダー・クエリ・Cookie）、Bearer、Basic
  - Cookie によるセッションと CSRF トークン（`SESSION` / `XSRF-TOKEN`）を自動で付け替える
* **変数と環境**
  - `{{name}}` の展開。優先順は「リクエストの上書き › 選んでいる環境 › グローバル」
  - 動的変数 `$uuid` `$timestamp` `$isoTimestamp` `$randomInt`
  - シークレットの値は OS のキーチェーンにだけ保存
* **履歴**
  - 全文検索（SQLite FTS5）と絞り込み、再送信、2 件の差分比較
  - シークレットは伏せ字で保存。件数の上限は 1,000 / 5,000 / 無制限
* **cURL の取り込みと書き出し**
* **レスポンス**
  - JSON の整形表示と Raw、ヘッダーの絞り込み、バイナリはファイルに保存
  - 受け取るサイズの上限（10 MB / 50 MB / 無制限）
* **使い心地**
  - タブ（右クリックで閉じる・複製・ツリーで表示・cURL コピー）、横スクロール、起動時に前回のタブを復元
  - サイドバーとレスポンス欄の幅をドラッグで変更
  - ダーク / ライト / OS に合わせる
  - プロキシ（OS の設定・手動）、クライアント証明書（.p12 / .pfx / .pem）、証明書検証やリダイレクトの設定
* **自動更新**
  - 起動後に新しい版を確認し、承認してから入れ替える

## 使い方

1. アプリを起動し、ヘッダーのプロジェクト名から「新しいプロジェクトを作成…」を選ぶ。
2. ヘッダーの「OpenAPI取り込み」から yaml / json（またはフォルダ・zip・URL）を選ぶ。
3. サイドバーのツリーから API を選び、値を入れて送信する。

| 操作 | macOS | Windows |
| --- | --- | --- |
| 送信 | <kbd>⌘</kbd> <kbd>Enter</kbd> | <kbd>Ctrl</kbd> <kbd>Enter</kbd> |
| 保存 | <kbd>⌘</kbd> <kbd>S</kbd> | <kbd>Ctrl</kbd> <kbd>S</kbd> |
| 空のタブを開く | <kbd>⌘</kbd> <kbd>T</kbd> | <kbd>Ctrl</kbd> <kbd>T</kbd> |
| タブを閉じる | <kbd>⌘</kbd> <kbd>W</kbd> | <kbd>Ctrl</kbd> <kbd>W</kbd> |
| API を検索 | <kbd>⌘</kbd> <kbd>K</kbd> | <kbd>Ctrl</kbd> <kbd>K</kbd> |
| 設定 | <kbd>⌘</kbd> <kbd>,</kbd> | <kbd>Ctrl</kbd> <kbd>,</kbd> |
| レスポンス欄の開閉 | <kbd>⌘</kbd> <kbd>Shift</kbd> <kbd>R</kbd> | <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>R</kbd> |

## ダウンロード

[Releases](https://github.com/r-murakami15/neko-req/releases/latest) から入手できます。

| OS | ファイル | 対応 |
| --- | --- | --- |
| macOS | `NekoReq_<版>_universal.dmg` | Apple Silicon / Intel、macOS 10.15 以降 |
| Windows | `NekoReq_<版>_x64_ja-JP.msi` | Windows 10 / 11（x64） |

> [!NOTE]
> OS のコード署名と公証をしていないので、初回の起動だけ許可が要ります。
> - **macOS**: 一度開いて閉じ、「システム設定」›「プライバシーとセキュリティ」で「このまま開く」を押す。または `xattr -dr com.apple.quarantine /Applications/NekoReq.app`
> - **Windows**: SmartScreen の画面で「詳細情報」›「実行」を押す
>
> 詳しくは [docs/07-build-and-release.md](docs/07-build-and-release.md#初回起動)。

## 開発

ホストに Rust / Node は入れず、Docker の workspace コンテナで開発します（Rust 1.90、Node 20）。

```bash
# コンテナを起動する
docker compose up -d --build workspace

# 画面: 依存のインストール、型チェック、テスト、ビルド
docker compose exec workspace bash -c 'npm install && npm run typecheck && npm test && npm run build'

# Rust: フォーマット、Lint、テスト（先に npm run build で dist/ を作っておく）
docker compose exec workspace bash -c 'cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace'

# 画面と Rust の型（src/bindings.ts）を作り直す
docker compose exec workspace bash -c 'cargo test -p nekoreq export_bindings'

# 画面だけをブラウザで確認する（モックのバックエンドで動く） → http://localhost:1420
docker compose exec workspace bash -c 'npm run dev -- --host 0.0.0.0'

# Mac 用の .app / .dmg をコンテナでクロスビルドする → dist-mac/
scripts/build-mac.sh --universal --dmg
```

> [!TIP]
> コンテナの中では GUI は動きません（Rust 側はテストとビルドの確認まで）。リリースは `v*` のタグを push すると GitHub Actions が .dmg と .msi を作り、下書きとして置きます。詳しくは [docs/09-development.md](docs/09-development.md) と [docs/07-build-and-release.md](docs/07-build-and-release.md)。

## 構成

```
.
├── src/                  画面（React + TypeScript + Vite）。src/bindings.ts は tauri-specta が生成
├── src-tauri/            アプリ本体（Tauri の起動・コマンド・イベント・自動更新）
└── crates/
    ├── core/             ドメインモデル、変数の展開、リクエストの組み立て、伏せ字
    ├── http/             送信（reqwest + rustls）、Cookie、タイミング、プロキシ・証明書
    ├── openapi/          yaml の読み込み、分割 $ref の解決、差分、行番号
    ├── storage/          SQLite（マイグレーション、履歴の全文検索）
    ├── secrets/          OS キーチェーンへの読み書き
    └── curl/             cURL の解析と書き出し
```

設計書と画面イメージは [docs/](docs/README.md) にあります。仕様の判断は docs を正とします。

## まだ無いもの

v1 では扱わず、v2 以降で考えるもの（[設計書](docs/01-design.md)）:

- 設計タブでの編集と yaml への書き戻し、OpenAPI のエクスポート
- レスポンスのスキーマ検証、テストケース、前処理・後処理スクリプト、モックサーバー
- OAuth 2.0・Digest 認証、Swagger 2.0 の取り込み
- Postman・Bruno のコレクションの取り込み、チームでの共有

## クレジット

NekoReq は次のオープンソースを使っています。

- [Tauri](https://tauri.app/) / [tauri-specta](https://github.com/specta-rs/tauri-specta)
- [React](https://react.dev/) / [Vite](https://vite.dev/) / [TypeScript](https://www.typescriptlang.org/)
- [CodeMirror](https://codemirror.net/) / [Radix UI](https://www.radix-ui.com/) / [TanStack Query・Virtual](https://tanstack.com/) / [Zustand](https://github.com/pmndrs/zustand)
- [react-markdown](https://github.com/remarkjs/react-markdown) / [rehype-sanitize](https://github.com/rehypejs/rehype-sanitize)
- [reqwest](https://github.com/seanmonstar/reqwest) / [rustls](https://github.com/rustls/rustls) / [Tokio](https://tokio.rs/)
- [rusqlite](https://github.com/rusqlite/rusqlite) / [keyring](https://github.com/hwchen/keyring-rs) / [saphyr](https://github.com/saphyr-rs/saphyr) / [similar](https://github.com/mitsuhiko/similar)
- フォント: [Inter](https://rsms.me/inter/) / [JetBrains Mono](https://www.jetbrains.com/lp/mono/) / [Noto Sans JP](https://fonts.google.com/noto/specimen/Noto+Sans+JP)

## ライセンス

MIT

---

> [GitHub](https://github.com/r-murakami15/neko-req) &nbsp;&middot;&nbsp;
> [Releases](https://github.com/r-murakami15/neko-req/releases) &nbsp;&middot;&nbsp;
> [ドキュメント](docs/README.md)
