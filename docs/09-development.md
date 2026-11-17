# 開発環境

[目次に戻る](README.md)

ホストには Rust / Node を入れず、Docker の workspace コンテナ（`docker/Dockerfile`、`compose.yaml` の `workspace` サービス）で開発する。コンテナには Rust 1.90・Node.js 20・Tauri の Linux 依存・Zig・cargo-zigbuild が入っている。

## よく使うコマンド

```bash
# 起動（初回はイメージのビルドを含む）
docker compose up -d --build workspace

# フロントエンドの依存インストールと型チェック・テスト
docker compose exec workspace bash -c 'npm install && npm run typecheck && npm test'

# Rust のフォーマット・Lint・テスト
docker compose exec workspace bash -c 'cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace'

# 画面だけをブラウザで確認（Rust なし。モックのバックエンドで動く）
docker compose exec workspace bash -c 'npm run dev -- --host 0.0.0.0'
# → http://localhost:1420
```

## 画面と Rust をつなぐ型（bindings.ts）の生成

画面と Rust の契約は tauri-specta が生成する `src/bindings.ts`。手で書き換えない。Rust のコマンドや型（`crates/core` のモデル、`src-tauri/src/commands.rs`）を変えたら、GUI なしで次のコマンドで生成し直し、差分をコミットする。

```bash
docker compose exec workspace bash -c 'cargo test -p nekoreq export_bindings'
```

- 生成の中身は `src-tauri/src/lib.rs` の `specta_builder()`（コマンドと、コマンドから直接は参照されないモデルの型の登録）。新しいコマンドは `collect_commands![...]` に足す。
- debug ビルドでアプリを起動したときも、起動時に同じ内容を書き出す。
- CI は `git diff --exit-code src/bindings.ts` で、生成し忘れを検出する。
- エラーは `AppError`（`{ kind, message }`）として TypeScript にも出る。画面側は `src/api/` の `unwrap` が `ApiError` に包んで投げる。

## 配布物のビルド

Mac 用の `.app` / `.dmg` は `scripts/build-mac.sh`（コンテナでクロスビルドし、ホストで組み立てと ad-hoc 署名）。リリースの手順と自動更新の鍵は [07-build-and-release.md](07-build-and-release.md)。

```bash
scripts/build-mac.sh --dmg   # → dist-mac/NekoReq.app と NekoReq_<版>_<arch>.dmg
```

## 注意

- `cargo clippy` / `cargo test` は Tauri の生成コードが `dist/`（`frontendDist`）を要求するため、先に `npm run build`（または空の `dist/` フォルダ）が必要。CI は画面のビルドを Rust より先に流す。
- 画面は Tauri の外（ブラウザ）で開くと `src/mock/` のモックに自動で切り替わる（`src/api/index.ts` の `getDefaultBackend`）。Tauri の中では bindings.ts のコマンドを呼ぶ。
- フォントはアプリに同梱する（`src/styles/fonts.css`。外部の CDN は CSP と「外部へ通信しない」方針に反するため使わない）。
- アイコンの元画像は `assets/icon.svg`。変えたら `npx tauri icon assets/icon.svg` で `src-tauri/icons/` を作り直し、不要な `android/` `ios/` は消す。

## コンテナの構成

| 項目 | 内容 |
| --- | --- |
| ソース | リポジトリを `/work` にマウント |
| 名前付きボリューム | `cargo-target`（`CARGO_TARGET_DIR=/cargo-target`）、`cargo-registry` / `cargo-git`、`node-modules`、`macos-sdk` |
| テスト用の API 定義 | `../nippon-animation-license-management` の `docs/20_design/api`（分割 25 ファイル）を `/fixtures/license-api`、`openapi.bundled.yaml` を `/fixtures/openapi.bundled.yaml` に読み取り専用でマウント |
| ホストの API | コンテナからは `http://host.docker.internal:8080` で届く |

## ディレクトリ構成

```
.
├── compose.yaml / docker/Dockerfile   開発コンテナ
├── scripts/                            build-mac.sh（Mac 用の .app / .dmg）、patch-sdk-tbd.sh
├── .github/workflows/                  ci.yml（検査）、release.yml（タグでビルド・GitHub Releases へ）
├── Cargo.toml                          Cargo ワークスペース
├── crates/
│   ├── core/       ドメインモデル、変数展開、リクエストの組み立て、伏せ字処理
│   ├── http/       送信、Cookie、時間の計測、中止
│   ├── openapi/    yaml の読み込み、分割ファイルの $ref 解決、Operation への変換、差分、行番号
│   ├── storage/    SQLite、マイグレーション、履歴の全文検索
│   ├── secrets/    キーチェーン
│   └── curl/       cURL の解析と書き出し
├── src-tauri/                          app（Tauri の起動、コマンド、イベント、自動更新）
├── index.html, src/                    画面（React + TypeScript + Vite）
│   ├── bindings.ts                     tauri-specta が生成する型とコマンド
│   ├── api/                            Tauri 呼び出しの薄い層（Backend、react-query のフック）
│   ├── mock/                           ブラウザ確認用のモックバックエンド
│   ├── store/                          zustand の状態（開いているプロジェクトなど）
│   ├── components/                     画面の部品
│   └── styles/                         theme.css（配色・フォント・余白の変数）、fonts.css、app.css
├── assets/                             アイコンの元画像
└── docs/                               設計書と画面イメージ
```
