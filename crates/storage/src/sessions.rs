//! タブの復元（`sessions`）。プロジェクトごとに JSON を 1 件だけ持つ。
//!
//! シークレットは呼び出し側（app）が取り除いてから渡す。ここでは中身を見ない。

use nekoreq_core::{SessionState, SessionTab};
use rusqlite::{params, OptionalExtension};

use crate::error::{Result, StorageError};
use crate::{now, Database};

impl Database {
    /// タブの状態を保存する（置き換え）。保存した日時を `saved_at` に入れて返す。
    pub fn session_save(&self, project_id: &str, state: &SessionState) -> Result<SessionState> {
        let mut saved = state.clone();
        saved.saved_at = Some(now());
        let json = serde_json::to_string(&saved)
            .map_err(|e| StorageError::Corrupt(format!("sessions.json: {e}")))?;
        let conn = self.conn();
        let exists = conn
            .query_row("SELECT 1 FROM projects WHERE id = ?1", [project_id], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if !exists {
            return Err(StorageError::NotFound {
                what: "プロジェクト",
                id: project_id.to_string(),
            });
        }
        conn.execute(
            "INSERT INTO sessions (project_id, json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(project_id) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
            params![project_id, json, saved.saved_at],
        )?;
        Ok(saved)
    }

    /// 前回のタブの状態。保存がなければ `None`。壊れていて読めないときも `None`（起動を止めない）。
    pub fn session_load(&self, project_id: &str) -> Result<Option<SessionState>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT json FROM sessions WHERE project_id = ?1",
                [project_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| lenient_session(&j)))
    }
}

/// 保存された JSON からタブの状態を作る。全体として読めればそのまま。読めないタブ（知らない `kind` など）が
/// あっても、読めるタブは復元する（読めなかった数は stderr に記録する。中身は出さない）。
fn lenient_session(json: &str) -> Option<SessionState> {
    if let Ok(state) = serde_json::from_str::<SessionState>(json) {
        return Some(state);
    }
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let object = value.as_object()?;
    let mut skipped = 0usize;
    let tabs: Vec<SessionTab> = object
        .get("tabs")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| match serde_json::from_value::<SessionTab>(t.clone()) {
            Ok(tab) => Some(tab),
            Err(_) => {
                skipped += 1;
                None
            }
        })
        .collect();
    if skipped == 0 && tabs.is_empty() && object.get("tabs").is_some_and(|t| !t.is_array()) {
        return None;
    }
    eprintln!("[nekoreq] 前回のタブのうち {skipped} 個は読めなかったので、復元しませんでした");
    let active_tab_id = object
        .get("activeTabId")
        .and_then(|a| a.as_str())
        .filter(|id| tabs.iter().any(|t| t.id == *id))
        .map(str::to_string);
    let saved_at = object
        .get("savedAt")
        .and_then(|a| a.as_str())
        .map(str::to_string);
    Some(SessionState {
        tabs,
        active_tab_id,
        saved_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_tabs_are_restored_even_when_one_tab_is_unreadable() {
        let json = r#"{"tabs":[
            {"id":"a","kind":"new","title":"A","dirty":false,"request":null,"opKey":null,"adhocId":null,"viewState":null},
            {"id":"b","kind":"from-the-future","title":"B","dirty":false},
            {"id":"c","kind":"new","title":"C","dirty":true,"request":null,"opKey":null,"adhocId":null,"viewState":null,"droppedFields":["x"]}
          ],"activeTabId":"b","savedAt":"2026-11-17T00:00:00.000Z"}"#;
        let state = lenient_session(json).unwrap();
        let ids: Vec<&str> = state.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
        assert_eq!(state.tabs[1].dropped_fields, ["x"]);
        // 選んでいたタブが読めなかったときは、選んでいるタブなし
        assert_eq!(state.active_tab_id, None);
        assert_eq!(state.saved_at.as_deref(), Some("2026-11-17T00:00:00.000Z"));
        assert!(lenient_session("not json").is_none());
    }
}
