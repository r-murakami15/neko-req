# 実装時の決定事項

[目次に戻る](README.md)

設計書（01-design.md）に書かれていない、または実装のときに決めたことを残す。

## 2026-11-17

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| バンドル ID | `com.nekoreq.desktop` | `.app` で終わる ID は macOS で問題になるため |
| Rust の版 | workspace コンテナは Rust 1.90 のまま | tauri-specta の最新 rc は Rust 1.93 以上が必要。rc.21（specta rc.22・specta-typescript 0.0.9）を `=` で固定して使う |
| グローバル変数 | プロジェクトごとに「グローバル」という特別な環境を 1 つ持つ | 画面イメージ（環境変数）の「共通 / グローバル」と同じ並び。変数の優先順は「リクエスト内の上書き › 選んでいる環境 › グローバル」 |
| macOS の最低版 | 10.15（設計書どおり）。`tauri.conf.json` の `minimumSystemVersion` に設定 | ローカルのクロスビルド用の `MACOSX_DEPLOYMENT_TARGET` は M7 で合わせる |
| 日時 | UTC の RFC 3339 文字列（ミリ秒、`Z`） | |
| ApiSource | 1 プロジェクトに 1 つ | |
| 履歴の全文検索 | FTS5 の trigram（日本語の部分一致。検索語は 3 文字以上） | |
| フォント | Inter・Noto Sans JP・JetBrains Mono を同梱（@fontsource） | CSP と「外部へ通信しない」方針のため Google Fonts は使わない |
| bindings.ts | tauri-specta の生成物。`cargo test -p nekoreq export_bindings` で生成し、コミットする。CI で生成し忘れを検出 | |
| 64 ビット整数 | 画面に渡す型では使わない（specta の BigInt 制限）。時間・サイズは `u32` / `f64` | |

## 2026-11-17 M2（送信と環境変数）

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 選んでいる環境 | `projects.default_environment_id` に保存する。最初に作った環境が自動で選ばれる。選んでいる環境を消すと、残りの最初の環境を選び直す | 新しい列を足さずに済む。「グローバル」は選べない |
| グローバル環境 | マイグレーション v2 で `environments.is_global` を足し、既存のプロジェクトにも作る。プロジェクトに 1 つ（部分ユニークインデックス）。選択・削除・複製・改名はできない | 優先順は「リクエスト内の上書き › 選んでいる環境 › グローバル」 |
| 環境変数の「有効」 | `env_vars.enabled`（既定 1）。無効の変数は展開に使わない | 画面イメージ（環境変数）に「有効」のチェックがある |
| シークレット | 値はキーチェーンだけ（サービス名 `NekoReq/<プロジェクト ID>`、アカウント名 `<環境 ID>/<変数名>`）。環境から消した・通常の値に変えた変数、環境やプロジェクトを消したときは、キーチェーンからも消す。キーチェーンは、リクエストが使っているシークレットだけを読む | 余計なアクセス（macOS の許可ダイアログ）を減らす |
| 変数展開 | 1 回だけ。値の中の `{{x}}` はさらに展開しない。値は書かれたまま差し込む（JSON の文字列のエスケープはしない）。無効の行・値のないシークレットは「ない」ものとして次の層へ | 単純で予測しやすい |
| 通信の部品 | reqwest 0.13（rustls は ring、証明書の検証は OS のストア）。送信ごとに新しい Client を作り、接続は使い回さない。プロキシは使わない | DNS・TCP・TLS の時間を毎回測るため。プロキシの設定は M6 の設定画面 |
| 時間の測り方 | DNS は独自の resolver、接続全体は `connector_layer`、TLS の開始は rustls のセッション置き場（ClientHello 作成時に呼ばれる）で測る。「リクエスト送信」（`sendMs`）は測らず常に `None`（送信の時間は応答待ちに含まれる）。リダイレクトを追うときは最後の区間の内訳（合計は全体） | reqwest 単体では取れない。ボディを `Bytes` のまま渡さないと POST / PUT の 307・308 でボディを送り直せないので、ストリームで送り終えを測るのをやめた |
| ボディ | 画面へ返すのは先頭 10 MB。全体は直近 8 件のレスポンスぶんだけメモリに持つ。512 MB を超える応答はエラー | 「ファイルに保存」用 |
| 送信結果のイベント | `request://finished`。tauri-specta の derive だと名前が固定されるので `Event` を手で実装 | 設計書の名前に合わせる |
| 実 API の送信テスト | `cargo test -p nekoreq real_api -- --ignored`（ホストの `host.docker.internal:8080`。API が動いているときだけ） | |
| レスポンスのヘッダーと展開 | reqwest の自動展開は使わない。受け取ったヘッダー（`content-encoding`・`content-length` を含む）をそのまま画面に返し、ボディの展開（gzip・x-gzip・deflate・br。重ねがけは逆順）は http クレートで自前で行う。知らない方式（zstd など）は展開せずバイナリとして返す。展開後が 512 MB を超えたら TooLarge、壊れていれば「展開できません」のエラー | 画面にサーバーが返したままのヘッダーを見せるため |
| 受信サイズ | `receivedBytes` は転送されたバイト数（ステータス行 + ヘッダー + 展開前のボディ）、`body.size` は展開後（文字コード変換前）のバイト数 | |
| Accept-Encoding | 手で指定がなければ `gzip, br, deflate`（`DEFAULT_ACCEPT_ENCODING`）。プレビュー・実際の送信・送信バイト数の見積もりで同じ値を使う。手で指定した値は尊重する（`identity` など） | |
| 文字コード | UTF-8 として読めるボディはそのままテキスト。そうでなく Content-Type の `charset` が UTF-8 以外（Shift_JIS・EUC-JP・UTF-16 など）なら `encoding_rs` で UTF-8 の文字列に変換して `text` で返す。変換できない（知らない charset・不正なバイト）ものだけ base64。「ファイルに保存」は元のバイト列 | charset 宣言が ISO-8859-1 でも中身が UTF-8 のときは UTF-8 として読む（実在の API で多い誤宣言） |
| multipart の Content-Type | 手入力の Content-Type は無視し、境界つきの値を自動で付ける | 境界の文字列を本文と合わせるため |
| Basic 認証 | ユーザー名に `:` があれば組み立て時に Validation エラー（`BuildError::InvalidAuth` → `SendError::InvalidRequest`。RFC 7617）。`Authorization` の base64 と `user:password` はシークレット（伏せる対象）に加える | Authorization をエコーする応答（httpbin 型）を履歴に残しても資格情報が戻せないように |
| 手入力の Authorization | 有効な手入力の `Authorization` ヘッダーがあるときは、Bearer / Basic の認証設定は使わない（認証欄の未解決の変数で送信を止めない）。API キー（ヘッダー）も同名の手入力があれば使わない | 手入力が優先されるため |
| パスパラメータ | URL の `{id}` に対応する行がない・値が空・無効の行しかない場合は `missingPathParams` に入れる（送信はできる）。値が `.` / `..` のときは、URL の仕様上 1 区間として送れない（`%2e%2e` も「1 つ上へ」と同じに扱われる）ので埋めずに `{id}` のまま残し、`missingPathParams` に入れて警告する | |
| プレビューの URL の見た目 | 展開できない変数は `{{name}}` のまま、値が入らなかった `{id}` も `{id}` のまま見せる（送る URL のエンコードは変えない） | bindings の説明どおり。`?q={{keyword}}` が `%7B%7B…` に見えないように |
| URL の組み立て | スキームの有無は `://` が最初の `/` `?` `#` より前にあるかで判定（クエリの `?next=http://x` で誤判定しない）。テンプレートのクエリが `&` で終わっていても `&&` にしない | |
| シークレットを伏せる範囲 | 値そのもの・パーセントエンコード（16 進は大文字小文字の両方）・フォームの `+`・url クレートが整えたあとの形（パス・クエリ・フラグメント・ユーザー情報・ホスト別。ホストは小文字化される）を `redact_text` が探す。`Proxy-Authorization` も名前で伏せるヘッダーに加えた（`Authorization`・`Proxy-Authorization`・`Cookie`・`Set-Cookie`・`X-XSRF-TOKEN`） | |
| 送信エラーの文 | http クレートが、reqwest のエラー文中の URL からクエリ・フラグメント・ユーザー情報を除き、さらに `PreparedRequest.secrets` を `redact_text` で伏せてから返す（どの種類のエラーでも） | 画面・履歴（M6）にクエリのシークレットを出さない |
| `final_url` | 伏せ字にしていない（クエリのシークレットを含みうる）。M6 の履歴に保存するときに伏せる | |
| Debug 表示 | シークレットを持つ型（`Expanded`・`VarLayer`・`VarScope`・`ScopedValue`・`PreparedRequest`・`MemorySecretStore`）の `Debug` は値を伏せる | panic メッセージやログに載っても漏れないように |
| SQLite に残る平文 | `PRAGMA secure_delete = ON` にし、環境の保存（`env_save`）のたびに `wal_checkpoint(TRUNCATE)` で WAL を空にする | 通常の変数をシークレットに変えたときの元の値をファイル（WAL 含む）に残さない |
| `secret_set` | その環境で「シークレットの変数」として宣言されている名前だけ書ける（違えば Validation）。先に `env_save` で変数をシークレットとして保存する。環境・プロジェクトを消すときは、その環境のシークレットの変数のキーをキーチェーンから消す | 宣言のない名前の値がキーチェーンに取り残されないように |
| 環境の名前 | 通常の環境に「グローバル」という名前は付けられない（作成・改名とも Validation）。環境が 1 つもなく「選んでいる環境」が未選択のときに環境を作ると、その環境が自動で選ばれる（そのまま） | 画面で紛らわしいため |
| AdHocRequest の説明 | `description`（任意、既定は空）。マイグレーション v3 で `adhoc_requests.description` を足す。TypeScript では `description?: string`（`#[serde(default)]`） | 保存ダイアログの「説明（任意）」 |

## 2026-11-17 M3・M4（OpenAPI 取り込みと設計タブ・同期）

### roas の検証結果と採用した方式

| 項目 | 結果 |
| --- | --- |
| roas の版と Rust | roas 0.22.2（edition 2024）は Rust 1.90 でビルドできる。3.0 / 3.1 / 3.2 の型（`v3_0::spec::Spec` など）とローダー（`Loader`）がある |
| ローダー | `Loader` は `$ref` を書いたファイルの URL を基準に相対パスを解決し、ファイルは 1 回だけ読む。ただし標準の fetcher（`JsonFileFetcher`）は JSON 専用なので、yaml は自前の fetcher を足す必要がある。分割 yaml（`../common.yaml#/...` など）は、yaml の fetcher を足せば `resolve_reference` で解決できた |
| そのまま使えなかった点 | (1) `Spec::collapse` / `validate` は paths の PathItem 自体が `$ref`（`./video/client.yaml#/paths/~1...`）のとき辿らない（分割版の入口は 64 件の PathItem が `$ref` のまま残り、Operation は 0 件になった）。(2) 型は `BTreeMap` で、properties・responses・content の順序が yaml の順でなくなる（スキーマ表は yaml の順で並べたい）。(3) 位置情報（行番号）を持たない。(4) fetcher は同期で、非同期の fetcher の future は `Send` でない（Tauri の async コマンドから URL を読めない）。(5) 足りないファイルを「参照元のファイル:行」の一覧にできない |
| 採用 | `$ref` の解決・読み込みは自作（`nekoreq-openapi`）。パースは位置つきの yaml パーサー saphyr（`MarkedYaml`）で `serde_json::Value`（`preserve_order`）と「JSON Pointer → 行番号」の表を同時に作り、型は `Value` のまま扱う（3.0 / 3.1 / 3.2 の差は `nullable`・`type: [x, "null"]`・`query` メソッドなどだけ）。roas は本体には入れず、dev-dependency としてテストで使う（bundle 済みの実 fixture を roas で読み、operationId・method・path の集合が自前の結果と一致することを確かめる） |

### 取り込みと同期

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 読むファイル | 入口から `$ref` をたどって届いたファイルだけ（絶対パス / URL で 1 回だけ）。実 fixture の分割版は 23 ファイル（ディレクトリの yaml は 25 個あるが、`redocly.yaml` と `.redocly.lint-ignore.yaml` はどこからも参照されない）。名前のマップ（`properties` の `example` という名前の項目など）と、値そのもの（`example` / `default` / `enum` / `const` / 配列の `examples`）の中の `$ref` は参照として扱わない | |
| 行番号 | Operation の method のキーの行（`get:` の行）。mapping の値はキーの行、配列の要素は要素の始まりの行 | 画面の「video/client.yaml:5」の形 |
| スキーマの持ち方 | `$ref` を展開した JSON Schema を Operation に持つ。参照の名前は `x-nekoreq-ref`、循環は 2 回目に出会った所で止めて `x-nekoreq-circular: true`、解決できない所は `x-nekoreq-unresolved`（検査結果にも出す）。allOf は展開せず元のまま持ち、スキーマ表・モックを作るときにまとめる。実 fixture の保存サイズは約 2.2 MB（86 件） | 表・モック・（今後の）入力中の検証に同じ値を使える |
| スキーマ表・例 | 表の行は `depth` つきのリスト。型は `$ref` の名前（配列は `名前[]`）。制約は文字列のリスト（`nullable`・`enum: A, B`・`format: ...`・`min` / `max` / `minLength` ...・`pattern: ...`・`default: ...`・`readOnly`・`循環`）。例は `example` / `examples` が yaml のメディア型にあれば `yaml`、スキーマの各項目の example・default・enum から作れたら `schemaExample`、型だけで作ったら `generated`。`generatedExample`（スキーマだけから作ったもの）は常に返す | 画面の「スキーマから自動生成したモック」「スキーマの example から生成」の出し分け |
| operationId の重複 | 重複したものは、すべて `METHOD path` をキーにする（出てくる順で片方だけ operationId になることを避ける）。検査結果に出す | キーが安定する |
| 解決できない `$ref` | ファイルがない・読めないときは取り込み・同期を止める。ファイルはあるがポインターがない `$ref` は止めず、検査結果（`unresolvedRefs`）に出して、その箇所のスキーマは `x-nekoreq-unresolved` にする | 設計書「足りない・読めないファイル」 |
| 同期の差分 | 前回と `op_key` が同じ、または `METHOD path` が同じものを同じ API とみなす（operationId の付け替えは入力値のキーも付け替える）。変更の中身は summary・description・parameters・requestBody・responses・security・tags・operationId。ハッシュは書かれた場所（ファイル・行）を含まない | 行がずれただけで「変更」にしない |
| 取り込む API の選択 | 選ばなかった API もキャッシュに持つ（`operations.imported = 0`）。同期で「追加」と出ない。既存プロジェクトへの統合で選ぶと取り込み済みになる | 選択を覚えておくため |
| 削除された API | 入力値があれば `operations.deleted_at` を付けて残す（ツリーに「削除された API」として出る。`operation_forget` で消すまで残る）。なければ行を消す。yaml に戻れば元に戻る | 設計書「自分で消すまで残す」 |
| yaml から消えたパラメータ | 入力値は残し、`request_drafts.stale_params_json` に印を付ける。`operation_open` は値を無効のまま `params` に入れ、`missingInYaml` に出す。画面で消して保存すると印も外れる | 手で足したパラメータと区別するため、取り込みのときの前後の差で決める |
| 差分がないとき | 反映はしないが、更新日時・ハッシュは覚え直す（パス・URL 方式）。「変更あり」の印が消える | 起動時の更新日時の確認が、コメントだけの変更で付きっぱなしにならない |
| servers → 環境 | 取り込みのときだけ作る（同期では作らない）。名前は servers の `description`（なければ URL）、変数は `baseUrl`。同名の環境があれば作らない。URL の `{変数}` は既定値で置き換える | 消した環境が同期で戻らないように |
| アップロード | 選んだパス（フォルダ・ファイル・zip の絶対パス。画面は Tauri のダイアログ / ドロップのパスを渡す）を一時フォルダに置いてから読み、取り込みで `sources/<プロジェクト ID>/` に移す。置くのは yaml / yml / json だけ（`node_modules`・`.git` などは除く）。上限は 5,000 ファイル・256 MB。zip は `..` や絶対パスのエントリを拒否する。フォルダ 1 つなら中身を直下に、複数なら共通の親からの相対パスを保つ。同期（コピー方式）は `upload` でもう一度選んでもらう | 設計書「コピー方式」 |
| URL | 取得は http クレートの `fetch_bytes`（送信と同じ TLS）。アクセストークンは `Authorization: Bearer` で入口と同じ origin にだけ送る（別ホストには送らない）。キーチェーンの場所は `NekoReq/<プロジェクト ID>` の `source/token`。入口と別のホスト（ローカルの入口が URL を参照する場合も）は、読む前に `needsConfirmation` で止め、確認したホストは `api_sources.allowed_hosts` に覚える | 設計書「URL 方式」 |
| 既定の認証 | `RequestInput.opKey` があり `request.auth` が `None` のとき、Operation の security（未指定なら全体）の最初の対応できる方式を使う。apiKey（header / query / cookie）・http bearer / basic だけ（OAuth 2.0 などは v2）。変数名は cookie の apiKey がキーの名前の小文字（`SESSION` → `session`）、それ以外の apiKey は scheme 名、bearer は `token`、basic は `username` / `password`。使う変数が展開できないときは認証なしで送る（cookieAuth の自動化は M5）。`security: []` は認証なし | 設計書「認証」。値がないだけで送信を止めない |
| パラメータの初期値 | すべて無効の状態（path も）。example → default の順。ボディは example（なければモック）。form / multipart はスキーマの項目から行を作る（ファイルの項目はパスを選ぶまで無効） | 設計書の対応表どおり |
| エディタ | `editor_open` は取り込んだファイルの一覧にあるものだけ開ける。`tauri-plugin-opener` の `open_path`（OS の既定のアプリ）を Rust から呼ぶ。行番号は渡せない。capabilities は増やさない（画面から opener は呼べない） | 最小限の権限 |
| workspace | `serde_json` の `preserve_order` を workspace で有効にした（yaml の順を保つ）。http クレートに `fetch_bytes` を足した | |

## 2026-11-17 M3・M4 の修正（テスト担当の指摘への対応）

画面と Rust の契約（`src/bindings.ts` の型・コマンド）は変えていない。

### 仕様の決定

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| URL 方式の入口のリダイレクト | 入口が 30x で別のホストへ飛ぶのは許可する（GitHub raw など）。アクセストークンは入口の origin にだけ送り、リダイレクト先には付けない。相対 `$ref` は、リダイレクト後の URL（実際に取得した URL）を基準に解決する（RFC 3986 の基準 URI）。リダイレクト先の origin を指す参照は、確認なしで読む（読む前の確認が要るのは、入口の origin でもリダイレクト先でもない origin） | 入口が `blob` の URL から `raw` のホストへ飛ぶ構成でも、相対参照が読める |
| 確認済みの別ホスト（`allowedHosts`） | スキーム・ホスト・ポートの origin 単位。`ForeignRef.host` と `allowedHosts` の値は `https://assets.example.com`・`http://127.0.0.1:8080` の形（既定のポートは省く・小文字）。https で確認したホストへの http の参照は、改めて確認が必要。スキームのない古い表記（`host` `host:port`）は https の origin だけに効く | `http://h` と `https://h` は別の相手。型は変えず、`host` の中身だけが origin になった |
| コピー方式（アップロード）の `$ref` | 置いたフォルダ（一時フォルダ。`sources/<ID>` に移したあとも同じ構成）の中に閉じ込める。解決したパスがその外（絶対パス・`../`・`file://`）なら、読まずに「足りないファイル」（参照元の `ファイル:行` つき。`$ref` に書かれた文字列を表示し、一時フォルダの名前は出さない）として止める。パス方式は従来どおり外のファイルも読める（ユーザー自身のファイル）。URL 方式は従来どおり | アップロードした zip・フォルダの yaml 次第で、PC の任意の yaml / json が取り込まれるのを防ぐ。`LoadParams.root` で渡す |
| `$ref` の扇状の展開 | 1 回の変換（1 Operation。PathItem の共通パラメータも別に数える）で展開するスキーマのノードを 20,000 個までにし（実 fixture の 86 件は当たらない）、超えたらそこで止めて `x-nekoreq-truncated: true` を付ける（以降のノードも同じ）。入れ子の深さは 100 段まで、複製するデータ（example・enum など）は 40 万ノードまで。スキーマ表は、その行の説明に「（大きすぎるため省略）」を入れ、制約に「省略」を足して子は出さない。モックは `null` にする | 循環ではない `$ref` の扇状の展開が指数的に膨れるのを止める |
| YAML の入れ子の深さ | パースの前に、字下げと `- ` から深さを見積もり（ブロックスカラーの中は数えない）、200 段を超えたら `YamlError`（「入れ子が深すぎます」・行つき）で止める。パースと変換は 64 MB のスタックのスレッドで行う。JSON・フロー形式はパーサー自身の再帰の上限に任せる | 深いブロック形式の入れ子でスタックオーバーフローしてプロセスごと落ちるのを防ぐ |
| 保存した Operation の読み戻し | 再帰の上限（serde_json の 128）を外し、スタックは `serde_stacker` で足りなければ伸ばして読む。保存できたものは必ず読み戻せる | 入れ子の深いスキーマの Operation で、一覧も同期も壊れるのを防ぐ |
| 入口 URL の認証情報 | `user:pass@` を含む URL はエラー（「アクセストークンはトークン欄に入力してください」）。取り込み・確認・同期の入口で弾く。クエリ（`?access_token=...`）はそのまま使い、保存もそのまま。トークンはトークン欄で渡すのが決まり（クエリのトークンを伏せる機能は持たない） | ユーザー情報は表示・保存に平文で残さない。クエリは URL の一部として扱う |
| トークン | 取り込みの入力のトークンは前後の空白・改行を除く（`source_token_set` と同じ）。`SourceInput`・`ImportRequest`・`LoadParams` の `Debug` はトークンの値を伏せる（`***`） | 貼り付けの改行でヘッダーが作れなくなる。ログ・panic に平文で出さない |
| 同期の差分 | 比べる対象は取り込んだ（`imported`）API だけ。取り込んでいない API の変更・削除は出さず、同期では「まだ保存にない API」だけが「追加」になる。確認画面（既存プロジェクトへ統合）は、取り込んでいない API も選べるので「追加」に出す。反映の件数（`ImportReport`）の取り込んでいない API の数え方は変えていない（変更・削除は数える） | 画面のツリーに出ていない API が確認ダイアログに出ない |
| yaml に戻ってきた API | 差分でも反映の件数でも「追加」（`ImportReport.added`。`changed` には入れない）。入力値は残る | 差分と件数を同じにする |
| 数値に見える値 | `info.version` は書かれた文字列のまま（`1.10`・`001` を数値にしない。OpenAPI では string）。それ以外の値（example など）は YAML の仕様どおり（`0.10` は数値の 0.1） | `info.version: 1.10` が 1.1 と表示されるのを防ぐ |
| 3.0 の `exclusiveMinimum: true` | 制約に「> 0」「< 10」と出す（`minimum` / `maximum` を含まない）。3.1 の数値の `exclusiveMinimum` は従来どおり `exclusiveMin: 0` | 0 を含むと誤解されない |
| モック | `maximum` / `exclusiveMaximum` を超えない数値、`minItems` 個（最大 10）・`maxItems` を超えない配列を作る。モック全体のノード数は 5,000 まで | そのまま送ると 400 になるのを避ける |
| YAML の merge key | `<<: *base`・`<<: [*a, *b]` を展開する（マッピングに直接書いたキーが `<<` の前後どちらでも勝ち、リストでは前のものが勝つ）。`<<` の値がマッピング（のリスト）でなければふつうのキー。行番号は `<<` の行 | YAML の仕様どおり |
| examples の複数保持 | 今回は見送り（M5）。`MediaTypeSpec.example` を複数にする型の変更で `src/bindings.ts` が変わるため | 画面の担当が作業中 |
| タブ字下げの JSON の行番号 | 今回は扱わない（行番号なしのまま） | |

## 2026-11-17 オーケストレータの判断（M3）

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 取り込んだ API を開いたときのパラメータ | path パラメータは有効、query・header・cookie は無効で並べる | path パラメータは OpenAPI で常に必須。設計書の「無効の状態で並べる」は任意の項目に当てはめる |
| 既定の認証の変数名 | cookie の apiKey はキー名の小文字（`SESSION` → `session`）、ほかの apiKey は scheme 名、bearer は `token`、basic は `username` / `password`。展開できないときは認証なしで送る | 設計書の cookieAuth（環境変数 `session`）に合わせる |
