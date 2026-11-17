# NekoReq — 開発メモ

- 設計書は docs/01-design.md、画面イメージは docs/screens/*.dc.html（ブラウザで開くとレイアウトと文言が分かる）。仕様の判断は docs を正とする。
- ホストの Rust / Java / Bun / Node は使わない。cargo / npm はすべて workspace コンテナで実行する。
  - 起動: `docker compose up -d --build workspace`
  - 実行: `docker compose exec workspace bash -c '<コマンド>'`
- 構成: ルートの Cargo.toml がワークスペース。`src-tauri/`（app）と `crates/{core,http,openapi,storage,secrets,curl}`。画面は `src/`（React + TypeScript + Vite）。
- 画面と Rust の契約は tauri-specta が生成する `src/bindings.ts`。Rust 側のコマンドや型を変えたら生成し直す。手で書き換えない。
- UI は日本語のみ。シークレット（session・xsrfToken・password など）はログ・SQLite・履歴に平文で残さない。
- テストの入力に使う実際の API 定義はコンテナの `/fixtures`（環境変数 NEKOREQ_FIXTURE_DIR / NEKOREQ_FIXTURE_BUNDLED）。無い環境ではスキップする。
- ブランチは main のみ。
