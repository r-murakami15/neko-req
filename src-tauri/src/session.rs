//! タブの復元（`session_save` / `session_load`）。プロジェクトごとに、開いているタブと未保存の入力・選んでいるタブを持つ。
//!
//! 未保存の入力のシークレット: 手で書かれた認証の値（認証欄の Bearer のトークン・Basic のパスワード・API キー、
//! `Authorization` / `Cookie` / `X-XSRF-TOKEN` ヘッダー、Cookie の値）は**保存しない**。空にして、何を空にしたかを
//! `droppedFields` に残す（画面は「認証の値は復元されません。入れ直してください」と出す）。`{{変数}}` で参照した
//! ものはそのまま残る。`droppedFields` は、受け取ったものと今回空にしたものの和集合（復元したタブを保存し直しても消えない）。変数の上書き（`VarOverride`）は保存しない（もともと入力に含まれない）。

use nekoreq_core::{sanitize_spec, Fill, SessionState, MAX_SESSION_TABS};

use crate::error::AppError;
use crate::state::AppState;

impl AppState {
    /// タブの状態を保存する（置き換え）。手書きのシークレットを空にした状態を返す（保存した日時つき）。
    pub fn session_save(
        &self,
        project_id: &str,
        state: &SessionState,
    ) -> Result<SessionState, AppError> {
        if state.tabs.len() > MAX_SESSION_TABS {
            return Err(AppError::Validation(format!(
                "保存できるタブは {MAX_SESSION_TABS} 個までです"
            )));
        }
        let mut clean = state.clone();
        for tab in &mut clean.tabs {
            // 今回空にしたものに、受け取った `droppedFields`（前に空にして、復元したタブが持っているもの）を足す。
            // 保存し直すたびに知らせが消えて、認証が空のタブが黙って開くのを防ぐ
            let mut fields = std::mem::take(&mut tab.dropped_fields);
            if let Some(spec) = &tab.request {
                let s = sanitize_spec(spec, Fill::Empty);
                tab.request = Some(s.spec);
                for name in s.fields {
                    if !fields.contains(&name) {
                        fields.push(name);
                    }
                }
            }
            tab.dropped_fields = fields;
        }
        Ok(self.db().session_save(project_id, &clean)?)
    }

    /// 前回のタブの状態。保存がなければ `None`（`AppSettings.appearance.restoreTabs` が `false` のときは画面が使わない）。
    pub fn session_load(&self, project_id: &str) -> Result<Option<SessionState>, AppError> {
        Ok(self.db().session_load(project_id)?)
    }
}
