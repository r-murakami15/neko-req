# ビルドと配布（M7）

[目次に戻る](README.md)

タグ（`v0.1.0` など）を push すると、GitHub Actions が macOS 用の .dmg と Windows 用の .msi をビルドし、GitHub Releases の下書きに置く。インストール済みのアプリは、起動時にリリースの `latest.json` を確認し、新しい版があれば知らせる。OS のコード署名と公証はしない（[設計書](01-design.md)の決定事項）。

## 成果物

| OS | ファイル | 内容 |
| --- | --- | --- |
| macOS | `NekoReq_<版>_universal.dmg` | Apple Silicon と Intel の両方で動く（最低 macOS 10.15）。ad-hoc 署名つき |
| macOS | `NekoReq_universal.app.tar.gz` と `.sig` | 自動更新用（人が使うものではない） |
| Windows | `NekoReq_<版>_x64_ja-JP.msi` と `.msi.sig` | インストーラー。`.sig` は自動更新用 |
| 共通 | `latest.json` | 自動更新が読む「最新の版・各 OS のダウンロード先・署名」 |

ファイル名の細かい形は tauri-action が決める。

## ローカルで Mac 用をビルドする

ホストに Rust は要らない。Docker の workspace コンテナ（Zig + cargo-zigbuild）でクロスビルドし、`.app` の組み立てと署名はホストの `codesign` / `hdiutil` / `lipo` で行う。

```bash
scripts/build-mac.sh                # このMacのCPU向け → dist-mac/NekoReq.app
scripts/build-mac.sh --dmg          # dist-mac/NekoReq_<版>_<arch>.dmg も作る
scripts/build-mac.sh --universal    # Apple Silicon + Intel のユニバーサル
open dist-mac/NekoReq.app
```

- 使う compose ファイルは `-f` で変える（複数可）。環境変数 `NEKOREQ_COMPOSE_FILES="compose.yaml other.yaml"` でも指定できる。既定は `compose.yaml`。
- macOS の SDK は、ホストの Command Line Tools（`xcode-select --install`）から `macos-sdk` ボリュームへ初回だけコピーする。`scripts/patch-sdk-tbd.sh` が、新しい SDK の `.tbd` を Zig のリンカが読める形に直す。
- 最低 macOS は `src-tauri/tauri.conf.json` の `bundle.macOS.minimumSystemVersion`（10.15）。`Info.plist` の `LSMinimumSystemVersion` と、ビルドの `MACOSX_DEPLOYMENT_TARGET`（Dockerfile の既定値も 10.15）に使われる。
- **ただし、このスクリプトで作った実行ファイルが実際に要求する macOS は 11.7.1**（Zig 0.13 の既定。cargo-zigbuild は `MACOSX_DEPLOYMENT_TARGET` を反映しない。`vtool -show-build` で確認できる）。10.15〜11.6 の Intel Mac で動かす確認には使えない。CI（macos-latest の Xcode）のビルドは `MACOSX_DEPLOYMENT_TARGET` どおり 10.15 になる。
- これは手元で動かして確かめる用。署名つきの更新ファイル（`.app.tar.gz` と `.sig`）や Windows の .msi は作れない。配布するものは CI で作る。
- Windows 版は CI（windows-latest）でしかビルドできない。

## リリースの手順（CI）

1. `src-tauri/tauri.conf.json` の `version`、ルートの `Cargo.toml` の `[workspace.package] version`、`package.json` の `version` を揃えて上げ、コミットして push する。
2. タグを打って push する。

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

3. Actions の「Release」が、macOS（`--target universal-apple-darwin`）と Windows を並行してビルドし、`latest.json` と署名つきの更新ファイルを添えて、リリースを**下書き**で作る。タグの版と `tauri.conf.json` の `version` が違うと、最初のステップで失敗する。
4. GitHub のリリース画面で下書きの内容（ファイル一式・リリースノート）を確かめ、「Publish release」を押す。
5. 公開された時点から、`https://github.com/<OWNER>/<REPO>/releases/latest/download/latest.json` が新しい版を返し、インストール済みのアプリが更新に気づく。**下書きのままでは更新は配られない**（下書きは `latest` に数えられない）。

CI の ci.yml（PR と main への push）は、画面の型チェック・テスト・ビルドと、`cargo fmt` / `clippy` / `test` と、`src/bindings.ts` が最新かの確認を行う。

## 自動更新の鍵と Secrets

自動更新の更新ファイルは、OS の署名とは別の、Tauri 専用の鍵で署名する。アプリは公開鍵で署名を確かめ、合わないファイルは入れ替えない。この鍵は省けない。

### 鍵を作る（最初の 1 回だけ）

workspace コンテナで作る。**秘密鍵はリポジトリに入れない**（`.tauri-keys/` と `*.key` は `.gitignore` 済み）。

```bash
docker compose exec workspace bash -c 'mkdir -p .tauri-keys && npx tauri signer generate -w .tauri-keys/nekoreq-updater.key'
```

パスワードを聞かれるので決める（空でもよいが、決めておくのを勧める）。次の 2 つができる。

| ファイル | 中身 | 置き場所 |
| --- | --- | --- |
| `.tauri-keys/nekoreq-updater.key` | 秘密鍵 | パスワードマネージャーなどに保管し、リポジトリのフォルダからは消す。**失くすと、すでに配った版を更新できなくなる** |
| `.tauri-keys/nekoreq-updater.key.pub` | 公開鍵 | `tauri.conf.json` に書く（次の節） |

### GitHub に登録するもの

リポジトリの Settings › Secrets and variables › Actions に次の 2 つを登録する。

| 名前 | 値 |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | 秘密鍵ファイルの中身（ファイルを開いて全文を貼る） |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 鍵のパスワード（パスワードなしで作ったなら空の Secret を作る） |

`GITHUB_TOKEN` は Actions が自動で渡すので登録は要らない。

## 置き換える値

自動更新の設定は `src-tauri/tauri.conf.json` の `plugins.updater` にある。2026-11-18 に次のとおり設定した（v0.1.0 から有効）。

| 場所 | 値 |
| --- | --- |
| `plugins.updater.endpoints[0]` | `https://github.com/r-murakami15/neko-req/releases/latest/download/latest.json` |
| `plugins.updater.pubkey` | `nekoreq-updater.key.pub` の中身（1 行の文字列） |

鍵を作り直したときは `pubkey` を書き換え、Secrets も入れ替える。秘密鍵をなくすと、すでに配った版へ次の更新を配れなくなる（新しい鍵で署名した更新は、古い公開鍵を持つ版では検証に失敗する）。秘密鍵とパスワードは、リポジトリの外（パスワード管理ソフトなど）に必ず控えておく。

仮の値のままでもアプリは起動する。起動時の確認が失敗するだけで、画面には何も出さず、標準エラーに `[updater] ...` と記録する。置き換えたら、**その設定を入れた版を最初に配る**必要がある（それより前の版は、更新先を知らないので自動では更新されない）。

リポジトリ（または少なくともそのリリース）は**公開**にしておく。アプリは認証なしで `latest.json` と更新ファイルを取りに行くため、非公開リポジトリでは確認が失敗する。

## 自動更新の流れ

1. 起動の数秒後に 1 回だけ、`latest.json` を確認する（開発中の debug ビルドでは確認しない）。
2. 現在の版より新しい版があれば、Rust が `update://available`（版・リリースノート）を画面に届ける。画面が取りこぼしたときは `update_pending` で取り出せる。確認に失敗しても画面には出さない。
3. 画面は、新しい版があることを知らせる（M6 の設定画面の「アップデートを確認」ボタンは `update_check` を呼ぶ）。**ユーザーが「入れ替える」を選ぶまで入れ替えない。**
4. `update_install` がダウンロードして署名を確かめ、入れ替える。進捗は `update://progress`（`downloading` › `installing` › `installed`）。
5. 入れ替えたあと、`update_restart` で再起動すると新しい版になる。再起動のタイミングは画面が決める（送信中のリクエストや編集中の内容を失わないため）。Windows は入れ替えの途中でインストーラーが前面に出る（`installMode: passive`）。

外へ出る通信は、自分が送るリクエストと、この確認・ダウンロードだけ（設計書の非機能要件）。画面（WebView）から updater のプラグインを直接呼ぶ権限は渡していない（`capabilities/default.json` に `updater:*` はない）。

画面から使うコマンドとイベントは `src/bindings.ts`（`updateCheck` / `updatePending` / `updateInstall` / `updateRestart`、`events.updateAvailable` / `events.updateProgress`）。

## 初回起動

OS のコード署名をしていないので、初回だけ一手間かかる。

### macOS

1. `.dmg` を開き、`NekoReq.app` を「アプリケーション」へドラッグする。
2. アプリを開くと「開けません」と出るので閉じる。
3. 「システム設定」›「プライバシーとセキュリティ」を開き、下のほうの「このまま開く」を押して認証する（macOS 15 以降は、右クリックの「開く」では許可できない）。
4. ターミナルに慣れているなら、手順 2〜3 の代わりに `xattr -dr com.apple.quarantine /Applications/NekoReq.app` でもよい。

自動更新で入れ替わった版は、ダウンロードの隔離属性がつかないので、更新のたびにこの許可は要らない。（初回の `.dmg` だけ）

ad-hoc 署名は版ごとに変わるため、初回と更新後の最初の起動で、キーチェーン（環境変数のシークレットの保存先）へのアクセス確認が出ることがある。「常に許可」を押す。

### Windows

1. `.msi` を実行する。
2. 「WindowsによってPCが保護されました」（SmartScreen）と出たら、「詳細情報」›「実行」を押す。

## ユーザーが手でやること

1. GitHub にリポジトリを作る（公開）。済: r-murakami15/neko-req。
2. updater の鍵を作り、公開鍵を `pubkey` に書く。済（2026-11-18）。秘密鍵とパスワードを GitHub の Secrets に登録し、リポジトリの外に控える。
3. 版を上げてタグを push し、Actions が作った下書きを確かめて公開する。
4. 別の Mac と Windows に入れ、初回起動の許可を通して動くことを確かめる。次の版を公開して、起動時に更新されることを確かめる。
