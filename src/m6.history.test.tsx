import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { RequestInput } from "./bindings";
import { createMockBackend, type MockBackend } from "./mock/backend";
import { openOp, setupWithApi } from "./test/openapi";

afterEach(() => vi.restoreAllMocks());

const listRegion = () => screen.getByRole("region", { name: "履歴の一覧" });
const detail = () => screen.getByRole("region", { name: "履歴の詳細" });
const total = () => within(listRegion()).getByRole("status", { name: "件数" });
const rows = () => [...document.querySelectorAll<HTMLButtonElement>(".history-row")];
const rowByText = (re: RegExp) => within(listRegion()).getByRole("button", { name: re });

async function openHistory(user: UserEvent) {
  await user.click(screen.getByRole("button", { name: "履歴" }));
  await screen.findByRole("heading", { level: 1, name: "履歴" });
}

/** モックに直接リクエストを送り、履歴に残るまで待つ。 */
async function sendDirect(mock: MockBackend, projectId: string, input: Partial<RequestInput> & Pick<RequestInput, "request">) {
  const before = (await mock.historyList(projectId, {})).total;
  await mock.requestSend({ projectId, environmentId: null, overrides: [], opKey: null, ...input }, null);
  await waitFor(async () => expect((await mock.historyList(projectId, {})).total).toBe(before + 1));
}

const get = (path: string, extra: Partial<RequestInput["request"]> = {}): RequestInput["request"] => ({
  method: "GET",
  url: `{{baseUrl}}${path}`,
  params: [],
  body: { kind: "none" },
  auth: null,
  ...extra,
});

function mockClipboard() {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  return writeText;
}

describe("履歴: 記録と一覧", () => {
  it("送信のたびに残り、日付ごとの一覧に出る。詳細は送った内容と受け取ったレスポンス。保存する件数は設定から出す", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user } = await setupWithApi(base);
    await openOp(user, "video-client", /取引先一覧取得/);
    await waitFor(() => expect(screen.getByRole("button", { name: "送信" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "送信" }));
    await screen.findByLabelText("ステータス");

    await openHistory(user);
    expect(screen.getByText("直近 1,000 件まで保存")).toBeInTheDocument();
    expect(await within(listRegion()).findByText("今日")).toBeInTheDocument();
    expect(total()).toHaveTextContent("1 件");
    const row = rowByText(/GET \/api\/v1\/video\/clients 200/);
    expect(row).toHaveAttribute("aria-current", "true"); // 先頭が選ばれている

    const d = within(detail());
    expect(await d.findByRole("heading", { level: 2, name: "取引先一覧取得" })).toBeInTheDocument();
    expect(d.getByLabelText("ステータス")).toHaveTextContent("200");
    expect(d.getByText(/ローカル \(Dev Container\) · \d{4}\/\d{2}\/\d{2} \d{2}:\d{2}/)).toBeInTheDocument();
    expect(d.getByLabelText("送信した内容")).toHaveTextContent("User-Agent");
    expect(d.getByLabelText("受け取ったボディ")).toHaveTextContent('"clientName"');

    // 戻るとメインに戻る
    await user.click(screen.getByRole("button", { name: "戻る" }));
    expect(await screen.findByRole("navigation", { name: "API ツリー" })).toBeInTheDocument();
  });

  it("シークレットは伏せ字で残る（手で書いた認証・環境変数のシークレット・Set-Cookie）。注記も出る", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base, async (p, b) => {
      const env = (await b.envList(p.id)).find((e) => !e.isGlobal)!;
      await b.envSave({ ...env, vars: [...env.vars, { name: "session", value: { kind: "secret" }, description: "", enabled: true }] });
      await b.secretSet(p.id, env.id, "session", "super-secret-session");
    });
    await sendDirect(mock, project.id, {
      request: get("/api/v1/auth/session", { auth: { type: "bearer", token: "manual-token-xyz" }, params: [{ location: "query", name: "s", value: "{{session}}", enabled: true }] }),
      overrides: [{ name: "extra", value: "override-secret" }],
    });
    await openHistory(user);
    const d = within(detail());
    const sent = await d.findByLabelText("送信した内容");
    expect(sent).toHaveTextContent("Authorization: ••••");
    expect(detail()).not.toHaveTextContent("manual-token-xyz");
    expect(detail()).not.toHaveTextContent("super-secret-session");
    expect(detail()).not.toHaveTextContent("override-secret");
    expect(detail().textContent).toContain("/api/v1/auth/session?s=••••");
    // 受け取った Set-Cookie も伏せ字
    await user.click(d.getByText(/レスポンスヘッダー/));
    expect(detail()).toHaveTextContent("set-cookie: ••••");
    // 手で書いた認証の値と、変数の上書きの注記
    expect(detail()).toHaveTextContent("手で書いた認証の値は伏せ字（••••）で保存しています: 認証（Bearer のトークン）");
    expect(detail()).toHaveTextContent("送信のときに指定していた変数の上書き（extra）は、値を保存していません");
  });

  it("失敗は「失敗」と種類・メッセージ。problem+json は code と detail を目立たせる。バイナリ・先頭だけ保存の注記も出る", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    await sendDirect(mock, project.id, { request: get("/refused") });
    await sendDirect(mock, project.id, { request: get("/error") });
    await sendDirect(mock, project.id, { request: get("/binary") });
    await sendDirect(mock, project.id, { request: get("/large") });
    await openHistory(user);
    await waitFor(() => expect(rows()).toHaveLength(4));

    // /large（新しい順の先頭）: 先頭だけ保存
    expect(await within(detail()).findByText(/先頭だけ保存しています（全体は 24\.0 MB）/)).toBeInTheDocument();
    // /binary
    await user.click(rowByText(/binary 200/));
    expect(await within(detail()).findByText(/バイナリのボディです（application\/pdf・9 B）。履歴には中身を保存していません/)).toBeInTheDocument();
    // /error: problem+json
    await user.click(rowByText(/error 400/));
    const problem = await within(detail()).findByRole("group", { name: "エラーの内容" });
    expect(problem).toHaveTextContent("VALIDATION_ERROR");
    expect(problem).toHaveTextContent("入力内容に誤りがあります（email: 必須です）");
    // /refused: 失敗の種類
    await user.click(rowByText(/refused 失敗/));
    const failure = await within(detail()).findByRole("group", { name: "失敗の内容" });
    expect(failure).toHaveTextContent("接続できません");
    expect(failure).toHaveTextContent("接続を拒否されました");
    expect(within(detail()).getByLabelText("ステータス")).toHaveTextContent("失敗");
  });
});

describe("履歴: 絞り込みと検索", () => {
  it("ステータス・環境・期間で絞り込み、URL とボディで検索できる", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    await sendDirect(mock, project.id, { request: get("/api/v1/real") }); // 環境つきの 1 件
    mock.mock.seedHistory(project.id, 12); // 400 ×2・200 ×4・422 ×2・500 ×2・失敗 ×2
    await openHistory(user);
    await waitFor(() => expect(total()).toHaveTextContent("13 件"));

    const chips = screen.getByRole("group", { name: "絞り込み" });
    await user.click(within(chips).getByRole("button", { name: "4xx" }));
    await waitFor(() => expect(total()).toHaveTextContent("4 件（絞り込み中）"));
    await user.click(within(chips).getByRole("button", { name: "5xx" }));
    await waitFor(() => expect(total()).toHaveTextContent("6 件"));
    await user.click(within(chips).getByRole("button", { name: "失敗" }));
    await waitFor(() => expect(total()).toHaveTextContent("8 件"));
    await user.click(within(chips).getByRole("button", { name: "すべて" }));
    await waitFor(() => expect(total()).toHaveTextContent("13 件"));
    expect(total()).not.toHaveTextContent("絞り込み中");

    // 環境
    await user.selectOptions(screen.getByLabelText("環境で絞り込み"), "ローカル (Dev Container)");
    await waitFor(() => expect(total()).toHaveTextContent("1 件（絞り込み中）"));
    await user.selectOptions(screen.getByLabelText("環境で絞り込み"), "すべての環境");
    await waitFor(() => expect(total()).toHaveTextContent("13 件"));

    // 検索（URL）
    await user.type(screen.getByLabelText("URLやボディで検索"), "password-login");
    await waitFor(() => expect(total()).toHaveTextContent("2 件"));
    await user.clear(screen.getByLabelText("URLやボディで検索"));
    // 検索（レスポンスのボディ）
    await user.type(screen.getByLabelText("URLやボディで検索"), "取引先 6");
    await waitFor(() => expect(total()).toHaveTextContent("1 件"));
    await user.clear(screen.getByLabelText("URLやボディで検索"));
    await user.type(screen.getByLabelText("URLやボディで検索"), "存在しない文字列");
    expect(await within(listRegion()).findByText("条件に当てはまる履歴はありません。")).toBeInTheDocument();
    await user.clear(screen.getByLabelText("URLやボディで検索"));
    await waitFor(() => expect(total()).toHaveTextContent("13 件"));

    // 期間（ローカルの日付）
    await user.click(within(chips).getByRole("button", { name: "期間" }));
    const from = screen.getByLabelText("期間の開始");
    fireEvent.change(from, { target: { value: "2999-01-01" } });
    await waitFor(() => expect(total()).toHaveTextContent("0 件"));
    await user.click(screen.getByRole("button", { name: "期間をクリア" }));
    await waitFor(() => expect(total()).toHaveTextContent("13 件"));
  });

  it("日付は端末の時差（utcOffsetMinutes）で区切り、続きは nextCursor で読む。仮想スクロールで描くのは見えている行だけ", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    mock.mock.seedHistory(project.id, 130);
    const list = vi.spyOn(mock, "historyList");
    await openHistory(user);
    await waitFor(() => expect(total()).toHaveTextContent("130 件"));
    expect(list.mock.calls[0][1]).toMatchObject({ utcOffsetMinutes: -new Date().getTimezoneOffset(), limit: 100 });
    // 100 件読んで、描いているのはそのうちの見えている分だけ
    const drawn = rows().length;
    expect(drawn).toBeGreaterThan(5);
    expect(drawn).toBeLessThan(60);

    await user.click(screen.getByRole("button", { name: /さらに読み込む（残り 30 件）/ }));
    await waitFor(() => expect(list.mock.calls.at(-1)![1]).toMatchObject({ cursor: "100" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: /さらに読み込む/ })).not.toBeInTheDocument());
  });
});

describe("履歴: 操作", () => {
  it("再送信: 元の入力をタブで開き、そのタブのレスポンス欄に結果が出る。新しい履歴もできる", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    await sendDirect(mock, project.id, { request: get("/api/v1/video/clients", { params: [{ location: "query", name: "q", value: "Netflix", enabled: true }] }), opKey: { kind: "id", value: "listClients" } });
    const resend = vi.spyOn(mock, "historyResend");
    await openHistory(user);
    await within(detail()).findByRole("heading", { level: 2, name: "取引先一覧取得" });
    await user.click(within(detail()).getByRole("button", { name: "再送信" }));

    // メインに戻り、元の API のタブでレスポンスが出る
    expect(await screen.findByRole("tab", { name: /取引先一覧取得/ })).toBeInTheDocument();
    expect(await within(screen.getByRole("region", { name: "レスポンス" })).findByLabelText("ステータス")).toHaveTextContent("200");
    expect(resend).toHaveBeenCalledWith(expect.any(String), null);
    expect(screen.getByLabelText("クエリパラメータ 1 行目の値")).toHaveValue("Netflix");
    await waitFor(async () => expect((await mock.historyList(project.id, {})).total).toBe(2));
  });

  it("編集して開く: 新しいタブに未保存の入力として開く。外した認証の値は、タブの上で知らせる", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    await sendDirect(mock, project.id, {
      request: get("/api/v1/video/clients", { auth: { type: "bearer", token: "manual-token-xyz" } }),
      opKey: { kind: "id", value: "listClients" },
    });
    await openHistory(user);
    await within(detail()).findByRole("heading", { level: 2, name: "取引先一覧取得" });
    await user.click(within(detail()).getByRole("button", { name: "編集して開く" }));

    const tab = await screen.findByRole("tab", { name: /取引先一覧取得/ });
    expect(within(tab).getByRole("img", { name: "未保存" })).toBeInTheDocument();
    const note = await screen.findByText(/履歴には伏せ字で残っているため、手で入力した認証の値は外しました/);
    expect(note).toHaveTextContent("認証（Bearer のトークン）");
    await user.click(screen.getByRole("tab", { name: /^認証/ }));
    expect(screen.getByLabelText("トークン")).toHaveValue("");
    // 閉じると消える
    await user.click(screen.getByRole("button", { name: "閉じる" }));
    expect(screen.queryByText(/履歴には伏せ字で残っているため/)).not.toBeInTheDocument();
  });

  it("入力値として保存: API なら入力値（RequestDraft）に、手で作ったものは「その他」に保存する", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    await sendDirect(mock, project.id, { request: get("/api/v1/video/clients", { params: [{ location: "query", name: "q", value: "Netflix", enabled: true }] }), opKey: { kind: "id", value: "listClients" } });
    await sendDirect(mock, project.id, { request: get("/api/v1/legacy/other") });
    await openHistory(user);
    // 新しい順: /legacy/other が先頭（opKey なし）
    await within(detail()).findByRole("button", { name: "入力値として保存" });
    await user.click(within(detail()).getByRole("button", { name: "入力値として保存" }));
    expect(await within(detail()).findByText(/「その他」に「GET \/api\/v1\/legacy\/other」として保存しました/)).toBeInTheDocument();

    await user.click(rowByText(/clients\?q=Netflix 200/));
    await within(detail()).findByRole("heading", { level: 2, name: "取引先一覧取得" });
    await user.click(within(detail()).getByRole("button", { name: "入力値として保存" }));
    expect(await within(detail()).findByText("この API の入力値として保存しました")).toBeInTheDocument();
    expect(await mock.draftGet(project.id, { kind: "id", value: "listClients" })).toMatchObject({ params: [{ name: "q", value: "Netflix" }] });
    expect((await mock.adhocList(project.id)).map((a) => a.name)).toEqual(["GET /api/v1/legacy/other"]);
  });

  it("cURL としてコピー: 履歴の内容（伏せ字のまま）をクリップボードに入れる", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    const writeText = mockClipboard();
    await sendDirect(mock, project.id, { request: get("/api/v1/auth/session", { auth: { type: "bearer", token: "manual-token-xyz" } }) });
    await openHistory(user);
    await within(detail()).findByRole("button", { name: "cURL としてコピー" });
    await user.click(within(detail()).getByRole("button", { name: "cURL としてコピー" }));
    expect(await within(detail()).findByText(/cURL をコピーしました/)).toBeInTheDocument();
    const text = writeText.mock.calls[0][0] as string;
    expect(text).toContain("curl -X GET 'http://localhost:8080/api/v1/auth/session'");
    expect(text).toContain("-H 'Authorization: ••••'");
    expect(text).not.toContain("manual-token-xyz");
  });

  it("別の履歴と比較: 2 件目を選ぶと、ステータス・ヘッダー・ボディを行単位の差分で見せる", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    await sendDirect(mock, project.id, { request: get("/api/v1/ok") }); // 200
    await sendDirect(mock, project.id, { request: get("/error") }); // 400 problem+json
    const compare = vi.spyOn(mock, "historyCompare");
    await openHistory(user);
    await waitFor(() => expect(rows()).toHaveLength(2));
    // 先頭（/error 400）を選んだまま、比較を始める
    await user.click(within(detail()).getByRole("button", { name: "別の履歴と比較" }));
    expect(screen.getByText("比較する 2 件目の履歴を、一覧から選んでください。")).toBeInTheDocument();
    await user.click(within(listRegion()).getByRole("button", { name: /\/api\/v1\/ok 200（この履歴と比較する）/ }));

    const region = await screen.findByRole("region", { name: "履歴の比較" });
    expect(compare).toHaveBeenCalledTimes(1);
    expect(within(region).getByRole("group", { name: "左（最初に選んだ履歴）" })).toHaveTextContent("/error");
    expect(within(region).getByRole("group", { name: "右（比べる履歴）" })).toHaveTextContent("/api/v1/ok");
    expect(within(region).getByLabelText("ステータスの比較")).toHaveTextContent("400 Bad Request → 200 OK");
    const bodyDiff = within(region).getByRole("table", { name: "ボディの差分" });
    expect(within(bodyDiff).getAllByRole("row").some((r) => r.className.includes("is-delete") && r.textContent?.includes("VALIDATION_ERROR"))).toBe(true);
    expect(within(bodyDiff).getAllByRole("row").some((r) => r.className.includes("is-insert") && r.textContent?.includes("clientId"))).toBe(true);
    expect(within(region).getByText("JSON として整形して比べました")).toBeInTheDocument();
    // ヘッダー: content-type が変わる
    expect(within(region).getByRole("table", { name: "ヘッダーの差分" })).toHaveTextContent("application/problem+json");

    await user.click(within(region).getByRole("button", { name: "比較をやめる" }));
    expect(screen.queryByRole("region", { name: "履歴の比較" })).not.toBeInTheDocument();
    expect(within(detail()).getByRole("button", { name: "別の履歴と比較" })).toBeInTheDocument();
  });

  it("同じレスポンスどうしの比較は「違いはありません」と出す", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    await sendDirect(mock, project.id, { request: get("/api/v1/same") });
    await sendDirect(mock, project.id, { request: get("/api/v1/same") });
    await openHistory(user);
    await waitFor(() => expect(rows()).toHaveLength(2));
    await user.click(within(detail()).getByRole("button", { name: "別の履歴と比較" }));
    await user.click(within(listRegion()).getByRole("button", { name: /この履歴と比較する/ }));
    expect(await screen.findByText("ステータス・ヘッダー・ボディに違いはありません。")).toBeInTheDocument();
  });

  it("1 件削除とすべて削除は確認つき（取り消せないことを伝える）", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    mock.mock.seedHistory(project.id, 3);
    const del = vi.spyOn(mock, "historyDelete");
    const clear = vi.spyOn(mock, "historyClear");
    await openHistory(user);
    await waitFor(() => expect(total()).toHaveTextContent("3 件"));

    await user.click(within(detail()).getByRole("button", { name: "この履歴を削除" }));
    let dialog = await screen.findByRole("dialog", { name: "この履歴を削除しますか？" });
    expect(dialog).toHaveTextContent("この操作は取り消せません");
    await user.click(within(dialog).getByRole("button", { name: "キャンセル" }));
    expect(del).not.toHaveBeenCalled();

    await user.click(within(detail()).getByRole("button", { name: "この履歴を削除" }));
    dialog = await screen.findByRole("dialog", { name: "この履歴を削除しますか？" });
    await user.click(within(dialog).getByRole("button", { name: "削除する" }));
    await waitFor(() => expect(total()).toHaveTextContent("2 件"));
    expect(del).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "すべて削除" }));
    dialog = await screen.findByRole("dialog", { name: "履歴をすべて削除しますか？" });
    expect(dialog).toHaveTextContent("この操作は取り消せません");
    await user.click(within(dialog).getByRole("button", { name: "すべて削除する" }));
    expect(await within(listRegion()).findByText(/まだ履歴がありません/)).toBeInTheDocument();
    expect(clear).toHaveBeenCalledWith(project.id);
    expect(screen.getByRole("button", { name: "すべて削除" })).toBeDisabled();
  });

  it("設定で履歴を保存しない（0 件）にすると、送っても残らない。件数の表示も変わる", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base, async (_p, b) => {
      const s = await b.appSettingsGet();
      await b.appSettingsSet({ ...s, history: { ...s.history, limit: 0 } });
    });
    await mock.requestSend({ projectId: project.id, environmentId: null, overrides: [], opKey: null, request: get("/api/v1/x") }, null);
    await new Promise((r) => setTimeout(r, 60));
    await openHistory(user);
    expect(await screen.findByText("履歴は保存しない設定です")).toBeInTheDocument();
    expect(await within(listRegion()).findByText(/まだ履歴がありません/)).toBeInTheDocument();
  });
});
