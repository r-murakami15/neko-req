import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createMockBackend } from "./mock/backend";
import { openOp, setupWithApi } from "./test/openapi";

afterEach(() => vi.restoreAllMocks());

const press = (key: string, init: KeyboardEventInit = {}) => fireEvent.keyDown(window, { key, ctrlKey: true, ...init });

async function setup() {
  const base = createMockBackend([], { sendDelayMs: 10 });
  const ctx = await setupWithApi(base, base);
  await openOp(ctx.user, "video-client", /取引先一覧取得/);
  await waitFor(() => expect(screen.getByRole("button", { name: "送信" })).toBeEnabled());
  return ctx;
}

describe("ショートカット（Windows は Ctrl、macOS は ⌘）", () => {
  it("Ctrl+Enter で送信する", async () => {
    const { mock } = await setup();
    const send = vi.spyOn(mock, "requestSend");
    press("Enter");
    await screen.findByLabelText("ステータス");
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("URL 欄の中の Ctrl+Enter でも、送るのは 1 回だけ", async () => {
    const { user, mock } = await setup();
    const send = vi.spyOn(mock, "requestSend");
    await user.click(screen.getByLabelText("リクエストURL"));
    await user.keyboard("{Control>}{Enter}{/Control}");
    await screen.findByLabelText("ステータス");
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("Ctrl+S で保存する（yaml の API はダイアログなしで入力値を保存）", async () => {
    const { user, mock } = await setup();
    const save = vi.spyOn(mock, "draftSave");
    await user.click(screen.getByRole("checkbox", { name: "クエリパラメータ 1 行目を有効にする" }));
    const tab = screen.getByRole("tab", { name: /取引先一覧取得/ });
    expect(within(tab).getByRole("img", { name: "未保存" })).toBeInTheDocument();
    press("s");
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(within(screen.getByRole("tab", { name: /取引先一覧取得/ })).queryByRole("img", { name: "未保存" })).not.toBeInTheDocument());
  });

  it("Ctrl+T は、ダイアログなしで空のリクエストを新しいタブに開いて URL 欄にフォーカスする。Ctrl+W でタブを閉じる（未保存なら確認する）", async () => {
    const { user } = await setup();
    press("t");
    const url = await screen.findByLabelText("リクエストURL");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /新しいリクエスト/ })).toHaveAttribute("aria-selected", "true");
    expect(url).toHaveValue("");
    expect(url).toHaveFocus();
    expect(screen.getAllByRole("tab", { name: /^(GET|POST|PUT|DELETE|PATCH)/ })).toHaveLength(2);

    // 空のタブは変更なしなので、そのまま閉じる。前のタブに戻る
    press("w");
    await waitFor(() => expect(screen.queryByRole("tab", { name: /新しいリクエスト/ })).not.toBeInTheDocument());

    // 変更なしのタブは、そのまま閉じる
    press("w");
    await waitFor(() => expect(screen.queryByRole("tab", { name: /取引先一覧取得/ })).not.toBeInTheDocument());

    // 未保存のタブは、確認が出る
    await user.click(screen.getByRole("button", { name: "新しいタブ" }));
    const draftUrl = await screen.findByLabelText("リクエストURL");
    await user.click(draftUrl);
    await user.paste("{{baseUrl}}/x");
    press("w");
    const confirm = await screen.findByRole("dialog", { name: "タブを閉じますか？" });
    await user.click(within(confirm).getByRole("button", { name: "保存せずに閉じる" }));
    await waitFor(() => expect(screen.queryByLabelText("リクエストURL")).not.toBeInTheDocument());
  });

  it("Ctrl+K で API の検索欄に入力を移す。Ctrl+, で設定を開く", async () => {
    await setup();
    press("k");
    expect(screen.getByLabelText("API を検索")).toHaveFocus();
    press(",");
    expect(await screen.findByRole("dialog", { name: "設定" })).toBeInTheDocument();
  });

  it("ダイアログが開いているあいだは効かせない。修飾キーなし・Shift つきも効かない", async () => {
    const { user, mock } = await setup();
    const send = vi.spyOn(mock, "requestSend");
    press("Enter", { shiftKey: true });
    fireEvent.keyDown(window, { key: "Enter" });
    fireEvent.keyDown(window, { key: "t", metaKey: true }); // macOS 以外では Ctrl が修飾キー
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(send).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "設定" }));
    await screen.findByRole("dialog", { name: "設定" });
    press("t");
    press("Enter");
    expect(screen.queryByRole("dialog", { name: "新規作成" })).not.toBeInTheDocument();
    expect(send).not.toHaveBeenCalled();
  });

  it("履歴の画面では、送信・保存・タブの操作は効かない（Ctrl+K はメインに戻って検索欄へ）", async () => {
    const { user, mock } = await setup();
    const send = vi.spyOn(mock, "requestSend");
    await user.click(screen.getByRole("button", { name: "履歴" }));
    await screen.findByRole("heading", { level: 1, name: "履歴" });
    press("Enter");
    press("t");
    expect(send).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    press("k");
    expect(await screen.findByLabelText("API を検索")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("API を検索")).toHaveFocus());
  });

  it("macOS では ⌘（meta）が修飾キー。Ctrl は効かない", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    const { mock } = await setup();
    const send = vi.spyOn(mock, "requestSend");
    press("Enter"); // Ctrl
    expect(send).not.toHaveBeenCalled();
    fireEvent.keyDown(window, { key: "Enter", metaKey: true });
    await screen.findByLabelText("ステータス");
    expect(send).toHaveBeenCalledTimes(1);
  });
});
