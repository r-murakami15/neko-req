//! macOS のアプリのメニュー。
//!
//! Tauri の既定のメニューには「Close Window（Cmd+W）」などがあり、メニューが先にキーを取ると、画面のショートカット
//! （タブを閉じる Cmd+W・設定を開く Cmd+,）に届かない。そこでメニューを明示的に作り、Cmd+W と Cmd+, を割り当てない。
//! 残すのはアプリメニュー（NekoReq について・隠す・終了 Cmd+Q）と編集メニュー（取り消し・やり直し・切り取り・コピー・
//! 貼り付け・すべて選択。テキスト欄でこれらのキーが効くのに必要）。
//! Windows・Linux はメニューバーを出さない（`run` が macOS のときだけ使う）。

use tauri::menu::{Menu, PredefinedMenuItem, Submenu};
use tauri::{Manager, Runtime};

/// macOS のアプリのメニューを組み立てる。
pub fn app_menu<R: Runtime, M: Manager<R>>(app: &M) -> tauri::Result<Menu<R>> {
    let app_submenu = Submenu::with_items(
        app,
        "NekoReq",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("NekoReq について"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some("NekoReq を隠す"))?,
            &PredefinedMenuItem::hide_others(app, Some("ほかを隠す"))?,
            &PredefinedMenuItem::show_all(app, Some("すべてを表示"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some("NekoReq を終了"))?,
        ],
    )?;
    let edit_submenu = Submenu::with_items(
        app,
        "編集",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("取り消す"))?,
            &PredefinedMenuItem::redo(app, Some("やり直す"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("切り取り"))?,
            &PredefinedMenuItem::copy(app, Some("コピー"))?,
            &PredefinedMenuItem::paste(app, Some("貼り付け"))?,
            &PredefinedMenuItem::select_all(app, Some("すべてを選択"))?,
        ],
    )?;
    Menu::with_items(app, &[&app_submenu, &edit_submenu])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::menu::MenuItemKind;
    use tauri::test::{mock_builder, mock_context, noop_assets};

    fn texts(items: Vec<MenuItemKind<tauri::test::MockRuntime>>) -> Vec<String> {
        items
            .iter()
            .filter_map(|i| match i {
                MenuItemKind::Predefined(p) => p.text().ok(),
                MenuItemKind::MenuItem(m) => m.text().ok(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn menu_has_app_and_edit_only_and_no_close_window_or_settings() {
        let app = mock_builder().build(mock_context(noop_assets())).unwrap();
        let menu = app_menu(app.handle()).unwrap();
        let top = menu.items().unwrap();
        let names: Vec<String> = top
            .iter()
            .filter_map(|i| i.as_submenu().and_then(|s| s.text().ok()))
            .collect();
        assert_eq!(names, ["NekoReq", "編集"]);

        let mut all: Vec<String> = Vec::new();
        for item in &top {
            if let Some(sub) = item.as_submenu() {
                all.extend(texts(sub.items().unwrap()));
            }
        }
        // 残すもの
        for want in [
            "NekoReq について",
            "NekoReq を終了",
            "NekoReq を隠す",
            "コピー",
            "貼り付け",
            "すべてを選択",
            "取り消す",
        ] {
            assert!(all.iter().any(|t| t == want), "{want} がない: {all:?}");
        }
        // Cmd+W（Close Window）・Cmd+,（設定）に当たるものは置かない
        for t in &all {
            let lower = t.to_lowercase();
            assert!(
                !lower.contains("close")
                    && !t.contains("閉じる")
                    && !lower.contains("settings")
                    && !t.contains("設定")
                    && !lower.contains("preferences"),
                "{t}"
            );
        }
    }
}
