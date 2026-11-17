import { screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Project } from "./bindings";
import { createMockBackend, type MockBackend } from "./mock/backend";
import { openOp, setupWithApi } from "./test/openapi";

afterEach(() => vi.restoreAllMocks());

async function openCurlDialog(user: UserEvent) {
  await user.click(screen.getByRole("button", { name: "新しいタブ" }));
  return screen.findByRole("dialog", { name: "新規作成" });
}

/** cURL を貼り付けて、解析の結果が出るのを待つ。 */
async function pasteCurl(user: UserEvent, dialog: HTMLElement, text: string) {
  const box = within(dialog).getByLabelText("cURL コマンド");
  await user.click(box);
  await user.paste(text);
}

async function addVars(b: MockBackend, p: Project, vars: Record<string, string>) {
  const env = (await b.envList(p.id)).find((e) => !e.isGlobal)!;
  await b.envSave({
    ...env,
    vars: [...env.vars, ...Object.entries(vars).map(([name, value]) => ({ name, value: { kind: "plain" as const, value }, description: "", enabled: true }))],
  });
}

function mockClipboard() {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  return writeText;
}

describe("新規作成: cURL から作成", () => {
  it("OpenAPI の API に当てはまると「createPasswordLogin と一致」と出て、その API のタブに未保存の入力として開く", async () => {
    const { user } = await setupWithApi();
    const dialog = await openCurlDialog(user);
    const create = within(dialog).getByRole("button", { name: "cURL から作成" });
    expect(create).toBeDisabled();

    await pasteCurl(
      user,
      dialog,
      "curl -X POST http://localhost:8080/api/v1/auth/password-login \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"email\":\"user@example.com\",\"password\":\"********\"}'",
    );
    const result = await within(dialog).findByRole("status", { name: "cURL の解析結果" });
    expect(result).toHaveTextContent("POST /api/v1/auth/password-login · JSON ボディ · OpenAPI の createPasswordLogin と一致");
    expect(result).toHaveTextContent("ホストを {{baseUrl}} に置き換えました");
    await user.click(create);

    const tab = await screen.findByRole("tab", { name: /パスワードログイン \(1 段階目\)/ });
    expect(within(tab).getByRole("img", { name: "未保存" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "新規作成" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("リクエストURL")).toHaveValue("{{baseUrl}}/api/v1/auth/password-login");
    await user.click(screen.getByRole("tab", { name: "ボディ" }));
    const editor = await screen.findByRole("textbox", { name: "JSON のボディ" });
    await waitFor(() => expect(editor).toHaveTextContent('"email": "user@example.com"'));
    // yaml の API なので、設計タブもある
    expect(screen.getByRole("tab", { name: "設計" })).toBeInTheDocument();
  });

  it("path パラメータ・クエリは、当てはまった API の入力値に入る", async () => {
    const { user } = await setupWithApi();
    const dialog = await openCurlDialog(user);
    await pasteCurl(user, dialog, "curl -X DELETE 'http://localhost:8080/api/v1/video/clients/42'");
    const result = await within(dialog).findByRole("status", { name: "cURL の解析結果" });
    expect(result).toHaveTextContent("DELETE /api/v1/video/clients/{clientId}");
    expect(result).toHaveTextContent("deleteClient と一致");
    await user.click(within(dialog).getByRole("button", { name: "cURL から作成" }));
    await screen.findByRole("tab", { name: /取引先削除|deleteClient|DELETE/ });
    expect(await screen.findByDisplayValue("42")).toBeInTheDocument();
  });

  it("当てはまらないときは「その他」の新しいタブで開く（ホストが違う cURL は絶対 URL のまま）", async () => {
    const { user } = await setupWithApi();
    const dialog = await openCurlDialog(user);
    await pasteCurl(user, dialog, "curl 'https://other.example/api/x?y=1' -H 'X-Foo: bar'");
    const result = await within(dialog).findByRole("status", { name: "cURL の解析結果" });
    expect(result).toHaveTextContent("「その他」の新しいリクエストとして開きます · GET https://other.example/api/x?y=1 · ボディなし");
    expect(result).not.toHaveTextContent("置き換えました");
    await user.click(within(dialog).getByRole("button", { name: "cURL から作成" }));

    const tab = await screen.findByRole("tab", { name: /GET \/api\/x/ });
    expect(within(tab).getByRole("img", { name: "未保存" })).toBeInTheDocument();
    expect(screen.getByLabelText("リクエストURL")).toHaveValue("https://other.example/api/x?y=1");
    await user.click(screen.getByRole("tab", { name: /^ヘッダー/ }));
    expect(await screen.findByDisplayValue("X-Foo")).toBeInTheDocument();
    expect(screen.getByDisplayValue("bar")).toBeInTheDocument();
    // yaml の API ではないので、設計タブには何もない
  });

  it("baseUrl に当てはまるが API にない path は、{{baseUrl}} を使った「その他」になる", async () => {
    const { user } = await setupWithApi();
    const dialog = await openCurlDialog(user);
    await pasteCurl(user, dialog, "curl -X PATCH http://localhost:8080/api/v1/unknown -d 'a=1'");
    const result = await within(dialog).findByRole("status", { name: "cURL の解析結果" });
    expect(result).toHaveTextContent("「その他」の新しいリクエストとして開きます · PATCH {{baseUrl}}/api/v1/unknown · フォームのボディ");
    expect(result).toHaveTextContent("ホストを {{baseUrl}} に置き換えました");
  });

  it("無視したオプション・読み取れないオプションは警告として出す。読めない入力はエラーにして、作れない", async () => {
    const { user } = await setupWithApi();
    const dialog = await openCurlDialog(user);
    await pasteCurl(user, dialog, "curl -s -L --weird-option http://localhost:8080/api/v1/auth/session");
    const warnings = await within(dialog).findByRole("list", { name: "cURL の警告" });
    expect(within(warnings).getAllByRole("listitem")).toHaveLength(3);
    expect(warnings).toHaveTextContent("無視");
    expect(warnings).toHaveTextContent("-s は無視しました");
    expect(warnings).toHaveTextContent("不明");
    expect(warnings).toHaveTextContent("--weird-option は読み取れないオプションなので無視しました");
    // 警告があっても作れる
    expect(within(dialog).getByRole("button", { name: "cURL から作成" })).toBeEnabled();
    await user.click(within(dialog).getByLabelText("cURL コマンド"));
    await user.clear(within(dialog).getByLabelText("cURL コマンド"));
    await user.paste("wget http://localhost:8080/x");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("curl で始まるコマンドを貼り付けてください");
    expect(within(dialog).getByRole("button", { name: "cURL から作成" })).toBeDisabled();
    // 空にすると何も出ない
    await user.clear(within(dialog).getByLabelText("cURL コマンド"));
    await waitFor(() => expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument());
    expect(within(dialog).queryByRole("status", { name: "cURL の解析結果" })).not.toBeInTheDocument();
  });

  it("入力のたびに（少し待って）解析する", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock } = await setupWithApi(base, base);
    const parse = vi.spyOn(mock, "curlParse");
    const dialog = await openCurlDialog(user);
    await user.click(within(dialog).getByLabelText("cURL コマンド"));
    await user.keyboard("curl http://localhost:8080/api/v1/auth/session");
    await within(dialog).findByRole("status", { name: "cURL の解析結果" });
    // 1 文字ずつの入力では呼ばれず、止まってから 1 回
    expect(parse.mock.calls.length).toBeLessThanOrEqual(2);
    expect(parse).toHaveBeenLastCalledWith(expect.any(String), "curl http://localhost:8080/api/v1/auth/session", null);
  });
});

describe("今のリクエストを cURL としてコピー", () => {
  async function setup() {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const ctx = await setupWithApi(base, base, (p, b) => addVars(b, p, { session: "abc123", xsrfToken: "tok789" }));
    await openOp(ctx.user, "video-client", /取引先登録/); // POST
    await waitFor(() => expect(screen.getByRole("button", { name: "送信" })).toBeEnabled());
    return ctx;
  }

  it("既定は秘密の値を伏せてコピーする", async () => {
    const { user, mock } = await setup();
    const writeText = mockClipboard();
    const exp = vi.spyOn(mock, "curlExport");
    await user.click(screen.getByRole("button", { name: "cURL のメニュー" }));
    await user.click(await screen.findByRole("menuitem", { name: "cURL としてコピー（秘密は伏せる）" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(exp).toHaveBeenCalledWith(expect.objectContaining({ opKey: { kind: "id", value: "createClient" } }), "redacted");
    const text = writeText.mock.calls[0][0] as string;
    expect(text).toContain("curl -X POST 'http://localhost:8080/api/v1/video/clients'");
    expect(text).toContain("-H 'Cookie: ••••'");
    expect(text).toContain("-H 'X-XSRF-TOKEN: ••••'");
    expect(text).toContain("-d '{");
    expect(text).not.toContain("abc123");
    expect(text).not.toContain("tok789");
    expect(await screen.findByText("cURL をコピーしました（秘密の値は伏せています）")).toBeInTheDocument();
  });

  it("そのまま（raw）は、秘密が平文でクリップボードに入ることを確認してからコピーする", async () => {
    const { user, mock } = await setup();
    const writeText = mockClipboard();
    const exp = vi.spyOn(mock, "curlExport");
    await user.click(screen.getByRole("button", { name: "cURL のメニュー" }));
    await user.click(await screen.findByRole("menuitem", { name: /秘密もそのまま/ }));

    let dialog = await screen.findByRole("dialog", { name: "秘密の値をそのままコピーしますか？" });
    expect(dialog).toHaveTextContent("平文のまま、クリップボードに入ります");
    await user.click(within(dialog).getByRole("button", { name: "キャンセル" }));
    expect(exp).not.toHaveBeenCalled();
    expect(writeText).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "cURL のメニュー" }));
    await user.click(await screen.findByRole("menuitem", { name: /秘密もそのまま/ }));
    dialog = await screen.findByRole("dialog", { name: "秘密の値をそのままコピーしますか？" });
    await user.click(within(dialog).getByRole("button", { name: "そのままコピー" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(exp).toHaveBeenCalledWith(expect.anything(), "raw");
    const text = writeText.mock.calls[0][0] as string;
    expect(text).toContain("SESSION=abc123");
    expect(text).toContain("X-XSRF-TOKEN: tok789");
    expect(await screen.findByText("cURL をコピーしました（秘密の値を含みます）")).toBeInTheDocument();
  });

  it("展開できない変数があるときは、メッセージを出す", async () => {
    const { user } = await setupWithApi();
    await user.click(screen.getByRole("button", { name: "新しいタブ" }));
    await user.click(await screen.findByRole("button", { name: /HTTP リクエスト/ }));
    const url = await screen.findByLabelText("リクエストURL");
    await user.click(url);
    await user.paste("{{nope}}/x");
    await user.click(screen.getByRole("button", { name: "cURL のメニュー" }));
    await user.click(await screen.findByRole("menuitem", { name: "cURL としてコピー（秘密は伏せる）" }));
    expect(await screen.findByText(/cURL にできませんでした: 展開できない変数があります: \{\{nope\}\}/)).toBeInTheDocument();
  });
});
