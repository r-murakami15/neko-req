# NekoReq 設計書（v1）
2026-11-17 · @村上龍平

## 概要
OpenAPI の yaml を正本にして API を呼び出し・確認する、Mac と Windows 向けのデスクトップアプリ「NekoReq」を Rust（Tauri）で作る。最初に使う API は「日本アニメーション ライセンス契約管理システム API」（OpenAPI 3.0.3、88 件）。
- 使う人: 自分ひとり。データは自分の PC にだけ保存し、チームとは共有しない。
- v1 に入れる機能:
  - リクエスト送信と環境変数（パラメータ・ヘッダー・ボディ・認証・レスポンス表示）
  - OpenAPI 取り込みと設計タブ（同期ボタンで yaml の変更を取り込む）
  - 履歴と cURL 取り込み
- v1 に入れない機能: OpenAPI エクスポート、設計の編集と yaml への書き戻し、テストケース、モック、チーム共有。
- 画面イメージ: [デザインキャンバス](https://claude.ai/artifact/LPzDqxMoi38Yq3ixpH8jV4)（13 画面）。

## 決定事項
画面は Web 技術、処理は Rust に分け、yaml を正本にしてアプリは「呼び出すための値」だけを持つ。
| 項目 | 決定 | 理由 |
|---|---|---|
| アプリ名 | NekoReq | バンドル ID とキーチェーンの名前にも使う |
| アプリの形 | Tauri v2（処理は Rust、画面は OS の WebView） | 表・タブ・Markdown の多い画面イメージをそのまま再現できる |
| 画面の実装 | React + TypeScript（Vite） | 表・コードエディタ・Markdown 表示の部品が揃っている |
| OpenAPI との関係 | yaml が正本。アプリは読むだけで、同期ボタンを押したときに読み直す。分割された yaml をそのまま読む（bundle 不要。bundle 済みも読める） | 設計の変更は yaml で行い、二重管理を避ける。常時のファイル監視はしない |
| アプリ側で保存するもの | 入力した値・環境変数・履歴。すべて自分の PC の SQLite 1 ファイル | 共有しないのでテキストファイルにする必要がない |
| 秘密の値 | OS のキーチェーン（macOS キーチェーン、Windows 資格情報マネージャー） | セッションやパスワードを平文でディスクに残さない |
| 対応 OS | macOS（Apple Silicon と Intel）、Windows 10 / 11 | 利用環境に合わせる |
| 取り込める版 | OpenAPI 3.0 / 3.1 / 3.2。指定は「パス・URL」と「ファイルのアップロード」の 2 通り | パーサーの roas が 3 つとも読める。Swagger 2.0 は v1 では扱わない |
| 配布と署名 | .dmg / .msi を GitHub Releases に置く。OS のコード署名はしない | 自分で使うため。初回起動に一手間かかる（非機能要件） |
| 自動更新 | あり（Tauri の updater プラグイン） | 新しい版を手で入れ直す手間を省く |
| レスポンスのスキーマ検証 | v1 では入れず、今後のアップデートで対応 | v1 は送信と確認に絞る |
| servers にない環境 | 環境変数の画面で手で追加する | 検証・本番など yaml にない接続先も同じ画面で扱える |


## 全体アーキテクチャ
画面は Tauri のコマンドで Rust を呼ぶだけで、通信・解析・保存はすべて Rust 側で行う。app が core を通して各クレートを使う。
[図: 全体構成 · 画面、7 クレート、外部 4 つ]
| クレート | 役割 | 主なライブラリ |
|---|---|---|
| app（src-tauri） | Tauri の起動、画面から呼ぶコマンド、送信結果のイベント通知、自動更新 | tauri 2、tauri-plugin-updater、tauri-specta（TypeScript の型を自動生成） |
| core | ドメインモデル、変数展開、リクエストの組み立て、伏せ字処理 | serde、thiserror |
| http | 送信、Cookie、時間の計測、中止 | reqwest（rustls）、tokio |
| openapi | yaml の読み込み（パス・URL・アップロード）、分割ファイルをまたぐ $ref の解決、Operation への変換、同期時の差分、行番号 | roas、位置つきの yaml パーサー、reqwest（URL の読み込み）、zip |
| storage | SQLite への保存、マイグレーション、履歴の全文検索 | rusqlite（bundled、FTS5） |
| secrets | キーチェーンの読み書き | keyring |
| curl | cURL コマンドの解析と書き出し | shell-words |

画面側の主なライブラリ: 状態管理は Zustand、Rust 呼び出しの結果は TanStack Query、エディタは CodeMirror 6、Markdown は react-markdown + remark-gfm + rehype-sanitize、長い一覧は TanStack Virtual、部品は Radix UI。
画面から呼ぶ主なコマンド:
- `openapi_preview` / `openapi_import` / `openapi_``sync`: 取り込みの確認・実行、同期ボタンでの差分の確認と反映
- `request_send` / `request_cancel`: 送信 ID をすぐ返し、結果は `request://finished` イベントで届ける
- `draft_save`、`env_list` / `env_save`、`secret_set`: 入力値と環境変数
- `history_list` / `history_get` / `history_delete`: 履歴
- `curl_parse` / `curl_export`: cURL の取り込みとコピー

## データモデル
yaml から作る「Operation」と、自分が入れた「RequestDraft」を分けて持つ。両者は operationId（なければ `METHOD path`）で結びつける。
| 型 | 中身 | 出どころ |
|---|---|---|
| Project | 名前、取り込み元 ApiSource、既定の環境 | アプリ |
| ApiSource | 指定方法（パス / URL / コピー）、入口ファイル、参照先ファイルの一覧とそれぞれの更新日時・ハッシュ、OpenAPI の版 | yaml |
| Operation | operationId、method、path、タグ、summary、description、parameters、requestBody、responses、security、書かれたファイルと行番号 | yaml（読み取り専用） |
| RequestDraft | Operation のキー、パラメータとヘッダーの値（有効・無効つき）、ボディ、認証の上書き | アプリ |
| AdHocRequest | yaml にないリクエスト（cURL 取り込みなど）。「その他」フォルダに入る | アプリ |
| Environment | 名前、変数の一覧（名前・値・通常かシークレットか・説明） | servers から作成、以後はアプリ |
| HistoryEntry | 送った内容（秘密は伏せ字）、受け取った内容、時間の内訳、環境、Operation のキー | アプリ |

```
// core クレート（抽粋）
pub enum OpKey { Id(String), MethodPath { method: String, path: String } }

pub struct RequestDraft {
    pub op_key: OpKey,
    pub params: Vec<KeyValue>,      // query / path / header / cookie を location で区別
    pub body: Body,                 // None | Json(String) | Form(Vec<KeyValue>) | Multipart(..) | Text(String) | Binary(PathBuf)
    pub auth: Option<AuthOverride>, // None = Operation の security に従う
}

pub struct KeyValue { pub location: ParamIn, pub name: String, pub value: String, pub enabled: bool }

pub struct EnvVar { pub name: String, pub value: VarValue, pub description: String }
pub enum VarValue { Plain(String), Secret }   // Secret の実体はキーチェーン
```

## 保存と同期
データは SQLite 1 ファイルとキーチェーンに分けて保存し、yaml は同期ボタンを押したときに読み直して反映する。

### 保存先
- SQLite: macOS は `~/Library/Application Support/<バンドル ID>/data.sqlite`、Windows は `%APPDATA%\<バンドル ID>\data.sqlite`。
- テーブル: `projects`、`api_sources`、`operations`（yaml を読んだ結果のキャッシュ）、`request_drafts`、`adhoc_requests`、`environments`、`env_vars`、`history`（検索用に FTS5）。
- スキーマの変更は起動時のマイグレーション（`PRAGMA user_version` で版を管理）。
- キーチェーン: サービス名 `NekoReq``/<プロジェクト ID>`、アカウント名 `<環境 ID>/<変数名>`。`session`・`xsrfToken`・`password` などシークレット指定の変数だけを置く。
- コピー方式（アップロード）で取り込んだ yaml 一式は、`<データフォルダ>/sources/<プロジェクト ID>/` に元のフォルダ構成のまま置く。パス・URL 方式では中身をコピーせず、読んだ結果（operations）だけを持つ。

### yaml の同期
- パス・URL 方式では、API ツリーの上の同期ボタンを押したときだけ、入口と参照先の全ファイルを読み直す。コピー方式では、同期ボタンからもう一度アップロードする。常時のファイル監視はしない。
- パス方式では、起動時とウィンドウに戻ったときに全ファイルの更新日時だけを見て、変わっていれば同期ボタンに印を付ける（読み直しはしない）。
- 読み直したら Operation ごとのハッシュを作り、前回と比べて「追加・変更・削除」を出す。
- 確認ダイアログで差分の中身を見てから反映する。差分がなければ「変更はありません」とだけ出す。
- 反映しても RequestDraft は消さない。yaml から消えたパラメータの値は「yaml にない」と表示して送信から外す。
- 削除された Operation の RequestDraft は「削除された API」に移し、自分で消すまで残す。operationId が変わった場合は `METHOD path` が同じものに引き継ぐ。
- yaml が壊れていて読めないときは同期を中止し、前回の Operation を使い続けて、エラーの行と理由を表示する。

## 機能仕様: リクエスト送信と環境変数
送信する内容は「Operation + RequestDraft + 選んでいる環境」から毎回組み立てる。cookieAuth と CSRF トークンは自動で扱い、手でコピーしなくても通るようにする。

### 変数
- 書き方は `{{変数名}}`。URL・パラメータ・ヘッダー・ボディ・認証のすべてで展開する。
- 優先される順: リクエスト内の上書き › 選んでいる環境 › グローバル。
- 動的な変数: `{{$uuid}}`、`{{$timestamp}}`、`{{$isoTimestamp}}`、`{{$randomInt}}`。
- 展開できない変数があれば送信せず、場所を赤で示す。URL 欄の下に展開後の URL を常に表示する。

### 組み立て
- パスパラメータ（`{clientId}` など）は RequestDraft の値で埋める。
- yaml で `required: true` の項目が空なら警告を出す（送信はできる）。
- ボディは JSON・form-urlencoded・multipart・テキスト・バイナリ。JSON は yaml のスキーマで入力中に検証する。

### 認証
- v1 の方式: なし、API キー（ヘッダー・クエリ・Cookie）、Bearer、Basic。
- 既定は yaml の `security` と `securitySchemes` に従う。`security: []` の 6 件は認証なしで送る。
- cookieAuth（Cookie の `SESSION`）: 値は環境変数 `session`。レスポンスの `Set-Cookie: SESSION=…` を受けたら自動で更新する。
- CSRF: `Set-Cookie: XSRF-TOKEN=…` を環境変数 `xsrfToken` に保持し、GET 以外では `X-XSRF-TOKEN` ヘッダーを自動で付ける。
- 自動で付くヘッダーはヘッダータブに「自動で付与されるヘッダー」として表示する。

### 送信
- `reqwest`（rustls）で送る。タイムアウトの既定は 30 秒。リダイレクトの追従と証明書の検証は設定で切り替える。
- 送信中は「中止」ボタンで止められる。送信は画面を止めない（非同期で結果をイベントで返す）。
- 時間の内訳（DNS・接続・TLS・応答待ち・ダウンロード）は、reqwest 単体では取れないため接続部分を包んで計測する。

### レスポンス表示
- ステータス・時間・サイズ、ボディ（JSON 整形と Raw）、ヘッダー、タイミングの 3 タブ。
- yaml の responses との照合（スキーマ検証）は v1 では行わず、今後のアップデートで入れる。`application/problem+json` は code と detail を目立たせる。
- 10 MB を超えるボディは先頭だけ表示し、全体はファイルに保存できるようにする。

### 環境変数の画面
- 環境の追加・複製・削除、変数の編集。取り込み時に servers から環境を作る（今回は「ローカル (Dev Container)」1 つ）。
- シークレットの値は伏せ字で表示し、目のアイコンで一時的に見せる。

## 機能仕様: OpenAPI 取り込みと設計タブ
取り込みは「yaml の指定 → 内容の確認 → 取り込み」の 3 段階で、取り込んだ API ごとに設計タブで yaml の内容を読み取り専用のドキュメントとして見せる。

### 取り込み
- 指定方法は 2 通り。どちらも分割された yaml と bundle 済みの yaml の両方を読める。
  - パス・URL を指定（参照方式）: 入口ファイル（例: `docs/20_design/api/openapi.yaml`）の場所だけを覚え、同期のたびに参照先ごと読み直す。
  - ファイルを直接アップロード（コピー方式）: ドラッグ＆ドロップかファイル選択で、フォルダごと・複数ファイル・zip を受け付ける。入口ファイルが複数候補になるときは、`openapi:` を持つファイルから選ばせる。
- URL 方式では、非公開リポジトリのためにアクセストークンをキーチェーンに置き、ヘッダーで送る。入口と別のサイトを指す参照は、読む前に確認を求める。
- 確認画面に出すもの: タイトル・版・servers・既定の認証、読んだファイルの数（今回は 25）、タグごとの API 一覧（チェックで選ぶ）、検査結果（解決できない $ref、operationId の重複、`security: []` の件数、Markdown の表を含む説明の件数）。
- 取り込み先: 新しいプロジェクト、または既存プロジェクトへの差分反映（「保存と同期」と同じ処理）。

### 分割された yaml の $ref の解決
- 入口ファイルから読み始め、外部ファイルを指す $ref（例: `./video/client.yaml#/paths/~1api~1v1~1video~1clients`）を見つけたらそのファイルを読む。
- 相対パスは、入口ではなくその $ref が書かれたファイルの場所を基準にする（`video/client.yaml` の `../common.yaml` は入口と同じフォルダの `common.yaml`）。`#/components/...` のようなファイル名のない参照は、書かれたファイル自身の中を指す。
- 読んだファイルは絶対パス（URL）で覚え、1 回だけ読む。循環した参照は 2 周目で止め、スキーマ表では「循環」と表示する。
- 参照先のファイルがない・読めないときは、足りないファイル名と参照元の行を一覧にして取り込みを止める。コピー方式ではそのファイルの追加を求める。
- 各 Operation には、実際に書かれたファイルと行番号を付けておく（設計タブの表示に使う）。
- roas のローダー（ファイル用・HTTP 用の読み込み部品あり）が 2 の解決の仕方に従うかは、M3 の最初にこのリポジトリの 25 ファイルで確かめる。足りなければ $ref の解決だけ openapi クレートで自作し、パースと型は roas を使う。
| yaml | アプリ |
|---|---|
| `tags[0]` | 左ツリーのフォルダ |
| `operationId` / `summary` | 結びつけのキー / 表示名 |
| `servers` | 環境（`baseUrl` 変数） |
| `parameters` の example・default | パラメータの初期値（無効の状態で並べる） |
| `requestBody` の example | ボディの初期値。なければスキーマから作ったモック |
| `securitySchemes` + `security` | 認証の既定 |
| `description` | 設計タブの本文 |


### 設計タブ
- 見出し: summary、タグ、認証の要否、operationId、その API が書かれたファイルと行番号（例: video/client.yaml:5）（クリックでエディタを開く）。
- 説明: description を GitHub 形式の Markdown（表を含む）で表示する。HTML は実行せず、無害化して表示する。
- リクエスト: パラメータの表、Content-Type、ボディのスキーマ表（名前・型・必須・制約・説明）、例。
- レスポンス: ステータスごとのタブ。説明、Content-Type、スキーマ表、例。エラーは返す code（LOGIN_FAILED など）を表示する。
- スキーマ表: $ref の名前を型として表示し、allOf はまとめる。nullable・enum・format・最小最大を「制約」に出す。循環参照は 2 回目で止める。
- 行番号: roas は位置情報を持たないため、各ファイルを位置つきで別途読み、JSON Pointer から行番号を引く表を作る。
- 例の扱い: yaml に example がないときは「スキーマから自動生成したモック」と明記する。

## 機能仕様: 履歴と cURL 取り込み
送信はすべて履歴に残し、cURL は取り込んだ時点で yaml の API に当てはめる。

### 履歴
- 保存するもの: 送った内容（展開後）、受け取ったステータス・ヘッダー・ボディ、時間の内訳、環境、Operation のキー。
- 伏せ字: `Authorization`・`Cookie`・`X-XSRF-TOKEN`・`Set-Cookie` と、シークレットの値が現れた箇所は保存前に `••••` に置き換える。
- 上限: 既定で 1,000 件。古いものから消す。1 MB を超えるボディは先頭だけ保存する。
- 一覧: 日付ごとに並べ、ステータス（2xx / 4xx / 5xx）・環境・期間で絞り込む。URL とボディは全文検索できる。
- 操作: 再送信、編集して開く（新しいタブ）、入力値として保存（RequestDraft に反映）、cURL としてコピー、2 件のレスポンスの差分比較。

### cURL 取り込み
- 読むオプション: `-X`、`-H`、`-d` / `--data` / `--data-raw` / `--json`、`-F`、`-u`、`-b`、`-G`、`--url`。行末の `\` と引用符を解釈する。
- method と path を yaml の path テンプレート（`/api/v1/video/clients/{clientId}` など）と照合し、一致すればその API の入力値として開く。
- 一致しなければ AdHocRequest として「その他」フォルダに入れる。
- ホスト部分が環境の `baseUrl` と同じなら `{{baseUrl}}` に置き換える。

## 画面一覧
v1 で作る画面は 13 で、すべて[デザインキャンバス](https://claude.ai/artifact/LPzDqxMoi38Yq3ixpH8jV4)に画面イメージがある。エクスポート画面は v2 に回す。
| 画面 | 中身 | 開き方 |
|---|---|---|
| メイン（パラメータ） | API ツリーと同期ボタン、開いているタブ、URL と送信、パラメータ、レスポンスのボディ | 起動時 |
| リクエスト: ヘッダー | 手入力のヘッダーと、自動で付くヘッダー | リクエストのタブ |
| リクエスト: ボディ | 種類の切り替え、JSON エディタ、スキーマ検証 | リクエストのタブ |
| リクエスト: 認証 | 認証方式、cookieAuth、CSRF の自動付与 | リクエストのタブ |
| リクエスト: 設計 | yaml のドキュメント（説明・スキーマ・ステータスごとの応答） | リクエストのタブ |
| レスポンス: ヘッダー | 受け取ったヘッダーの一覧と絞り込み | レスポンスのタブ |
| レスポンス: タイミング | 時間の内訳と接続情報 | レスポンスのタブ |
| 新規作成 | 空のリクエスト、OpenAPI 取り込み、cURL から作成 | ツリーの＋、タブの＋ |
| 保存 | 「その他」のリクエストに名前を付ける | 保存ボタン（AdHocRequest のときだけ） |
| OpenAPI 取り込み | パス・URL の指定かファイルのアップロード（フォルダ・複数・zip）、内容の確認、取り込み | ヘッダーのボタン、新規作成 |
| 環境変数 | 環境の一覧と変数の編集 | ヘッダーの鉳筆アイコン、サイドバー |
| 履歴 | 一覧、絞り込み、詳細、再送信・比較 | サイドバー |
| 設定 | 外観、リクエスト、プロキシ・証明書、履歴、ショートカット、アップデートの確認 | ヘッダーの歯車 |

画面イメージと変える点: yaml の API はタグでフォルダが決まるため、保存ボタンは入力値をそのまま保存し、ダイアログは出さない。ヘッダーの「エクスポート」ボタンは v1 では隠す。

## 非機能要件
| 項目 | 要件 |
|---|---|
| 対応 OS | macOS 10.15 以降（Apple Silicon と Intel）、Windows 10 / 11（WebView2）。Tauri v2 の対応範囲に合わせる |
| 取り込みの速さ | 分割された openapi.yaml（25 ファイル、88 件）をローカルのパスから 2 秒以内。同期ボタンを押してから差分が出るまで 1 秒以内 |
| 画面の軽さ | API 1,000 件、履歴 10,000 件でもツリーと一覧が引っかからない（仮想スクロール） |
| 起動 | 2 秒以内に前回のタブを復元して操作できる |
| 秘密の値 | キーチェーンにだけ置き、SQLite・履歴・ログには平文で残さない |
| 画面と処理の境界 | Tauri の capabilities で画面から呼べるコマンドを限る。CSP で外部スクリプトを読まない。Markdown の HTML は無害化する |
| 通信 | 自分が送るリクエストと更新の確認以外に、外部へは通信しない（利用状況の送信なし） |
| テスト | core・openapi・curl は単体テスト。openapi.bundled.yaml をテスト用の入力に使う。http はローカルのモックサーバー（wiremock）で確かめる |
| 配布 | macOS は .dmg、Windows は .msi。GitHub Actions（tauri-action）で両 OS 向けにビルドし、GitHub Releases に置く |
| 署名 | OS のコード署名と公証はしない。Apple Silicon で動かすため ad-hoc 署名（`signingIdentity: "-"`）だけ付ける。初回起動時は、macOS は「システム設定 › プライバシーとセキュリティ」で許可、Windows は SmartScreen で「詳細情報 › 実行」が必要 |
| 自動更新 | Tauri の updater プラグイン。起動時と設定画面から GitHub Releases の `latest.json` を確認し、更新があれば知らせてから入れ替える。更新ファイルは Tauri 専用の鍵で署名する（OS の署名とは別で省けない。秘密鍵は GitHub の Secrets に置く） |


## 実装計画
実際の API（ローカルの Dev Container）に対して毎回動かせるよう、「送信 → 取り込み → 認証の自動化 → 履歴」の順に積み上げる。日程は未定。
| # | 作るもの | 完了の条件 |
|---|---|---|
| M1 | Tauri + React の土台（アプリ名 NekoReq）、Cargo ワークスペース、SQLite のマイグレーション、画面と処理をつなぐ型の自動生成 | Mac と Windows で起動し、空のプロジェクトを作って再起動後も残る |
| M2 | 送信（http）、パラメータ・ヘッダー・ボディの編集、環境変数（手での追加を含む）、キーチェーン | 手で作った GET /api/v1/auth/session が送れ、レスポンスの 3 タブが表示される |
| M3 | OpenAPI 取り込み（openapi）、分割 yaml の $ref 解決、パス・URL・アップロードの指定、確認画面、API ツリー | 分割された docs/20_design/api/openapi.yaml と openapi.bundled.yaml のどちらから取り込んでも同じ 88 件がタグごとに並び、「取引先一覧取得」がそのまま送れる |
| M4 | 設計タブ、スキーマ表、同期ボタンと差分の反映 | yaml の description を書き換えて同期ボタンを押すと、設計タブに反映され、入力値は残る |
| M5 | cookieAuth と CSRF の自動化 | パスワードログイン → メール認証コードログイン → 取引先登録（POST）が、セッションやトークンを手でコピーせずに通る |
| M6 | 履歴、cURL の取り込みとコピー、設定画面 | 履歴から再送信でき、シークレットが履歴に平文で残らない。cURL が該当する API に当てはまる |
| M7 | 配布物のビルド（.dmg / .msi）、GitHub Releases への公開、自動更新 | 別の Mac と Windows に入れて M2〜M6 の条件が通り、新しい版を公開すると起動時に更新される |


## 未確定事項と v2 以降
未確定のものはない。アプリ名・署名・自動更新・スキーマ検証・同期の方式・手で足す環境・分割 yaml の読み方は 2026-11-17 に決定し、各章に反映した。
- 分割された yaml はそのまま読む。指定は「パス・URL」と「ファイルのアップロード」の 2 通り（「OpenAPI 取り込みと設計タブ」を参照）
v2 以降に回すもの:
- レスポンスのスキーマ検証（yaml の responses との照合、「✓ ClientListApiResponse に適合」の表示）
- OpenAPI エクスポート（3.0.3 / 3.2、YAML / JSON、`x-app` 拡張）。画面イメージは作成済み
- 設計タブでの編集と yaml への書き戻し
- テストケース、前処理・後処理のスクリプト、モックサーバー
- OAuth 2.0・Digest 認証、Swagger 2.0 の取り込み
- Postman・Bruno のコレクションの取り込み
- チームでの共有
