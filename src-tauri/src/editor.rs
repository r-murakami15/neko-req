//! yaml をエディタで開く（設計タブの「video/client.yaml:5」のクリック）。
//!
//! - 設定にコマンド（`AppSettings.editorCommand`。例 `code -g {file}:{line}`）があればそれを使う。
//! - なければ、よく使うエディタを VS Code（`code -g file:line`）・Cursor・Sublime Text・JetBrains の順に探し、
//!   見つかったもので開く。どれもなければ `NoneFound`（呼び出し側が OS の既定のアプリで開く。行番号は渡せない）。
//! - シェルを通さずに直接起動する（`Command` に引数を 1 つずつ渡す）。コマンドは空白で区切り、`"` `'` で囲める
//!   （Windows のパスの `\` はそのまま）。`{file}` は絶対パス、`{line}` は行番号（なければ 1）。`{file}` がなければ
//!   パスを最後の引数に足す。
//! - 開くファイルは `AppState::editor_target` が取り込み元のフォルダの中のものに限る。ここでは検査しない。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::AppError;

/// 起動するコマンド。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// 開いた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorLaunch {
    /// エディタを起動した（`program` は起動したコマンド）。
    Spawned { program: String },
    /// 使えるエディタが見つからなかった（OS の既定のアプリで開く）。
    NoneFound,
}

/// コマンドを空白で区切る（`"` `'` で囲んだ部分は 1 つの引数。バックスラッシュは特別扱いしない）。
pub fn split_command(template: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut has_token = false;
    let mut quote: Option<char> = None;
    for c in template.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                has_token = true;
            }
            None if c.is_whitespace() => {
                if has_token {
                    out.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            None => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if quote.is_some() {
        return Err("エディタのコマンドの引用符が閉じていません".to_string());
    }
    if has_token {
        out.push(cur);
    }
    if out.is_empty() {
        return Err("エディタのコマンドが空です".to_string());
    }
    Ok(out)
}

/// 設定のコマンドから、起動するコマンドを作る。`{file}`・`{line}` を各引数の中で置き換える
/// （パスに空白があっても引数は分かれない）。
pub fn expand_template(
    template: &str,
    file: &Path,
    line: Option<u32>,
) -> Result<EditorCommand, String> {
    let tokens = split_command(template)?;
    let file_text = file.display().to_string();
    let line_text = line.unwrap_or(1).max(1).to_string();
    let has_file = tokens.iter().any(|t| t.contains("{file}"));
    let mut expanded: Vec<String> = tokens
        .iter()
        .map(|t| {
            t.replace("{file}", &file_text)
                .replace("{line}", &line_text)
        })
        .collect();
    if !has_file {
        expanded.push(file_text);
    }
    let program = expanded.remove(0);
    Ok(EditorCommand {
        program,
        args: expanded,
    })
}

/// 自動で探すエディタ。
struct Candidate {
    /// 実行ファイルの名前（`PATH` から探す。上のものが先）。
    programs: &'static [&'static str],
    /// 引数（`{file}`・`{line}` を置き換える）。
    args: &'static [&'static str],
}

const CANDIDATES: &[Candidate] = &[
    // VS Code
    Candidate {
        programs: &["code"],
        args: &["-g", "{file}:{line}"],
    },
    // Cursor
    Candidate {
        programs: &["cursor"],
        args: &["-g", "{file}:{line}"],
    },
    // Sublime Text
    Candidate {
        programs: &["subl"],
        args: &["{file}:{line}"],
    },
    // JetBrains の IDE（Toolbox のスクリプトや `Create Command-line Launcher` で入る名前）
    Candidate {
        programs: &[
            "idea", "webstorm", "pycharm", "phpstorm", "goland", "clion", "rider", "rubymine",
        ],
        args: &["--line", "{line}", "{file}"],
    },
];

/// 実行ファイルを探すフォルダ。`PATH` と、GUI アプリから起動したときに `PATH` が短い OS の
/// よくある置き場所（macOS の Homebrew・各エディタの `bin`、Windows の各エディタ、Linux の snap など）。
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let mut extra: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        for d in [
            "/usr/local/bin",
            "/opt/homebrew/bin",
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin",
            "/Applications/Cursor.app/Contents/Resources/app/bin",
            "/Applications/Sublime Text.app/Contents/SharedSupport/bin",
        ] {
            extra.push(PathBuf::from(d));
        }
        if let Some(home) = std::env::var_os("HOME") {
            extra.push(
                PathBuf::from(&home).join("Library/Application Support/JetBrains/Toolbox/scripts"),
            );
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            extra.push(local.join("Programs/Microsoft VS Code/bin"));
            extra.push(local.join("Programs/cursor/resources/app/bin"));
            extra.push(local.join("JetBrains/Toolbox/scripts"));
        }
        extra.push(PathBuf::from(r"C:\Program Files\Sublime Text"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for d in ["/usr/local/bin", "/usr/bin", "/snap/bin"] {
            extra.push(PathBuf::from(d));
        }
    }
    for d in extra {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// `dirs` から実行ファイル `name` を探す（Windows は `.exe`・`.cmd`・`.bat`・`.com` も試す）。
pub fn find_executable(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    for dir in dirs {
        for ext in exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

/// 実際にプロセスを起動する（待たない。標準入出力は閉じる）。
pub fn spawn_detached(command: &EditorCommand) -> std::io::Result<()> {
    let mut cmd = Command::new(OsString::from(&command.program));
    cmd.args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // コンソールの窓を出さない
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn()?;
    // 終了を待つ（ゾンビを残さない）のは別のスレッドで。エディタの起動を止めない
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// エディタで `file` の `line` 行を開く。`spawn` はプロセスの起動（テストでは差し替える）。
///
/// - `custom`（設定のコマンド）があれば、それだけを使う。起動できなければエラー
///   （設定の間違いに気づけるよう、ほかのエディタには切り替えない）。
/// - なければ `dirs` から自動で探し、起動できたものを使う。1 つも見つからなければ `NoneFound`。
pub fn launch_with(
    custom: Option<&str>,
    file: &Path,
    line: Option<u32>,
    dirs: &[PathBuf],
    spawn: &dyn Fn(&EditorCommand) -> std::io::Result<()>,
) -> Result<EditorLaunch, AppError> {
    if let Some(template) = custom.map(str::trim).filter(|t| !t.is_empty()) {
        let mut command = expand_template(template, file, line).map_err(AppError::Validation)?;
        // パスを含まない名前は、探したフォルダにあればそのパスで起動する（GUI アプリは PATH が短いため）
        if !command.program.contains(['/', '\\']) {
            if let Some(found) = find_executable(&command.program, dirs) {
                command.program = found.display().to_string();
            }
        }
        spawn(&command).map_err(|e| {
            AppError::Io(format!(
                "エディタを起動できませんでした（{}）: {e}。設定のエディタのコマンドを確かめてください",
                command.program
            ))
        })?;
        return Ok(EditorLaunch::Spawned {
            program: command.program,
        });
    }

    let file_text = file.display().to_string();
    let line_text = line.unwrap_or(1).max(1).to_string();
    for candidate in CANDIDATES {
        for name in candidate.programs {
            let Some(found) = find_executable(name, dirs) else {
                continue;
            };
            let command = EditorCommand {
                program: found.display().to_string(),
                args: candidate
                    .args
                    .iter()
                    .map(|a| {
                        a.replace("{file}", &file_text)
                            .replace("{line}", &line_text)
                    })
                    .collect(),
            };
            if spawn(&command).is_ok() {
                return Ok(EditorLaunch::Spawned {
                    program: command.program,
                });
            }
        }
    }
    Ok(EditorLaunch::NoneFound)
}

/// `launch_with` を実際の起動と `search_dirs()` で行う。
pub fn launch(
    custom: Option<&str>,
    file: &Path,
    line: Option<u32>,
) -> Result<EditorLaunch, AppError> {
    launch_with(custom, file, line, &search_dirs(), &spawn_detached)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn splits_on_spaces_and_quotes_without_a_shell() {
        assert_eq!(
            split_command("code -g {file}:{line}").unwrap(),
            vec!["code", "-g", "{file}:{line}"]
        );
        assert_eq!(
            split_command(r#""C:\Program Files\Microsoft VS Code\Code.exe" -g "{file}:{line}""#)
                .unwrap(),
            vec![
                r"C:\Program Files\Microsoft VS Code\Code.exe",
                "-g",
                "{file}:{line}"
            ]
        );
        assert_eq!(
            split_command("  subl   '{file}:{line}' ").unwrap(),
            vec!["subl", "{file}:{line}"]
        );
        // 空の引数
        assert_eq!(split_command(r#"a "" b"#).unwrap(), vec!["a", "", "b"]);
        // シェルの記号は特別扱いしない
        assert_eq!(
            split_command("code; rm -rf / && echo $HOME").unwrap(),
            vec!["code;", "rm", "-rf", "/", "&&", "echo", "$HOME"]
        );
        assert!(split_command("").is_err());
        assert!(split_command("   ").is_err());
        assert!(split_command("code \"-g").is_err());
    }

    #[test]
    fn template_replaces_file_and_line_inside_each_argument() {
        let f = Path::new("/Users/me/my api/video/client.yaml");
        let c = expand_template("code -g {file}:{line}", f, Some(42)).unwrap();
        assert_eq!(c.program, "code");
        // 空白を含むパスでも 1 つの引数のまま
        assert_eq!(c.args, vec!["-g", "/Users/me/my api/video/client.yaml:42"]);
        // 行番号がなければ 1
        let c = expand_template("code -g {file}:{line}", f, None).unwrap();
        assert_eq!(c.args[1], "/Users/me/my api/video/client.yaml:1");
        // {file} がなければ最後に足す
        let c = expand_template("myeditor --wait", f, Some(3)).unwrap();
        assert_eq!(c.args, vec!["--wait", "/Users/me/my api/video/client.yaml"]);
        // JetBrains 形式
        let c = expand_template("idea --line {line} {file}", f, Some(7)).unwrap();
        assert_eq!(
            c.args,
            vec!["--line", "7", "/Users/me/my api/video/client.yaml"]
        );
    }

    #[cfg(unix)]
    fn fake_exe(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn auto_detect_prefers_vscode_then_cursor_then_sublime_then_jetbrains() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = vec![dir.path().to_path_buf()];
        let calls: RefCell<Vec<EditorCommand>> = RefCell::new(vec![]);
        let spawn = |c: &EditorCommand| {
            calls.borrow_mut().push(c.clone());
            Ok(())
        };
        let file = Path::new("/p/a.yaml");

        // 何もなければ NoneFound
        assert_eq!(
            launch_with(None, file, Some(5), &dirs, &spawn).unwrap(),
            EditorLaunch::NoneFound
        );
        assert!(calls.borrow().is_empty());

        // JetBrains だけ
        let idea = fake_exe(dir.path(), "idea");
        launch_with(None, file, Some(5), &dirs, &spawn).unwrap();
        assert_eq!(calls.borrow()[0].program, idea.display().to_string());
        assert_eq!(calls.borrow()[0].args, vec!["--line", "5", "/p/a.yaml"]);

        // Sublime があれば Sublime
        fake_exe(dir.path(), "subl");
        calls.borrow_mut().clear();
        launch_with(None, file, Some(5), &dirs, &spawn).unwrap();
        assert!(calls.borrow()[0].program.ends_with("/subl"));
        assert_eq!(calls.borrow()[0].args, vec!["/p/a.yaml:5"]);

        // Cursor があれば Cursor
        fake_exe(dir.path(), "cursor");
        calls.borrow_mut().clear();
        launch_with(None, file, Some(5), &dirs, &spawn).unwrap();
        assert!(calls.borrow()[0].program.ends_with("/cursor"));
        assert_eq!(calls.borrow()[0].args, vec!["-g", "/p/a.yaml:5"]);

        // VS Code が最優先
        fake_exe(dir.path(), "code");
        calls.borrow_mut().clear();
        let r = launch_with(None, file, Some(5), &dirs, &spawn).unwrap();
        assert!(matches!(r, EditorLaunch::Spawned { ref program } if program.ends_with("/code")));
        assert_eq!(calls.borrow().len(), 1);
        assert_eq!(calls.borrow()[0].args, vec!["-g", "/p/a.yaml:5"]);
    }

    #[cfg(unix)]
    #[test]
    fn auto_detect_skips_an_editor_that_fails_to_start_and_does_not_pick_a_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = vec![dir.path().to_path_buf()];
        fake_exe(dir.path(), "code");
        fake_exe(dir.path(), "cursor");
        // 実行権限のないファイルは見つけない
        std::fs::write(dir.path().join("subl"), "x").unwrap();
        let calls: RefCell<Vec<String>> = RefCell::new(vec![]);
        let spawn = |c: &EditorCommand| {
            calls.borrow_mut().push(c.program.clone());
            if c.program.ends_with("/code") {
                Err(std::io::Error::other("boom"))
            } else {
                Ok(())
            }
        };
        let r = launch_with(None, Path::new("/p/a.yaml"), None, &dirs, &spawn).unwrap();
        assert!(matches!(r, EditorLaunch::Spawned { ref program } if program.ends_with("/cursor")));
        assert_eq!(calls.borrow().len(), 2);
        assert!(find_executable("subl", &dirs).is_none());
    }

    #[test]
    fn custom_command_is_used_alone_and_its_failure_is_an_error() {
        let dirs: Vec<PathBuf> = vec![];
        let calls: RefCell<Vec<EditorCommand>> = RefCell::new(vec![]);
        let ok = |c: &EditorCommand| {
            calls.borrow_mut().push(c.clone());
            Ok(())
        };
        let file = Path::new("/p/a b.yaml");
        let r = launch_with(
            Some("  myeditor --goto {file}:{line}  "),
            file,
            Some(9),
            &dirs,
            &ok,
        )
        .unwrap();
        assert_eq!(
            r,
            EditorLaunch::Spawned {
                program: "myeditor".into()
            }
        );
        assert_eq!(calls.borrow()[0].args, vec!["--goto", "/p/a b.yaml:9"]);
        // 空の設定は未設定と同じ（自動で探す）
        let r = launch_with(Some("   "), file, None, &dirs, &ok).unwrap();
        assert_eq!(r, EditorLaunch::NoneFound);
        // 起動できなければエラー（ほかのエディタには切り替えない）
        let fail = |_: &EditorCommand| Err(std::io::Error::other("no such file"));
        let e = launch_with(Some("nope {file}"), file, None, &dirs, &fail).unwrap_err();
        assert!(
            matches!(e, AppError::Io(ref m) if m.contains("nope")),
            "{e:?}"
        );
        // 設定が壊れている
        let e = launch_with(Some("code \"{file}"), file, None, &dirs, &ok).unwrap_err();
        assert!(matches!(e, AppError::Validation(_)), "{e:?}");
    }

    #[cfg(unix)]
    #[test]
    fn custom_command_name_is_resolved_from_the_search_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let exe = fake_exe(dir.path(), "myeditor");
        let calls: RefCell<Vec<EditorCommand>> = RefCell::new(vec![]);
        let spawn = |c: &EditorCommand| {
            calls.borrow_mut().push(c.clone());
            Ok(())
        };
        launch_with(
            Some("myeditor {file}"),
            Path::new("/p/a.yaml"),
            None,
            &[dir.path().to_path_buf()],
            &spawn,
        )
        .unwrap();
        assert_eq!(calls.borrow()[0].program, exe.display().to_string());
    }

    #[cfg(unix)]
    #[test]
    fn really_spawns_without_a_shell_and_keeps_special_characters_in_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("args.txt");
        let script = dir.path().join("fake-editor.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // シェルが解釈する文字（`;` `$()` `*`）を含むパスでも、そのまま 1 つの引数で届く
        let file = dir.path().join("a; touch HACKED $(echo x) *.yaml");
        std::fs::write(&file, "x").unwrap();
        let template = format!("{} --line {{line}} {{file}}", script.display());
        let r = launch(Some(&template), &file, Some(12)).unwrap();
        assert!(matches!(r, EditorLaunch::Spawned { .. }));
        let mut text = String::new();
        for _ in 0..100 {
            text = std::fs::read_to_string(&out).unwrap_or_default();
            if text.lines().count() >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines, vec!["--line", "12", file.to_str().unwrap()]);
        assert!(!dir.path().join("HACKED").exists());
    }
}
