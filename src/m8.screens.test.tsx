import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { OperationListItem } from "./bindings";
import { suppressNativeContextMenu } from "./lib/contextmenu";
import { emptySpec } from "./lib/spec";
import { locateRow } from "./lib/tree";
import { createMockBackend } from "./mock/backend";
import { useTabsStore } from "./store/tabs";
import { useUiStore } from "./store/ui";
import { openOp, setupWithApi, sidebar } from "./test/openapi";
import { renderApp } from "./test/render";

afterEach(() => vi.restoreAllMocks());

const press = (key: string, init: KeyboardEventInit = {}) => fireEvent.keyDown(window, { key, ctrlKey: true, ...init });
const nav = () => within(sidebar());
const tree = () => nav().getByRole("tree", { name: "API 一覧" });
const item = (name: RegExp | string) => nav().getByRole("treeitem", { name });
const items = () => within(tree()).getAllByRole("treeitem");

describe("1. プロジェクト名のプレースホルダー", () => {
  it("作成画面は架空のプロジェクト名", async () => {
    renderApp();
    const field = await screen.findByLabelText("プロジェクト名");
    expect(field).toHaveAttribute("placeholder", "例: サンプル書店 API");
    expect(field.getAttribute("placeholder")).not.toMatch(/日本アニメーション|ライセンス/);
  });

  it("ヘッダーから開く「新しいプロジェクト」のダイアログも同じ", async () => {
    await setupWithApi();
    act(() => useUiStore.getState().setCreateDialogOpen(true));
    const dialog = await screen.findByRole("dialog", { name: "新しいプロジェクト" });
    expect(within(dialog).getByLabelText("プロジェクト名")).toHaveAttribute("placeholder", "例: サンプル書店 API");
  });
});

describe("2. タブの「＋」と Cmd/Ctrl+T は、空のタブをすぐ開く", () => {
  it("「＋」を押すとダイアログなしで空の HTTP リクエストのタブが開き、URL 欄にフォーカスが入る", async () => {
    const { user } = await setupWithApi();
    await user.click(screen.getByRole("button", { name: "新しいタブ" }));

    const url = await screen.findByLabelText("リクエストURL");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /新しいリクエスト/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByLabelText("HTTPメソッド")).toHaveValue("GET");
    expect(url).toHaveValue("");
    expect(url).toHaveFocus();

    // もう一度押すと、もう 1 つ空のタブが開く
    await user.click(screen.getByRole("button", { name: "新しいタブ" }));
    await waitFor(() => expect(screen.getAllByRole("tab", { name: /新しいリクエスト/ })).toHaveLength(2));
    expect(screen.getByLabelText("リクエストURL")).toHaveFocus();
  });

  it("Ctrl+T も同じ（ダイアログなし）。macOS は ⌘+T", async () => {
    await setupWithApi();
    press("t");
    expect(await screen.findByLabelText("リクエストURL")).toHaveFocus();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    fireEvent.keyDown(window, { key: "t", metaKey: true });
    await waitFor(() => expect(screen.getAllByRole("tab", { name: /新しいリクエスト/ })).toHaveLength(2));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("サイドバーの「＋」は、これまでどおり新規作成ダイアログ（OpenAPI・cURL・プロジェクト）", async () => {
    const { user } = await setupWithApi();
    await user.click(nav().getByRole("button", { name: "新規作成" }));
    const dialog = await screen.findByRole("dialog", { name: "新規作成" });
    expect(within(dialog).getByRole("button", { name: /HTTP リクエスト/ })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /^プロジェクト/ })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /OpenAPI から/ })).toBeInTheDocument();
    expect(within(dialog).getByLabelText("cURL コマンド")).toBeInTheDocument();
    expect(screen.queryByLabelText("リクエストURL")).not.toBeInTheDocument();
  });

  it("開いているタブがないときの「新しいリクエスト」ボタンは、ダイアログを開く", async () => {
    const { user } = await setupWithApi();
    await user.click(screen.getByRole("button", { name: "新しいリクエスト" }));
    expect(await screen.findByRole("dialog", { name: "新規作成" })).toBeInTheDocument();
  });
});

describe("3. ステータスバーにアプリの版", () => {
  it("取り込み済みのプロジェクトでも「NekoReq v版」だけで、取り込み状態の文言は出さない", async () => {
    await setupWithApi();
    const footer = screen.getByRole("contentinfo");
    expect(await within(footer).findByText("NekoReq v0.1.0")).toBeInTheDocument();
    expect(footer).not.toHaveTextContent(/取り込み|未取り込み|保存先/);
  });
});

describe("4. レスポンス欄の開閉", () => {
  async function openAndWait() {
    const ctx = await setupWithApi();
    await openOp(ctx.user, "video-client", /取引先一覧取得/);
    await waitFor(() => expect(screen.getByRole("button", { name: "送信" })).toBeEnabled());
    return ctx;
  }
  // 設計タブの中にも「レスポンス」の見出しの節があるので、レスポンス欄は class で選ぶ
  const response = () => document.querySelector(".pane-response");
  const panes = () => document.querySelector(".panes");

  it("「レスポンスを閉じる」で閉じ、リクエスト側が全幅になる。「レスポンスを表示」で開く", async () => {
    const { user } = await openAndWait();
    expect(response()).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /レスポンスを表示/ })).not.toBeInTheDocument();
    expect(panes()).not.toHaveClass("is-response-closed");

    // 送信前（空の見出し）にも閉じるボタンがある
    await user.click(screen.getByRole("button", { name: "レスポンスを閉じる" }));
    expect(response()).not.toBeInTheDocument();
    expect(panes()).toHaveClass("is-response-closed");
    expect(screen.getByRole("region", { name: "リクエスト" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "レスポンスを表示" }));
    expect(response()).toBeInTheDocument();
    expect(panes()).not.toHaveClass("is-response-closed");
  });

  it("送信の結果（成功）の見出しにも閉じるボタンがある", async () => {
    const { user } = await openAndWait();
    await user.click(screen.getByRole("button", { name: "送信" }));
    const bar = (await screen.findByLabelText("ステータス")).closest(".response-bar") as HTMLElement;
    await user.click(within(bar).getByRole("button", { name: "レスポンスを閉じる" }));
    expect(response()).not.toBeInTheDocument();
  });

  it("閉じている間に送信しても自動では開かず、「レスポンスを表示」に印が付く。開くと印が消える", async () => {
    const { user } = await openAndWait();
    await user.click(screen.getByRole("button", { name: "レスポンスを閉じる" }));

    await user.click(screen.getByRole("button", { name: "送信" }));
    const shown = await screen.findByRole("button", { name: "レスポンスを表示（新しい結果があります）" });
    await waitFor(() => expect(screen.getByRole("button", { name: "送信" })).toBeEnabled());
    expect(response()).not.toBeInTheDocument();
    expect(shown.querySelector(".response-open-dot")).not.toBeNull();

    await user.click(shown);
    expect(await screen.findByLabelText("ステータス")).toBeInTheDocument();
    expect(response()).toBeInTheDocument();

    // もう一度閉じても、見たあとなので印はない
    await user.click(screen.getByRole("button", { name: "レスポンスを閉じる" }));
    expect(screen.getByRole("button", { name: "レスポンスを表示" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /新しい結果/ })).not.toBeInTheDocument();
  });

  it("開閉の状態はアプリ全体で 1 つ（別のタブでも閉じたまま）。印はタブごと", async () => {
    const { user } = await openAndWait();
    await user.click(screen.getByRole("button", { name: "レスポンスを閉じる" }));
    await user.click(screen.getByRole("button", { name: "送信" }));
    await screen.findByRole("button", { name: "レスポンスを表示（新しい結果があります）" });

    await user.click(screen.getByRole("button", { name: "新しいタブ" }));
    await screen.findByLabelText("リクエストURL");
    expect(response()).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "レスポンスを表示" })).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: /取引先一覧取得/ }));
    expect(await screen.findByRole("button", { name: "レスポンスを表示（新しい結果があります）" })).toBeInTheDocument();
  });

  it("開閉の状態は覚える（ストアを作り直しても戻る）", async () => {
    const { user } = await openAndWait();
    await user.click(screen.getByRole("button", { name: "レスポンスを閉じる" }));
    expect(JSON.parse(localStorage.getItem("nekoreq-ui") ?? "{}").state.responseOpen).toBe(false);

    // 再起動のつもりで、メモリの状態を初期値に戻してから、保存した値を読み直す
    const stored = localStorage.getItem("nekoreq-ui") ?? "";
    act(() => useUiStore.setState({ responseOpen: true }));
    localStorage.setItem("nekoreq-ui", stored);
    await act(async () => {
      await useUiStore.persist.rehydrate();
    });
    expect(useUiStore.getState().responseOpen).toBe(false);
    expect(response()).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "レスポンスを表示" }));
    expect(JSON.parse(localStorage.getItem("nekoreq-ui") ?? "{}").state.responseOpen).toBe(true);
    // 「新しい結果」の印は覚えない
    expect(JSON.parse(localStorage.getItem("nekoreq-ui") ?? "{}").state.unseenResponses).toBeUndefined();
  });

  it("Ctrl+Shift+R で開閉する（設定のショートカット一覧にも出る）", async () => {
    await openAndWait();
    press("R", { shiftKey: true });
    await waitFor(() => expect(response()).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "レスポンスを表示" })).toBeInTheDocument();
    press("R", { shiftKey: true });
    await waitFor(() => expect(response()).toBeInTheDocument());
    // Shift だけでは効かず、Shift つきの Ctrl+Enter などは今までどおり効かない
    fireEvent.keyDown(window, { key: "R", shiftKey: true });
    expect(response()).toBeInTheDocument();
  });

  it("設計タブ（レスポンス欄を隠す）とぶつからない: 設計タブでは閉じる・表示のボタンもショートカットも効かず、状態も変えない", async () => {
    const { user } = await openAndWait();
    await user.click(screen.getByRole("tab", { name: "設計" }));
    expect(response()).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /レスポンスを(表示|閉じる)/ })).not.toBeInTheDocument();
    press("R", { shiftKey: true });
    expect(useUiStore.getState().responseOpen).toBe(true);

    // 設計から戻ると、状態どおり開いている
    await user.click(screen.getByRole("tab", { name: "パラメータ" }));
    expect(response()).toBeInTheDocument();

    // 閉じたまま設計タブへ行っても、表示のボタンは出ない（設計は幅いっぱい）
    await user.click(screen.getByRole("button", { name: "レスポンスを閉じる" }));
    await user.click(screen.getByRole("tab", { name: "設計" }));
    expect(screen.queryByRole("button", { name: /レスポンスを表示/ })).not.toBeInTheDocument();
    expect(panes()).toHaveClass("is-design");
    expect(panes()).not.toHaveClass("is-response-closed");
    await user.click(screen.getByRole("tab", { name: "パラメータ" }));
    expect(screen.getByRole("button", { name: "レスポンスを表示" })).toBeInTheDocument();
  });
});

describe("5. 保存先のパスを出さない", () => {
  it("画面のどこにも「保存先: …」の表示がない", async () => {
    await setupWithApi();
    await screen.findByText("NekoReq v0.1.0");
    expect(screen.queryByText(/保存先/)).not.toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/ブラウザ上のモック/);
  });
});

describe("6. API ツリーのキーボード操作（WAI-ARIA tree）", () => {
  it("tree / treeitem の属性: aria-level・setsize・posinset・aria-expanded。Tab で入れる行は 1 つだけ（roving tabindex）", async () => {
    await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    expect(tree()).toBeInTheDocument();

    const auth = item(/^auth(\s|$)/);
    expect(auth).toHaveAttribute("aria-level", "1");
    expect(auth).toHaveAttribute("aria-setsize", "6");
    expect(auth).toHaveAttribute("aria-posinset", "1");
    expect(auth).toHaveAttribute("aria-expanded", "true");
    const logout = item(/ログアウト/);
    expect(logout).toHaveAttribute("aria-level", "2");
    expect(logout).toHaveAttribute("aria-setsize", "4");
    expect(logout).toHaveAttribute("aria-posinset", "2");
    expect(logout).not.toHaveAttribute("aria-expanded");
    expect(item(/^video-client(\s|$)/)).toHaveAttribute("aria-expanded", "false");
    expect(item(/^video-client(\s|$)/)).toHaveAttribute("aria-posinset", "2");

    expect(items().filter((el) => el.tabIndex === 0)).toEqual([auth]);
    expect(items().filter((el) => el.tabIndex === -1)).toHaveLength(items().length - 1);
  });

  it("Tab でツリーに入るのは 1 か所。次の Tab でツリーを出る（中のボタンでは止まらない）", async () => {
    const { user } = await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    nav().getByLabelText("API を検索").focus();
    // 検索 → ＋ → 同期 → ツリー（先頭の行）→ 環境変数
    await user.tab();
    await user.tab();
    await user.tab();
    expect(item(/^auth(\s|$)/)).toHaveFocus();
    await user.tab();
    expect(nav().getByRole("button", { name: "環境変数" })).toHaveFocus();
    // Shift+Tab で戻ると、同じ行に戻る
    await user.tab({ shift: true });
    expect(item(/^auth(\s|$)/)).toHaveFocus();
  });

  it("↑↓ で前後の行へ、Home / End で先頭・末尾へ（案内の行は飛ばす）。Tab で入る行は最後にフォーカスした行に動く", async () => {
    const { user } = await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    act(() => item(/^auth(\s|$)/).focus());

    await user.keyboard("{ArrowDown}");
    expect(item(/ログイン状態取得/)).toHaveFocus();
    expect(item(/ログイン状態取得/).tabIndex).toBe(0);
    expect(item(/^auth(\s|$)/).tabIndex).toBe(-1);
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(item(/パスワードログイン/)).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(item(/ログアウト/)).toHaveFocus();

    await user.keyboard("{End}");
    expect(item(/^その他(\s|$)/)).toHaveFocus(); // 最後の行は案内の文なので、その手前のフォルダ
    await user.keyboard("{ArrowDown}");
    expect(item(/^その他(\s|$)/)).toHaveFocus();
    await user.keyboard("{Home}");
    expect(item(/^auth(\s|$)/)).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(item(/^auth(\s|$)/)).toHaveFocus();
  });

  it("→ は閉じたフォルダを開き、開いたフォルダでは最初の子へ。← は開いたフォルダを閉じ、API では親のフォルダへ", async () => {
    await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    const clients = item(/^video-client(\s|$)/);
    act(() => clients.focus());

    fireEvent.keyDown(clients, { key: "ArrowRight" });
    expect(clients).toHaveAttribute("aria-expanded", "true");
    expect(clients).toHaveFocus();

    fireEvent.keyDown(clients, { key: "ArrowRight" });
    await waitFor(() => expect(item(/取引先一覧取得/)).toHaveFocus());
    // API で → は何もしない
    fireEvent.keyDown(item(/取引先一覧取得/), { key: "ArrowRight" });
    expect(item(/取引先一覧取得/)).toHaveFocus();

    fireEvent.keyDown(item(/取引先一覧取得/), { key: "ArrowLeft" });
    await waitFor(() => expect(item(/^video-client(\s|$)/)).toHaveFocus());
    expect(item(/^video-client(\s|$)/)).toHaveAttribute("aria-expanded", "true");

    fireEvent.keyDown(item(/^video-client(\s|$)/), { key: "ArrowLeft" });
    expect(item(/^video-client(\s|$)/)).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("treeitem", { name: /取引先一覧取得/ })).not.toBeInTheDocument();
    // 閉じたフォルダで ← は何もしない
    fireEvent.keyDown(item(/^video-client(\s|$)/), { key: "ArrowLeft" });
    expect(item(/^video-client(\s|$)/)).toHaveFocus();
  });

  it("Enter と Space は、フォルダなら開閉、API ならそのリクエストをタブで開く", async () => {
    const { user } = await setupWithApi();
    await nav().findByText("11 件 · タグ 4");

    act(() => item(/^video-client(\s|$)/).focus());
    await user.keyboard("{Enter}");
    expect(item(/^video-client(\s|$)/)).toHaveAttribute("aria-expanded", "true");
    await user.keyboard(" ");
    expect(item(/^video-client(\s|$)/)).toHaveAttribute("aria-expanded", "false");
    await user.keyboard(" ");
    expect(item(/^video-client(\s|$)/)).toHaveAttribute("aria-expanded", "true");

    await user.keyboard("{ArrowDown}");
    expect(item(/取引先一覧取得/)).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("tab", { name: /取引先一覧取得/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByLabelText("リクエストURL")).toHaveValue("{{baseUrl}}/api/v1/video/clients");
    // フォーカスはツリーに残り、行は「いま開いているタブ」の印になる
    expect(item(/取引先一覧取得/)).toHaveFocus();
    expect(item(/取引先一覧取得/)).toHaveAttribute("aria-selected", "true");

    // 別の API は Space で開く
    await user.keyboard("{ArrowDown}");
    expect(item(/取引先登録/)).toHaveFocus();
    await user.keyboard(" ");
    expect(await screen.findByRole("tab", { name: /取引先登録/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getAllByRole("tab", { name: /^(GET|POST) / })).toHaveLength(2);
  });

  it("Ctrl/⌘ の組み合わせは、ツリーが取らない（画面全体のショートカットに任せる）", async () => {
    await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    const logout = item(/ログアウト/);
    act(() => logout.focus());
    fireEvent.keyDown(logout, { key: "Enter", ctrlKey: true });
    fireEvent.keyDown(logout, { key: "Enter", metaKey: true });
    fireEvent.keyDown(logout, { key: "ArrowDown", shiftKey: true });
    expect(screen.queryByRole("tab", { name: /ログアウト/ })).not.toBeInTheDocument();
    expect(logout).toHaveFocus();
  });

  it("マウスのクリックはこれまでどおり（フォルダは開閉、API はタブで開く）で、クリックした行が Tab で入る行になる", async () => {
    const { user } = await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    await user.click(item(/^video-work(\s|$)/));
    expect(item(/^video-work(\s|$)/)).toHaveAttribute("aria-expanded", "true");
    expect(item(/^video-work(\s|$)/)).toHaveFocus();
    expect(item(/^video-work(\s|$)/).tabIndex).toBe(0);
    expect(item(/^auth(\s|$)/).tabIndex).toBe(-1);
    await user.click(item(/ログアウト/));
    expect(await screen.findByRole("tab", { name: /ログアウト/ })).toBeInTheDocument();
  });

  it("検索で絞り込んでいるときも動く（当たった行だけをたどる）", async () => {
    const { user } = await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    await user.type(nav().getByLabelText("API を検索"), "取引先");
    await nav().findByText("検索: 3 件");
    // 当たったフォルダ（video-client）とその API 3 件、「その他」は当たりがないので出ない
    expect(items().map((el) => el.textContent)).toEqual([expect.stringMatching(/video-client/), expect.stringMatching(/取引先/), expect.stringMatching(/取引先/), expect.stringMatching(/取引先/)]);

    act(() => item(/^video-client(\s|$)/).focus());
    await user.keyboard("{ArrowDown}");
    expect(items()[1]).toHaveFocus();
    await user.keyboard("{End}");
    expect(items()[3]).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(item(/^video-client(\s|$)/)).toHaveFocus();
    await user.keyboard("{Home}{ArrowDown}{Enter}");
    expect(await screen.findByRole("tab", { name: /取引先/ })).toBeInTheDocument();
  });

  it("「削除された API」「その他」のフォルダも同じ操作で。その他の API は Enter で開き、Delete キーで削除の確認が出る", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const gone: OperationListItem = {
      opKey: { kind: "id", value: "legacyPing" },
      operationId: "legacyPing",
      method: "GET",
      path: "/api/v1/legacy/ping",
      summary: "旧ヘルスチェック",
      folder: "legacy",
      tags: ["legacy"],
      requiresAuth: false,
      deleted: true,
      hasDraft: true,
    };
    const live: OperationListItem = { ...gone, opKey: { kind: "id", value: "ping" }, operationId: "ping", summary: "ヘルスチェック", deleted: false };
    const backend = { ...base, operationList: async () => [live, gone] };
    const { user } = await setupWithApi(base, backend, async (project) => {
      await base.adhocSave({
        id: "",
        projectId: project.id,
        name: "メモ用のリクエスト",
        description: "",
        method: "GET",
        url: "{{baseUrl}}/memo",
        params: emptySpec().params,
        body: emptySpec().body,
        auth: null,
        createdAt: "",
        updatedAt: "",
      });
    });

    // 「削除された API」を開く
    const deleted = await nav().findByRole("treeitem", { name: /^削除された API(\s|$)/ });
    expect(deleted).toHaveAttribute("aria-expanded", "false");
    act(() => deleted.focus());
    await user.keyboard("{ArrowRight}");
    expect(deleted).toHaveAttribute("aria-expanded", "true");
    await user.keyboard("{ArrowRight}");
    expect(item(/旧ヘルスチェック/)).toHaveFocus();
    expect(item(/旧ヘルスチェック/)).toHaveAttribute("aria-level", "2");
    await user.keyboard("{ArrowLeft}");
    expect(item(/^削除された API(\s|$)/)).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(item(/^削除された API(\s|$)/)).toHaveAttribute("aria-expanded", "false");

    // 「その他」: 閉じて開く
    await user.keyboard("{ArrowDown}");
    expect(item(/^その他(\s|$)/)).toHaveFocus();
    expect(item(/^その他(\s|$)/)).toHaveAttribute("aria-expanded", "true");
    await user.keyboard("{ArrowLeft}");
    expect(item(/^その他(\s|$)/)).toHaveAttribute("aria-expanded", "false");
    await user.keyboard("{ArrowRight}{ArrowRight}");
    const memo = screen.getByRole("treeitem", { name: /メモ用のリクエスト/ });
    expect(memo).toHaveFocus();
    expect(memo).toHaveAttribute("aria-level", "2");
    await user.keyboard("{Delete}");
    expect(await screen.findByRole("dialog", { name: "リクエストを削除しますか？" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    // Enter で、その他の API をタブで開く
    act(() => screen.getByRole("treeitem", { name: /メモ用のリクエスト/ }).focus());
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("tab", { name: /メモ用のリクエスト/ })).toHaveAttribute("aria-selected", "true");
  });

  it("仮想スクロール: 画面の外の行へ移ると、その行を描いてスクロールし、フォーカスを移す", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const many: OperationListItem[] = Array.from({ length: 1000 }, (_, i) => ({
      opKey: { kind: "id", value: `op${i + 1}` },
      operationId: `op${i + 1}`,
      method: "GET",
      path: `/api/v1/items/${i + 1}`,
      summary: `API ${i + 1}`,
      folder: "all",
      tags: ["all"],
      requiresAuth: true,
      deleted: false,
      hasDraft: false,
    }));
    const backend = { ...base, operationList: async () => many };
    const { user } = await setupWithApi(base, backend);
    await nav().findByText("1000 件 · タグ 1");

    // jsdom にはスクロールの実装がないので、呼ばれた位置を記録する
    const scrollTo = vi.fn();
    // 仮想スクロールが「どこまでスクロールできるか」を決めるのに使う scrollHeight（jsdom は 0）
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
      configurable: true,
      get(this: HTMLElement) {
        return this.classList.contains("tree-scroll") ? (1000 + 3) * 28 : 0;
      },
    });
    Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, writable: true, value: scrollTo });
    try {
      const slots = () => sidebar().querySelectorAll(".tree-slot").length;
      expect(slots()).toBeLessThan(60);
      expect(screen.queryByRole("treeitem", { name: /^GET API 1000$/ })).not.toBeInTheDocument();

      act(() => item(/^all(\s|$)/).focus());
      await user.keyboard("{End}");
      expect(item(/^その他(\s|$)/)).toHaveFocus();
      expect(item(/^その他(\s|$)/)).toHaveAttribute("aria-posinset", "2");
      // 末尾の近くまでスクロールを頼んでいる。DOM の行は少ないまま
      await waitFor(() => {
        const tops = scrollTo.mock.calls.map((c) => (c[0] as { top: number }).top);
        expect(Math.max(...tops)).toBeGreaterThan(1000 * 28 - 600);
      });
      expect(slots()).toBeLessThan(60);
      // Tab で入る行は、フォーカスのある行（遠くにあっても描かれ続ける）
      expect(item(/^その他(\s|$)/).tabIndex).toBe(0);

      // 末尾から ↑ で、1000 件目の API へ（範囲外だった行が出る）
      await user.keyboard("{ArrowUp}");
      expect(await screen.findByRole("treeitem", { name: /API 1000$/ })).toHaveFocus();
      expect(slots()).toBeLessThan(80);

      await user.keyboard("{Home}");
      expect(item(/^all(\s|$)/)).toHaveFocus();
    } finally {
      delete (HTMLElement.prototype as { scrollTo?: unknown }).scrollTo;
      delete (HTMLElement.prototype as { scrollHeight?: unknown }).scrollHeight;
    }
  });

  it("フォーカスの枠（focus-visible）は、ダーク・ライトとも見える色（アクセント色）を内側に描く", async () => {
    const { readFileSync } = await import("node:fs");
    const css = readFileSync(`${process.cwd()}/src/styles/request.css`, "utf8");
    expect(css).toMatch(/\.tree-folder:focus-visible,\s*\.tree-row:focus-visible,\s*\.tree-folder\.is-focus-ring,\s*\.tree-row\.is-focus-ring\s*\{[^}]*outline: 2px solid var\(--accent\);[^}]*outline-offset: -2px;/);
  });

  it("ツリーにフォーカスがある間は、キーでもクリックでもフォーカス行に枠のクラス（is-focus-ring）が付く。ツリーの外では付かない。文字は選択させない", async () => {
    const { user } = await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    const ringed = () => items().filter((el) => el.classList.contains("is-focus-ring"));

    // キーで入って移る（WebKit では el.focus() に :focus-visible が付かないことがあるので、クラスで描く）
    await user.click(nav().getByLabelText("API を検索"));
    for (let i = 0; i < 10 && document.activeElement?.getAttribute("role") !== "treeitem"; i++) await user.tab();
    expect(document.activeElement).toHaveAttribute("role", "treeitem");
    expect(ringed()).toEqual([document.activeElement]);
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(ringed()).toHaveLength(1);
    expect(ringed()[0]).toHaveFocus();

    // クリックした行にも付く。続けてキーで動けば移った先に付く
    await user.click(item(/^video-work(\s|$)/));
    expect(ringed()).toEqual([item(/^video-work(\s|$)/)]);
    await user.keyboard("{ArrowUp}");
    expect(ringed()).toHaveLength(1);
    expect(ringed()[0]).toHaveFocus();

    // ツリーの外へ出たら消える
    await user.click(nav().getByLabelText("API を検索"));
    expect(ringed()).toHaveLength(0);

    // 文字は選択させない（WebKit 用の接頭辞つきも）
    const { readFileSync } = await import("node:fs");
    const css = readFileSync(`${process.cwd()}/src/styles/openapi.css`, "utf8");
    expect(css).toMatch(/\.tree-scroll\s*\{[^}]*-webkit-user-select: none;[^}]*user-select: none;/);
  });
});

describe("7. 開いているタブとツリーの連動", () => {
  const ringed = () => items().filter((el) => el.classList.contains("is-focus-ring"));
  const tab = (name: RegExp) => screen.getByRole("tab", { name });
  /** ログアウト（auth）と取引先一覧取得（video-client）をタブで開き、フォルダをすべて閉じる。 */
  async function openTwoAndCloseFolders() {
    const r = await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    await openOp(r.user, "auth", /ログアウト/);
    await openOp(r.user, "video-client", /取引先一覧取得/);
    await r.user.click(item(/^auth(\s|$)/));
    await r.user.click(item(/^video-client(\s|$)/));
    await r.user.click(item(/^その他(\s|$)/));
    for (const name of [/^auth(\s|$)/, /^video-client(\s|$)/, /^その他(\s|$)/]) expect(item(name)).toHaveAttribute("aria-expanded", "false");
    return r;
  }

  it("タブを切り替えると、閉じたフォルダが開いて、その行に枠（is-focus-ring）が付く。フォーカスはタブに残る", async () => {
    const { user } = await openTwoAndCloseFolders();
    // ツリーにフォーカスがない間は、前面のタブ（取引先一覧取得）の行が閉じたフォルダに隠れている → フォルダの行に枠
    await user.click(tab(/ログアウト/));
    expect(item(/^auth(\s|$)/)).toHaveAttribute("aria-expanded", "true");
    expect(item(/ログアウト/)).toHaveClass("is-active");
    expect(ringed()).toEqual([item(/ログアウト/)]);
    expect(tab(/ログアウト/)).toHaveFocus();

    // 別のフォルダの API へ。前のフォルダは閉じない
    await user.click(tab(/取引先一覧取得/));
    expect(item(/^video-client(\s|$)/)).toHaveAttribute("aria-expanded", "true");
    expect(item(/^auth(\s|$)/)).toHaveAttribute("aria-expanded", "true");
    expect(ringed()).toEqual([item(/取引先一覧取得/)]);
    expect(tab(/取引先一覧取得/)).toHaveFocus();

    // タブを閉じて別のタブが前に来たときも同じ
    await user.click(item(/^auth(\s|$)/));
    await user.click(screen.getByRole("button", { name: /ログアウト を閉じる/ }));
    expect(ringed()).toEqual([item(/取引先一覧取得/)]);
  });

  it("「その他」の保存済みリクエストのタブも、「その他」を開いて行に枠を付ける", async () => {
    const { user } = await setupWithApi(undefined, undefined, async (p, backend) => {
      await backend.adhocSave({ id: "", projectId: p.id, name: "メモ用のリクエスト", description: "", method: "GET", url: "{{baseUrl}}/memo", params: emptySpec().params, body: emptySpec().body, auth: null, createdAt: "", updatedAt: "" });
    });
    await nav().findByText("11 件 · タグ 4");
    await user.click(await nav().findByRole("treeitem", { name: /メモ用のリクエスト/ }));
    await openOp(user, "auth", /ログアウト/);
    await user.click(item(/^その他(\s|$)/));
    expect(item(/^その他(\s|$)/)).toHaveAttribute("aria-expanded", "false");

    await user.click(tab(/メモ用のリクエスト/));
    expect(item(/^その他(\s|$)/)).toHaveAttribute("aria-expanded", "true");
    expect(ringed()).toEqual([item(/メモ用のリクエスト/)]);
  });

  it("枠は常に 1 つ: ツリーにフォーカスがある間はフォーカス行、外に出たら前面のタブの行。前面のタブがツリーにないときは枠なし", async () => {
    const { user } = await openTwoAndCloseFolders();
    await user.click(tab(/ログアウト/));
    expect(ringed()).toEqual([item(/ログアウト/)]);

    // ツリーへ入ってフォーカスを動かすと、枠はフォーカス行へ（前面のタブの行からは外れる）
    await user.click(item(/^video-client(\s|$)/));
    expect(ringed()).toEqual([item(/^video-client(\s|$)/)]);
    expect(item(/ログアウト/)).toHaveClass("is-active");
    await user.keyboard("{ArrowUp}");
    expect(ringed()).toHaveLength(1);
    expect(ringed()[0]).toHaveFocus();

    // 外へ出ると、前面のタブの行へ戻る
    await user.click(nav().getByLabelText("API を検索"));
    expect(ringed()).toEqual([item(/ログアウト/)]);

    // 空のタブ（ツリーにない）が前面なら枠なし
    await user.click(screen.getByRole("button", { name: "新しいタブ" }));
    expect(ringed()).toHaveLength(0);
  });

  it("タブを切り替えたあとツリーに Tab で入ると、前面のタブの行にフォーカスが入る", async () => {
    const { user } = await openTwoAndCloseFolders();
    await user.click(tab(/ログアウト/));
    expect(item(/ログアウト/).tabIndex).toBe(0);
    expect(items().filter((el) => el.tabIndex === 0)).toEqual([item(/ログアウト/)]);

    // ツリーで別の行にフォーカスしたあと、タブを切り替えると、Tab で入る行は前面のタブの行に合わせ直す
    await user.click(item(/^video-client(\s|$)/));
    await user.click(tab(/取引先一覧取得/));
    expect(items().filter((el) => el.tabIndex === 0)).toEqual([item(/取引先一覧取得/)]);
    nav().getByLabelText("API を検索").focus();
    await user.tab();
    await user.tab();
    await user.tab();
    expect(item(/取引先一覧取得/)).toHaveFocus();
    expect(ringed()).toEqual([item(/取引先一覧取得/)]);
  });

  it("手でフォルダを閉じると閉じたまま（次に前面のタブが変わるまで開かない）。隠れている間はフォルダの行に枠", async () => {
    const { user } = await openTwoAndCloseFolders();
    await user.click(tab(/ログアウト/));
    expect(item(/^auth(\s|$)/)).toHaveAttribute("aria-expanded", "true");

    await user.click(item(/^auth(\s|$)/)); // 手で閉じる
    await user.click(nav().getByLabelText("API を検索"));
    expect(item(/^auth(\s|$)/)).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("treeitem", { name: /ログアウト/ })).not.toBeInTheDocument();
    expect(ringed()).toEqual([item(/^auth(\s|$)/)]);
    // 隠れていても、Tab で入る行はそのフォルダ
    expect(items().filter((el) => el.tabIndex === 0)).toEqual([item(/^auth(\s|$)/)]);

    // 別のタブへ切り替えて戻すと、また開く
    await user.click(tab(/取引先一覧取得/));
    expect(item(/^auth(\s|$)/)).toHaveAttribute("aria-expanded", "false");
    await user.click(tab(/ログアウト/));
    expect(item(/^auth(\s|$)/)).toHaveAttribute("aria-expanded", "true");
    expect(ringed()).toEqual([item(/ログアウト/)]);
  });

  it("検索で絞り込み中に、当たっていない行のタブへ切り替えても何も起きない（フォルダは開かず、枠もない）", async () => {
    const { user } = await openTwoAndCloseFolders();
    await user.click(tab(/ログアウト/));
    await user.click(item(/^video-client(\s|$)/)); // video-client は開いた状態
    await user.click(item(/^video-client(\s|$)/)); // 閉じる
    await user.type(nav().getByLabelText("API を検索"), "ログアウト");
    expect(ringed()).toEqual([item(/ログアウト/)]);

    await user.click(tab(/取引先一覧取得/));
    expect(ringed()).toHaveLength(0);
    expect(screen.queryByRole("treeitem", { name: /取引先一覧取得/ })).not.toBeInTheDocument();

    // 検索を消しても、そのときのことは遡って開かない（フォルダは閉じたまま。行が隠れているのでフォルダに枠）
    await user.clear(nav().getByLabelText("API を検索"));
    expect(item(/^video-client(\s|$)/)).toHaveAttribute("aria-expanded", "false");
    expect(ringed()).toEqual([item(/^video-client(\s|$)/)]);
  });

  it("1,000 件の仮想スクロール: 画面の外の行（閉じたフォルダの中）のタブへ切り替えると、フォルダが開いたあとのその行までスクロールを頼む", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const many: OperationListItem[] = Array.from({ length: 1000 }, (_, i) => ({
      opKey: { kind: "id", value: `op${i + 1}` },
      operationId: `op${i + 1}`,
      method: "GET",
      path: `/api/v1/items/${i + 1}`,
      summary: `API ${i + 1}`,
      folder: "all",
      tags: ["all"],
      requiresAuth: true,
      deleted: false,
      hasDraft: false,
    }));
    const backend = { ...base, operationList: async () => many };
    const { user, project } = await setupWithApi(base, backend);
    await nav().findByText("1000 件 · タグ 1");

    // 取り込み済みの API の開いた結果をひな形にして、先頭と末尾の API のタブを作る
    const template = await base.operationOpen(project.id, (await base.operationList(project.id))[0]!.opKey);
    act(() => {
      const tabs = useTabsStore.getState();
      tabs.openOp(project.id, { ...template, opKey: { kind: "id", value: "op1000" }, summary: "API 1000" });
      tabs.openOp(project.id, { ...template, opKey: { kind: "id", value: "op1" }, summary: "API 1" });
    });

    const scrollTo = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
      configurable: true,
      get(this: HTMLElement) {
        return this.classList.contains("tree-scroll") ? (1000 + 3) * 28 : 0;
      },
    });
    Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, writable: true, value: scrollTo });
    try {
      // フォルダを閉じてから、末尾の API のタブへ
      await user.click(item(/^all(\s|$)/));
      expect(item(/^all(\s|$)/)).toHaveAttribute("aria-expanded", "false");
      await user.click(screen.getByRole("tab", { name: /API 1$/ }));
      scrollTo.mockClear();

      await user.click(screen.getByRole("tab", { name: /API 1000$/ }));
      expect(item(/^all(\s|$)/)).toHaveAttribute("aria-expanded", "true");
      // 行が増えたあとの位置（1000 行目の近く）へスクロールを頼んでいる。行を全部は描かない
      await waitFor(() => {
        const tops = scrollTo.mock.calls.map((c) => (c[0] as { top: number }).top);
        expect(Math.max(...tops)).toBeGreaterThan(1000 * 28 - 600);
      });
      expect(await screen.findByRole("treeitem", { name: /API 1000$/ })).toHaveClass("is-focus-ring");
      expect(sidebar().querySelectorAll(".tree-slot").length).toBeLessThan(80);
    } finally {
      delete (HTMLElement.prototype as { scrollTo?: unknown }).scrollTo;
      delete (HTMLElement.prototype as { scrollHeight?: unknown }).scrollHeight;
    }
  });

  it("行の場所の調べ方（locateRow）: 削除された API は「削除された API」、「その他」は adhoc、検索に当たらなければ hidden、ないものは missing", () => {
    const op = (value: string, deleted: boolean, folder: string | null): OperationListItem => ({
      opKey: { kind: "id", value },
      operationId: value,
      method: "GET",
      path: `/${value}`,
      summary: value,
      folder,
      tags: folder ? [folder] : [],
      requiresAuth: false,
      deleted,
      hasDraft: false,
    });
    const ops = [op("a", false, "t1"), op("gone", true, "t1")];
    const adhocs = [{ id: "m1", projectId: "p", name: "メモ", description: "", method: "GET", url: "/memo", params: emptySpec().params, body: emptySpec().body, auth: null, createdAt: "", updatedAt: "" }];
    expect(locateRow("op:id:a", { ops, adhocs, query: "" })).toEqual({ status: "ok", folderId: "tag:t1" });
    expect(locateRow("op:id:gone", { ops, adhocs, query: "" })).toEqual({ status: "ok", folderId: "deleted" });
    expect(locateRow("adhoc:m1", { ops, adhocs, query: "" })).toEqual({ status: "ok", folderId: "adhoc" });
    expect(locateRow("adhoc:m1", { ops, adhocs, query: "zzz" })).toEqual({ status: "hidden", folderId: null });
    expect(locateRow("op:id:nothing", { ops, adhocs, query: "" }).status).toBe("missing");
    // タグが 1 つもなければ、API はフォルダに入らない
    expect(locateRow("op:id:a", { ops: [op("a", false, null)], adhocs, query: "" })).toEqual({ status: "ok", folderId: null });
  });
});

describe("8. 右クリックメニューを出さない", () => {
  it("contextmenu の既定の動きを止める（document・入力欄・ツリーの行）。main.tsx で登録している", async () => {
    const { readFileSync } = await import("node:fs");
    expect(readFileSync(`${process.cwd()}/src/main.tsx`, "utf8")).toMatch(/suppressNativeContextMenu\(\);/);

    await setupWithApi();
    await nav().findByText("11 件 · タグ 4");
    const off = suppressNativeContextMenu();
    try {
      const prevented = (el: EventTarget) => {
        const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
        el.dispatchEvent(e);
        return e.defaultPrevented;
      };
      expect(prevented(document.body)).toBe(true);
      expect(prevented(nav().getByLabelText("API を検索"))).toBe(true);
      expect(prevented(item(/^auth(\s|$)/))).toBe(true);
    } finally {
      off();
    }
    // 外すと、既定の動きに戻る
    const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.body.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
  });
});
