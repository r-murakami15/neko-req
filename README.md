# NekoReq

OpenAPI の yaml を正本にして API を呼び出し・確認する、Mac と Windows 向けのデスクトップアプリ（Rust + Tauri 2、画面は React + TypeScript）。

- yaml を読むだけで、設計の変更は yaml で行う。アプリは「呼び出すための値」（入力値・環境変数・履歴）だけを持つ
- データは自分の PC の SQLite 1 ファイルに保存し、シークレット（セッション・パスワードなど）は OS のキーチェーンにだけ置く
- 外部へは、自分が送るリクエストと更新の確認以外に通信しない

> 開発中。いまは M1（土台）まで。プロジェクトを作って、再起動後も残るところまで動く。実装の順番は [設計書](docs/01-design.md) の「実装計画」を参照。

## 開発

ホストには Rust / Node を入れず、Docker の workspace コンテナで開発する。

```bash
docker compose up -d --build workspace

# 画面の依存インストール、型チェック、テスト、ビルド
docker compose exec workspace bash -c 'npm install && npm run typecheck && npm test && npm run build'

# Rust のフォーマット・Lint・テスト（先に npm run build で dist/ を作っておく）
docker compose exec workspace bash -c 'cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace'

# 画面だけをブラウザで確認（モックのバックエンドで動く）
docker compose exec workspace bash -c 'npm run dev -- --host 0.0.0.0'   # → http://localhost:1420
```

コンテナ内では GUI は動かない（Rust 側はテストとビルドの確認まで）。詳しくは [docs/09-development.md](docs/09-development.md)。

## ドキュメント

設計書と画面イメージは [docs/](docs/README.md) を参照。
