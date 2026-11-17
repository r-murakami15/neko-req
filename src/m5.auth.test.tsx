import { screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { Project } from "./bindings";
import { createMockBackend, type MockBackend } from "./mock/backend";
import { footerButton, openOp, requestRegion, setupWithApi } from "./test/openapi";

/** 取り込み済みのプロジェクトの環境に、変数を足す。 */
async function addVars(b: MockBackend, p: Project, vars: Record<string, string>) {
  const env = (await b.envList(p.id)).find((e) => !e.isGlobal)!;
  await b.envSave({
    ...env,
    vars: [...env.vars, ...Object.entries(vars).map(([name, value]) => ({ name, value: { kind: "plain" as const, value }, description: "", enabled: true }))],
  });
}

const resPane = () => screen.getByRole("region", { name: "レスポンス" });
const syncButton = () => screen.getByRole("button", { name: /OpenAPI を同期/ });

async function openTab(user: UserEvent, name: string | RegExp) {
  await user.click(await screen.findByRole("tab", { name }));
}

describe("認証タブ: 自動化のチェック（M5）", () => {
  it("2 つのチェックはプロジェクトの設定（autoSession・autoCsrf）に結びつき、保存される", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock, project } = await setupWithApi(base, base);
    const save = vi.spyOn(mock, "projectSettingsSet");
    await openOp(user, "video-client", /取引先一覧取得/);
    await openTab(user, /^認証/);

    const session = await screen.findByRole("checkbox", { name: /ログインの API の Set-Cookie から SESSION を自動で更新/ });
    const csrf = screen.getByRole("checkbox", { name: /GET 以外では CSRF トークンを自動で付ける/ });
    expect(session).toBeChecked();
    expect(csrf).toBeChecked();

    await user.click(session);
    await waitFor(() => expect(save).toHaveBeenLastCalledWith(project.id, { autoSession: false, autoCsrf: true }));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: /SESSION を自動で更新/ })).not.toBeChecked());

    await user.click(csrf);
    await waitFor(() => expect(save).toHaveBeenLastCalledWith(project.id, { autoSession: false, autoCsrf: false }));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: /CSRF トークンを自動で付ける/ })).not.toBeChecked());

    // 別のタブに移って戻っても、保存した設定のまま
    await openTab(user, "ヘッダー");
    await openTab(user, /^認証/);
    expect(screen.getByRole("checkbox", { name: /SESSION を自動で更新/ })).not.toBeChecked();
    expect((await mock.projectList())[0].settings).toEqual({ autoSession: false, autoCsrf: false });
  });

  it("「送信されるヘッダー」に Cookie と X-XSRF-TOKEN の両方を伏せ字で出す（自動化をオフにすると CSRF の分は付かない）", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user } = await setupWithApi(base, base, (p, b) => addVars(b, p, { session: "abc123", xsrfToken: "tok789" }));
    await openOp(user, "video-client", /取引先登録/); // POST
    await openTab(user, /^認証/);

    const box = await within(requestRegion()).findByLabelText("送信されるヘッダー");
    await waitFor(() => expect(box).toHaveTextContent("Cookie: SESSION=••••••••; XSRF-TOKEN=••••••••"));
    expect(box).toHaveTextContent("X-XSRF-TOKEN: ••••");
    expect(box).not.toHaveTextContent("abc123");
    expect(box).not.toHaveTextContent("tok789");

    // 自動化をオフにすると、どちらも付かない
    await user.click(screen.getByRole("checkbox", { name: /SESSION を自動で更新/ }));
    await user.click(screen.getByRole("checkbox", { name: /CSRF トークンを自動で付ける/ }));
    await waitFor(() => expect(within(requestRegion()).getByLabelText("送信されるヘッダー")).not.toHaveTextContent("XSRF-TOKEN"));
    // この API の既定の認証（cookieAuth）の SESSION だけは、自動化に関係なく付く
    expect(within(requestRegion()).getByLabelText("送信されるヘッダー")).toHaveTextContent("Cookie: SESSION=••••••••");
  });

  it("ヘッダータブの「自動で付与されるヘッダー」に Cookie・X-XSRF-TOKEN と出どころが出る（GET の X-XSRF-TOKEN は付かない）", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user } = await setupWithApi(base, base, (p, b) => addVars(b, p, { session: "abc123", xsrfToken: "tok789" }));
    await openOp(user, "video-client", /取引先登録/); // POST
    await openTab(user, /^ヘッダー/);
    const auto = await screen.findByRole("region", { name: "自動で付与されるヘッダー" });
    await waitFor(() => expect(within(auto).getByText("X-XSRF-TOKEN")).toBeInTheDocument());
    expect(auto).toHaveTextContent("SESSION=••••••••; XSRF-TOKEN=••••••••");
    expect(auto).toHaveTextContent("認証タブ（cookieAuth）、CSRF の自動付与");
    expect(auto).toHaveTextContent("CSRF の自動付与");

    // GET では Cookie の XSRF-TOKEN だけ（X-XSRF-TOKEN ヘッダーは付かない）
    await openOp(user, "video-client", /取引先一覧取得/);
    await openTab(user, /^ヘッダー/);
    const getAuto = await screen.findByRole("region", { name: "自動で付与されるヘッダー" });
    await waitFor(() => expect(getAuto).toHaveTextContent("XSRF-TOKEN=••••••••"));
    expect(within(getAuto).queryByText("X-XSRF-TOKEN")).not.toBeInTheDocument();
  });

  it("付かない理由（autoNotes）を、ヘッダータブと認証タブに出す: トークンがまだない・送り先が baseUrl と違う", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user } = await setupWithApi(base, base, (p, b) => addVars(b, p, { session: "abc123" }));
    // xsrfToken がまだない POST
    await openOp(user, "video-client", /取引先登録/);
    await openTab(user, /^ヘッダー/);
    const notes = await screen.findByRole("list", { name: "自動で付かないもの" });
    expect(notes).toHaveTextContent("CSRF トークン（xsrfToken）がまだありません");
    await openTab(user, /^認証/);
    expect(await within(requestRegion()).findByRole("list", { name: "自動で付かないもの" })).toHaveTextContent("CSRF トークン（xsrfToken）がまだありません");

    // baseUrl と違う送り先（手で作ったリクエスト）には、セッションを付けない
    await user.click(screen.getByRole("button", { name: "新しいタブ" }));
    const url = await screen.findByLabelText("リクエストURL");
    await user.click(url);
    await user.paste("https://other.example/api/v1/x");
    await openTab(user, /^ヘッダー/);
    await waitFor(() => expect(screen.getByRole("list", { name: "自動で付かないもの" })).toHaveTextContent("送り先が環境の baseUrl と違うため"));
    expect(within(screen.getByRole("region", { name: "自動で付与されるヘッダー" })).queryByText("Cookie")).not.toBeInTheDocument();
  });
});

describe("送信結果: 自動更新の通知（M5）", () => {
  async function sendAndWait(user: UserEvent) {
    // 送れる状態になる（展開後の URL が出る）のを待ってから押す
    await waitFor(() => expect(screen.getByRole("button", { name: "送信" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "送信" }));
    await screen.findByLabelText("ステータス");
  }

  it("session・xsrfToken を更新したら「更新しました（環境名）」と出し、消したら「消しました」と出す。環境の一覧も取り直す", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, project, mock } = await setupWithApi(base, base);
    const list = vi.spyOn(mock, "envList");

    await openOp(user, "auth", /パスワードログイン/); // POST /auth/password-login（SESSION を受け取る）
    await sendAndWait(user);
    const notice = within(resPane()).getByRole("status", { name: "認証の自動更新" });
    expect(notice).toHaveTextContent("session を更新しました（ローカル (Dev Container)）");
    const before = list.mock.calls.length;
    await waitFor(() => expect(list.mock.calls.length).toBeGreaterThan(0));
    // 値はシークレットとして環境に入る（画面には出さない）
    const env = (await mock.envList(project.id)).find((e) => !e.isGlobal)!;
    expect(env.vars.find((v) => v.name === "session")).toMatchObject({ value: { kind: "secret" }, description: "自動で更新されます（Set-Cookie）" });
    expect(notice).not.toHaveTextContent(/mock-session/);
    expect(list.mock.calls.length).toBeGreaterThanOrEqual(before);

    // GET /auth/session は XSRF-TOKEN を発行する。レスポンスのヘッダータブに注記も出る
    await openOp(user, "auth", /ログイン状態取得/);
    await sendAndWait(user);
    expect(within(resPane()).getByRole("status", { name: "認証の自動更新" })).toHaveTextContent("xsrfToken を更新しました（ローカル (Dev Container)）");
    await user.click(within(resPane()).getByRole("tab", { name: /^ヘッダー/ }));
    expect(within(resPane()).getByRole("note")).toHaveTextContent("XSRF-TOKEN は保持され、次の GET 以外のリクエストで X-XSRF-TOKEN ヘッダーに自動で入ります");

    // 変数が増えたので、環境変数の画面にも出る
    await user.click(screen.getByRole("button", { name: "環境変数を編集" }));
    expect(await screen.findByDisplayValue("xsrfToken")).toBeInTheDocument();
    expect(screen.getByDisplayValue("session")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "戻る" }));

    // ログアウト（DELETE /auth/session）は SESSION を消す
    await openOp(user, "auth", /ログアウト/);
    await sendAndWait(user);
    expect(within(resPane()).getByRole("status", { name: "認証の自動更新" })).toHaveTextContent("session を消しました（ローカル (Dev Container)）");
  });

  it("更新できなかったときは理由を出す（環境が選ばれていない）。自動化をオフにすると何も更新しない", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock } = await setupWithApi(base, base, async (p, b) => {
      // baseUrl はグローバルに置き、環境は選ばない
      const g = (await b.envList(p.id)).find((e) => e.isGlobal)!;
      await b.envSave({ ...g, vars: [{ name: "baseUrl", value: { kind: "plain", value: "http://localhost:8080" }, description: "", enabled: true }] });
      await b.envSelect(p.id, null);
    });
    await openOp(user, "auth", /パスワードログイン/);
    await sendAndWait(user);
    const notice = within(resPane()).getByRole("status", { name: "認証の自動更新" });
    expect(notice).toHaveTextContent("更新できませんでした: 環境が選ばれていないため、session は保存しませんでした");

    // 認証タブで SESSION の自動更新をオフにする
    await openTab(user, /^認証/);
    await user.click(screen.getByRole("checkbox", { name: /SESSION を自動で更新/ }));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: /SESSION を自動で更新/ })).not.toBeChecked());
    expect((await mock.projectList())[0].settings?.autoSession).toBe(false);
    await openOp(user, "auth", /ログイン状態取得/);
    await sendAndWait(user);
    expect(within(resPane()).getByRole("status", { name: "認証の自動更新" })).toHaveTextContent("xsrfToken");
    // session（autoSession オフ）の更新はない
    await openOp(user, "auth", /パスワードログイン/);
    await sendAndWait(user);
    expect(within(resPane()).queryByRole("status", { name: "認証の自動更新" })).not.toBeInTheDocument();
  });
});

describe("設計タブ: 名前つきの例（MediaDoc.examples）", () => {
  it("複数の例を切り替えて見られ、「ボディに反映」は選んでいる例が入る", async () => {
    const { user } = await setupWithApi();
    await openOp(user, "auth", /メール認証コードログイン/);
    await openTab(user, "設計");
    const doc = within(await screen.findByRole("article", { name: "設計" }));

    const switcher = doc.getByRole("tablist", { name: "リクエストボディの例を切り替え" });
    expect(within(switcher).getAllByRole("tab").map((t) => t.textContent)).toEqual(["正しいコード", "期限切れのコード"]);
    expect(within(switcher).getByRole("tab", { name: "正しいコード" })).toHaveAttribute("aria-selected", "true");
    expect(doc.getByLabelText("例（JSON）")).toHaveTextContent('"code": "123456"');
    expect(doc.getByText("メールで届いた 6 桁のコード")).toBeInTheDocument();

    await user.click(within(switcher).getByRole("tab", { name: "期限切れのコード" }));
    expect(doc.getByLabelText("例（JSON）")).toHaveTextContent('"code": "000000"');
    expect(doc.getByText("LOGIN_FAILED になる")).toBeInTheDocument();

    await user.click(doc.getByRole("button", { name: "ボディに反映" }));
    const editor = await screen.findByRole("textbox", { name: "JSON のボディ" });
    await waitFor(() => expect(editor).toHaveTextContent('"code": "000000"'));
    expect(editor).not.toHaveTextContent("123456");
  });

  it("例が 1 つだけの API では切り替えを出さない", async () => {
    const { user } = await setupWithApi();
    await openOp(user, "video-client", /取引先登録/);
    await openTab(user, "設計");
    const doc = within(await screen.findByRole("article", { name: "設計" }));
    expect(doc.queryByRole("tablist", { name: "リクエストボディの例を切り替え" })).not.toBeInTheDocument();
    expect(doc.getByLabelText("例（JSON）")).toHaveTextContent("テスト取引先");
  });
});

describe("同期ダイアログ: 別のホストの確認（needsConfirmation）", () => {
  it("確認が要るときは foreign のホストを見せ、許可すると allowedHosts つきで呼び直して、反映できる", async () => {
    const { user, mock } = await setupWithApi();
    mock.mock.touchSource();
    mock.mock.requireHostConfirm();
    const check = vi.spyOn(mock, "openapiSyncCheck");
    const apply = vi.spyOn(mock, "openapiSyncApply");

    await user.click(syncButton());
    const dialog = await screen.findByRole("dialog", { name: "OpenAPI の同期" });
    const ask = await within(dialog).findByRole("group", { name: "別のホストの確認" });
    expect(ask).toHaveTextContent("assets.other-host.example");
    expect(ask).toHaveTextContent("openapi.yaml:12");
    // 取り込み直しの案内ではなく、その場で許可できる
    expect(ask).not.toHaveTextContent("取り込み直す");
    expect(within(dialog).queryByRole("button", { name: "反映する" })).not.toBeInTheDocument();

    await user.click(within(ask).getByRole("button", { name: "許可して続ける" }));
    expect(await within(dialog).findByText(/追加 1 · 変更 1 · 削除 1/)).toBeInTheDocument();
    expect(check).toHaveBeenLastCalledWith(expect.any(String), null, ["assets.other-host.example"]);

    await user.click(within(dialog).getByRole("button", { name: "反映する" }));
    expect(await within(dialog).findByText(/反映しました/)).toBeInTheDocument();
    expect(apply).toHaveBeenCalledWith(expect.any(String), null, ["assets.other-host.example"]);
    await user.click(footerButton(dialog, "閉じる"));
  });
});
