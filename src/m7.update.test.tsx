import { screen, waitFor, within } from "@testing-library/react";
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createMockBackend, type MockBackend } from "./mock/backend";
import { setupWithApi } from "./test/openapi";
import { useUpdateStore } from "./store/update";

afterEach(() => vi.restoreAllMocks());

const banner = () => screen.getByRole("region", { name: "アップデート" });

async function setup(prepare?: (b: MockBackend) => void | Promise<void>) {
  const base = createMockBackend([], { sendDelayMs: 10 });
  return setupWithApi(base, base, async (_p, b) => {
    await prepare?.(b);
  });
}

describe("更新の知らせ（M7）", () => {
  it("update://available で「新しい版 x.y.z があります」と更新内容を出す。勝手には入れ替えない", async () => {
    const { user, mock } = await setup();
    const install = vi.spyOn(mock, "updateInstall");
    expect(screen.queryByRole("region", { name: "アップデート" })).not.toBeInTheDocument();

    act(() => mock.mock.emitUpdateAvailable({ version: "0.2.0", currentVersion: "0.1.0" }));
    const b = await screen.findByRole("region", { name: "アップデート" });
    expect(b).toHaveTextContent("新しい版 0.2.0 があります");
    expect(b).toHaveTextContent("現在 0.1.0");
    // 更新内容（Markdown）
    await user.click(within(b).getByText("更新内容"));
    expect(within(b).getByText("履歴の検索が速くなりました")).toBeInTheDocument();
    // ユーザーが選ぶまで何もしない
    await new Promise((r) => setTimeout(r, 100));
    expect(install).not.toHaveBeenCalled();
    expect(within(b).getByRole("button", { name: "今すぐ更新" })).toBeEnabled();
  });

  it("起動時に取りこぼした更新は update_pending で取り出して出す", async () => {
    await setup((b) => b.mock.setPendingUpdate({ version: "0.3.0", currentVersion: "0.1.0", notes: null, date: null }));
    const b = await screen.findByRole("region", { name: "アップデート" });
    expect(b).toHaveTextContent("新しい版 0.3.0 があります");
    expect(within(b).queryByText("更新内容")).not.toBeInTheDocument(); // 更新内容がなければ出さない
  });

  it("「今すぐ更新」で入れ替え（進捗を出す）、入れ替わったら「再起動して更新を完了」で update_restart を呼ぶ", async () => {
    const { user, mock } = await setup();
    const install = vi.spyOn(mock, "updateInstall");
    const phases: string[] = [];
    const unsubscribe = useUpdateStore.subscribe((s) => {
      if (s.progress && phases.at(-1) !== s.progress.phase) phases.push(s.progress.phase);
    });
    act(() => mock.mock.emitUpdateAvailable());
    const b = await screen.findByRole("region", { name: "アップデート" });
    await user.click(within(b).getByRole("button", { name: "今すぐ更新" }));
    expect(install).toHaveBeenCalledTimes(1);

    const restart = await within(screen.getByRole("region", { name: "アップデート" })).findByRole("button", { name: "再起動して更新を完了" });
    unsubscribe();
    expect(phases).toEqual(["downloading", "installing", "installed"]);
    expect(banner()).toHaveTextContent("新しい版に入れ替えました");
    expect(mock.mock.restartCount()).toBe(0); // 再起動はユーザーが選ぶ
    await user.click(restart);
    await waitFor(() => expect(mock.mock.restartCount()).toBe(1));
    expect(banner()).toHaveTextContent("再起動しています");
  });

  it("入れ替えに失敗したら理由を出し、もう一度できる", async () => {
    const { user, mock } = await setup();
    mock.mock.failInstall("署名の検証に失敗しました");
    act(() => mock.mock.emitUpdateAvailable());
    await user.click(await screen.findByRole("button", { name: "今すぐ更新" }));
    expect(await within(banner()).findByRole("alert")).toHaveTextContent("更新できませんでした: 署名の検証に失敗しました");
    mock.mock.failInstall(false);
    await user.click(within(banner()).getByRole("button", { name: "もう一度更新する" }));
    expect(await within(banner()).findByRole("button", { name: "再起動して更新を完了" })).toBeInTheDocument();
  });

  it("「あとで」でバナーを隠す。設定の「アップデートを確認」から、また入れ替えられる", async () => {
    const { user, mock } = await setup();
    mock.mock.setUpdateCheckResult({ version: "0.2.0", currentVersion: "0.1.0", notes: null, date: null });
    act(() => mock.mock.emitUpdateAvailable());
    await user.click(await screen.findByRole("button", { name: "あとで" }));
    expect(screen.queryByRole("region", { name: "アップデート" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "設定" }));
    const dialog = await screen.findByRole("dialog", { name: "設定" });
    // 知らせは設定画面にも出ている（バナーを隠しても、ここから入れ替えられる）。「あとで」は出ない
    const inline = await within(dialog).findByRole("region", { name: "アップデート" });
    expect(inline).toHaveTextContent("新しい版 0.2.0 があります");
    expect(within(inline).queryByRole("button", { name: "あとで" })).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "アップデートを確認" }));
    expect(await within(dialog).findByRole("region", { name: "アップデート" })).toHaveTextContent("新しい版 0.2.0 があります");
    await user.click(within(dialog).getByRole("button", { name: "今すぐ更新" }));
    expect(await within(dialog).findByRole("button", { name: "再起動して更新を完了" })).toBeInTheDocument();
  });
});

describe("設定の「アップデートを確認」（update_check）", () => {
  async function openAbout(user: Awaited<ReturnType<typeof setup>>["user"]) {
    await user.click(screen.getByRole("button", { name: "設定" }));
    return screen.findByRole("dialog", { name: "設定" });
  }

  it("最新なら「最新です」と出す", async () => {
    const { user, mock } = await setup();
    const check = vi.spyOn(mock, "updateCheck");
    const dialog = await openAbout(user);
    await user.click(within(dialog).getByRole("button", { name: "アップデートを確認" }));
    expect(await within(dialog).findByText("最新です（バージョン 0.1.0）")).toBeInTheDocument();
    expect(check).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("region", { name: "アップデート" })).not.toBeInTheDocument();
  });

  it("新しい版があれば知らせる（バナーにも出る）", async () => {
    const { user, mock } = await setup();
    mock.mock.setUpdateCheckResult({ version: "0.4.0", currentVersion: "0.1.0", notes: "- 直しました", date: null });
    const dialog = await openAbout(user);
    await user.click(within(dialog).getByRole("button", { name: "アップデートを確認" }));
    expect(await within(dialog).findByRole("region", { name: "アップデート" })).toHaveTextContent("新しい版 0.4.0 があります");
    expect(within(dialog).queryByText(/最新です/)).not.toBeInTheDocument();
  });

  it("確認できなければ、エラーの文を出す", async () => {
    const { user, mock } = await setup();
    mock.mock.setUpdateCheckResult(new Error("更新サーバーに接続できません"));
    const dialog = await openAbout(user);
    await user.click(within(dialog).getByRole("button", { name: "アップデートを確認" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("確認できませんでした: 更新サーバーに接続できません");
  });
});
