//! NekoReq の app（Tauri の起動とコマンド）。
//!
//! 通信・解析・保存はすべて Rust 側で行い、画面は Tauri のコマンドで呼ぶだけ。
//! 画面との型の契約は tauri-specta が `src/bindings.ts` に生成する（手で書き換えない）。

pub mod commands;
pub mod error;
pub mod openapi;
pub mod send;
pub mod state;

use specta_typescript::Typescript;
use tauri::Manager;
use tauri_specta::{collect_commands, collect_events, Builder};

use crate::state::AppState;

/// 生成する TypeScript の出力先（リポジトリルートの `src/bindings.ts`）。
pub const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/bindings.ts");

/// コマンドと型の登録。起動時と `export_bindings` テストの両方がこれを使う。
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::app_info,
            commands::project_create,
            commands::project_list,
            commands::project_rename,
            commands::project_delete,
            commands::request_send,
            commands::request_cancel,
            commands::request_preview,
            commands::response_save_body,
            commands::env_list,
            commands::env_save,
            commands::env_create,
            commands::env_duplicate,
            commands::env_delete,
            commands::env_select,
            commands::secret_set,
            commands::secret_delete,
            commands::secret_reveal,
            commands::adhoc_list,
            commands::adhoc_save,
            commands::adhoc_delete,
            commands::draft_save,
            commands::draft_get,
            commands::openapi_preview,
            commands::openapi_import,
            commands::openapi_sync_check,
            commands::openapi_sync_apply,
            commands::openapi_source_changed,
            commands::operation_list,
            commands::operation_get,
            commands::operation_open,
            commands::operation_forget,
            commands::source_token_set,
            commands::editor_open,
        ])
        .events(collect_events![send::RequestFinished])
        // コマンドから直接は参照されないモデルの型も TypeScript に出しておく（M3 以降で使う）
        .typ::<error::AppError>()
        .typ::<nekoreq_core::Operation>()
        .typ::<nekoreq_openapi::DefaultAuth>()
        .typ::<nekoreq_core::HistoryEntry>()
}

/// TypeScript の型とコマンドを `path` に書き出す。
pub fn export_bindings(
    path: impl AsRef<std::path::Path>,
) -> Result<(), specta_typescript::ExportError> {
    // 生成物は手で直さないので、型チェックの対象から外す（未使用の宣言が警告になるため）
    specta_builder().export(Typescript::default().header("// @ts-nocheck"), path)
}

pub fn run() {
    let builder = specta_builder();

    // debug ビルドで起動したときは最新の型を書き出す。
    // GUI なしで生成するときは `cargo test -p nekoreq export_bindings`。
    #[cfg(debug_assertions)]
    export_bindings(BINDINGS_PATH).expect("src/bindings.ts を書き出せませんでした");

    tauri::Builder::default()
        // 保存ダイアログ（レスポンスのボディをファイルに保存する）を画面から開く
        .plugin(tauri_plugin_dialog::init())
        // yaml をエディタ（OS の既定のアプリ）で開く。呼ぶのは Rust 側（editor_open）だけで、画面には権限を渡さない
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let data_dir = app.path().app_data_dir()?;
            let state = AppState::open(&data_dir)?;
            app.manage(state);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("NekoReq の起動に失敗しました");
}

#[cfg(test)]
mod tests {
    /// GUI なしで `src/bindings.ts` を生成する。
    /// `cargo test -p nekoreq export_bindings` で実行する（コマンドや型を変えたら実行し直す）。
    #[test]
    fn export_bindings() {
        super::export_bindings(super::BINDINGS_PATH).expect("bindings を書き出せません");
        let text = std::fs::read_to_string(super::BINDINGS_PATH).unwrap();
        for name in [
            "projectCreate",
            "projectList",
            "projectRename",
            "projectDelete",
            "appInfo",
            "requestSend",
            "requestCancel",
            "requestPreview",
            "responseSaveBody",
            "envList",
            "envSave",
            "envCreate",
            "envDuplicate",
            "envDelete",
            "envSelect",
            "secretSet",
            "secretDelete",
            "secretReveal",
            "adhocList",
            "adhocSave",
            "adhocDelete",
            "draftSave",
            "draftGet",
            "openapiPreview",
            "openapiImport",
            "openapiSyncCheck",
            "openapiSyncApply",
            "openapiSourceChanged",
            "operationList",
            "operationGet",
            "operationOpen",
            "operationForget",
            "sourceTokenSet",
            "editorOpen",
        ] {
            assert!(text.contains(name), "bindings に {name} がない");
        }
        assert!(text.contains("export type AppError"));
        assert!(text.contains("export type Project "));
        // イベント（設計書の名前）
        assert!(text.contains("requestFinished"));
        assert!(text.contains("request://finished"));
        assert!(text.contains("export type RequestFinished"));
    }
}
