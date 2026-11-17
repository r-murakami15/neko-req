import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { OpKey, OperationDetail, RequestDraft } from "./bindings";
import { createMockBackend, type MockBackend } from "./mock/backend";
import { footerButton, openOp, requestRegion, setupWithApi, sidebar } from "./test/openapi";

const key = (id: string): OpKey => ({ kind: "id", value: id });

/** 設計タブを開く。 */
async function openDesign(user: Parameters<typeof openOp>[0], folder: string, name: RegExp | string) {
  await openOp(user, folder, name);
  await user.click(await screen.findByRole("tab", { name: "設計" }));
  return screen.findByRole("article", { name: "設計" });
}

describe("設計タブ", () => {
  it("見出し・説明（Markdown の表）・リクエスト（スキーマ表と例）・ステータスごとのレスポンスを出す", async () => {
    const { user, project, mock } = await setupWithApi();
    const editor = vi.spyOn(mock, "editorOpen");
    const doc = within(await openDesign(user, "auth", /パスワードログイン/));

    // 見出し
    expect(doc.getByRole("heading", { level: 1, name: "パスワードログイン (1 段階目)" })).toBeInTheDocument();
    expect(doc.getByText("tag: auth")).toBeInTheDocument();
    expect(doc.getByText("認証不要（security: []）")).toBeInTheDocument();
    expect(doc.getByText("createPasswordLogin")).toBeInTheDocument();
    expect(doc.getByText(/最終取り込み \d{4}\/\d{2}\/\d{2} \d{2}:\d{2}/)).toBeInTheDocument();
    // ファイルと行番号のクリックでエディタを開く
    await user.click(doc.getByRole("button", { name: "auth.yaml:52" }));
    expect(editor).toHaveBeenCalledWith(project.id, "auth.yaml", 52);

    // 説明: GitHub 形式の Markdown。表は表として出る
    const description = within(doc.getByRole("region", { name: "説明" }));
    expect(description.getByText("createEmailCodeLogin", { selector: "code" })).toBeInTheDocument();
    expect(description.getAllByRole("listitem")).toHaveLength(3);
    const tables = description.getAllByRole("table");
    expect(tables).toHaveLength(2);
    expect(within(tables[0]).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["状況", "メッセージ", "返し方"]);
    expect(within(tables[1]).getByRole("cell", { name: "LOGIN_FAILED" })).toBeInTheDocument();

    // リクエスト: 概要のカードと、ボディのスキーマ表（名前・型・必須・制約・説明）
    const request = within(doc.getByRole("region", { name: "リクエスト" }));
    expect(request.getAllByText("application/json").length).toBeGreaterThanOrEqual(2); // カードと、データモデルの見出し
    expect(request.getByText(/必須 ·/)).toHaveTextContent("必須 · PasswordLoginRequest");
    const schema = within(request.getByRole("table", { name: "リクエストボディのスキーマ" }));
    expect(schema.getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["名前", "型", "必須", "制約", "説明"]);
    const emailRow = schema.getByRole("row", { name: /email/ });
    expect(emailRow).toHaveTextContent("string");
    expect(emailRow).toHaveTextContent("必須");
    expect(emailRow).toHaveTextContent("format: email");
    expect(emailRow).toHaveTextContent("maxLength: 254");
    expect(emailRow).toHaveTextContent("メールアドレス");
    // 字下げ（入れ子の深さ）: ルートより項目のほうが深い
    const rootCell = schema.getByRole("cell", { name: "(ルート)" });
    expect(parseInt(rootCell.style.paddingLeft)).toBeLessThan(parseInt(within(emailRow).getByRole("cell", { name: "email" }).style.paddingLeft));
    // 例: yaml に example がないので、自動生成のモックと明記する
    expect(request.getByText("（スキーマから自動生成したモック）")).toBeInTheDocument();
    expect(request.getByLabelText("例（JSON）")).toHaveTextContent('"email": "user@example.com"');

    // レスポンス: ステータスごとのタブ。エラーは返す code を出す
    const responses = within(doc.getByRole("region", { name: "レスポンス" }));
    const tabs = responses.getAllByRole("tab");
    expect(tabs.map((t) => t.textContent?.slice(0, 3))).toEqual(["200", "400", "401", "500"]);
    expect(tabs[0]).toHaveAttribute("aria-selected", "true");
    const panel = within(responses.getByRole("tabpanel", { name: "ステータス 200" }));
    expect(panel.getAllByText("application/json").length).toBeGreaterThanOrEqual(2);
    expect(panel.getAllByText("EmailCodeSentApiResponse").length).toBeGreaterThanOrEqual(1);
    expect(panel.getByRole("row", { name: /expiresAt/ })).toHaveTextContent("format: date-time");
    // example のある項目から作った例
    expect(panel.getByText("（スキーマの example から生成）")).toBeInTheDocument();
    expect(panel.getByLabelText("例（JSON）")).toHaveTextContent('"expiresAt": "2026-11-11T10:10:00+09:00"');

    await user.click(responses.getByRole("tab", { name: /401/ }));
    expect(responses.getByRole("tab", { name: /401/ })).toHaveAttribute("aria-selected", "true");
    const errPanel = within(responses.getByRole("tabpanel", { name: "ステータス 401" }));
    expect(errPanel.getByLabelText("この API で返す code")).toHaveTextContent("LOGIN_FAILED");
    expect(errPanel.getByText(/メールアドレスまたはパスワードが正しくありません/)).toBeInTheDocument();
    expect(errPanel.getAllByText("application/problem+json").length).toBeGreaterThanOrEqual(1);
    expect(errPanel.getAllByText("ApiProblem").length).toBeGreaterThan(0);
    // 設計タブは幅いっぱい（レスポンスの送信結果欄は隠す）
    expect(document.querySelector(".pane-response")).toBeNull();
  });

  it("循環参照のスキーマは「循環」と出して、そこで展開を止める", async () => {
    const { user } = await setupWithApi();
    const doc = within(await openDesign(user, "video-work", /作品取得/));
    const table = within(doc.getByRole("table", { name: "レスポンス 200のスキーマ" }));
    const parent = table.getByRole("row", { name: /parent/ });
    expect(parent).toHaveTextContent("Work");
    expect(parent).toHaveTextContent("循環");
    expect(table.getByRole("row", { name: /children/ })).toHaveTextContent("Work[]");
    expect(table.getByRole("row", { name: /children/ })).toHaveTextContent("循環");
    // 循環の先は行が増えない（Work の中身は 1 回だけ）
    expect(table.getAllByRole("row", { name: /workTitle/ })).toHaveLength(1);
  });

  it("説明の Markdown は無害化する（HTML・スクリプトは実行しない。リンクは画面を遷移させず、既定のブラウザで開く）", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const evil = [
      "通常の文。",
      "",
      '<script>window.__pwned = "script"</script>',
      "",
      '<img src="x" onerror="window.__pwned = \'img\'">',
      "",
      "[危険なリンク](javascript:window.__pwned='link')",
      "",
      "[安全なリンク](https://example.com/docs)",
      "",
      "| 項目 | 内容 |",
      "|---|---|",
      "| a | <b onclick=\"window.__pwned='b'\">太字</b> |",
    ].join("\n");
    const backend: MockBackend = Object.assign(Object.create(base) as MockBackend, {
      operationGet: async (projectId: string, opKey: OpKey): Promise<OperationDetail> => ({ ...(await base.operationGet(projectId, opKey)), description: evil }),
    });
    const { user, mock } = await setupWithApi(base, backend);
    const open = vi.spyOn(mock, "openExternal");
    const doc = within(await openDesign(user, "auth", /ログイン状態取得/));

    const description = doc.getByRole("region", { name: "説明" });
    expect(within(description).getByText("通常の文。")).toBeInTheDocument();
    // 表は表として出る
    expect(within(description).getByRole("table")).toBeInTheDocument();
    expect(within(description).getByRole("cell", { name: "a" })).toBeInTheDocument();
    // スクリプトも img も onclick も、DOM に入らない
    expect(document.querySelector("script")).toBeNull();
    expect(description.querySelector("img")).toBeNull();
    expect(description.querySelector("[onclick],[onerror]")).toBeNull();
    expect((window as unknown as { __pwned?: string }).__pwned).toBeUndefined();
    // javascript: のリンクは href を持たない
    const bad = within(description).getByText("危険なリンク");
    expect(bad.getAttribute("href") ?? "").not.toMatch(/javascript:/i);
    await user.click(bad);
    expect(open).not.toHaveBeenCalled();
    // 安全なリンクは、画面を遷移させずに既定のブラウザで開く
    const link = within(description).getByRole("link", { name: "安全なリンク" });
    await user.click(link);
    expect(open).toHaveBeenCalledWith("https://example.com/docs");
    expect((window as unknown as { __pwned?: string }).__pwned).toBeUndefined();
  });

  it("「ボディに反映」で、例を送信タブの JSON ボディに入れる", async () => {
    const { user } = await setupWithApi();
    await openDesign(user, "auth", /パスワードログイン/);
    // いったんボディを「なし」にしておく
    await user.click(screen.getByRole("tab", { name: "ボディ" }));
    await user.click(screen.getByRole("radio", { name: "なし" }));
    expect(screen.getByText("このリクエストにはボディがありません。")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "設計" }));

    const doc = within(await screen.findByRole("article", { name: "設計" }));
    await user.click(doc.getByRole("button", { name: "ボディに反映" }));
    // ボディのタブに切り替わり、JSON が入る。入力は未保存の変更になる
    expect(screen.getByRole("tab", { name: "ボディ" })).toHaveAttribute("aria-selected", "true");
    const editor = await screen.findByRole("textbox", { name: "JSON のボディ" });
    await waitFor(() => expect(editor).toHaveTextContent('"email": "user@example.com"'));
    expect(editor).toHaveTextContent('"password": "********"');
    // レスポンスの例には「ボディに反映」はない
    await user.click(screen.getByRole("tab", { name: "設計" }));
    expect(screen.getAllByRole("button", { name: "ボディに反映" })).toHaveLength(1);
  });

  it("yaml の API ではないタブには設計がない（取り込んでいないプロジェクトでも）", async () => {
    const { user } = await setupWithApi();
    await user.click(screen.getByRole("button", { name: "新しいタブ" }));
    await user.click(await screen.findByRole("button", { name: /HTTP リクエスト/ }));
    await user.click(await screen.findByRole("tab", { name: "設計" }));
    expect(screen.getByText(/OpenAPI の API ではないので、設計はありません/)).toBeInTheDocument();
  });
});

describe("yaml の API のタブ（M2 の画面への追加）", () => {
  it("パラメータ・ヘッダーの表に yaml の説明が出る。必須で空の項目は警告する（送信はできる）", async () => {
    const { user } = await setupWithApi();
    await openOp(user, "video-client", /取引先一覧取得/);
    const region = within(await screen.findByRole("region", { name: "リクエスト" }));

    // 説明（表示専用）
    expect(region.getAllByRole("columnheader", { name: "説明" }).length).toBeGreaterThan(0);
    expect(region.getByText("取引先名の部分一致")).toBeInTheDocument();
    expect(region.getByText("契約種別で絞り込む（DIRECT / AGENCY）")).toBeInTheDocument();
    // 必須の印と警告
    expect(region.getByText("必須")).toBeInTheDocument();
    expect(screen.getByText(/必須（required）の項目が空か無効です: perPage（クエリ）（送信はできます）/)).toBeInTheDocument();
    // 送信はできる
    await waitFor(() => expect(screen.getByRole("button", { name: "送信" })).toBeEnabled());

    // 有効にして値があれば、警告は消える
    await user.click(region.getByRole("checkbox", { name: "クエリパラメータ 4 行目を有効にする" }));
    await waitFor(() => expect(screen.queryByText(/必須（required）の項目が空か無効です/)).not.toBeInTheDocument());

    // 値を空にすると、また警告が出る
    const value = region.getByLabelText("クエリパラメータ 4 行目の値");
    await user.clear(value);
    expect(await screen.findByText(/必須（required）の項目が空か無効です: perPage/)).toBeInTheDocument();
  });

  it("ヘッダーの説明も出る（タグなしの API）", async () => {
    const { user } = await setupWithApi();
    await openOp(user, "（タグなし）", /ファイルアップロード/);
    await user.click(await screen.findByRole("tab", { name: /^ヘッダー/ }));
    expect(await screen.findByText("ログと突き合わせる ID")).toBeInTheDocument();
    expect(screen.getByLabelText("ヘッダー 1 行目のキー")).toHaveValue("X-Request-Id");
  });

  it("保存ボタンは、ダイアログを出さずにそのまま保存する（入力値は開き直しても残る）", async () => {
    const { user, mock, project } = await setupWithApi();
    await openOp(user, "video-client", /取引先一覧取得/);
    await user.click(await screen.findByRole("checkbox", { name: "クエリパラメータ 4 行目を有効にする" }));
    const tab = () => screen.getByRole("tab", { name: /取引先一覧取得/ });
    expect(within(tab()).getByRole("img", { name: "未保存" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(within(tab()).queryByRole("img", { name: "未保存" })).not.toBeInTheDocument());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    const saved = (await mock.draftGet(project.id, key("listClients"))) as RequestDraft;
    expect(saved.params.find((p) => p.name === "perPage")).toMatchObject({ enabled: true, value: "50" });

    // 閉じて開き直しても、保存した入力値が戻る
    await user.click(screen.getByRole("button", { name: "取引先一覧取得 を閉じる" }));
    await user.click(await within(sidebar()).findByRole("button", { name: /取引先一覧取得/ }));
    expect(await screen.findByRole("checkbox", { name: "クエリパラメータ 4 行目を有効にする" })).toBeChecked();
    expect(within(screen.getByRole("tab", { name: /取引先一覧取得/ })).queryByRole("img", { name: "未保存" })).not.toBeInTheDocument();
  });

  it("認証タブの「プロジェクトの既定を使う」は yaml の security の内容を見せる", async () => {
    const { user } = await setupWithApi();
    await openOp(user, "video-client", /取引先一覧取得/);
    await user.click(await screen.findByRole("tab", { name: /^認証/ }));
    const region = within(requestRegion());

    const def = region.getByRole("radio", { name: /プロジェクトの既定を使う/ });
    expect(def).toBeEnabled();
    expect(def).toBeChecked();
    expect(def.closest("label")).toHaveTextContent("security: cookieAuth");
    expect(region.getByText(/OpenAPI の securitySchemes「cookieAuth」から設定されました/)).toBeInTheDocument();
    expect(region.getByLabelText("送り先")).toHaveValue("Cookie");
    expect(region.getByLabelText("名前")).toHaveValue("SESSION");
    expect(region.getByLabelText("値")).toHaveValue("{{session}}");
    expect(region.getByLabelText("値")).toHaveAttribute("readonly");

    // 上書きできて、「既定」に戻せる
    await user.click(region.getByRole("radio", { name: /^Bearer トークン/ }));
    expect(region.getByLabelText("トークン")).toBeInTheDocument();
    await user.click(region.getByRole("radio", { name: /プロジェクトの既定を使う/ }));
    expect(region.getByRole("radio", { name: /プロジェクトの既定を使う/ })).toBeChecked();
    expect(region.queryByLabelText("トークン")).not.toBeInTheDocument();
    await user.click(region.getByRole("radio", { name: /^なし/ }));
    expect(region.getByText("認証を付けずに送ります。")).toBeInTheDocument();

    // security: [] の API
    await openOp(user, "auth", /パスワードログイン/);
    await user.click(await screen.findByRole("tab", { name: /^認証/ }));
    const none = within(requestRegion());
    expect(none.getByRole("radio", { name: /プロジェクトの既定を使う/ })).toBeChecked();
    expect(none.getByRole("radio", { name: /プロジェクトの既定を使う/ }).closest("label")).toHaveTextContent("security: []");
    expect(none.getByText(/security: \[\]（認証なし）なので、認証を付けずに送ります/)).toBeInTheDocument();
  });

  it("既定の認証は、環境変数 session があれば Cookie として付き、送信には opKey が付く", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base, async (p, b) => {
      const env = (await b.envList(p.id)).find((e) => !e.isGlobal)!;
      await b.envSave({ ...env, vars: [...env.vars, { name: "session", value: { kind: "plain", value: "abc123" }, description: "", enabled: true }] });
    });
    const send = vi.spyOn(mock, "requestSend");
    await openOp(user, "video-client", /取引先一覧取得/);
    await user.click(await screen.findByRole("tab", { name: /^認証/ }));
    await waitFor(() => expect(within(requestRegion()).getByText(/Cookie: SESSION=/)).toBeInTheDocument());
    expect(within(requestRegion()).queryByText(/abc123/)).not.toBeInTheDocument(); // 値は伏せ字

    await user.click(await screen.findByRole("button", { name: "送信" }));
    await screen.findByLabelText("ステータス");
    expect(send.mock.calls[0][0]).toMatchObject({ projectId: project.id, opKey: key("listClients"), request: { auth: null, method: "GET" } });
  });

  it("ボディの「スキーマから例を生成」は、スキーマの型だけから作った例で置き換える（確認つき）", async () => {
    const { user } = await setupWithApi();
    await openOp(user, "video-client", /取引先登録/);
    await user.click(await screen.findByRole("tab", { name: "ボディ" }));
    const editor = await screen.findByRole("textbox", { name: "JSON のボディ" });
    // 最初は yaml の example
    await waitFor(() => expect(editor).toHaveTextContent('"clientName": "テスト取引先"'));

    await user.click(screen.getByRole("button", { name: "スキーマから例を生成" }));
    const confirm = await screen.findByRole("dialog", { name: "ボディを置き換えますか？" });
    await user.click(within(confirm).getByRole("button", { name: "キャンセル" }));
    expect(editor).toHaveTextContent('"clientName": "テスト取引先"');

    await user.click(screen.getByRole("button", { name: "スキーマから例を生成" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "置き換える" }));
    await waitFor(() => expect(editor).toHaveTextContent('"clientName": "string"'));
    expect(editor).toHaveTextContent('"contractType": "string"');

    // yaml の API ではないタブには、このボタンはない
    await user.click(screen.getByRole("button", { name: "新しいタブ" }));
    await user.click(await screen.findByRole("button", { name: /HTTP リクエスト/ }));
    await user.click(await screen.findByRole("tab", { name: "ボディ" }));
    await user.click(screen.getByRole("radio", { name: "JSON" }));
    expect(screen.queryByRole("button", { name: "スキーマから例を生成" })).not.toBeInTheDocument();
  });
});

describe("yaml にないパラメータ", () => {
  it("同期で yaml から消えたパラメータは「yaml にない」と表示して、有効にしても送信から外す", async () => {
    const { user, mock, project } = await setupWithApi(undefined, undefined, async (p, b) => {
      // 入力値（contractType を有効にして保存）を作ってから、yaml を更新して同期する
      await b.draftSave({
        id: "",
        projectId: p.id,
        opKey: key("listClients"),
        params: [
          { location: "query", name: "q", value: "Netflix", enabled: true },
          { location: "query", name: "contractType", value: "DIRECT", enabled: true },
          { location: "query", name: "perPage", value: "20", enabled: true },
        ],
        body: { kind: "none" },
        auth: null,
        updatedAt: "",
      });
      b.mock.touchSource();
      await b.openapiSyncApply(p.id, null);
    });
    const send = vi.spyOn(mock, "requestSend");
    await openOp(user, "video-client", /取引先一覧取得/);
    const region = within(await screen.findByRole("region", { name: "リクエスト" }));

    // 値は残り、「yaml にない」の印が付く。チェックは外れて、押せない
    expect(await region.findByText("yaml にない")).toBeInTheDocument();
    expect(region.getByLabelText("クエリパラメータ 2 行目のキー")).toHaveValue("contractType");
    expect(region.getByLabelText("クエリパラメータ 2 行目の値")).toHaveValue("DIRECT");
    const check = region.getByRole("checkbox", { name: "クエリパラメータ 2 行目は yaml にないので送信されません" });
    expect(check).toBeDisabled();
    expect(check).not.toBeChecked();

    // 送信すると、contractType は付かない（URL の確認にも出ない）
    await waitFor(() => expect(screen.getByRole("status", { name: "展開後の URL" })).toHaveTextContent("q=Netflix&perPage=20"));
    expect(screen.getByRole("status", { name: "展開後の URL" })).not.toHaveTextContent("contractType");
    await user.click(screen.getByRole("button", { name: "送信" }));
    await screen.findByLabelText("ステータス");
    const sent = send.mock.calls[0][0].request.params;
    expect(sent.find((p) => p.name === "contractType")).toMatchObject({ enabled: false });
    expect(sent.find((p) => p.name === "q")).toMatchObject({ enabled: true });

    // 行を消して保存すると、印も外れる
    await user.click(region.getByRole("button", { name: "クエリパラメータ 2 行目を削除" }));
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(region.queryByText("yaml にない")).not.toBeInTheDocument());
    expect((await mock.draftGet(project.id, key("listClients")))?.params.map((p) => p.name)).toEqual(["q", "perPage"]);
  });

  it("開いているタブは、同期のあと、入力した内容はそのままに、yaml 側の情報（yaml にない）だけ更新される", async () => {
    const { user, mock } = await setupWithApi();
    await openOp(user, "video-client", /取引先一覧取得/);
    await user.click(await screen.findByRole("checkbox", { name: "クエリパラメータ 2 行目を有効にする" }));
    await user.click(screen.getByRole("button", { name: "保存" }));
    const tab = () => screen.getByRole("tab", { name: /取引先一覧取得/ });
    await waitFor(() => expect(within(tab()).queryByRole("img", { name: "未保存" })).not.toBeInTheDocument());
    // 保存したあとに、さらに入力する（未保存の変更）
    const q = screen.getByLabelText("クエリパラメータ 1 行目の値");
    await user.clear(q);
    await user.type(q, "Disney");
    expect(screen.queryByText("yaml にない")).not.toBeInTheDocument();

    // yaml を更新して、同期ボタンから反映する
    mock.mock.touchSource();
    await user.click(within(sidebar()).getByRole("button", { name: /OpenAPI を同期/ }));
    const dialog = await screen.findByRole("dialog", { name: "OpenAPI の同期" });
    await user.click(await within(dialog).findByRole("button", { name: "反映する" }));
    await within(dialog).findByText(/反映しました/);
    await user.click(footerButton(dialog, "閉じる"));

    // 同じタブのまま、「yaml にない」の印が付く。入力した内容（未保存の Disney も）は消えない
    expect(await screen.findByText("yaml にない")).toBeInTheDocument();
    expect(screen.getByLabelText("クエリパラメータ 1 行目の値")).toHaveValue("Disney");
    expect(screen.getByLabelText("クエリパラメータ 2 行目のキー")).toHaveValue("contractType");
    expect(within(tab()).getByRole("img", { name: "未保存" })).toBeInTheDocument();
  });
});
