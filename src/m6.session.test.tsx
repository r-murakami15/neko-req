import { cleanup, screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createMockBackend } from "./mock/backend";
import { SESSION_SAVE_DEBOUNCE_MS } from "./store/session";
import { openOp, setupWithApi } from "./test/openapi";
import { renderApp } from "./test/render";

afterEach(() => vi.restoreAllMocks());

const SAVE_WAIT = { timeout: SESSION_SAVE_DEBOUNCE_MS + 1500 };

async function newRequestTab(user: UserEvent) {
  await user.click(screen.getByRole("button", { name: "新しいタブ" }));
  return screen.findByLabelText("リクエストURL");
}

describe("タブの復元", () => {
  it("開いていたタブ・未保存の入力・選んでいたタブ・表示を、再起動のあとに戻す。手で入力した認証の値は戻さず、案内する", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const first = await setupWithApi(base, base);
    const { user, mock, project } = first;
    const save = vi.spyOn(mock, "sessionSave");

    // 1. API のタブ（未保存の変更・ボディのタブを表示）
    await openOp(user, "video-client", /取引先登録/);
    await user.click(await screen.findByRole("tab", { name: "ボディ" }));
    const editor = await screen.findByRole("textbox", { name: "JSON のボディ" });
    await waitFor(() => expect(editor).toHaveTextContent("テスト取引先"));
    await user.click(screen.getByRole("radio", { name: "なし" }));
    // 2. 手で作ったタブ（URL と、手で入力した Bearer トークン）
    const url = await newRequestTab(user);
    await user.click(url);
    await user.paste("{{baseUrl}}/api/v1/draft");
    await user.click(screen.getByRole("tab", { name: /^認証/ }));
    await user.click(screen.getByRole("radio", { name: /Bearer トークン/ }));
    await user.click(screen.getByLabelText("トークン"));
    await user.paste("manual-secret-token");
    // 3. 前面にするのは API のタブ
    await user.click(screen.getByRole("tab", { name: /取引先登録/ }));

    await waitFor(() => {
      const last = save.mock.calls.at(-1)?.[1];
      expect(last?.tabs).toHaveLength(2);
      expect(last?.activeTabId).toBe(last?.tabs[0].id);
      expect(JSON.stringify(last)).toContain("manual-secret-token"); // 画面からは渡す（保存するときに Rust が外す）
    }, SAVE_WAIT);
    // 保存された内容: 手で入力した認証の値は含まれない
    const stored = mock.mock.savedSession(project.id)!;
    expect(JSON.stringify(stored)).not.toContain("manual-secret-token");
    expect(stored.tabs.map((t) => t.kind)).toEqual(["operation", "new"]);
    expect(stored.tabs[1].droppedFields).toEqual(["認証（Bearer のトークン）"]);

    // 再起動（画面を作り直す。保存したタブの状態は、バックエンドが覚えている）
    first.unmount();
    cleanup();
    save.mockClear();
    renderApp(base);
    await screen.findByRole("banner");

    await waitFor(() => expect(screen.getAllByRole("tab", { name: /取引先登録|新しいリクエスト/ })).toHaveLength(2));
    // 選んでいたタブ・表示（ボディのタブ・「なし」にした未保存の入力）
    const apiTab = screen.getByRole("tab", { name: /取引先登録/ });
    expect(apiTab).toHaveAttribute("aria-selected", "true");
    expect(within(apiTab).getByRole("img", { name: "未保存" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "ボディ" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByText("このリクエストにはボディがありません。")).toBeInTheDocument();

    // 手で作ったタブ: URL は戻り、認証の値は空。入れ直しの案内が出る
    await user.click(screen.getByRole("tab", { name: /新しいリクエスト/ }));
    expect(screen.getByLabelText("リクエストURL")).toHaveValue("{{baseUrl}}/api/v1/draft");
    const note = await screen.findByText(/手で入力した認証の値は保存していません。入れ直してください/);
    expect(note).toHaveTextContent("認証（Bearer のトークン）");
    await user.click(screen.getByRole("tab", { name: /^認証/ }));
    expect(screen.getByLabelText("トークン")).toHaveValue("");
    // 復元の前に、空の状態で保存し直していない
    expect(save.mock.calls.every(([, s]) => s.tabs.length > 0)).toBe(true);
  });

  it("設定で「起動時に前回のタブを復元」をオフにしていると戻さない（保存はいつもする）。タブをすべて閉じると、次は何も戻らない", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const first = await setupWithApi(base, base);
    const { user, mock, project } = first;
    await openOp(first.user, "video-client", /取引先一覧取得/);
    await waitFor(() => expect(mock.mock.savedSession(project.id)?.tabs).toHaveLength(1), SAVE_WAIT);
    const s = await mock.appSettingsGet();
    await mock.appSettingsSet({ ...s, appearance: { ...s.appearance, restoreTabs: false } });

    first.unmount();
    cleanup();
    renderApp(base);
    await screen.findByRole("banner");
    expect(await screen.findByText("リクエストを開いていません。")).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /取引先一覧取得/ })).not.toBeInTheDocument();

    // 復元をオンに戻して、タブを閉じる → 空の状態が保存される
    await mock.appSettingsSet({ ...s, appearance: { ...s.appearance, restoreTabs: true } });
    await openOp(user, "video-client", /取引先一覧取得/);
    await user.click(screen.getByRole("button", { name: /取引先一覧取得 を閉じる/ }));
    await waitFor(() => expect(mock.mock.savedSession(project.id)?.tabs).toHaveLength(0), SAVE_WAIT);
  });

  it("消えた API や削除した「その他」のタブは戻さない。プロジェクトを切り替えると、そのプロジェクトのタブを戻す", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user } = await setupWithApi(base, base, async (p, b) => {
      const other = await b.projectCreate("別のプロジェクト");
      await b.sessionSave(other.id, {
        tabs: [{ id: "x1", kind: "new", opKey: null, adhocId: null, title: "別プロジェクトのタブ", request: { method: "GET", url: "https://other.example/y", params: [], body: { kind: "none" }, auth: null }, dirty: true, viewState: null }],
        activeTabId: "x1",
        savedAt: null,
      });
      await b.sessionSave(p.id, {
        tabs: [
          { id: "a1", kind: "operation", opKey: { kind: "id", value: "noSuchOperation" }, adhocId: null, title: "消えた API", request: null, dirty: false, viewState: null },
          { id: "a2", kind: "adhoc", opKey: null, adhocId: "no-such-adhoc", title: "消した保存", request: null, dirty: false, viewState: null },
          { id: "a3", kind: "operation", opKey: { kind: "id", value: "listClients" }, adhocId: null, title: "取引先一覧取得", request: null, dirty: false, viewState: JSON.stringify({ requestPane: "headers", responsePane: "timing" }) },
        ],
        activeTabId: "a3",
        savedAt: null,
      });
    });
    expect(await screen.findByRole("tab", { name: /取引先一覧取得/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getAllByRole("tab", { name: /取引先|消え|消した/ })).toHaveLength(1);
    expect(screen.getByRole("tab", { name: /^ヘッダー/ })).toHaveAttribute("aria-selected", "true");

    // 別のプロジェクトに切り替える
    await user.click(screen.getByRole("button", { name: /プロジェクトを切り替え/ }));
    await user.click(await screen.findByRole("menuitemradio", { name: /別のプロジェクト/ }));
    const restored = await screen.findByRole("tab", { name: /別プロジェクトのタブ/ });
    expect(within(restored).getByRole("img", { name: "未保存" })).toBeInTheDocument();
    expect(screen.getByLabelText("リクエストURL")).toHaveValue("https://other.example/y");
  });
});
