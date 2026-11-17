//! nekoreq-storage: SQLite への保存、起動時のマイグレーション、履歴の全文検索。
//!
//! - DB は 1 ファイル（`<app_data_dir>/data.sqlite`）。`Database::open` が開いて最新の版まで移行する。
//! - シークレットの値は保存しない（`env_vars.kind = 'secret'` の `value` は NULL）。通常の変数だった値が
//!   ファイルに残らないよう、`secure_delete` を有効にし、環境の保存のたびに WAL を空にする。
//! - リポジトリ関数: プロジェクト（M1）、環境と変数・手で作ったリクエスト（AdHocRequest）・
//!   入力値（RequestDraft）（M2）、取り込み元（ApiSource）と Operation のキャッシュ・同期の反映（M3・M4）。
//!   履歴の関数は M6 で足す。
//! - プロジェクトを作ると「グローバル」環境（`environments.is_global`）も 1 つ作る。

mod adhoc;
mod drafts;
mod environments;
mod error;
mod migrations;
mod operations;
mod projects;

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

pub use error::{Result, StorageError};
pub use migrations::LATEST_VERSION;
pub use operations::{ApplyReport, Selection, StaleParam, StoredOperation};

/// SQLite への接続。`Send + Sync` なので Tauri の State に置ける。
#[derive(Debug)]
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// ファイルの DB を開く（なければ親フォルダごと作る）。開いたら最新の版まで移行する。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        // WAL は途中で電源が落ちても壊れにくく、読み書きが互いを待たない
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        Self::init(conn)
    }

    /// メモリ上の DB（テスト用）。
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        // secure_delete: 消した・上書きした値（通常の変数をシークレットに変えたときの元の平文など）を
        // ページに残さず 0 で埋める。WAL に残った分は、変数を保存するたびのチェックポイントで消す
        conn.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000; PRAGMA secure_delete = ON;",
        )?;
        migrations::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// WAL の内容を DB ファイルに書き戻して WAL を空にする（古いページの平文を WAL に残さない）。
    /// できなくても（別の接続が読んでいるなど）エラーにしない。
    pub(crate) fn truncate_wal(conn: &Connection) {
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }

    /// 現在のスキーマの版（`PRAGMA user_version`）。
    pub fn schema_version(&self) -> Result<u32> {
        migrations::current_version(&self.conn())
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // 別のスレッドが panic しても、接続自体は使える
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 保存する日時。UTC の RFC 3339（ミリ秒、`Z`）。文字列のまま並べ替えられる。
pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_m2;
#[cfg(test)]
mod tests_m3;
