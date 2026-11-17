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
| 既定の認証 | `RequestInput.opKey` があり `request.auth` が `None` のとき、Operation の security（未指定なら全体）の最初の対応できる方式を使う。apiKey（header / query / cookie）・http bearer / basic だけ（OAuth 2.0 などは v2）。変数名は cookie の apiKey がキーの名前の小文字（`SESSION` → `session`）、それ以外の apiKey は scheme 名、bearer は `token`、basic は `username` / `password`。使う変数が展開できないときは認証なしで送る。`security: []` は認証なし（cookieAuth の自動化がオンなら、M5 で SESSION は `security: []` の API にも付く。下の「M5」の表）| 設計書「認証」。値がないだけで送信を止めない |
| パラメータの初期値 | すべて無効の状態（path も）。example → default の順。ボディは example（なければモック）。form / multipart はスキーマの項目から行を作る（ファイルの項目はパスを選ぶまで無効） | 設計書の対応表どおり |
| エディタ | `editor_open` は取り込んだファイルの一覧にあるものだけ開ける。`tauri-plugin-opener` の `open_path`（OS の既定のアプリ）を Rust から呼ぶ。行番号は渡せない（M5 で行番号つきのエディタ起動を足した。下の「M5」の表）。capabilities は増やさない（画面から opener は呼べない） | 最小限の権限 |
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
| examples の複数保持 | 今回は見送り → M5 で対応（下の「M5」の表） | 画面の担当が作業中だったため |
| タブ字下げの JSON の行番号 | 今回は扱わない（行番号なしのまま） | |

## 2026-11-17 オーケストレータの判断（M3）

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 取り込んだ API を開いたときのパラメータ | path パラメータは有効、query・header・cookie は無効で並べる | path パラメータは OpenAPI で常に必須。設計書の「無効の状態で並べる」は任意の項目に当てはめる |
| 既定の認証の変数名 | cookie の apiKey はキー名の小文字（`SESSION` → `session`）、ほかの apiKey は scheme 名、bearer は `token`、basic は `username` / `password`。展開できないときは認証なしで送る | 設計書の cookieAuth（環境変数 `session`）に合わせる |

## 2026-11-17 M5（cookieAuth と CSRF の自動化）

実 API（バックエンド apps/backend の SecurityConfig・AuthController）で確かめたこと: CSRF は Cookie 方式（`CookieCsrfTokenRepository`）で、
`XSRF-TOKEN` Cookie の値と `X-XSRF-TOKEN` ヘッダーの値の**両方**がそろって同じでないと 403（FORBIDDEN）。`GET /api/v1/auth/session` が
`XSRF-TOKEN` を発行する（Cookie に値がなければ）。ログインは自作の AuthController で、2 段階目（`email-code-login`）はセッション ID を作り直す。
ログインの API 3 つ（`password-login`・`email-code-login`・`session`）は yaml で `security: []`。`dev` プロファイル（docker-compose の既定）では
CSRF が無効で `XSRF-TOKEN` も出ない（取引先 API も認証不要）。CSRF まで確かめるときは dev 以外で起動する。

### 設定と送るとき

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 設定 | `Project.settings`（`autoSession`・`autoCsrf`。既定はどちらもオン）。マイグレーション v5 で `projects.settings_json`、コマンド `project_settings_set`。画面の認証タブの 2 つのチェック | プロジェクトごとに個別に切り替える |
| 使う変数と名前 | 環境変数 `session`（Cookie `SESSION`）・`xsrfToken`（Cookie `XSRF-TOKEN`、ヘッダー `X-XSRF-TOKEN`）・`baseUrl`。名前は固定（yaml の securityScheme の名前は見ない）。値は選んでいる環境の変数（変数の層は「上書き › 環境 › グローバル」のままなので、グローバルの `session` も読める） | 設計書「認証」の名前 |
| SESSION を付ける範囲 | 自動化がオンで環境が選ばれていて `session` の値があるとき、Cookie `SESSION` を付ける。**`security: []` の API にも付ける**（ログインの 2 段階目が 1 段階目の SESSION を要るため。ブラウザの Cookie と同じ）。認証を自分で選んだリクエスト（`request.auth` が `None` でない。なし・API キー・Bearer など）には付けない。既定の認証がすでに `SESSION` を付けていれば重ねない | 完了条件（ログイン → メール認証コード → 登録）を手でコピーせずに通すため。M3 の「`security: []` には付けない」は、自動化をオフにしたときの動き |
| CSRF を付ける範囲 | オンで `xsrfToken` の値があるとき、Cookie `XSRF-TOKEN` を全メソッドに、ヘッダー `X-XSRF-TOKEN` を GET・HEAD・OPTIONS 以外に付ける（TRACE は付ける）。トークンがまだないときはプレビューに理由を出す | Spring の Cookie 方式は Cookie とヘッダーの両方を照合する |
| 手入力が優先 | 有効な手入力の `X-XSRF-TOKEN` ヘッダーがあればヘッダーは足さない。Cookie は、パラメータの Cookie・手入力の Cookie ヘッダー・既定の認証に同じ名前があれば足さない（Cookie ヘッダーは 1 本にまとめる） | 設計書の要件 |
| 送り先の制限 | 送り先の origin（スキーム・ホスト・ポート）が環境の `baseUrl` の origin と同じときだけ付ける・保存する。`baseUrl` がない環境では、付けも保存もしない（理由をプレビュー・送信結果に出す）。別のサイトにセッションを送らず、別のサイトの `Set-Cookie` でセッションを上書きしない | ブラウザの Cookie がドメインで分かれるのと同じ。手で作ったリクエストで別の URL に送っても漏れない |
| 環境が未選択 | 付けない・保存しない（グローバルには保存しない）。`Set-Cookie` が来たら `autoUpdateSkipped` に「環境が選ばれていないため…」を入れる。環境 ID を明示して送れば（`RequestInput.environmentId`）その環境に保存する。「グローバル」を環境として指定しても保存しない | グローバルは全環境で共有され、別の環境のセッションを上書きしてしまうため |
| プレビュー | `autoHeaders` に `Cookie`（出どころ `認証タブ（cookieAuth）`・`CSRF の自動付与`・パラメータの Cookie は `Cookie`。複数は「、」でつなぐ）と `X-XSRF-TOKEN`（出どころ `CSRF の自動付与`）を伏せ字で出す。`RequestPreview.autoNotes` に付かない理由（トークンがまだない・送り先が `baseUrl` と違う・`baseUrl` がない・キーチェーンを読めない）を出す。以前の出どころ `Cookie・認証` は `認証タブ（cookieAuth）`（認証タブの API キー（Cookie））と `Cookie`（パラメータ）に分けた | 画面イメージ（ヘッダー・認証）の文言 |
| 伏せ字 | 付けた値は `PreparedRequest.secrets` に入る（URL・エラー文・履歴の伏せ字の対象）。`AutoAuth`・`SetCookie`・`CookieChange` の `Debug` は値を出さない。キーチェーンから読んだ値は、保存された変数がシークレットなので、以後も `secret = true` で扱われる | 設計書「秘密の値」 |
| キーチェーンを読めないとき | 自動化のための読み出しの失敗は、リクエストを止めず、自動化の値だけ諦める（プレビューに理由）。リクエスト自身が使うシークレット（既定の認証の `session` など）の失敗は、これまでどおりエラー | 認証のいらない API まで送れなくならないように |

### 受け取ったとき

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 何を見るか | どのステータスのレスポンスでも、`Set-Cookie` の `SESSION` と `XSRF-TOKEN`（名前は大文字小文字を区別）を見る。同じ名前が複数あれば、あとのものが勝つ。ログインの 2 つ目の API だけではなく、1 段階目（`password-login`）の SESSION も保持する（2 段階目が使うため）。画面イメージの「createEmailCodeLogin・createPasskeyLogin の成功時」より広い | 実際のバックエンドは 1 段階目でもセッションを作る |
| 消す指示 | 値が空・`Max-Age` が 0 以下・（`Max-Age` がなければ）`Expires` が過去（RFC 1123 と Netscape 形式）なら、環境の値を消す。変数の宣言は残す | ログアウトの `SESSION=; Max-Age=0` |
| 書き込み | 送信に使った環境の `session` / `xsrfToken` をシークレットとして更新する。変数がなければシークレットとして作る（説明は「自動で更新されます（Set-Cookie）」、有効）。通常の変数だったときはシークレットに変える（元の値は SQLite・WAL に残さない）。有効・無効は変えない。値が変わらなければ書かず、報告にも出さない。消したときは、値があったときだけ `cleared` として報告する。自動更新どうしは直列（`auto_lock`） | シークレットはキーチェーンだけ |
| 送信結果 | `RequestFinished.autoUpdates`（`name`・`environmentId`・`environmentName`・`action: updated / cleared`。値は出さない）と `autoUpdateSkipped`（`name`・`reason`。環境が選ばれていない・送り元が `baseUrl` と違う・`baseUrl` がない・キーチェーンに書けない）。キーチェーンに書けなくても送信は成功のまま。理由の文は値を伏せる | 画面で「session を更新しました」と出す。更新後は環境の一覧を取り直すこと（変数が増えている） |
| 画面へ返すレスポンスのヘッダー | `Set-Cookie`・`Cookie`・`Authorization`・`X-XSRF-TOKEN` などの値は `••••` にして返す（`Set-Cookie` は名前と属性を残す。`XSRF-TOKEN=••••; Path=/`）。ほかのヘッダーはそのまま | 画面イメージ（レスポンス・ヘッダー）。M2 の「サーバーが返したまま」は、これらのヘッダーだけ変えた。ヘッダー名はそのまま |
| 実 API の結合テスト | `cargo test -p nekoreq --test real_api_m5 -- --ignored --nocapture`。環境変数 `NEKOREQ_REAL_EMAIL`・`NEKOREQ_REAL_PASSWORD`（必須）、`NEKOREQ_REAL_API_URL`、`NEKOREQ_MAILPIT_URL`（メール認証コードは Mailpit の API から自動で取る。バックエンドの docker-compose.yml: SES → MiniStack → Mailpit）、`NEKOREQ_REAL_EMAIL_CODE`（手で渡すとき）、`NEKOREQ_REAL_KEEP=1`（登録した取引先を残す） | 認証情報はコードに書かない |

### 持ち越し

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| examples の複数保持 | `MediaTypeSpec.examples`（`NamedExample`: `name`・`summary`・`description`・`value`）。`example` があれば名前 `example` で先頭に、続いて `examples` を書かれた順（`$ref` はたどる。`value` のないもの（`externalValue` だけ）は入れない。配列の形は `example 1`・`example 2`…）。`example`（これまでの動き）は先頭の値。`MediaDoc.examples` に同じものを返す（設計タブで切り替えて見る）。保存済みのデータには `examples` がないので空で読む。ハッシュに含まれるので、例の変更は同期の差分（変更）になる。アップデート後の最初の同期では、例のある API が「変更」に出ることがある（保存済みの Operation に例の一覧がないため） | 設計書「例（複数）」 |
| 同期の確認済みホスト | `openapi_sync_check` / `openapi_sync_apply` に `allowedHosts`（`Option<Vec<String>>`）を足した。`needsConfirmation` の `foreign[].host`（origin）を渡して呼び直す。前後の空白・空は除く。`check` は読み込めたところで、確認したホストを `api_sources.allowed_hosts` に保存する（`apply` は反映と一緒に保存）。以後は渡さなくても確認を求めない | 確認画面で確認した相手を、同期のたびに聞かない |
| エディタで行番号へ | `editor_open` は、設定のコマンド（`AppSettings.editorCommand`。`app_settings_get` / `app_settings_set`。保存は `app_settings` テーブル（v5）のキー `app`）があればそれだけを使い、なければ VS Code（`code -g file:line`）・Cursor（同）・Sublime（`subl file:line`）・JetBrains（`idea` など `--line N file`）の順に、`PATH` と macOS・Windows・Linux のよくある置き場所から見つかったものを使う。どれもなければ従来どおり OS の既定のアプリ（行番号なし）。コマンドは空白で区切り（`"` `'` で囲める。Windows のパスの `\` はそのまま）、`{file}`（絶対パス）・`{line}`（なければ 1）を各引数の中で置き換える（`{file}` がなければ最後に足す）。シェルを通さず直接起動する。設定のコマンドが起動できなければエラー（ほかには切り替えない）。開けるファイルは、これまでどおり取り込んだファイルの一覧にあるものだけ（`editor_target`） | 設計書「クリックでエディタを開く」。設定画面は M6 |
| `ForeignRef.host` の説明 | 実際の値（origin の形）に直した。`ApiSource.allowedHosts` の説明も同じ | M4 の修正で中身が origin になっていた |

## 2026-11-17 M6（履歴・cURL の取り込みとコピー・設定画面）

Rust 側だけ（画面は別の担当が bindings を使って作る）。コマンドと型は `src/bindings.ts`。

### 履歴

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 残すもの | 成功・4xx / 5xx・失敗（接続できない `connect`・TLS `tls`・タイムアウト `timeout`・中止 `cancelled`・大きすぎる `tooLarge`・その他 `other`）を送信のたびに 1 件。**送っていないもの**（変数が展開できない・URL やヘッダーが正しくない・ボディのファイルを読めない）は残さない。履歴を作ってから `request://finished` を出す | 画面が結果を受けて一覧を取り直せば、すでに入っている |
| 保存する中身 | 送った内容（method・展開後の URL・アプリが付けた `User-Agent` / `Accept` / `Accept-Encoding` を含む実際のヘッダー・ボディ）、受け取ったステータス・ヘッダー・ボディ・時間の内訳・最終 URL（URL と違うときだけ）、環境の ID と名前、`opKey` と Operation の summary、送信日時（UTC）、送信に使った元の入力（`input`。`{{変数}}` のままの `RequestSpec`）。ボディは `requestBodyKind`（`none` / `text` / `binary` / `file` / `multipart`）と大きさ・パートの一覧（ファイルは中身を保存せずパスだけ）を持つ。multipart の `requestBody` は、テキストのパートを `name=value` の行にしたもの（検索・簡易表示用） | 設計書「履歴」 |
| 伏せ字（保存する前） | (1) `Authorization`・`Proxy-Authorization`・`Cookie`・`Set-Cookie`・`X-XSRF-TOKEN` ヘッダーの値（送った・受け取った両方。`redact_headers`）。(2) `PreparedRequest.secrets`（環境のシークレット・自動付与の SESSION / XSRF-TOKEN・Basic の資格情報）に、**そのレスポンスの `Set-Cookie` で受け取った SESSION / XSRF-TOKEN の値**と、**手で認証欄・ヘッダー・Cookie に書かれた値**（`sanitize_spec` の `literals`。Basic のヘッダーは base64 の中身も）を加えたものが現れた箇所すべて（URL・最終 URL・ヘッダー・ボディ・multipart のパート・ボディのファイルのパス・エラー文）。(3) 元の入力（`input`）は手で書かれた認証の値・ヘッダー・Cookie を `••••` にする（`maskedFields` に「認証（Bearer のトークン）」などの名前を残す） | 設計書「伏せ字」と、「手で書いた認証も伏せる」 |
| 手で書いた値の判定 | `{{変数}}` だけでできた値（`{{token}}`・`Bearer {{token}}`）はテンプレートなのでそのまま。それ以外が認証欄の Bearer のトークン・Basic のパスワード・API キーの値、上記ヘッダー、Cookie パラメータの値に入っていれば伏せる。クエリ・ボディ・ほかのヘッダーに手で書いたシークレットは、名前では見分けられないので対象外（`{{変数}}` で参照するのが決まり） | 平文のシークレットを増やさずに、手入力の邪魔をしない |
| 変数の上書き | `VarOverride` の値は保存しない（名前だけ `overrideNames`）。再送信では使えない | 値にシークレットが入りうる |
| 切り詰め | ボディ（送った・受け取った）が 1 MB を超えたら先頭だけ（文字の途中では切らない。`...Truncated` と全体の大きさ `...Size` を残す）。**伏せ字にしてから切る**（切り口をまたぐシークレットの前半を残さない）。設定 `history.omitLargeBodies` をオフにすると全体を保存（受け取るボディは設定の表示上限までなので、その範囲）。元の入力（`input`）の JSON・テキストのボディも 1 MB を超えるなら残さず（`Body::None`）、`maskedFields` に「ボディ（1 MB を超えるため保存していません）」を入れる（再送信・編集して開くでは使えなかったものとして `droppedFields` に出る） | 設計書「1 MB を超えるボディは先頭だけ」 |
| バイナリ | 受け取ったボディがバイナリ（base64 で返ってくるもの）なら中身は保存しない（`responseBinary`・大きさ・Content-Type だけ） | 履歴が膨らむのを避ける |
| 件数 | 設定 `history.limit`（1,000 / 5,000 / 0 = 保存しない。既定 1,000）。**プロジェクトごと**に、挿入と同じトランザクションで古いもの（`sent_at`・`seq` の順）から消す。0 のときは残さない（すでにある履歴は消さない）。設定を減らしても、次の送信までは消えない | 履歴は「プロジェクト単位」で見る・消すため |
| 表 | マイグレーション v6 で `history` を作り直す（それまで書き込む処理はなかったので消える行はない）。軽い列（一覧が読む）を前、重い列（ヘッダー・ボディ・元の入力）を後ろに並べる。索引 `(project_id, sent_at DESC, seq DESC)`。`history_fts`（`url`、`body` = 送ったボディ + 受け取ったボディ）は trigram のまま作り直し、`secure-delete` を有効にする | 一覧が長いボディのオーバーフローページを読まない。消した文字を FTS の領域に残さない |
| 消す | `history_delete` / `history_clear` / プロジェクト削除のあとに WAL を空にする（`wal_checkpoint(TRUNCATE)`）。`secure_delete` はコネクションで有効（M2 から）。本体・WAL に消した文字列が残らないことをテストで確認 | 「VACUUM は不要」 |
| 一覧 | `history_list(projectId, query)`。新しい順、既定 100 件・最大 500 件、続きは `nextCursor`（`sent_at\|seq`）。`total` は絞り込みに当てはまる全件。各項目に**ローカルの日付** `localDate` を付ける（画面の日付ごとの区切り）。`query.utcOffsetMinutes`（画面が `-new Date().getTimezoneOffset()` で渡す。なければ OS のタイムゾーン）で日付を決め、期間 `from` / `to`（`YYYY-MM-DD`、その日を含む）もそのタイムゾーンの 0 時で UTC に直す。ステータス `statuses`（`2xx` `3xx` `4xx` `5xx` `failed`。複数は OR）・`environmentId` | 設計書「一覧」 |
| 全文検索 | 検索語（前後の空白を除く）が 3 文字以上なら FTS5 trigram の phrase（`"` は二重にして記号を無効にする。大文字小文字は区別しない）、3 文字未満は `LIKE '%…%'`（`%` `_` `\` は文字として探す。URL・送ったボディ・受け取ったボディ）。10,000 件（本文つき）で 1 ページ目 約 2 ms・全文検索 約 4 ms・LIKE 約 15 ms・絞り込み 約 6 ms（デバッグビルド） | trigram は 3 文字未満を引けない |
| 再送信 | `history_resend(id, settings)`。元の入力を**今の環境**（プロジェクトの「選んでいる環境」）で `request_send` と同じに送る（送信 ID をすぐ返し、結果は `request://finished`。新しい履歴ができる）。伏せ字（`••••`）が入った認証・ヘッダー・Cookie の行は外してから送る（`••••` を送らない）。Operation の既定の認証・cookieAuth の自動付与は今の環境の値で付く | 設計書「再送信」 |
| 編集して開く | `history_open(id)`。元の入力（伏せ字の行は外す）・`opKey`・外したものの名前 `droppedFields`・上書きの名前を返す。`opKey` があればその API のタブに未保存の入力として、なければ新しい「その他」のタブで開く | 設計書「編集して開く（新しいタブ）」 |
| 入力値として保存 | `history_save_as_input(id, name?)`。`opKey` があれば RequestDraft（同じ `opKey` の入力値を置き換え）、なければ AdHocRequest（名前は指定か `METHOD /path`。200 文字まで）。伏せ字の行・認証は保存しない（`dropped`） | 設計書「入力値として保存」 |
| cURL としてコピー | `history_curl(id)`。履歴の送った内容（伏せ字済み）から作る。アプリが付けた既定のヘッダーは外す。URL に入った `••••` はそのまま読める形で出す（`%E2%80%A2` にしない） | 貼り付けて読める |
| 比較 | `history_compare(leftId, rightId)`。ステータス（`200 OK` / `失敗: …`）・ヘッダー（名前の小文字順にそろえた `name: value` の行）・ボディ（JSON は整形して、キーの順は保つ）を `similar`（Myers。2 秒で打ち切り）で行単位に比べる。変わらない行は前後 3 行だけ残して `skip`（「N 行省略」）にする。バイナリは比べない。片側 20,000 行まで。同じなら差は空（`...Changed` が `false`） | 設計書「2 件の比較」 |

### cURL

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| `curl_parse(projectId, text, environmentId?)` | `nekoreq-curl` で解析 → `baseUrl` と照合 → Operation と照合。何も保存しない。`target`（`operation` / `adhoc`）・`opKey`・`summary`・`matchedPath`・`request`（そのまま開ける `RequestSpec`）・`baseReplaced`・`environmentId`（ホストが当てはまった環境）・`suggestedName`・`warnings` を返す | 設計書「cURL 取り込み」 |
| `baseUrl` との照合 | 指定された環境（なければ選んでいる環境）› ほかの環境（作った順）› グローバルの順に、通常の変数 `baseUrl` と照合する（スキーム・ホスト・ポート。`baseUrl` にパスがあればそのパスで始まること）。当てはまれば URL の先頭を `{{baseUrl}}` にする。`baseUrl` が違うホストは置き換えず、「その他」で絶対 URL のまま | 別の環境の cURL を貼っても、そのホストの環境を案内できる |
| Operation との照合 | **`baseUrl` に置き換えられたときだけ**照合する（別のホストの cURL が、選んでいる環境へ送る API として開かれないように）。取り込み済み（削除されていない）の Operation のうち、method が同じで path テンプレートに当てはまり、`literal_segments` が多いものを優先（同数なら先のもの）。path パラメータの値はパーセントデコードして `params`（`location: path`、有効）に入れる。yaml にあって cURL にないパラメータは、無効の行で並べる（`open_operation` の「入力値あり」で作る）。cURL にだけあるパラメータ・ヘッダーは有効の行で残す。照合できなかった「ホストは違うが path は一致」は、警告で知らせる | 画面の「OpenAPI の createPasswordLogin と一致」 |
| 警告 | `nekoreq-curl` の警告（無視したオプション `ignored`・知らないオプション `unknown`・扱えない指定 `unsupported`・読み替え `adjusted`）をそのまま返す | 設計書 |
| `Accept-Encoding` の zstd | Chrome の `Accept-Encoding: gzip, deflate, br, zstd` から `zstd` を外す（空になれば行ごと外す）。外したら警告（`adjusted`） | http クレートが zstd を展開できない |
| multipart の `;type=` | core の `MultipartPart.content_type`（`Option<String>`。TypeScript では `contentType?: string \| null`）を足した。http の送信ではパートの `Content-Type` に使う（ファイルは指定がなければ拡張子から決める。改行は空白にしてヘッダーを増やせないようにする）。cURL の解析・書き出しも対応（`;type=` の警告は消えた。`;filename=` は従来どおり警告） | M5 のライブラリからの引き継ぎ |
| `curl_export(input, mode)` | 今の入力を**展開**して（`request_send` と同じ組み立て。cookieAuth・CSRF の自動付与も入る）cURL にする。`mode`: `redacted`（既定。`Authorization`・`Cookie`・`X-XSRF-TOKEN` などの値とシークレットの値を `••••`）/ `raw`（そのまま）。展開できない変数があれば `Validation`（変数名を含む）。書き方は bash（`\` の行継続）固定 | 設計書「cURL としてコピー」 |

### 設定

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| `AppSettings` | M5 の `editorCommand` に、`appearance`（`theme`: `dark` / `light` / `system`（既定）・`editorFontSize`: 13 / 14 / 15・`restoreTabs`）、`request`（`timeoutSecs`: 0 で無制限・既定 30、`followRedirects`、`verifyCertificates`、`responseLimit`: `mb10`（既定）/ `mb50` / `unlimited`）、`network`（`proxy`: `mode` `system`（既定）/ `none` / `manual`・`url`・`username`・`hasPassword`・`noProxy`、`caCertPath`、`clientCerts`: `id`・`hostPattern`・`path`・`hasPassword`）、`history`（`limit`・`omitLargeBodies`）を足した。保存は `app_settings` の JSON のまま（足りない項目は既定値）。コマンドは M5 の `app_settings_get` / `app_settings_set`。言語の設定は作らない | 設計書・画面イメージ「設定」 |
| 検証 | 選択肢にない値（履歴の件数・文字サイズ・10 万秒を超えるタイムアウト）・手動のプロキシで URL が空 / `http(s)` でない / `user:pass@` を含む・読めない CA 証明書（PEM の証明書がないもの）・存在しないクライアント証明書のファイルは `Validation`（保存されず、前の設定が残る）。前後の空白・空のホストは整える。クライアント証明書の `id` は空なら付ける。`hasPassword` は画面から書き換えられない（保存済みの値を引き継ぐ） | |
| パスワード | プロキシ（`proxy_password_set`）・クライアント証明書（`client_cert_password_set(id, …)`。先に `app_settings_set` で証明書を設定に入れて `id` を決める）のパスワードは**キーチェーンだけ**（サービス `NekoReq/app`、アカウント `proxy/password`・`client-cert/<id>`）。空を渡すと消える。設定から証明書を消すとパスワードも消す。`hasPassword` の印は設定の JSON に持つ（設定を読むたびにキーチェーンを開かない） | 設計書「秘密の値」 |
| 送信への反映 | `request_send` の `settings` が `null` なら、保存された設定（タイムアウト秒 → ms・リダイレクト・証明書の検証）を使う。渡されたときはその 3 つが優先。プロキシ・CA 証明書・クライアント証明書・表示上限は、いつも保存された設定。`SendSettings`（画面とやりとりする型）は変えていない（画面は `null` を渡す） | 設定を保存しても、画面が固定値を渡していると効かないため |
| プロキシ | `system`: reqwest の `system-proxy`（macOS・Windows のシステム設定と、環境変数 `HTTP_PROXY` など）。`none`: 使わない（環境変数も無視）。`manual`: URL（ユーザー名・パスワードは別に持ち、`Proxy-Authorization` の Basic で送る）と、通さないホスト（reqwest の `NO_PROXY` 形式）。時間の内訳（DNS・接続・TLS）はプロキシへの接続も含めた接続全体で測る | 設計書 |
| CA 証明書 | PEM のファイル（複数の証明書を並べてよい）を OS の証明書ストアに**足して**信頼する（`rustls-platform-verifier` の `new_with_extra_roots`）。検証を切っているときは使わない | |
| クライアント証明書 | `hostPattern`（`*` は何文字でも・大文字小文字は区別しない。`:` が 1 つあれば `host:port` と比べる。例 `*.internal.example`）が**最初の送り先**のホストに当てはまる最初の 1 件を使う。ファイルは `.p12` / `.pfx`（PKCS#12。`p12-keystore`。パスワードはキーチェーン）か PEM（証明書と秘密鍵を同じファイルに）。当てはまる証明書のファイルとキーチェーンだけをそのつど読む（無関係な証明書のためにキーチェーンを開かない）。リダイレクトで別のホストへ飛んでも、同じクライアントなので同じ証明書を（サーバーが求めれば）送る | rustls の `ResolvesClientCert` は送り先のホスト名を受け取れないため |
| 読めない証明書 | CA 証明書・クライアント証明書のファイルが読めない・パスワードが違うときは、送信の失敗（`SendError::Other`。メッセージは日本語で、パスワードを含まない）。履歴には残さない（送っていない） | |
| 表示上限 | `responseLimit` → http の `Limits.preview_bytes`（10 MB / 50 MB / 無制限 = 受け取れる上限の 512 MB）。「ファイルに保存」用の全体の保持（直近 8 件）は従来どおり | |
| yaml の URL の取得 | 取り込み・同期の URL 方式（入口と参照先）も、同じプロキシ・CA 証明書・クライアント証明書で取得する（`LoadParams.transport`。クライアント証明書はすべて読み、取得する URL ごとにホストで選ぶ） | 設計書 |
| 設定画面のほかの項目 | 「プロジェクトの既定の保存先」「ショートカット」「アップデートを確認」は、この担当の範囲外（作っていない） | |

### リダイレクト（M5 の持ち越し）

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 自前で追う | http クレートがリダイレクトを自分で追う（reqwest のポリシーは `none`）。最大 10 回（超えたら「リダイレクトを追えません: リダイレクトが 10 回を超えました」）。301・302 の POST と 303（HEAD・GET 以外）は GET にしてボディと `Content-Type` などを外し、307・308 はメソッドとボディをそのまま送り直す。`Location` が読めない・http(s) でないときは、その 3xx をそのまま返す。時間の内訳は最後の区間 | 別の origin に `X-XSRF-TOKEN` を送らないため（reqwest のポリシーはヘッダーを触れない） |
| 外すヘッダー | **別の origin**（スキーム・ホスト・ポートのどれかが違う）へのリダイレクトでは、`Authorization`・`Cookie`・`Proxy-Authorization`・`X-XSRF-TOKEN` を外す（ほかのヘッダーは送る）。同じ origin では付けたまま。reqwest の自動の `Referer` は付かなくなった | 設計書「CSRF」・M5 の持ち越し |

### タブの復元

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| コマンド | `session_save(projectId, session)`（置き換え。保存した内容を `savedAt` つきで返す）・`session_load(projectId)`（なければ `null`）。マイグレーション v6 の `sessions`（プロジェクトごとに JSON 1 件。プロジェクトを消すと一緒に消える）。タブは 200 個まで（超えると `Validation`） | 設計書「起動 2 秒以内に前回のタブを復元」 |
| 中身 | `tabs`（`id`・`kind`: `operation` / `adhoc` / `new`・`opKey`・`adhocId`・`title`・`request`（未保存の入力。変えていなければ `null`、`new` は常にある）・`dirty`・`viewState`（画面が決める文字列。Rust は見ない）・`droppedFields`）と `activeTabId`。`AppSettings.appearance.restoreTabs` が `false` のとき復元するかどうかは画面が決める（保存はいつもする。`session_load` は設定を見ない） | |
| シークレット | 未保存の入力のうち、**手で書かれた認証の値**（認証欄の Bearer のトークン・Basic のパスワード・API キー、`Authorization` / `Proxy-Authorization` / `Cookie` / `X-XSRF-TOKEN` ヘッダー、Cookie の値）は**保存しない**。空文字にして、空にしたものの名前を `droppedFields` に残す（画面は「認証の値は復元されません」と出す）。`{{変数}}` で参照したものは残る。変数の上書きはもともと入力に含まれない。テスト: 値が SQLite のファイルに残らない | 履歴の伏せ字と同じ考え方。履歴は表示用に `••••`、こちらは復元して送るものなので空にする（`••••` を送らない） |
| 速さ | 50 タブ（うち 1 つは 1 MB の本文）で、保存 約 40 ms・読み込み 約 4 ms（デバッグビルド） | |

### 実装上の注意

- `nekoreq-http` に `Transport`（プロキシ・CA・クライアント証明書・上限）と `send_with_transport` / `fetch_bytes_with` を足した。`send` / `send_with_limits` / `fetch_bytes` / `fetch_bytes_at` は従来どおり（プロキシなし・OS の証明書ストアだけ）。reqwest の features に `system-proxy` を足した。新しい依存: `p12-keystore`（PKCS#12。RustCrypto の rc 版に依存）・`similar`（差分）。
- コンテナ（Linux）では、macOS・Windows の OS のプロキシ設定（`system-proxy`）の読み取りと、macOS 向けのビルドは確かめていない（環境変数 `HTTP_PROXY` の読み取りだけテスト）。
- `cargo test --workspace` は、同時にリンクするテストが多いとコンテナのメモリを使い切ることがある（`ld` が `Killed`）。そのときは `-j 2` を付ける。

## 2026-11-17 M5・M6 の修正（テスト担当の指摘への対応）

### 仕様の決定

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| localhost はプロキシを通さない | プロキシが「OS の設定を使う」と「手動」のとき、`localhost`・`*.localhost`・`127.0.0.0/8`・`::1` への送信は、環境変数・OS の設定の `NO_PROXY` に書かれていなくても、常にプロキシを通さない（「使わない」は元から通さない）。yaml の URL の取得（取り込み・同期）も同じ。「OS の設定を使う」は、reqwest が内部で使う設定の読み取り（`hyper-util` の `Matcher::from_system`。OS の設定と `HTTP_PROXY` など。`NO_PROXY` も従う）を直接呼び、`Proxy::custom` でループバックだけ外す。環境変数に書かれた認証（`http://user:pw@host:port`）はそのまま送る。設定の読み取りはクライアントを作るとき（送信ごと） | ローカルの API を叩くアプリなので、会社のプロキシ環境でも localhost は直接つなぐ |
| `Set-Cookie` の見方 | `Domain`・`Path`・`SameSite` は見ない。送り先が環境の `baseUrl` と同じ origin かどうかだけで決める（今の動き）。`Secure` 付きを http で受け取っても保存する（開発環境の http を想定）。テスト `set_cookie_domain_path_and_secure_attributes_are_ignored_today` で固定 | ブラウザの Cookie の完全な再現はしない |
| リダイレクトの途中の `Set-Cookie` | 追従の途中（301・302・303・307・308）のレスポンスの `Set-Cookie` も読み、`SESSION`・`XSRF-TOKEN` を環境に保存する（順に読み、あとのものが勝つ。送り元の origin は各レスポンスの URL で `baseUrl` と比べる）。追従先のリクエストには、**同じ origin のとき**、新しい `SESSION`・`XSRF-TOKEN`（Cookie ヘッダー）を付け、`X-XSRF-TOKEN` は（GET・HEAD・OPTIONS 以外か、すでに付いているとき）更新する。消す指示なら外す。別の origin へ追従するときは付けない（今までどおり Cookie などを外す）。自動化の対象の origin・オン/オフは `Transport.redirect_cookies` で渡し、途中のレスポンスの `Set-Cookie` は `SentResponse.redirect_hops` で返す（`ResponseData`・画面へ返す型は変えていない）。履歴の伏せ字にも途中で受け取った値を含める | ログイン後の 302 で発行されたセッションを失わない |
| 自動更新の確認のプロキシ | 設定の手動プロキシ（URL・ユーザー名・パスワード）を、updater の確認とダウンロードにも使う（`UpdaterBuilder::proxy`）。「使わない」は `no_proxy`、「OS の設定を使う」はプラグインの既定。**CA 証明書・クライアント証明書は updater プラグインに渡す口がないので効かない** | 社内プロキシ越しでも更新を確認できるように |
| updater のコマンド | `update_check`・`update_install`・`update_restart` と内部の関数を `R: tauri::Runtime` で総称にした（`bindings.ts` には `Wry` で出す。形は変わらない）。モックのランタイムで、`update_install` を更新なしで呼んだときのエラー（「入れ替える新しい版はありません」）・debug ビルドでは起動時の確認をしないこと・確認のエラー文に URL のクエリを出さないことをテストする。エラー文（確認・入れ替え）から URL のユーザー情報・クエリ・フラグメントを落とす | テストできるように。将来 endpoint にトークンを入れても出ない |
| タブの復元 | 保存された JSON に読めないタブ（知らない `kind` など）があっても、読めるタブは復元する。読めなかった数は標準エラーに記録する（`SessionState` に知らせる項目がなく、型を変えないため）。選んでいたタブが読めなかったときは選んでいるタブなし。`session_save` は、受け取ったタブの `droppedFields` と今回空にしたものの和集合を保存する（復元したタブを保存し直しても知らせが消えない） | 1 つの不具合で前回のタブが全部消えない |
| 履歴の記録の失敗 | 記録に失敗したら標準エラーに記録する（シークレットは伏せる）。送信の結果は変えない | 失敗に気づけるように |

### 伏せ字・保存の修正

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 伏せ字の探す形 | 値の前後の空白を除いた形（4 文字以上のとき。ヘッダーの値は HTTP で前後の空白が落ちる）、JSON の文字列のエスケープ形（`\"`・`\\`・`\/`・非 ASCII だけ／すべての `\uXXXX`（16 進は大文字・小文字））、base64（標準・URL セーフ、パディングあり・なし。4 文字以上のとき）を、これまでの形（そのまま・URL・フォーム）に加えた。4 文字未満の値からは、空白除きと base64 の形を作らない（関係のない文字列まで伏せないため）。値の途中に埋まった base64（前の文字との境目で 3 バイトの区切りがずれたもの）は探さない | 設計書「値が現れた箇所」 |
| ヘッダー名 | `redact_headers` はヘッダー名に入ったシークレットも伏せる（`X-{{token}}`） | |
| URL のユーザー情報 | URL の `user:pass@` にシークレットが入っているとき、reqwest が送る `Authorization: Basic base64(user:pass)` の base64 と `user:pass` も伏せ字の対象（`PreparedRequest.secrets`）に入れる | サーバーがヘッダーをエコーしても履歴に残さない |
| 10 MB の切り口 | 受け取るときの表示上限で切れたボディは、伏せ字にしたあと、末尾がシークレットの先頭の断片（1 文字以上）と一致していたらその部分を落とす（履歴の 1 MB の切り詰めは、伏せ字のあとに切るので従来どおり） | 切り口をまたぐシークレットの前半を残さない |
| 保存済みの設定の読み込み | 保存された設定の JSON の 1 項目が不正でも、ほかの正しい項目は残し、不正な項目だけ既定値に戻す（配列は読める要素だけ残す）。JSON として読めなければ全体が既定値 | 1 項目の不具合でプロキシ・証明書の設定が消えない |
| `Expires` | RFC 6265 5.1.1 の寛容な読み方（区切り文字で分けた語から時刻・日・月名・年を拾う。asctime 形式・`UTC` などのタイムゾーン・1 桁の時刻・2 桁の年）。読めない日付は今までどおり無視 | 標準でない形の過去の日付でも消す指示として扱う |
| 検索語の NUL | 履歴の検索語から NUL（U+0000）を取り除く | FTS5 の構文エラーになる |
| 件数の上限で消した行 | 上限で行を消したとき（そのときだけ）WAL を空にする（`wal_checkpoint(TRUNCATE)`）。手動の削除と同じ | 消した行の文字を WAL に残さない |

### 画面側で決めたこと

| 項目 | 決定 |
| --- | --- |
| 履歴の再送信 | 元の入力をタブで開いてから送り、結果はメインのレスポンス欄に出す |
| 設定の保存 | 保存ボタンなしの自動保存（文字入力は欄を離れたとき） |
| ショートカット | Ctrl/Cmd+T は新規作成ダイアログを開く |
| 認証タブの注記 | 「レスポンスの SESSION を環境の session に保存し、次のリクエストの Cookie に付ける」（どのレスポンスの `Set-Cookie` でも更新するため） |
| テーマ | `localStorage` にも覚え、起動時の最初の描画から反映する |

### macOS のメニュー

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| アプリのメニュー | macOS では `src-tauri/src/menu.rs` でメニューを明示的に作る（アプリメニュー: NekoReq について・隠す・ほかを隠す・すべてを表示・終了 Cmd+Q／編集メニュー: 取り消し・やり直し・切り取り・コピー・貼り付け・すべてを選択）。**Close Window（Cmd+W）と設定（Cmd+,）はメニューに置かない**。Windows・Linux はメニューバーを出さない。単体テストでメニューの中身（Close Window・設定がない）を確かめる。macOS の実機では目視していない | メニューが先にキーを取ると、画面のショートカット（タブを閉じる・設定を開く）に届かない |

### 実装上の注意（追加）

- `nekoreq-http` の依存に `hyper-util`（`client-proxy`・`client-proxy-system`。reqwest がすでに使っているものと同じ版）・`http`・`base64`・`chrono` を足した（新しいクレートは増えない）。
- テスト担当の BUG の `#[ignore]` は外した。残る `#[ignore]` は `updater_config_has_no_placeholders_left`（RELEASE: 公開前に docs/07 の置き換え値を入れる。ユーザーが入れるもの）と、ホストの API を使うテストだけ。
