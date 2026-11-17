//! 画面から呼ぶコマンド。ロジックは `AppState` にあり、ここは Tauri との受け渡しだけ。

use nekoreq_core::{
    AdHocRequest, Environment, OpKey, Project, RequestDraft, RequestPreview, SendSettings,
};
use nekoreq_openapi::{OperationDetail, OperationListItem, OperationOpen, SourceChanged};
use serde::Serialize;
use specta::Type;
use tauri::State;
use tauri_specta::Event;

use crate::error::AppError;
use crate::openapi::{
    ImportRequest, ImportResult, PreviewResult, SourceInput, SyncApplyResult, SyncCheckResult,
    UploadInput,
};
use crate::send::RequestInput;
use crate::state::AppState;

/// アプリの情報（版・データの場所など）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub identifier: String,
    pub data_dir: String,
    pub db_path: String,
    pub schema_version: u32,
}

#[tauri::command]
#[specta::specta]
pub fn app_info(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<AppInfo, AppError> {
    state.app_info(
        &app.package_info().version.to_string(),
        &app.config().identifier,
    )
}

#[tauri::command]
#[specta::specta]
pub fn project_create(state: State<'_, AppState>, name: String) -> Result<Project, AppError> {
    state.project_create(&name)
}

#[tauri::command]
#[specta::specta]
pub fn project_list(state: State<'_, AppState>) -> Result<Vec<Project>, AppError> {
    state.project_list()
}

#[tauri::command]
#[specta::specta]
pub fn project_rename(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<Project, AppError> {
    state.project_rename(&id, &name)
}

#[tauri::command]
#[specta::specta]
pub fn project_delete(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    state.project_delete(&id)
}

// ---- 送信（M2） ----

/// 送信を始めて、送信 ID をすぐ返す。結果は `request://finished`（`events.requestFinished`）で届く。
/// 戻り値より先にイベントが届くことがあるので、画面は先に listen しておき、送信 ID で突き合わせる。
#[tauri::command]
#[specta::specta]
pub async fn request_send(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    input: RequestInput,
    settings: Option<SendSettings>,
) -> Result<String, AppError> {
    state.request_send(input, settings, move |finished| {
        let _ = finished.emit(&app);
    })
}

/// 送信中のリクエストを止める。止めるものがあれば `true`。
#[tauri::command]
#[specta::specta]
pub fn request_cancel(state: State<'_, AppState>, send_id: String) -> bool {
    state.request_cancel(&send_id)
}

/// 展開後の URL（シークレットは `••••`）・展開できない変数・自動で付くヘッダー。
#[tauri::command]
#[specta::specta]
pub fn request_preview(
    state: State<'_, AppState>,
    input: RequestInput,
) -> Result<RequestPreview, AppError> {
    state.request_preview(&input)
}

/// 受け取ったボディの全体を `path`（絶対パス。保存ダイアログで選ぶ）に保存する。
#[tauri::command]
#[specta::specta]
pub fn response_save_body(
    state: State<'_, AppState>,
    send_id: String,
    path: String,
) -> Result<(), AppError> {
    state.response_save_body(&send_id, &path)
}

// ---- 環境と変数 ----

#[tauri::command]
#[specta::specta]
pub fn env_list(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<Environment>, AppError> {
    state.env_list(&project_id)
}

#[tauri::command]
#[specta::specta]
pub fn env_save(
    state: State<'_, AppState>,
    environment: Environment,
) -> Result<Environment, AppError> {
    state.env_save(&environment)
}

#[tauri::command]
#[specta::specta]
pub fn env_create(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
) -> Result<Environment, AppError> {
    state.env_create(&project_id, &name)
}

#[tauri::command]
#[specta::specta]
pub fn env_duplicate(state: State<'_, AppState>, id: String) -> Result<Environment, AppError> {
    state.env_duplicate(&id)
}

#[tauri::command]
#[specta::specta]
pub fn env_delete(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    state.env_delete(&id)
}

/// 「選んでいる環境」を決める（`null` で未選択）。更新後のプロジェクトを返す。
#[tauri::command]
#[specta::specta]
pub fn env_select(
    state: State<'_, AppState>,
    project_id: String,
    environment_id: Option<String>,
) -> Result<Project, AppError> {
    state.env_select(&project_id, environment_id.as_deref())
}

// ---- シークレット ----

#[tauri::command]
#[specta::specta]
pub fn secret_set(
    state: State<'_, AppState>,
    project_id: String,
    environment_id: String,
    name: String,
    value: String,
) -> Result<(), AppError> {
    state.secret_set(&project_id, &environment_id, &name, &value)
}

#[tauri::command]
#[specta::specta]
pub fn secret_delete(
    state: State<'_, AppState>,
    project_id: String,
    environment_id: String,
    name: String,
) -> Result<(), AppError> {
    state.secret_delete(&project_id, &environment_id, &name)
}

/// 目のアイコンで一時的に見せる用。値がなければ `null`。
#[tauri::command]
#[specta::specta]
pub fn secret_reveal(
    state: State<'_, AppState>,
    project_id: String,
    environment_id: String,
    name: String,
) -> Result<Option<String>, AppError> {
    state.secret_reveal(&project_id, &environment_id, &name)
}

// ---- 手で作ったリクエストと入力値 ----

#[tauri::command]
#[specta::specta]
pub fn adhoc_list(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<AdHocRequest>, AppError> {
    state.adhoc_list(&project_id)
}

/// 保存する。`id` が空なら新しく作る。保存した内容を返す。
#[tauri::command]
#[specta::specta]
pub fn adhoc_save(
    state: State<'_, AppState>,
    request: AdHocRequest,
) -> Result<AdHocRequest, AppError> {
    state.adhoc_save(&request)
}

#[tauri::command]
#[specta::specta]
pub fn adhoc_delete(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    state.adhoc_delete(&id)
}

#[tauri::command]
#[specta::specta]
pub fn draft_save(
    state: State<'_, AppState>,
    draft: RequestDraft,
) -> Result<RequestDraft, AppError> {
    state.draft_save(&draft)
}

#[tauri::command]
#[specta::specta]
pub fn draft_get(
    state: State<'_, AppState>,
    project_id: String,
    op_key: OpKey,
) -> Result<Option<RequestDraft>, AppError> {
    state.draft_get(&project_id, &op_key)
}

// ---- OpenAPI の取り込みと同期（M3・M4） ----

/// 取り込みの確認画面のデータ（何も保存しない）。`projectId` があれば、そのプロジェクトとの差分も付く。
/// 入口の候補が複数・別ホストの確認待ち・足りないファイル・壊れた yaml は、エラーではなく `status` で返る。
#[tauri::command]
#[specta::specta]
pub async fn openapi_preview(
    state: State<'_, AppState>,
    source: SourceInput,
    project_id: Option<String>,
) -> Result<PreviewResult, AppError> {
    state.openapi_preview(source, project_id).await
}

/// 取り込む（新しいプロジェクト、または既存プロジェクトへの差分の反映）。選んだ Operation だけ取り込む。
#[tauri::command]
#[specta::specta]
pub async fn openapi_import(
    state: State<'_, AppState>,
    request: ImportRequest,
) -> Result<ImportResult, AppError> {
    state.openapi_import(request).await
}

/// 同期ボタン: yaml を読み直して、前回との差分（追加・変更・削除）を返す。何も反映しない。
/// コピー方式のときは `upload`（もう一度選んだファイル）が要る（なければ `needsUpload`）。
#[tauri::command]
#[specta::specta]
pub async fn openapi_sync_check(
    state: State<'_, AppState>,
    project_id: String,
    upload: Option<UploadInput>,
) -> Result<SyncCheckResult, AppError> {
    state.openapi_sync_check(&project_id, upload).await
}

/// 同期を反映する。入力値（RequestDraft）は消さない。
#[tauri::command]
#[specta::specta]
pub async fn openapi_sync_apply(
    state: State<'_, AppState>,
    project_id: String,
    upload: Option<UploadInput>,
) -> Result<SyncApplyResult, AppError> {
    state.openapi_sync_apply(&project_id, upload).await
}

/// 更新日時の確認（パス方式）。読み直さず、全ファイルの更新日時だけを見る。起動時とウィンドウに戻ったときに呼ぶ。
#[tauri::command]
#[specta::specta]
pub fn openapi_source_changed(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<SourceChanged, AppError> {
    state.openapi_source_changed(&project_id)
}

/// API ツリー用の一覧（取り込んだ Operation と、「削除された API」）。
#[tauri::command]
#[specta::specta]
pub fn operation_list(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<OperationListItem>, AppError> {
    state.operation_list(&project_id)
}

/// 設計タブの内容（説明・パラメータ・ボディ・レスポンス・スキーマ表・例・書かれたファイルと行番号）。
#[tauri::command]
#[specta::specta]
pub fn operation_get(
    state: State<'_, AppState>,
    project_id: String,
    op_key: OpKey,
) -> Result<OperationDetail, AppError> {
    state.operation_get(&project_id, &op_key)
}

/// 送信タブを開く。入力値があればそれを、なければ example・default から初期値を作る
/// （パラメータは無効の状態で並ぶ）。
#[tauri::command]
#[specta::specta]
pub fn operation_open(
    state: State<'_, AppState>,
    project_id: String,
    op_key: OpKey,
) -> Result<OperationOpen, AppError> {
    state.operation_open(&project_id, &op_key)
}

/// 「削除された API」を自分で消す（入力値も消える）。
#[tauri::command]
#[specta::specta]
pub fn operation_forget(
    state: State<'_, AppState>,
    project_id: String,
    op_key: OpKey,
) -> Result<(), AppError> {
    state.operation_forget(&project_id, &op_key)
}

/// URL 方式のアクセストークンをキーチェーンに保存する（空なら消す）。更新後のプロジェクトを返す。
#[tauri::command]
#[specta::specta]
pub fn source_token_set(
    state: State<'_, AppState>,
    project_id: String,
    token: String,
) -> Result<Project, AppError> {
    state.source_token_set(&project_id, &token)
}

/// 取り込んだ yaml を OS の既定のアプリで開く。`file` は `Operation.sourceFile`（入口からの相対パス）。
/// 行番号は既定のアプリに渡せないので、いまは使わない（OS の既定のアプリが開くだけ）。
#[tauri::command]
#[specta::specta]
pub fn editor_open(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    file: String,
    line: Option<u32>,
) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let _ = line;
    let path = state.editor_target(&project_id, &file)?;
    app.opener()
        .open_path(path.display().to_string(), None::<&str>)
        .map_err(|e| AppError::Io(format!("ファイルを開けませんでした: {e}")))
}
