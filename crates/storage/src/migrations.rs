//! 起動時のマイグレーション。`PRAGMA user_version` で版を管理する。
//!
//! スキーマを変えるときは `MIGRATIONS` の末尾に SQL を足す（既存の要素は書き換えない）。
//! 配列の長さがそのまま最新の版になる。

use rusqlite::Connection;

use crate::error::{Result, StorageError};

/// 版 N への移行 SQL は `MIGRATIONS[N - 1]`。
pub(crate) const MIGRATIONS: &[&str] = &[V1, V2, V3, V4];

/// このアプリが対応する最新のスキーマの版。
pub const LATEST_VERSION: u32 = MIGRATIONS.len() as u32;

/// v1: 全テーブル。
///
/// - 日時はすべて UTC の RFC 3339 文字列（TEXT）。並べ替えは文字列比較でよい。
/// - `env_vars` の `kind = 'secret'` は値を持たない（`value` は NULL。CHECK で強制）。
///   実体は OS のキーチェーンにある。
/// - `history_fts` は URL とボディの全文検索。日本語を部分一致で探せるよう trigram を使い、
///   `history` のトリガーで同期する。
const V1: &str = r#"
CREATE TABLE projects (
    id                     TEXT PRIMARY KEY NOT NULL,
    name                   TEXT NOT NULL,
    default_environment_id TEXT,
    created_at             TEXT NOT NULL,
    updated_at             TEXT NOT NULL
);

CREATE TABLE api_sources (
    id              TEXT PRIMARY KEY NOT NULL,
    project_id      TEXT NOT NULL UNIQUE REFERENCES projects(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL CHECK (kind IN ('path', 'url', 'copy')),
    entry           TEXT NOT NULL,
    files_json      TEXT NOT NULL DEFAULT '[]',
    openapi_version TEXT NOT NULL,
    synced_at       TEXT
);

-- yaml を読んだ結果のキャッシュ（同期のたびに作り直す）
CREATE TABLE operations (
    id                 TEXT PRIMARY KEY NOT NULL,
    project_id         TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    op_key             TEXT NOT NULL,
    operation_id       TEXT,
    method             TEXT NOT NULL,
    path               TEXT NOT NULL,
    tags_json          TEXT NOT NULL DEFAULT '[]',
    summary            TEXT,
    description        TEXT,
    parameters_json    TEXT NOT NULL DEFAULT '[]',
    request_body_json  TEXT,
    responses_json     TEXT NOT NULL DEFAULT '[]',
    security_json      TEXT,
    source_file        TEXT,
    source_line        INTEGER,
    content_hash       TEXT NOT NULL,
    UNIQUE (project_id, op_key)
);
CREATE INDEX idx_operations_project ON operations(project_id);

-- Operation が消えても残す（「削除された API」）ので operations への外部キーは張らない
CREATE TABLE request_drafts (
    id          TEXT PRIMARY KEY NOT NULL,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    op_key      TEXT NOT NULL,
    params_json TEXT NOT NULL DEFAULT '[]',
    body_json   TEXT NOT NULL,
    auth_json   TEXT,
    updated_at  TEXT NOT NULL,
    UNIQUE (project_id, op_key)
);

CREATE TABLE adhoc_requests (
    id          TEXT PRIMARY KEY NOT NULL,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    method      TEXT NOT NULL,
    url         TEXT NOT NULL,
    params_json TEXT NOT NULL DEFAULT '[]',
    body_json   TEXT NOT NULL,
    auth_json   TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_adhoc_requests_project ON adhoc_requests(project_id);

CREATE TABLE environments (
    id         TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_environments_project ON environments(project_id);

CREATE TABLE env_vars (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    environment_id TEXT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('plain', 'secret')),
    value          TEXT,
    description    TEXT NOT NULL DEFAULT '',
    sort_order     INTEGER NOT NULL DEFAULT 0,
    UNIQUE (environment_id, name),
    -- シークレットの値は SQLite に保存しない
    CHECK (kind = 'plain' OR value IS NULL)
);

-- 履歴。保存する前に伏せ字にした内容だけが入る。environment_id / op_key は消えた後も残すので外部キーなし
CREATE TABLE history (
    seq                     INTEGER PRIMARY KEY AUTOINCREMENT,
    id                      TEXT NOT NULL UNIQUE,
    project_id              TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    sent_at                 TEXT NOT NULL,
    environment_id          TEXT,
    environment_name        TEXT,
    op_key                  TEXT,
    method                  TEXT NOT NULL,
    url                     TEXT NOT NULL,
    request_headers_json    TEXT NOT NULL DEFAULT '[]',
    request_body            TEXT,
    status                  INTEGER,
    status_text             TEXT,
    response_headers_json   TEXT NOT NULL DEFAULT '[]',
    response_body           TEXT,
    response_body_truncated INTEGER NOT NULL DEFAULT 0,
    response_size           INTEGER NOT NULL DEFAULT 0,
    timing_json             TEXT,
    error                   TEXT
);
CREATE INDEX idx_history_project_sent ON history(project_id, sent_at DESC);

-- 全文検索（URL とボディ）。rowid = history.seq
CREATE VIRTUAL TABLE history_fts USING fts5(url, body, tokenize = 'trigram');

CREATE TRIGGER history_fts_insert AFTER INSERT ON history BEGIN
    INSERT INTO history_fts (rowid, url, body)
    VALUES (new.seq, new.url, coalesce(new.request_body, '') || char(10) || coalesce(new.response_body, ''));
END;
CREATE TRIGGER history_fts_delete AFTER DELETE ON history BEGIN
    DELETE FROM history_fts WHERE rowid = old.seq;
END;
CREATE TRIGGER history_fts_update AFTER UPDATE OF url, request_body, response_body ON history BEGIN
    DELETE FROM history_fts WHERE rowid = old.seq;
    INSERT INTO history_fts (rowid, url, body)
    VALUES (new.seq, new.url, coalesce(new.request_body, '') || char(10) || coalesce(new.response_body, ''));
END;
"#;

/// v2: グローバル環境と変数の有効・無効（M2）。
///
/// - `environments.is_global = 1` がプロジェクトごとの「グローバル」環境（プロジェクトに 1 つだけ）。
///   既存のプロジェクトにも作る。
/// - `env_vars.enabled`: 無効の変数は展開に使わない。
/// - `env_vars` の「シークレットは値を持たない」CHECK は v1 のまま残る（列を足すだけで表は作り直さない）。
const V2: &str = r#"
ALTER TABLE environments ADD COLUMN is_global INTEGER NOT NULL DEFAULT 0;
ALTER TABLE env_vars ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
CREATE UNIQUE INDEX idx_environments_one_global ON environments(project_id) WHERE is_global = 1;

INSERT INTO environments (id, project_id, name, sort_order, created_at, updated_at, is_global)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4'
        || substr(lower(hex(randomblob(2))), 2) || '-'
        || substr('89ab', 1 + (abs(random()) % 4), 1) || substr(lower(hex(randomblob(2))), 2)
        || '-' || lower(hex(randomblob(6))),
    p.id,
    'グローバル',
    0,
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
    1
FROM projects p;
"#;

/// v3: 手で作ったリクエストの説明（M2）。保存ダイアログの「説明（任意）」。既存の行は空。
const V3: &str = r#"
ALTER TABLE adhoc_requests ADD COLUMN description TEXT NOT NULL DEFAULT '';
"#;

/// v4: OpenAPI の取り込みと同期（M3・M4）。
///
/// - `api_sources`: URL 方式で確認済みの別ホスト（`allowed_hosts_json`）、アクセストークンの有無（`has_token`。
///   値はキーチェーン）、取り込みの変換オプション（`options_json`）、yaml の文書全体の情報（`meta_json`: タイトル・
///   servers・認証の定義・ルートの security）。
/// - `operations.imported`: 確認画面で選んだ API だけ 1。選ばなかった API もキャッシュには持つ（同期のたびに
///   「追加」と出さないため）。`operations.deleted_at`: yaml から消えたが入力値が残っている API（「削除された API」）。
/// - `request_drafts.stale_params_json`: yaml から消えたパラメータ（値は残し、「yaml にない」と表示して送信から外す）。
const V4: &str = r#"
ALTER TABLE api_sources ADD COLUMN allowed_hosts_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE api_sources ADD COLUMN has_token INTEGER NOT NULL DEFAULT 0;
ALTER TABLE api_sources ADD COLUMN options_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE api_sources ADD COLUMN meta_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE operations ADD COLUMN imported INTEGER NOT NULL DEFAULT 1;
ALTER TABLE operations ADD COLUMN deleted_at TEXT;
ALTER TABLE request_drafts ADD COLUMN stale_params_json TEXT NOT NULL DEFAULT '[]';
"#;

/// 現在の `user_version` を返す。
pub fn current_version(conn: &Connection) -> Result<u32> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(u32::try_from(v).unwrap_or(0))
}

/// 最新の版まで順に移行する。すでに最新なら何もしない。
/// 1 つの版ごとにトランザクションで流すので、途中で失敗しても前の版のまま残る。
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let current = current_version(conn)?;
    if current > LATEST_VERSION {
        return Err(StorageError::SchemaTooNew {
            found: current,
            supported: LATEST_VERSION,
        });
    }
    for version in (current + 1)..=LATEST_VERSION {
        let tx = conn.transaction()?;
        tx.execute_batch(MIGRATIONS[(version - 1) as usize])?;
        // PRAGMA は値にバインドできないので、定数から作った数値を直接埋める
        tx.execute_batch(&format!("PRAGMA user_version = {version};"))?;
        tx.commit()?;
    }
    Ok(())
}
