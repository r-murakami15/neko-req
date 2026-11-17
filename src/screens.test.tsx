import { screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { Backend } from "./api/backend";
import type { RequestFinished } from "./bindings";
import { createMockBackend } from "./mock/backend";
import { renderApp } from "./test/render";

/** プロジェクトと（任意で）baseUrl を持つ環境を作って、アプリを描画する。 */
async function setup({ delay = 20, withEnv = true }: { delay?: number; withEnv?: boolean } = {}) {
  const backend = createMockBackend([], { sendDelayMs: delay });
  const project = await backend.projectCreate("日本アニメーション API");
  if (withEnv) {
    const env = await backend.envCreate(project.id, "ローカル (Dev Container)");
    await backend.envSave({
      ...env,
      vars: [{ name: "baseUrl", value: { kind: "plain", value: "http://localhost:8080" }, description: "", enabled: true }],
    });
  }
  const view = renderApp(backend);
  await screen.findByRole("banner");
  return { ...view, project };
}

async function openNewRequest(user: UserEvent) {
  await user.click(screen.getByRole("button", { name: "新しいタブ" }));
  await user.click(await screen.findByRole("button", { name: /HTTP リクエスト/ }));
  await screen.findByRole("region", { name: "リクエスト" });
}

/** 入力欄を空にして、`{{` を含む文字列をそのまま貼り付ける（user.type は `{` を特別扱いするため）。 */
async function setField(user: UserEvent, field: HTMLElement, text: string) {
  await user.clear(field);
  await user.click(field);
  await user.paste(text);
}

const expanded = () => screen.getByRole("status", { name: "展開後の URL" });
const responseRegion = () => screen.getByRole("region", { name: "レスポンス" });
const sendButton = () => screen.queryByRole("button", { name: "送信" });

describe("展開できない変数", () => {
  it("あると送信できず、場所が赤で示される。環境に入れると送れるようになる", async () => {
    const { user } = await setup({ withEnv: false });
    await openNewRequest(user);

    const url = screen.getByLabelText("リクエストURL");
    await setField(user, url, "{{baseUrl}}/api/v1/auth/session");

    // 展開後は途中の姿のまま。展開できない変数は URL 欄の中でも赤で示す
    await waitFor(() => expect(expanded()).toHaveTextContent("展開後: http://{{baseUrl}}/api/v1/auth/session"));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("{{baseUrl}}（URL）");
    expect(url).toHaveAttribute("aria-invalid", "true");
    expect(document.querySelector(".var-token-bad")).toHaveTextContent("{{baseUrl}}");
    expect(sendButton()).toBeDisabled();

    // パラメータの 1 行目の値に置いても、その場所が出る
    await user.click(screen.getByRole("button", { name: "クエリパラメータを追加" }));
    await setField(user, screen.getByLabelText("クエリパラメータ 1 行目のキー"), "q");
    await setField(user, screen.getByLabelText("クエリパラメータ 1 行目の値"), "{{keyword}}");
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("{{keyword}}（パラメータ（クエリ） 1 行目の値）"),
    );
    expect(screen.getByRole("tab", { name: /^パラメータ/ })).toHaveClass("is-bad");

    // 環境変数の画面で baseUrl を入れる
    await user.click(screen.getByRole("button", { name: "環境変数を編集" }));
    await user.click(await screen.findByRole("button", { name: "+ 環境を追加" }));
    await user.type(screen.getByLabelText("新しい環境の名前"), "ローカル{Enter}");
    await user.click(await screen.findByRole("button", { name: "＋ 変数を追加" }));
    await user.type(screen.getByLabelText("変数 1 の名前"), "baseUrl");
    await setField(user, screen.getByLabelText("変数 1 の値"), "http://localhost:8080");
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "保存" })).toBeDisabled());
    await user.click(screen.getByRole("button", { name: "戻る" }));

    // keyword だけが残る
    await waitFor(() => expect(expanded()).toHaveTextContent(/^展開後: http:\/\/localhost:8080\/api\/v1\/auth\/session\?q=/));
    expect(screen.getByRole("alert")).not.toHaveTextContent("{{baseUrl}}");
    expect(sendButton()).toBeDisabled();

    await setField(user, screen.getByLabelText("クエリパラメータ 1 行目の値"), "Netflix");
    await waitFor(() => expect(sendButton()).toBeEnabled());
    expect(expanded()).toHaveTextContent("展開後: http://localhost:8080/api/v1/auth/session?q=Netflix");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("環境を作って baseUrl を入れると展開後の URL が変わる（ヘッダーの環境選択も効く）", async () => {
    const { user, backend, project } = await setup({ withEnv: false });
    await openNewRequest(user);
    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/health");
    await waitFor(() => expect(expanded()).toHaveTextContent("展開後: http://{{baseUrl}}/health"));

    // 環境を 2 つ作る（最初のものが自動で選ばれる）
    const dev = await backend.envCreate(project.id, "ローカル");
    const stg = await backend.envCreate(project.id, "ステージング");
    for (const [env, url] of [
      [dev, "http://localhost:8080"],
      [stg, "https://stg.example.com"],
    ] as const) {
      await backend.envSave({ ...env, vars: [{ name: "baseUrl", value: { kind: "plain", value: url }, description: "", enabled: true }] });
    }

    // 画面の側に知らせる（環境変数の画面を開いて閉じる）
    await user.click(screen.getByRole("button", { name: "環境変数を編集" }));
    await screen.findByRole("heading", { name: "環境変数" });
    await user.click(screen.getByRole("button", { name: "戻る" }));

    await waitFor(() => expect(expanded()).toHaveTextContent("展開後: http://localhost:8080/health"));
    const select = screen.getByLabelText("環境");
    expect(select).toBeEnabled();
    await user.selectOptions(select, "ステージング");
    await waitFor(() => expect(expanded()).toHaveTextContent("展開後: https://stg.example.com/health"));
  });
});

describe("送信", () => {
  it("送るとレスポンスの 3 タブ（ボディ・ヘッダー・タイミング）が出る", async () => {
    const { user } = await setup();
    await openNewRequest(user);
    expect(screen.getByRole("region", { name: "レスポンス" })).toHaveTextContent("送信すると");

    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/api/v1/auth/session");
    await waitFor(() => expect(sendButton()).toBeEnabled());
    await user.click(sendButton()!);

    // ステータス・時間・サイズ
    const response = { get current() { return responseRegion(); } };
    expect(await screen.findByLabelText("ステータス")).toHaveTextContent("200 OK");
    expect(within(response.current).getByLabelText("ステータス")).toHaveClass("status-2xx");
    expect(within(response.current).getByLabelText("時間")).toHaveTextContent("38 ms");
    expect(within(response.current).getByLabelText("サイズ")).toHaveTextContent(/B$/);

    // ボディ（JSON を整形して表示）
    expect(within(response.current).getByRole("tab", { name: "ボディ" })).toHaveAttribute("aria-selected", "true");
    expect(within(response.current).getByLabelText("レスポンスのボディ")).toHaveTextContent('"clientName": "Netflix（Netflix Services Japan合同会社）"');
    await user.click(within(response.current).getByRole("button", { name: "Raw" }));
    expect(within(response.current).getByLabelText("レスポンスのボディ").textContent).toMatch(/^\{"data":\{"items"/);

    // ヘッダー（絞り込み）
    await user.click(within(response.current).getByRole("tab", { name: "ヘッダー (8)" }));
    expect(within(response.current).getByText("content-type")).toBeInTheDocument();
    await user.type(within(response.current).getByLabelText("ヘッダーを絞り込み"), "cache");
    expect(within(response.current).getByText("cache-control")).toBeInTheDocument();
    expect(within(response.current).queryByText("content-type")).not.toBeInTheDocument();
    expect(within(response.current).getByRole("button", { name: "すべてコピー" })).toBeInTheDocument();

    // タイミング（null の区間は「—」）
    await user.click(within(response.current).getByRole("tab", { name: "タイミング" }));
    expect(within(response.current).getByText("応答待ち（TTFB）")).toBeInTheDocument();
    expect(within(response.current).getByText("TLS ハンドシェイク").closest(".timing-row")).toHaveTextContent("—");
    expect(within(response.current).getByText("接続先").nextSibling).toHaveTextContent("127.0.0.1:8080（localhost）");
    expect(within(response.current).getByText("TLS").nextSibling).toHaveTextContent("なし（http）");
    expect(within(response.current).getByText("環境").nextSibling).toHaveTextContent("ローカル (Dev Container)");
  });

  it("送信中は「中止」に変わり、中止すると失敗として知らせる", async () => {
    const { user, backend } = await setup({ delay: 5000 });
    const cancel = vi.spyOn(backend, "requestCancel");
    await openNewRequest(user);
    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/slow");
    await waitFor(() => expect(sendButton()).toBeEnabled());
    await user.click(sendButton()!);

    const stop = await screen.findByRole("button", { name: "中止" });
    expect(sendButton()).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "レスポンス" })).toHaveTextContent("送信しています");
    await user.click(stop);

    const failure = await screen.findByRole("alert");
    expect(failure).toHaveTextContent("送信を中止しました");
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("button", { name: "送信" })).toBeEnabled();
  });

  it("結果のイベントが戻り値より先に届いても取りこぼさない", async () => {
    const base = createMockBackend([], { sendDelayMs: 5000 });
    const handlers: ((e: RequestFinished) => void)[] = [];
    const backend: Backend = {
      ...base,
      onRequestFinished: async (h) => {
        handlers.push(h);
        return base.onRequestFinished(h);
      },
      requestSend: async (input, settings) => {
        const sendId = await base.requestSend(input, settings);
        // 戻り値を返す前に、結果のイベントを届ける
        for (const h of handlers) h({ sendId, result: { status: "failure", error: { kind: "timeout", message: "30 秒以内に応答がありませんでした" } }, autoUpdates: [], autoUpdateSkipped: [] });
        return sendId;
      },
    };
    const project = await base.projectCreate("P");
    const { user } = renderApp(backend);
    await screen.findByRole("banner");
    expect(project.id).toBeTruthy();
    await openNewRequest(user);
    await setField(user, screen.getByLabelText("リクエストURL"), "http://localhost:8080/api/v1/x");
    await waitFor(() => expect(sendButton()).toBeEnabled());
    await user.click(sendButton()!);

    const failure = await screen.findByRole("alert");
    expect(failure).toHaveTextContent("タイムアウトしました");
    expect(failure).toHaveTextContent("30 秒以内に応答がありませんでした");
  });

  it("4xx は成功として扱い、problem+json の code と detail を目立たせる。接続の失敗は message を出す", async () => {
    const { user } = await setup();
    await openNewRequest(user);
    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/api/v1/error");
    await waitFor(() => expect(sendButton()).toBeEnabled());
    await user.click(sendButton()!);

    const response = { get current() { return responseRegion(); } };
    const chip = await screen.findByLabelText("ステータス");
    expect(chip).toHaveTextContent("400 Bad Request");
    expect(chip).toHaveClass("status-4xx");
    const problem = within(response.current).getByRole("group", { name: "エラーの内容" });
    expect(within(problem).getByText("VALIDATION_ERROR")).toHaveClass("problem-code");
    expect(problem).toHaveTextContent("入力内容に誤りがあります（email: 必須です）");

    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/refused");
    await waitFor(() => expect(sendButton()).toBeEnabled());
    await user.click(sendButton()!);
    expect(await screen.findByRole("alert")).toHaveTextContent("接続できません");
    expect(screen.getByRole("alert")).toHaveTextContent("localhost:8080 に接続を拒否されました");
    expect(within(response.current).queryByLabelText("ステータス")).not.toBeInTheDocument();
  });

  it("ボディが先頭だけのときは「先頭だけ表示しています」と出し、ファイルに保存できる", async () => {
    const { user, backend } = await setup();
    const saveBody = vi.spyOn(backend, "responseSaveBody");
    await openNewRequest(user);
    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/large");
    await waitFor(() => expect(sendButton()).toBeEnabled());
    await user.click(sendButton()!);

    const response = { get current() { return responseRegion(); } };
    await waitFor(() => expect(within(response.current).getByRole("status")).toHaveTextContent("先頭だけ表示しています"));
    expect(within(response.current).getByRole("status")).toHaveTextContent("24.0 MB");
    // JSON として読めない（途中で切れている）ので Raw だけ
    expect(within(response.current).getByRole("button", { name: "JSON" })).toBeDisabled();

    await user.click(within(response.current).getByRole("button", { name: "ファイルに保存" }));
    await waitFor(() => expect(saveBody).toHaveBeenCalledTimes(1));
    expect(saveBody.mock.calls[0][1]).toBe("/mock/downloads/response.json");
    expect(await screen.findByText(/保存しました: \/mock\/downloads/)).toBeInTheDocument();
  });
});

describe("リクエストの入力", () => {
  it("パラメータ・ヘッダーの件数がタブ名に出て、ヘッダータブに自動のヘッダーが出る", async () => {
    const { user } = await setup();
    await openNewRequest(user);
    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/api/v1/video/clients");

    await user.click(screen.getByRole("button", { name: "クエリパラメータを追加" }));
    await setField(user, screen.getByLabelText("クエリパラメータ 1 行目のキー"), "q");
    await setField(user, screen.getByLabelText("クエリパラメータ 1 行目の値"), "Netflix");
    await user.click(screen.getByRole("button", { name: "クエリパラメータを追加" }));
    await setField(user, screen.getByLabelText("クエリパラメータ 2 行目のキー"), "perPage");
    await user.click(screen.getByLabelText("クエリパラメータ 2 行目を有効にする")); // 無効にする
    expect(screen.getByRole("tab", { name: "パラメータ (1)" })).toBeInTheDocument();
    await waitFor(() => expect(expanded()).toHaveTextContent("展開後: http://localhost:8080/api/v1/video/clients?q=Netflix"));

    await user.click(screen.getByRole("tab", { name: "ヘッダー" }));
    await user.click(screen.getByRole("button", { name: "ヘッダーを追加" }));
    await setField(user, screen.getByLabelText("ヘッダー 1 行目のキー"), "Accept");
    await setField(user, screen.getByLabelText("ヘッダー 1 行目の値"), "application/json");
    expect(screen.getByRole("tab", { name: "ヘッダー (1)" })).toBeInTheDocument();

    const auto = await screen.findByRole("region", { name: "自動で付与されるヘッダー" });
    expect(within(auto).getByText("User-Agent")).toBeInTheDocument();
    expect(within(auto).getByText("Host").nextSibling).toHaveTextContent("localhost:8080");
    expect(within(auto).getAllByText("アプリ既定").length).toBeGreaterThan(0);
  });

  it("ボディは種類を切り替えられ（JSON は CodeMirror）、認証は Bearer を入れると自動ヘッダーに出る", async () => {
    const { user } = await setup();
    await openNewRequest(user);
    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/api/v1/auth/password-login");
    await user.selectOptions(screen.getByLabelText("HTTPメソッド"), "POST");

    await user.click(screen.getByRole("tab", { name: "ボディ" }));
    const kinds = screen.getByRole("radiogroup", { name: "ボディの種類" });
    expect(within(kinds).getAllByRole("radio").map((r) => r.textContent)).toEqual(["なし", "JSON", "フォーム", "multipart", "テキスト", "バイナリ"]);
    await user.click(within(kinds).getByRole("radio", { name: "JSON" }));
    expect(await screen.findByRole("textbox", { name: "JSON のボディ" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "整形" })).toBeInTheDocument();
    await user.click(within(kinds).getByRole("radio", { name: "バイナリ" }));
    await user.click(screen.getByRole("button", { name: "ファイルを選択…" }));
    expect(await screen.findByDisplayValue("/mock/files/sample.pdf")).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "認証" }));
    // v1 にない方式は出さず、プロジェクトの既定は OpenAPI の API のタブだけで選べる
    expect(screen.queryByText("OAuth 2.0")).not.toBeInTheDocument();
    expect(screen.queryByText("Digest 認証")).not.toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /プロジェクトの既定を使う/ })).toBeDisabled();
    await user.click(screen.getByRole("radio", { name: /Bearer トークン/ }));
    await setField(user, screen.getByLabelText("トークン"), "{{token}}");
    await waitFor(() => expect(screen.getByRole("tab", { name: /^認証/ })).toHaveClass("is-bad"));
    expect(screen.getByRole("alert")).toHaveTextContent("{{token}}（認証（トークン））");

    await setField(user, screen.getByLabelText("トークン"), "abc");
    await waitFor(() => expect(screen.getByText(/Authorization: Bearer/)).toBeInTheDocument());
    expect(screen.queryByText(/abc/, { selector: ".code-box *" })).not.toBeInTheDocument(); // 値は伏せ字
  });

  it("OpenAPI の API ではないタブの設計は、設計がないと分かる", async () => {
    const { user } = await setup();
    await openNewRequest(user);
    await user.click(screen.getByRole("tab", { name: "設計" }));
    expect(screen.getByText(/OpenAPI の API ではないので、設計はありません/)).toBeInTheDocument();
  });
});

describe("環境変数の画面", () => {
  async function openEnvironments(user: UserEvent) {
    await user.click(within(screen.getByRole("navigation", { name: "API ツリー" })).getByRole("button", { name: "環境変数" }));
    await screen.findByRole("heading", { name: "環境変数" });
  }

  it("グローバルは選択・削除・複製・改名できない。環境は追加・複製・削除できる", async () => {
    const { user } = await setup({ withEnv: false });
    await openEnvironments(user);

    const list = screen.getByRole("navigation", { name: "環境の一覧" });
    // 環境がないので、グローバルが開いている
    expect(within(list).getByRole("button", { name: /グローバル/ })).toHaveAttribute("aria-current", "true");
    expect(screen.queryByRole("button", { name: "削除" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "複製" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("環境名")).not.toBeInTheDocument();

    await user.click(within(list).getByRole("button", { name: "+ 環境を追加" }));
    await user.type(screen.getByLabelText("新しい環境の名前"), "ローカル{Enter}");
    expect(await within(list).findByRole("button", { name: /ローカル/ })).toHaveAttribute("aria-current", "true");
    expect(screen.getByText("使用中")).toBeInTheDocument(); // 最初の環境が自動で選ばれる

    await user.click(screen.getByRole("button", { name: "複製" }));
    expect(await within(list).findByRole("button", { name: /ローカル のコピー/ })).toHaveAttribute("aria-current", "true");
    expect(screen.queryByText("使用中")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "削除" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "削除する" }));
    await waitFor(() => expect(within(list).queryByRole("button", { name: /ローカル のコピー/ })).not.toBeInTheDocument());
    expect(within(list).getByRole("button", { name: /ローカル/ })).toBeInTheDocument();

    // 名前を変えて保存
    const name = await screen.findByLabelText("環境名");
    await user.clear(name);
    await user.type(name, "ローカル (Dev Container)");
    await user.click(screen.getByRole("button", { name: "保存" }));
    expect(await within(list).findByRole("button", { name: /ローカル \(Dev Container\)/ })).toBeInTheDocument();
  });

  it("シークレットは伏せ字で、目のアイコンで見える。保存し直しても値が残り、名前を変えても引き継がれる", async () => {
    const { user, backend, project } = await setup({ withEnv: false });
    await openEnvironments(user);
    await user.click(screen.getByRole("button", { name: "+ 環境を追加" }));
    await user.type(screen.getByLabelText("新しい環境の名前"), "本番{Enter}");
    await screen.findByLabelText("環境名");

    await user.click(screen.getByRole("button", { name: "＋ 変数を追加" }));
    await user.type(screen.getByLabelText("変数 1 の名前"), "session");
    await user.selectOptions(screen.getByLabelText("変数 1 の種類"), "シークレット");
    await user.type(screen.getByLabelText("変数 1 の値"), "s3cr3t-value");
    // 入力中も伏せ字
    expect(screen.getByLabelText("変数 1 の値")).toHaveAttribute("type", "password");
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "保存" })).toBeDisabled());

    // 値は DB には載らず、キーチェーンにだけある
    const [, saved] = await backend.envList(project.id);
    expect(saved.vars[0]).toEqual({ name: "session", value: { kind: "secret" }, description: "", enabled: true });
    expect(await backend.secretReveal(project.id, saved.id, "session")).toBe("s3cr3t-value");

    // 保存したあとは値が画面にない。目のアイコンで見える
    const field = screen.getByLabelText("変数 1 の値");
    expect(field).toHaveAttribute("type", "password");
    expect(field).toHaveValue("");
    expect(field).toHaveAttribute("placeholder", "••••••••");
    await user.click(screen.getByRole("button", { name: "変数 1 の値を表示" }));
    await waitFor(() => expect(screen.getByLabelText("変数 1 の値")).toHaveValue("s3cr3t-value"));
    expect(screen.getByLabelText("変数 1 の値")).toHaveAttribute("type", "text");
    await user.click(screen.getByRole("button", { name: "変数 1 の値を隠す" }));
    expect(screen.getByLabelText("変数 1 の値")).toHaveAttribute("type", "password");
    expect(screen.getByLabelText("変数 1 の値")).toHaveValue("");

    // 名前を変えて保存すると、キーチェーンの値も引き継がれる
    await user.clear(screen.getByLabelText("変数 1 の名前"));
    await user.type(screen.getByLabelText("変数 1 の名前"), "sessionId");
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "保存" })).toBeDisabled());
    expect(await backend.secretReveal(project.id, saved.id, "sessionId")).toBe("s3cr3t-value");
    expect(await backend.secretReveal(project.id, saved.id, "session")).toBeNull();

    // 目のアイコンを押す前の「展開後」にも、値は出ない（伏せ字）
    expect(screen.queryByText("s3cr3t-value")).not.toBeInTheDocument();
  });

  it("変更があれば保存・破棄ができ、同じ変数名はエラーを出す", async () => {
    const { user } = await setup();
    await openEnvironments(user);
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
    expect(screen.getByLabelText("変数 1 の値")).toHaveValue("http://localhost:8080");

    await user.click(screen.getByRole("button", { name: "＋ 変数を追加" }));
    await user.type(screen.getByLabelText("変数 2 の名前"), "baseUrl");
    await user.click(screen.getByRole("button", { name: "保存" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("変数名が重複しています: baseUrl");

    await user.click(screen.getByRole("button", { name: "破棄" }));
    expect(screen.queryByLabelText("変数 2 の名前")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});

describe("保存と「その他」", () => {
  it("新しいリクエストは名前を付けて保存でき、「その他」に出て、閉じても開き直せる。保存した後の保存は上書き", async () => {
    const { user, backend, project } = await setup();
    await openNewRequest(user);
    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/api/v1/auth/session");

    // 触ったので未保存の印が付く
    const tab = screen.getByRole("tab", { name: /新しいリクエスト/ });
    expect(within(tab).getByRole("img", { name: "未保存" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "保存" }));
    const dialog = await screen.findByRole("dialog", { name: "リクエストを保存" });
    // 保存先は「その他」だけ。名前は URL から提案される
    expect(within(dialog).getByRole("option", { name: "その他" })).toHaveAttribute("aria-selected", "true");
    const name = within(dialog).getByLabelText("名前");
    expect(name).toHaveValue("GET /api/v1/auth/session");
    await user.clear(name);
    await user.type(name, "セッション取得");
    await user.type(within(dialog).getByLabelText("説明（任意）"), "ログイン状態を確かめる");
    await user.click(within(dialog).getByRole("button", { name: "保存" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const sidebar = screen.getByRole("navigation", { name: "API ツリー" });
    const item = await within(sidebar).findByText("セッション取得");
    expect(item).toBeInTheDocument();
    expect(within(sidebar).getByText("1 件")).toBeInTheDocument();
    const saved = screen.getByRole("tab", { name: /セッション取得/ });
    expect(within(saved).queryByRole("img", { name: "未保存" })).not.toBeInTheDocument();
    expect((await backend.adhocList(project.id))[0]).toMatchObject({
      name: "セッション取得",
      description: "ログイン状態を確かめる",
      method: "GET",
      url: "{{baseUrl}}/api/v1/auth/session",
    });

    // 編集すると未保存の印。保存ボタンはダイアログなしで上書きする
    await user.selectOptions(screen.getByLabelText("HTTPメソッド"), "DELETE");
    expect(within(screen.getByRole("tab", { name: /セッション取得/ })).getByRole("img", { name: "未保存" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(within(screen.getByRole("tab", { name: /セッション取得/ })).queryByRole("img", { name: "未保存" })).not.toBeInTheDocument());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    const list = await backend.adhocList(project.id);
    expect(list).toHaveLength(1);
    expect(list[0].method).toBe("DELETE");
    expect(list[0].description).toBe("ログイン状態を確かめる"); // 上書き保存で説明は消えない

    // タブを閉じても、サイドバーから開き直せる
    await user.click(screen.getByRole("button", { name: "セッション取得 を閉じる" }));
    expect(screen.queryByRole("tab", { name: /セッション取得/ })).not.toBeInTheDocument();
    await user.click(within(sidebar).getByText("セッション取得"));
    expect(await screen.findByRole("tab", { name: /セッション取得/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByLabelText("リクエストURL")).toHaveValue("{{baseUrl}}/api/v1/auth/session");
    expect(screen.getByLabelText("HTTPメソッド")).toHaveValue("DELETE");
  });

  it("未保存のタブを閉じるときは確認し、サイドバーから削除するとタブも閉じる", async () => {
    const { user, backend, project } = await setup();
    await backend.adhocSave({
      id: "",
      projectId: project.id,
      name: "ヘルスチェック",
      method: "GET",
      url: "{{baseUrl}}/health",
      params: [],
      body: { kind: "none" },
      auth: null,
      createdAt: "",
      updatedAt: "",
    });
    await user.click(screen.getByRole("button", { name: "環境変数を編集" })); // 画面を作り直して一覧を読ませる
    await user.click(await screen.findByRole("button", { name: "戻る" }));
    const sidebar = screen.getByRole("navigation", { name: "API ツリー" });

    await openNewRequest(user);
    await setField(user, screen.getByLabelText("リクエストURL"), "{{baseUrl}}/x");
    await user.click(screen.getByRole("button", { name: "新しいリクエスト を閉じる" }));
    const confirm = await screen.findByRole("dialog", { name: "タブを閉じますか？" });
    await user.click(within(confirm).getByRole("button", { name: "キャンセル" }));
    expect(screen.getByRole("tab", { name: /新しいリクエスト/ })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "新しいリクエスト を閉じる" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "保存せずに閉じる" }));
    expect(screen.queryByRole("tab", { name: /新しいリクエスト/ })).not.toBeInTheDocument();

    await user.click(await within(sidebar).findByText("ヘルスチェック"));
    expect(await screen.findByRole("tab", { name: /ヘルスチェック/ })).toBeInTheDocument();
    await user.click(within(sidebar).getByRole("button", { name: "ヘルスチェック を削除" }));
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "削除する" }));
    await waitFor(() => expect(screen.queryByRole("tab", { name: /ヘルスチェック/ })).not.toBeInTheDocument());
    expect(within(sidebar).queryByText("ヘルスチェック")).not.toBeInTheDocument();
    expect(await backend.adhocList(project.id)).toEqual([]);
  });

  it("プロジェクトのカードは既存の作成ダイアログを開く", async () => {
    const { user } = await setup();
    await user.click(screen.getByRole("button", { name: "新しいタブ" }));
    await user.click(await screen.findByRole("button", { name: /^プロジェクト/ }));
    expect(await screen.findByRole("dialog", { name: "新しいプロジェクト" })).toBeInTheDocument();
  });
});
