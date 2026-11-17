import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createMockBackend, type MockBackend } from "./mock/backend";
import { setupWithApi } from "./test/openapi";

afterEach(() => vi.restoreAllMocks());

async function openSettings(user: UserEvent) {
  await user.click(screen.getByRole("button", { name: "設定" }));
  const dialog = await screen.findByRole("dialog", { name: "設定" });
  await within(dialog).findByRole("heading", { level: 2, name: "外観" });
  return dialog;
}

const saved = (mock: MockBackend) => mock.mock.appSettings();

async function setup() {
  const base = createMockBackend([], { sendDelayMs: 10 });
  const ctx = await setupWithApi(base, base);
  const set = vi.spyOn(ctx.mock, "appSettingsSet");
  const dialog = await openSettings(ctx.user);
  return { ...ctx, set, dialog };
}

describe("設定ダイアログ", () => {
  it("ヘッダーの歯車から開き、画面イメージの項目が並ぶ（言語・プロジェクトの既定の保存先は出さない）", async () => {
    const { dialog } = await setup();
    const nav = within(dialog).getByRole("navigation", { name: "設定の項目" });
    expect(within(nav).getAllByRole("button").map((b) => b.textContent)).toEqual(["外観", "リクエスト", "プロキシ・証明書", "履歴とデータ", "エディタ", "ショートカット", "このアプリについて"]);
    for (const h of ["外観", "リクエスト", "プロキシ・証明書", "履歴とデータ", "エディタ", "ショートカット", "このアプリについて"]) {
      expect(within(dialog).getByRole("heading", { level: 2, name: h })).toBeInTheDocument();
    }
    expect(within(dialog).queryByText("言語")).not.toBeInTheDocument();
    expect(within(dialog).queryByText(/プロジェクトの既定の保存先/)).not.toBeInTheDocument();
    expect(within(dialog).getByText("NekoReq バージョン 0.1.0 · Windows")).toBeInTheDocument();
    // 項目を押すとその節へ
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    await userClickNav(nav, "履歴とデータ");
    expect(scroll).toHaveBeenCalled();
    expect(within(nav).getByRole("button", { name: "履歴とデータ" })).toHaveAttribute("aria-current", "true");
  });

  it("外観: テーマを切り替えると、すぐ画面に効いて保存される。文字サイズ・タブの復元も保存される", async () => {
    const { user, dialog, mock, set } = await setup();
    const html = document.documentElement;
    expect(html.hasAttribute("data-theme")).toBe(false); // OS に合わせる（既定）
    expect(within(dialog).getByRole("radio", { name: "OSに合わせる" })).toHaveAttribute("aria-checked", "true");

    await user.click(within(dialog).getByRole("radio", { name: "ライト" }));
    await waitFor(() => expect(html.getAttribute("data-theme")).toBe("light"));
    expect(saved(mock).appearance.theme).toBe("light");
    expect(within(dialog).getByRole("radio", { name: "ライト" })).toHaveAttribute("aria-checked", "true");

    await user.click(within(dialog).getByRole("radio", { name: "ダーク" }));
    await waitFor(() => expect(html.getAttribute("data-theme")).toBe("dark"));
    await user.click(within(dialog).getByRole("radio", { name: "OSに合わせる" }));
    await waitFor(() => expect(html.hasAttribute("data-theme")).toBe(false));
    expect(saved(mock).appearance.theme).toBe("system");

    await user.selectOptions(within(dialog).getByLabelText("エディタの文字サイズ"), "15 px");
    await waitFor(() => expect(html.style.getPropertyValue("--editor-font-size")).toBe("15px"));
    expect(saved(mock).appearance.editorFontSize).toBe(15);

    const restore = within(dialog).getByRole("switch", { name: "起動時に前回のタブを復元" });
    expect(restore).toBeChecked();
    await user.click(restore);
    await waitFor(() => expect(saved(mock).appearance.restoreTabs).toBe(false));
    expect(set).toHaveBeenCalled();

    // 閉じて開き直しても、保存した値のまま
    await user.click(within(dialog).getAllByRole("button", { name: "閉じる" })[0]);
    const again = await openSettings(user);
    expect(within(again).getByRole("switch", { name: "起動時に前回のタブを復元" })).not.toBeChecked();
    expect(within(again).getByLabelText("エディタの文字サイズ")).toHaveValue("15");
  });

  it("起動したとき、保存してあるテーマと文字サイズを最初から反映する", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    await setupWithApi(base, base, async (_p, b) => {
      const s = await b.appSettingsGet();
      await b.appSettingsSet({ ...s, appearance: { ...s.appearance, theme: "light", editorFontSize: 14 } });
    });
    await waitFor(() => expect(document.documentElement.getAttribute("data-theme")).toBe("light"));
    expect(document.documentElement.style.getPropertyValue("--editor-font-size")).toBe("14px");
  });

  it("リクエスト: タイムアウト・リダイレクト・証明書の検証・表示するレスポンスの上限を保存する。検証エラーは Rust のメッセージをそのまま出す", async () => {
    const { user, dialog, mock } = await setup();
    const timeout = within(dialog).getByLabelText("タイムアウト");
    expect(timeout).toHaveValue("30");
    await user.clear(timeout);
    await user.type(timeout, "45{Enter}");
    await waitFor(() => expect(saved(mock).request.timeoutSecs).toBe(45));

    // 数字でなければ、保存せずにその場で知らせる
    await user.clear(timeout);
    await user.type(timeout, "abc");
    await user.tab();
    expect(await within(dialog).findByText("タイムアウトは 0 以上の整数（秒）で入力してください")).toBeInTheDocument();
    expect(saved(mock).request.timeoutSecs).toBe(45);

    // Rust の検証エラー（10 万秒を超える）はそのまま出し、前の設定が残る
    await user.clear(timeout);
    await user.type(timeout, "200000");
    await user.tab();
    const alert = await within(dialog).findByText(/保存できませんでした: タイムアウトは 100000 秒以内にしてください（前の設定のままです）/);
    expect(alert).toBeInTheDocument();
    expect(saved(mock).request.timeoutSecs).toBe(45);
    // 直すと消える
    await user.clear(timeout);
    await user.type(timeout, "0");
    await user.tab();
    await waitFor(() => expect(saved(mock).request.timeoutSecs).toBe(0));
    await waitFor(() => expect(within(dialog).queryByText(/保存できませんでした/)).not.toBeInTheDocument());

    await user.click(within(dialog).getByRole("switch", { name: "リダイレクトを自動で追う" }));
    await waitFor(() => expect(saved(mock).request.followRedirects).toBe(false));
    await user.click(within(dialog).getByRole("switch", { name: "SSL 証明書を検証する" }));
    await waitFor(() => expect(saved(mock).request.verifyCertificates).toBe(false));
    await user.selectOptions(within(dialog).getByLabelText("表示するレスポンスの上限"), "50 MB");
    await waitFor(() => expect(saved(mock).request.responseLimit).toBe("mb50"));
    await user.selectOptions(within(dialog).getByLabelText("表示するレスポンスの上限"), "無制限");
    await waitFor(() => expect(saved(mock).request.responseLimit).toBe("unlimited"));
  });

  it("プロキシ: OS の設定・使わない・手動。手動は URL を入れるまで保存せず、URL・ユーザー名・除外を保存し、パスワードはキーチェーンへ", async () => {
    const { user, dialog, mock, set } = await setup();
    const radio = (name: string) => within(dialog).getByRole("radio", { name });
    expect(radio("OSの設定を使う")).toBeChecked();

    await user.click(radio("使わない"));
    await waitFor(() => expect(saved(mock).network.proxy.mode).toBe("none"));

    // 手動を選んだだけでは、空の URL を保存しようとしない（検証エラーにならない）
    set.mockClear();
    await user.click(radio("手動"));
    expect(radio("手動")).toBeChecked();
    expect(set).not.toHaveBeenCalled();
    const url = within(dialog).getByLabelText("プロキシの URL");

    // 認証情報つきの URL は Rust の検証エラーをそのまま出す
    await user.click(url);
    await user.paste("http://user:pass@proxy.example:8080");
    await user.tab();
    expect(await within(dialog).findByText(/プロキシの URL に user:pass@ は書けません/)).toBeInTheDocument();
    expect(saved(mock).network.proxy.mode).toBe("none");

    await user.clear(url);
    await user.paste("http://proxy.example:8080");
    await user.tab();
    await waitFor(() => expect(saved(mock).network.proxy).toMatchObject({ mode: "manual", url: "http://proxy.example:8080" }));
    await waitFor(() => expect(within(dialog).queryByText(/保存できませんでした/)).not.toBeInTheDocument());

    await user.type(within(dialog).getByLabelText("ユーザー名"), "taro");
    await user.tab();
    await waitFor(() => expect(saved(mock).network.proxy.username).toBe("taro"));
    await user.type(within(dialog).getByLabelText("プロキシを通さないホスト"), "localhost, *.internal.example{Enter}");
    await waitFor(() => expect(saved(mock).network.proxy.noProxy).toEqual(["localhost", "*.internal.example"]));

    // パスワード（キーチェーンだけ。設定には入らない）
    const pw = vi.spyOn(mock, "proxyPasswordSet");
    expect(within(dialog).getByText("未設定", { selector: ".settings-hint" })).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("プロキシのパスワード"), "s3cret");
    await user.click(within(dialog).getByRole("button", { name: "パスワードを保存" }));
    await waitFor(() => expect(pw).toHaveBeenCalledWith("s3cret"));
    expect(await within(dialog).findByText("保存済み（OS のキーチェーン）")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("プロキシのパスワード")).toHaveValue("");
    expect(JSON.stringify(saved(mock))).not.toContain("s3cret");
    expect(mock.mock.passwords().proxy).toBe(true);
    await user.click(within(dialog).getByRole("button", { name: "消す" }));
    await waitFor(() => expect(pw).toHaveBeenLastCalledWith(""));
    await waitFor(() => expect(mock.mock.passwords().proxy).toBe(false));
  });

  it("CA 証明書: ファイルを選んで設定し、解除できる", async () => {
    const { user, dialog, mock } = await setup();
    expect(within(dialog).getAllByText("未設定").length).toBeGreaterThan(0);
    await user.click(within(dialog).getAllByRole("button", { name: "ファイルを選択" })[0]);
    await waitFor(() => expect(saved(mock).network.caCertPath).toBe("/mock/certs/ca.pem"));
    expect(await within(dialog).findByText("/mock/certs/ca.pem")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "解除" }));
    await waitFor(() => expect(saved(mock).network.caCertPath).toBeNull());
  });

  it("クライアント証明書: ホストのパターンとファイルとパスワードを追加し、パスワードの設定・削除ができる", async () => {
    const { user, dialog, mock } = await setup();
    expect(within(dialog).getByText("未設定", { selector: ".client-certs .muted-note" })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "+ 追加" }));
    const form = within(dialog).getByRole("group", { name: "クライアント証明書を追加" });
    const add = within(form).getByRole("button", { name: "追加する" });
    expect(add).toBeDisabled();
    await user.type(within(form).getByLabelText("ホストのパターン"), "*.internal.example");
    await user.click(within(form).getByRole("button", { name: "ファイルを選択" }));
    expect(await within(form).findByText("/mock/certs/client.p12")).toBeInTheDocument();
    await user.type(within(form).getByLabelText("証明書のパスワード"), "p12-pass");
    await user.click(add);

    const row = await within(dialog).findByRole("group", { name: "クライアント証明書 *.internal.example" });
    expect(row).toHaveTextContent("*.internal.example → client.p12");
    await waitFor(() => expect(row).toHaveTextContent("パスワード: 保存済み"));
    expect(saved(mock).network.clientCerts).toHaveLength(1);
    expect(JSON.stringify(saved(mock))).not.toContain("p12-pass");
    const id = saved(mock).network.clientCerts[0].id;
    expect(mock.mock.passwords().clientCerts).toEqual([id]);

    // パスワードを消す
    await user.click(within(row).getByRole("button", { name: "パスワード" }));
    await user.click(within(row).getByRole("button", { name: "消す" }));
    await waitFor(() => expect(mock.mock.passwords().clientCerts).toEqual([]));
    await waitFor(() => expect(row).toHaveTextContent("パスワード: なし"));

    // 削除
    await user.click(within(row).getByRole("button", { name: "*.internal.example の証明書を削除" }));
    await waitFor(() => expect(saved(mock).network.clientCerts).toEqual([]));
    expect(within(dialog).queryByRole("group", { name: /クライアント証明書 \*/ })).not.toBeInTheDocument();
  });

  it("履歴: 保存する件数と、1 MB を超えるボディの省略", async () => {
    const { user, dialog, mock } = await setup();
    await user.selectOptions(within(dialog).getByLabelText("保存する件数"), "5,000 件");
    await waitFor(() => expect(saved(mock).history.limit).toBe(5000));
    await user.selectOptions(within(dialog).getByLabelText("保存する件数"), "保存しない");
    await waitFor(() => expect(saved(mock).history.limit).toBe(0));
    await user.click(within(dialog).getByRole("switch", { name: "1 MB を超えるボディを省略" }));
    await waitFor(() => expect(saved(mock).history.omitLargeBodies).toBe(false));
  });

  it("エディタのコマンド（{file}・{line}）を保存する。空にすると自動で探す", async () => {
    const { user, dialog, mock } = await setup();
    const field = within(dialog).getByLabelText("yaml を開くエディタのコマンド");
    await user.click(field);
    await user.paste("code -g {file}:{line}");
    await user.tab();
    await waitFor(() => expect(saved(mock).editorCommand).toBe("code -g {file}:{line}"));
    await user.clear(field);
    await user.tab();
    await waitFor(() => expect(saved(mock).editorCommand).toBeNull());
  });

  it("ショートカットの一覧は OS の表記（Windows は Ctrl、macOS は ⌘）で出る", async () => {
    const { dialog } = await setup();
    const keys = within(dialog).getByRole("heading", { level: 2, name: "ショートカット" }).closest("section")!;
    expect([...keys.querySelectorAll("kbd")].map((k) => k.textContent)).toEqual(["Ctrl Enter", "Ctrl S", "Ctrl T", "Ctrl W", "Ctrl K", "Ctrl ,"]);
    expect(within(keys).getByText("リクエストを送信")).toBeInTheDocument();
    expect(within(keys).getByText("タブを閉じる")).toBeInTheDocument();
  });

  it("macOS では ⌘ で表記する", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    const { dialog } = await setup();
    const keys = within(dialog).getByRole("heading", { level: 2, name: "ショートカット" }).closest("section")!;
    expect([...keys.querySelectorAll("kbd")].map((k) => k.textContent)).toEqual(["⌘ Enter", "⌘ S", "⌘ T", "⌘ W", "⌘ K", "⌘ ,"]);
    expect(within(dialog).getByText("NekoReq バージョン 0.1.0 · macOS")).toBeInTheDocument();
  });
});

async function userClickNav(nav: HTMLElement, name: string) {
  fireEvent.click(within(nav).getByRole("button", { name }));
}
