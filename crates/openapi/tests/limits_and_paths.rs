//! 取り込みの安全性（M3）。テスト担当が独立に書いた観点:
//! - yaml の爆弾（billion laughs）・深い入れ子・巨大ファイルで暴走しないか（子プロセスでメモリと時間に上限を掛けて確かめる）
//! - `$ref` が取り込んだフォルダの外・絶対パス・file:// を指すとき
//! - コピー方式（アップロード）の zip slip・シンボリックリンク・上限
//! - URL 方式のリダイレクトでトークンが別ホストに出ないこと

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{blocked, load_path};
use nekoreq_openapi::upload;
use nekoreq_openapi::*;

// ---- 子プロセスで上限を掛けて走らせる道具 ----

/// 子プロセスで走らせているときは true（本体の処理をする）。
fn in_child() -> bool {
    std::env::var("NEKOREQ_TEST_CHILD").is_ok()
}

/// 同じテスト関数を子プロセスで走らせ、仮想メモリ `mem_mb` と `secs` 秒の上限を掛ける。
/// 子が上限を超えて落ちる（abort・SIGSEGV）か、時間切れなら Err。
fn run_in_child(test_name: &str, mem_mb: u64, secs: u64) -> Result<Duration, String> {
    let exe = std::env::current_exe().unwrap();
    let started = Instant::now();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "ulimit -v {}; exec \"$0\" --exact {test_name} --nocapture --test-threads=1",
            mem_mb * 1024
        ))
        .arg(&exe)
        .env("NEKOREQ_TEST_CHILD", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                use std::io::Read;
                let _ = e.read_to_string(&mut err);
            }
            if status.success() {
                return Ok(started.elapsed());
            }
            let tail: String = err
                .chars()
                .rev()
                .take(600)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return Err(format!("子プロセスが異常終了: {status:?}\n{tail}"));
        }
        if started.elapsed() > Duration::from_secs(secs) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{secs} 秒で終わらなかった（暴走）"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn entry_with(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("openapi.yaml");
    fs::write(&p, body).unwrap();
    p
}

const HEAD: &str = "openapi: 3.0.3\ninfo: {title: T, version: '1'}\n";

/// 結果が「読めた」か「問題つきで止まった」かのどちらかで、パニックも暴走もしないこと。
async fn finishes(p: &Path) -> bool {
    let t = Instant::now();
    let r = load_path(p).await;
    let (ok, what) = match &r {
        LoadResult::Loaded(l) => (true, format!("Loaded {} ops", l.operations.len())),
        LoadResult::Blocked(pr) => (
            false,
            format!(
                "Blocked {}",
                pr.iter()
                    .map(|x| x.describe())
                    .collect::<Vec<_>>()
                    .join(" | ")
                    .chars()
                    .take(200)
                    .collect::<String>()
            ),
        ),
        LoadResult::NeedsConfirmation(_) => (false, "NeedsConfirmation".into()),
    };
    eprintln!("RESULT {what} in {:?}", t.elapsed());
    ok
}

// ---- yaml の爆弾 ----

#[tokio::test(flavor = "current_thread")]
async fn billion_laughs_does_not_exhaust_memory_or_time() {
    if !in_child() {
        match run_in_child("billion_laughs_does_not_exhaust_memory_or_time", 2048, 20) {
            Ok(t) => eprintln!("billion laughs: {t:?}"),
            Err(e) => panic!("{e}"),
        }
        return;
    }
    let mut y = String::from(HEAD);
    y.push_str("x-bomb:\n");
    y.push_str(
        "  a: &a [\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\"]\n",
    );
    let names = ["b", "c", "d", "e", "f", "g", "h", "i"];
    let mut prev = "a";
    for n in names {
        let refs = vec![format!("*{prev}"); 9].join(",");
        y.push_str(&format!("  {n}: &{n} [{refs}]\n"));
        prev = n;
    }
    y.push_str("paths: {}\n");
    let dir = tempfile::tempdir().unwrap();
    let p = entry_with(dir.path(), &y);
    // 9^9 個の要素になる爆弾は、読まずに止める（メモリ・時間は子プロセスの上限でも判定）
    assert!(!finishes(&p).await, "爆弾を読み込んでしまった");
}

#[tokio::test(flavor = "current_thread")]
async fn deeply_nested_flow_sequence_does_not_overflow_the_stack() {
    if !in_child() {
        match run_in_child(
            "deeply_nested_flow_sequence_does_not_overflow_the_stack",
            2048,
            20,
        ) {
            Ok(_) => {}
            Err(e) => panic!("{e}"),
        }
        return;
    }
    let depth = 200_000;
    let y = format!(
        "{HEAD}x-deep: {}{}\npaths: {{}}\n",
        "[".repeat(depth),
        "]".repeat(depth)
    );
    let dir = tempfile::tempdir().unwrap();
    let p = entry_with(dir.path(), &y);
    let _ = finishes(&p).await;
}

#[tokio::test(flavor = "current_thread")]
async fn deeply_nested_block_mapping_does_not_overflow_the_stack() {
    if !in_child() {
        match run_in_child(
            "deeply_nested_block_mapping_does_not_overflow_the_stack",
            2048,
            30,
        ) {
            Ok(_) => {}
            Err(e) => panic!("{e}"),
        }
        return;
    }
    let depth = 3_000;
    let mut y = String::from(HEAD);
    y.push_str("x-deep:\n");
    for i in 0..depth {
        y.push_str(&" ".repeat(i + 1));
        y.push_str("k:\n");
    }
    y.push_str(&" ".repeat(depth + 1));
    y.push_str("v: 1\npaths: {}\n");
    let dir = tempfile::tempdir().unwrap();
    let p = entry_with(dir.path(), &y);
    let _ = finishes(&p).await;
}

#[tokio::test(flavor = "current_thread")]
async fn deeply_nested_json_does_not_overflow_the_stack() {
    if !in_child() {
        match run_in_child("deeply_nested_json_does_not_overflow_the_stack", 2048, 20) {
            Ok(_) => {}
            Err(e) => panic!("{e}"),
        }
        return;
    }
    let depth = 100_000;
    let body = format!(
        "{{\"openapi\":\"3.0.3\",\"info\":{{\"title\":\"T\",\"version\":\"1\"}},\"x\":{}{},\"paths\":{{}}}}",
        "[".repeat(depth),
        "]".repeat(depth)
    );
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("openapi.json");
    fs::write(&p, body).unwrap();
    let _ = finishes(&p).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_schema_ref_chain_that_fans_out_is_bounded() {
    // S0 -> (a, b) -> S1 -> (a, b) -> S2 ... 展開すると 2^n。循環ではないので「2 回目で止める」では止まらない
    if !in_child() {
        match run_in_child("a_schema_ref_chain_that_fans_out_is_bounded", 2048, 30) {
            Ok(t) => eprintln!("fan-out refs: {t:?}"),
            Err(e) => panic!("{e}"),
        }
        return;
    }
    let n = 22;
    let mut y = String::from(HEAD);
    y.push_str("paths:\n  /a:\n    get:\n      operationId: getA\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                $ref: '#/components/schemas/S0'\ncomponents:\n  schemas:\n");
    for i in 0..n {
        y.push_str(&format!(
            "    S{i}:\n      type: object\n      properties:\n        a: {{$ref: '#/components/schemas/S{}'}}\n        b: {{$ref: '#/components/schemas/S{}'}}\n",
            i + 1,
            i + 1
        ));
    }
    y.push_str(&format!("    S{n}:\n      type: string\n"));
    let dir = tempfile::tempdir().unwrap();
    let p = entry_with(dir.path(), &y);
    let _ = finishes(&p).await;
}

#[tokio::test]
async fn a_file_over_the_size_limit_is_refused_without_parsing() {
    let dir = tempfile::tempdir().unwrap();
    let mut body = String::from(HEAD);
    body.push_str("paths: {}\nx-pad: ");
    body.push_str(&"a".repeat(33 * 1024 * 1024));
    body.push('\n');
    let p = entry_with(dir.path(), &body);
    let t = Instant::now();
    let problems = blocked(&p).await;
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert!(
        matches!(&problems[0], LoadProblem::Unreadable { reason, .. } if reason.contains("大きすぎ")),
        "{problems:#?}"
    );
}

#[tokio::test]
async fn a_referenced_file_over_the_limit_is_listed_with_its_referencing_line() {
    let dir = tempfile::tempdir().unwrap();
    let mut body = String::from("paths: {}\nx-pad: ");
    body.push_str(&"a".repeat(33 * 1024 * 1024));
    fs::write(dir.path().join("big.yaml"), body).unwrap();
    let p = entry_with(
        dir.path(),
        &format!("{HEAD}paths:\n  /a:\n    $ref: './big.yaml#/paths'\n"),
    );
    let problems = blocked(&p).await;
    let LoadProblem::MissingFile {
        file,
        referenced_from,
        ..
    } = &problems[0]
    else {
        panic!("{problems:#?}")
    };
    assert_eq!(file, "big.yaml");
    assert_eq!(referenced_from[0].line, Some(5));
}

#[tokio::test(flavor = "current_thread")]
async fn thousands_of_referenced_files_stop_at_the_file_count_limit() {
    // 3000 個の別ファイルを参照する入口。上限（2000）で止まり、暴走しない
    let dir = tempfile::tempdir().unwrap();
    let mut y = String::from(HEAD);
    y.push_str("paths:\n");
    for i in 0..3000 {
        fs::write(
            dir.path().join(format!("f{i}.yaml")),
            format!("get:\n  operationId: op{i}\n  responses:\n    '200': {{description: ok}}\n"),
        )
        .unwrap();
        y.push_str(&format!("  /p{i}:\n    $ref: './f{i}.yaml'\n"));
    }
    let p = entry_with(dir.path(), &y);
    let t = Instant::now();
    let problems = blocked(&p).await;
    assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
    assert!(
        problems.iter().any(|p| p.describe().contains("2000")),
        "{problems:#?}"
    );
}

// ---- 取り込んだフォルダの外を指す $ref ----

/// `inside/openapi.yaml` が `ref_target` を参照する構成を作り、読んだ結果を返す。
async fn load_with_ref(ref_target: &str) -> (tempfile::TempDir, LoadResult) {
    let root = tempfile::tempdir().unwrap();
    let inside = root.path().join("inside");
    fs::create_dir_all(&inside).unwrap();
    fs::write(
        root.path().join("secret.yaml"),
        "get:\n  operationId: stolen\n  summary: TOP-SECRET-SUMMARY\n  responses:\n    '200': {description: ok}\n",
    )
    .unwrap();
    let p = entry_with(
        &inside,
        &format!("{HEAD}paths:\n  /a:\n    $ref: '{ref_target}'\n"),
    );
    let r = load_path(&p).await;
    (root, r)
}

#[tokio::test]
async fn path_entry_follows_refs_outside_the_folder_by_design_and_shows_a_relative_name() {
    // パス方式はユーザーが自分のファイルを指す。外へ出る相対参照は読める（仕様）。表示は入口からの相対
    let (_root, r) = load_with_ref("../secret.yaml").await;
    let LoadResult::Loaded(l) = r else { panic!() };
    assert_eq!(l.operations.len(), 1);
    assert_eq!(
        l.operations[0].source_file.as_deref(),
        Some("../secret.yaml")
    );
}

#[tokio::test]
async fn absolute_and_file_scheme_refs_are_read_from_the_local_filesystem_in_path_mode() {
    let (root, r) = load_with_ref("../secret.yaml").await;
    drop(r);
    let abs = root.path().join("secret.yaml");
    for target in [
        abs.display().to_string(),
        format!("file://{}", abs.display()),
    ] {
        let (root2, _) = load_with_ref("../secret.yaml").await;
        let abs2 = root2.path().join("secret.yaml");
        let t = target.replace(&abs.display().to_string(), &abs2.display().to_string());
        let inside = root2.path().join("inside");
        let p = entry_with(&inside, &format!("{HEAD}paths:\n  /a:\n    $ref: '{t}'\n"));
        let LoadResult::Loaded(l) = load_path(&p).await else {
            panic!("{t}")
        };
        assert_eq!(l.operations.len(), 1, "{t}");
    }
}

#[tokio::test]
async fn refs_to_non_yaml_files_report_a_yaml_error_without_echoing_file_contents() {
    // /etc/passwd のような yaml でないファイルを参照しても、中身をエラー文に載せない
    let root = tempfile::tempdir().unwrap();
    let secret = root.path().join("passwd.txt");
    fs::write(
        &secret,
        "root:x:0:0:SUPERSECRETLINE:/root:/bin/bash\n  : : bad [\n",
    )
    .unwrap();
    let p = entry_with(
        root.path(),
        &format!("{HEAD}paths:\n  /a:\n    $ref: './passwd.txt#/x'\n"),
    );
    let problems = blocked(&p).await;
    for pr in &problems {
        assert!(
            !pr.describe().contains("SUPERSECRETLINE"),
            "ファイルの中身がエラー文に出ている: {}",
            pr.describe()
        );
    }
}

// ---- アップロード（コピー方式） ----

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    use std::io::Write;
    let f = fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, body) in entries {
        z.start_file(*name, opts).unwrap();
        z.write_all(body).unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn zip_slip_variants_are_all_rejected_and_nothing_is_written_outside() {
    for name in [
        "../escape.yaml",
        "a/../../escape.yaml",
        "/abs/escape.yaml",
        "..\\escape.yaml",
        "a/../../../../escape.yaml",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let zp = tmp.path().join("evil.zip");
        write_zip(&zp, &[("ok.yaml", b"a: 1\n"), (name, b"a: 1\n")]);
        let dest = tmp.path().join("dest");
        let r = upload::stage(&[zp], &dest);
        // 拒否される、または安全な場所だけに置かれる
        let escaped: Vec<_> = walk(tmp.path())
            .into_iter()
            .filter(|p| !p.starts_with(&dest) && p.file_name().is_some_and(|n| n == "escape.yaml"))
            .collect();
        assert!(
            escaped.is_empty(),
            "{name}: dest の外に書かれた {escaped:?} ({r:?})"
        );
        if let Ok(files) = &r {
            assert!(
                files
                    .iter()
                    .all(|f| !f.starts_with("..") && !f.starts_with('/')),
                "{name}: {files:?}"
            );
        }
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(unix)]
#[test]
fn symlinks_inside_a_selected_folder_are_not_followed() {
    use std::os::unix::fs::symlink;
    let src = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.yaml"), "k: SECRET\n").unwrap();
    fs::create_dir_all(outside.path().join("dir")).unwrap();
    fs::write(outside.path().join("dir/inner.yaml"), "k: SECRET\n").unwrap();
    fs::write(src.path().join("openapi.yaml"), "openapi: 3.0.3\n").unwrap();
    symlink(
        outside.path().join("secret.yaml"),
        src.path().join("link.yaml"),
    )
    .unwrap();
    symlink(outside.path().join("dir"), src.path().join("linkdir")).unwrap();
    let dest = tempfile::tempdir().unwrap();
    let files = upload::stage(&[src.path().to_path_buf()], dest.path()).unwrap();
    assert_eq!(files, vec!["openapi.yaml"], "{files:?}");
    assert!(walk(dest.path())
        .iter()
        .all(|p| !fs::read_to_string(p).unwrap().contains("SECRET")));
}

#[cfg(unix)]
#[test]
fn a_symlink_loop_in_the_folder_does_not_hang() {
    use std::os::unix::fs::symlink;
    let src = tempfile::tempdir().unwrap();
    fs::write(src.path().join("openapi.yaml"), "openapi: 3.0.3\n").unwrap();
    symlink(src.path(), src.path().join("loop")).unwrap();
    let dest = tempfile::tempdir().unwrap();
    let files = upload::stage(&[src.path().to_path_buf()], dest.path()).unwrap();
    assert_eq!(files, vec!["openapi.yaml"]);
    // 入口の候補探索（パス方式でフォルダを指定）も止まる
    let t = Instant::now();
    let found = upload::scan_dir_for_entries(src.path());
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(found.len(), 1);
}

#[test]
fn zip_with_a_symlink_entry_does_not_create_a_symlink() {
    // unix のモードがシンボリックリンクの zip エントリ。通常のファイルとして展開されるか、無視される
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let zp = tmp.path().join("l.zip");
    {
        let f = fs::File::create(&zp).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o120777);
        z.start_file("link.yaml", opts).unwrap();
        z.write_all(b"/etc/passwd").unwrap();
        z.start_file("openapi.yaml", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(b"openapi: 3.0.3\n").unwrap();
        z.finish().unwrap();
    }
    let dest = tmp.path().join("dest");
    let _ = upload::stage(&[zp], &dest);
    for p in walk(&dest) {
        let md = fs::symlink_metadata(&p).unwrap();
        assert!(!md.file_type().is_symlink(), "{p:?} がシンボリックリンク");
    }
}

#[test]
fn upload_limits_on_file_count_are_enforced() {
    let src = tempfile::tempdir().unwrap();
    for i in 0..(upload::MAX_UPLOAD_FILES + 1) {
        fs::write(src.path().join(format!("f{i}.yaml")), "a: 1\n").unwrap();
    }
    let dest = tempfile::tempdir().unwrap();
    let r = upload::stage(&[src.path().to_path_buf()], dest.path());
    assert!(matches!(r, Err(upload::UploadError::Invalid(_))), "{r:?}");
}

#[test]
fn a_zip_bomb_is_refused_by_the_declared_size_before_anything_is_written() {
    // 300 MB の（よく縮む）yaml 1 つ。展開前に上限（256 MB）で断る
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let zp = tmp.path().join("bomb.zip");
    {
        let f = fs::File::create(&zp).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);
        z.start_file("openapi.yaml", opts).unwrap();
        let chunk = vec![b'a'; 1024 * 1024];
        z.write_all(b"# ").unwrap();
        for _ in 0..300 {
            z.write_all(&chunk).unwrap();
        }
        z.finish().unwrap();
    }
    assert!(fs::metadata(&zp).unwrap().len() < 5 * 1024 * 1024);
    let dest = tmp.path().join("dest");
    let r = upload::stage(&[zp], &dest);
    assert!(matches!(r, Err(upload::UploadError::Invalid(_))), "{r:?}");
    assert!(
        walk(&dest).is_empty(),
        "展開が始まっている: {:?}",
        walk(&dest)
    );
}

#[test]
fn many_small_zip_entries_over_the_total_limit_stop_early() {
    // 5,001 個のエントリ（ファイル数の上限）。途中で止まり、上限を超えた分は置かない
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let zp = tmp.path().join("many.zip");
    {
        let f = fs::File::create(&zp).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for i in 0..(upload::MAX_UPLOAD_FILES + 1) {
            z.start_file(format!("f{i}.yaml"), opts).unwrap();
            z.write_all(b"a: 1\n").unwrap();
        }
        z.finish().unwrap();
    }
    let dest = tmp.path().join("dest");
    let r = upload::stage(&[zp], &dest);
    assert!(matches!(r, Err(upload::UploadError::Invalid(_))), "{r:?}");
    assert!(walk(&dest).len() <= upload::MAX_UPLOAD_FILES);
}

#[tokio::test]
async fn refs_escaping_the_staged_copy_must_not_read_host_files() {
    // コピー方式: 置いたフォルダの外を指す $ref（../../ や絶対パス）で、ホストのファイルを読んではいけない。
    // コピー方式の読み込みは、置いた一時フォルダの中に閉じ込められているべき
    let host = tempfile::tempdir().unwrap();
    fs::write(
        host.path().join("host_secret.yaml"),
        "get:\n  operationId: leaked\n  summary: HOST-FILE-CONTENT\n  responses:\n    '200': {description: ok}\n",
    )
    .unwrap();
    let up = tempfile::tempdir().unwrap();
    let abs = host.path().join("host_secret.yaml");
    fs::write(
        up.path().join("openapi.yaml"),
        format!("{HEAD}paths:\n  /a:\n    $ref: '{}#/'\n", abs.display()),
    )
    .unwrap();
    let staged = tempfile::tempdir().unwrap();
    let files = upload::stage(&[up.path().to_path_buf()], staged.path()).unwrap();
    assert_eq!(files, vec!["openapi.yaml"]);
    // アプリはステージしたフォルダを LoadEntry::Path で読み、root にそのフォルダを渡す（src-tauri/src/openapi.rs の prepare）。
    // ホストの絶対パスを読めてしまうなら、zip・フォルダのアップロードで任意ファイルを読ませられる
    let entry = staged.path().join("openapi.yaml");
    let r = load(&LoadParams {
        entry: LoadEntry::Path(entry.clone()),
        token: None,
        allowed_hosts: vec![],
        root: Some(staged.path().to_path_buf()),
    })
    .await;
    match r {
        LoadResult::Loaded(l) => panic!(
            "コピー方式でフォルダの外のホストのファイルを読んでしまった: files={:?}",
            l.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        ),
        LoadResult::NeedsConfirmation(_) => panic!("確認待ちではなく足りないファイルのはず"),
        LoadResult::Blocked(problems) => {
            let LoadProblem::MissingFile {
                file,
                reason,
                referenced_from,
            } = &problems[0]
            else {
                panic!("{problems:#?}")
            };
            // 参照元の行つきで止まり、表示には一時フォルダ・ホストの実際のパスが出ない
            assert_eq!(referenced_from[0].line, Some(5), "{problems:#?}");
            assert_eq!(referenced_from[0].file, "openapi.yaml");
            let shown = format!("{problems:#?}");
            let staged_name = staged.path().file_name().unwrap().to_string_lossy();
            assert!(
                !shown.contains(staged_name.as_ref()),
                "一時フォルダの名前が出ている: {shown}"
            );
            assert!(reason.contains("外"), "{reason}");
            assert!(!file.is_empty());
        }
    }
}

/// コピー方式の root: 置いたフォルダの中の相対参照（`../` で出て戻るものを含む）は読める。外へ出る相対参照・
/// file:// は読まない。パス方式（root なし）は外のファイルも読める（別のテストで確認）。
#[tokio::test]
async fn a_root_allows_refs_inside_and_blocks_relative_and_file_url_escapes() {
    let top = tempfile::tempdir().unwrap();
    let root = top.path().join("staged");
    fs::create_dir_all(root.join("sub")).unwrap();
    let op = |id: &str| {
        format!("get:\n  operationId: {id}\n  responses:\n    '200': {{description: ok}}\n")
    };
    fs::write(root.join("sub/a.yaml"), op("a")).unwrap();
    fs::write(root.join("b.yaml"), op("b")).unwrap();
    fs::write(top.path().join("outside.yaml"), op("outside")).unwrap();
    let load_root = |entry_body: String| {
        let root = root.clone();
        async move {
            fs::write(root.join("openapi.yaml"), entry_body).unwrap();
            load(&LoadParams {
                entry: LoadEntry::Path(root.join("openapi.yaml")),
                token: None,
                allowed_hosts: vec![],
                root: Some(root.clone()),
            })
            .await
        }
    };
    // 中: sub を出入りする参照は読める
    let r = load_root(format!(
        "{HEAD}paths:\n  /a: {{$ref: './sub/a.yaml'}}\n  /b: {{$ref: './sub/../b.yaml'}}\n"
    ))
    .await;
    let LoadResult::Loaded(l) = r else {
        panic!("中の参照は読める")
    };
    assert_eq!(l.operations.len(), 2);
    // 外: ../outside.yaml・file://・絶対パス
    let abs = top.path().join("outside.yaml").display().to_string();
    for target in [
        "../outside.yaml".to_string(),
        "sub/../../outside.yaml".to_string(),
        format!("file://{abs}"),
        abs,
    ] {
        let r = load_root(format!("{HEAD}paths:\n  /x: {{$ref: '{target}'}}\n")).await;
        let LoadResult::Blocked(p) = r else {
            panic!("{target} は読めないはず")
        };
        assert!(
            matches!(&p[0], LoadProblem::MissingFile { referenced_from, .. } if referenced_from[0].line == Some(4)),
            "{target}: {p:#?}"
        );
        assert!(!format!("{p:?}").contains(root.to_str().unwrap()));
    }
}

// ---- URL 方式（wiremock） ----

mod url_mode {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const MIN_SPEC: &str = "openapi: 3.0.3\ninfo: {title: R, version: '1'}\npaths:\n  /a:\n    get:\n      operationId: getA\n      responses:\n        '200': {description: ok}\n";

    fn params(url: String, token: Option<&str>, allowed: Vec<String>) -> LoadParams {
        LoadParams {
            entry: LoadEntry::Url(url),
            token: token.map(String::from),
            allowed_hosts: allowed,
            root: None,
        }
    }

    fn describe_all(p: &[LoadProblem]) -> String {
        p.iter()
            .map(LoadProblem::describe)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn the_token_is_not_forwarded_when_the_entry_redirects_to_another_origin() {
        let main = MockServer::start().await;
        let other = MockServer::start().await; // 同じ 127.0.0.1 でポートだけ違う
        Mock::given(method("GET"))
            .and(path("/api/openapi.yaml"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/landing.yaml", other.uri())),
            )
            .mount(&main)
            .await;
        Mock::given(method("GET"))
            .and(path("/landing.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(MIN_SPEC))
            .mount(&other)
            .await;
        let r = load(&params(
            format!("{}/api/openapi.yaml", main.uri()),
            Some("TOP-SECRET-TOKEN"),
            vec![],
        ))
        .await;
        assert!(matches!(r, LoadResult::Loaded(_)));
        let reqs = other.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 1);
        assert!(
            reqs[0].headers.get("authorization").is_none(),
            "リダイレクト先にトークンが送られた"
        );
        // 入口へは送っている
        let m = main.received_requests().await.unwrap();
        assert!(m[0].headers.get("authorization").is_some());
    }

    #[tokio::test]
    async fn the_token_follows_a_redirect_inside_the_same_origin() {
        let main = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/openapi.yaml"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/v2/openapi.yaml"))
            .mount(&main)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/openapi.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(MIN_SPEC))
            .mount(&main)
            .await;
        let r = load(&params(
            format!("{}/api/openapi.yaml", main.uri()),
            Some("tok"),
            vec![],
        ))
        .await;
        assert!(matches!(r, LoadResult::Loaded(_)));
        let reqs = main.received_requests().await.unwrap();
        assert!(reqs
            .iter()
            .all(|r| r.headers.get("authorization").is_some()));
    }

    #[tokio::test]
    async fn a_redirect_loop_or_a_redirect_to_file_scheme_is_an_error_without_secrets() {
        let main = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/loop.yaml"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "/loop.yaml?token=QUERY-SECRET"),
            )
            .mount(&main)
            .await;
        Mock::given(method("GET"))
            .and(path("/file.yaml"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "file:///etc/passwd"),
            )
            .mount(&main)
            .await;
        for (p, why) in [
            ("/loop.yaml?token=QUERY-SECRET", "loop"),
            ("/file.yaml", "file"),
        ] {
            let r = load(&params(
                format!("{}{p}", main.uri()),
                Some("HEADER-SECRET"),
                vec![],
            ))
            .await;
            let LoadResult::Blocked(problems) = r else {
                panic!("{why}: 止まるはず")
            };
            let text = describe_all(&problems);
            assert!(
                !text.contains("QUERY-SECRET") && !text.contains("HEADER-SECRET"),
                "{why}: {text}"
            );
        }
    }

    #[tokio::test]
    async fn non_http_entry_schemes_are_refused_without_any_access() {
        for u in [
            "file:///etc/passwd",
            "ftp://example.com/x.yaml",
            "javascript:alert(1)",
            "data:text/yaml;base64,YTogMQ==",
            "//example.com/x.yaml",
            "gopher://x/",
        ] {
            let r = load(&params(u.into(), Some("t"), vec![])).await;
            let LoadResult::Blocked(p) = r else {
                panic!("{u}: 止まるはず")
            };
            assert!(
                matches!(&p[0], LoadProblem::Unreadable { .. }),
                "{u}: {p:?}"
            );
        }
    }

    #[tokio::test]
    async fn server_errors_and_the_displayed_names_never_contain_query_or_userinfo_secrets() {
        let main = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x/openapi.yaml"))
            .respond_with(ResponseTemplate::new(500).set_body_string("TOKEN-IN-BODY"))
            .mount(&main)
            .await;
        let host = main.uri().trim_start_matches("http://").to_string();
        let url =
            format!("http://user:PASS-SECRET@{host}/x/openapi.yaml?access_token=QUERY-SECRET#frag");
        let r = load(&params(url, Some("HEADER-SECRET"), vec![])).await;
        let LoadResult::Blocked(p) = r else { panic!() };
        let text = describe_all(&p);
        for secret in [
            "PASS-SECRET",
            "QUERY-SECRET",
            "HEADER-SECRET",
            "access_token",
        ] {
            assert!(!text.contains(secret), "{secret} が出ている: {text}");
        }
        assert!(!format!("{p:?}").contains("HEADER-SECRET"));
    }

    #[tokio::test]
    async fn loaded_file_names_hide_query_and_userinfo() {
        let main = MockServer::start().await;
        let other = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/openapi.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "openapi: 3.0.3\ninfo: {{title: R, version: '1'}}\npaths:\n  /a:\n    $ref: '{}/shared.yaml?sig=SIG-SECRET#/paths/~1a'\n",
                other.uri()
            )))
            .mount(&main)
            .await;
        Mock::given(method("GET"))
            .and(path("/shared.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "paths:\n  /a:\n    get:\n      operationId: getA\n      responses:\n        '200': {description: ok}\n",
            ))
            .mount(&other)
            .await;
        let host = other.uri();
        let url = format!("{}/api/openapi.yaml?access_token=QUERY-SECRET", main.uri());
        // 確認前の表示
        let r = load(&params(url.clone(), None, vec![])).await;
        let LoadResult::NeedsConfirmation(f) = r else {
            panic!()
        };
        assert!(!f[0].url.contains("SIG-SECRET"), "{:?}", f[0]);
        // 確認後の表示
        let r = load(&params(url, None, vec![host])).await;
        let LoadResult::Loaded(l) = r else { panic!() };
        let shown = format!("{:?} {:?}", l.files, l.operations[0].source_file);
        assert!(
            !shown.contains("SIG-SECRET") && !shown.contains("QUERY-SECRET"),
            "{shown}"
        );
        assert!(!l.entry.contains("QUERY-SECRET"), "{}", l.entry);
    }

    #[tokio::test]
    async fn tricky_reference_urls_all_need_confirmation_and_are_not_fetched() {
        let main = MockServer::start().await;
        let other = MockServer::start().await;
        let main_host = main.uri().trim_start_matches("http://").to_string();
        let other_host = other.uri().trim_start_matches("http://").to_string();
        let refs = [
            // 入口のホストを userinfo に置いた別ホスト
            format!("http://{main_host}@{other_host}/x.yaml"),
            // スキームだけ変える（http の入口に対して https の同ホスト）
            format!("https://{main_host}/x.yaml"),
            // プロトコル相対（入口と同じスキームで別ホスト）
            format!("//{other_host}/x.yaml"),
            // 大文字小文字
            format!("HTTP://{}/x.yaml", other_host.to_uppercase()),
            // localhost は 127.0.0.1 とは別のホスト
            format!(
                "http://localhost:{}/x.yaml",
                main_host.rsplit(':').next().unwrap()
            ),
        ];
        for r in refs {
            let entry = format!(
                "openapi: 3.0.3\ninfo: {{title: R, version: '1'}}\npaths:\n  /a:\n    $ref: '{r}#/p'\n"
            );
            main.reset().await;
            Mock::given(method("GET"))
                .and(path("/api/openapi.yaml"))
                .respond_with(ResponseTemplate::new(200).set_body_string(entry))
                .mount(&main)
                .await;
            let res = load(&params(
                format!("{}/api/openapi.yaml", main.uri()),
                Some("tok"),
                vec![],
            ))
            .await;
            match res {
                LoadResult::NeedsConfirmation(f) => assert_eq!(f.len(), 1, "{r}"),
                other_r => panic!(
                    "{r}: 確認待ちのはず（{}）",
                    match other_r {
                        LoadResult::Loaded(_) => "Loaded".to_string(),
                        LoadResult::Blocked(p) => describe_all(&p),
                        LoadResult::NeedsConfirmation(_) => unreachable!(),
                    }
                ),
            }
            assert!(
                other.received_requests().await.unwrap().is_empty(),
                "{r}: 確認前に別ホストへ接続した"
            );
        }
    }

    #[tokio::test]
    async fn a_url_entry_cannot_read_local_files_through_refs() {
        let main = MockServer::start().await;
        for (r, _) in [
            ("file:///etc/passwd", 0),
            ("../../../../etc/passwd", 1),
            ("/etc/passwd", 2),
        ] {
            main.reset().await;
            Mock::given(method("GET"))
                .and(path("/api/openapi.yaml"))
                .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                    "openapi: 3.0.3\ninfo: {{title: R, version: '1'}}\npaths:\n  /a:\n    $ref: '{r}#/x'\n"
                )))
                .mount(&main)
                .await;
            let res = load(&params(
                format!("{}/api/openapi.yaml", main.uri()),
                None,
                vec![],
            ))
            .await;
            // file:// は拒否。相対・絶対パスは「同じホストの URL」として取りに行く（ローカルのファイルは読まない）
            let LoadResult::Blocked(p) = res else {
                panic!("{r}")
            };
            let text = describe_all(&p);
            assert!(!text.contains("root:"), "{r}: {text}");
        }
    }

    #[tokio::test]
    async fn a_response_over_the_limit_is_refused() {
        let main = MockServer::start().await;
        let body = format!("{MIN_SPEC}x-pad: {}\n", "a".repeat(33 * 1024 * 1024));
        Mock::given(method("GET"))
            .and(path("/big.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&main)
            .await;
        let r = load(&params(format!("{}/big.yaml", main.uri()), None, vec![])).await;
        let LoadResult::Blocked(p) = r else { panic!() };
        assert!(describe_all(&p).contains("大きすぎ"), "{p:?}");
    }
}
