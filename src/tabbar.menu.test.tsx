import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { suppressNativeContextMenu } from "./lib/contextmenu";
import { emptySpec } from "./lib/spec";
import { createMockBackend } from "./mock/backend";
import { useTabsStore } from "./store/tabs";
import { openOp, setupWithApi, sidebar } from "./test/openapi";

afterEach(() => vi.restoreAllMocks());

// メニューが開いている間は、Radix が画面のほかの部分を aria-hidden にするので、hidden も含めて探す
const tabsOf = (name: string | RegExp) => screen.getAllByRole("tab", { name, hidden: true });
/** 同じ名前のタブが複数あるときは、左から `nth` 番目。 */
const tab = (name: string | RegExp, nth = 0) => tabsOf(name)[nth];
const tabNames = () => tabsOf(/^(GET|POST|PUT|DELETE|PATCH) /).map((t) => t.textContent ?? "");
const countTabs = () => screen.queryAllByRole("tab", { name: /^(GET|POST|PUT|DELETE|PATCH) /, hidden: true }).length;
const menu = () => screen.getByRole("menu");
const item = (name: string) => within(menu()).getByRole("menuitem", { name });
const disabled = (name: string) => expect(item(name)).toHaveAttribute("aria-disabled", "true");
const enabled = (name: string) => expect(item(name)).not.toHaveAttribute("aria-disabled", "true");

/** タブを右クリックしてメニューを開く。 */
async function rightClick(name: string | RegExp, nth = 0) {
  fireEvent.contextMenu(tab(name, nth));
  await screen.findByRole("menu");
}

/** 空のタブを開いて URL を入れる（入れると未保存の印が付く）。 */
async function openBlank(user: UserEvent, url?: string) {
  await user.click(screen.getByRole("button", { name: "新しいタブ" }));
  if (url) {
    await user.click(await screen.findByLabelText("リクエストURL"));
    await user.paste(url);
  }
}

/** 取引先一覧取得・取引先登録・ログアウト（API）を開く。前面は最後に開いたログアウト。 */
async function setupThree() {
  const ctx = await setupWithApi(createMockBackend([], { sendDelayMs: 10 }));
  await openOp(ctx.user, "video-client", /取引先一覧取得/);
  await openOp(ctx.user, "video-client", /取引先登録/);
  await openOp(ctx.user, "auth", /ログアウト/);
  return ctx;
}

const isActive = (name: string | RegExp, nth = 0) => tab(name, nth).getAttribute("aria-selected") === "true";

describe("タブの右クリックメニュー", () => {
  it("右クリックしたタブが対象のメニューが、決めた順に出る（閉じるに Ctrl/⌘ W を添え、複製の前に区切り線）", async () => {
    await setupThree();
    await rightClick(/取引先登録/);
    expect(menu()).toHaveAccessibleName("取引先登録 のメニュー");
    const items = within(menu()).getAllByRole("menuitem");
    expect(items.map((i) => i.textContent)).toEqual([
      expect.stringMatching(/^タブを閉じる(Ctrl W|⌘ W)$/),
      "他のタブをすべて閉じる",
      "右側のタブをすべて閉じる",
      "すべてのタブを閉じる",
      "タブを複製",
      "ツリーで表示",
      "cURL としてコピー",
    ]);
    // 4 つ目と 5 つ目の間に区切り線
    const separators = within(menu()).getAllByRole("separator");
    expect(separators).toHaveLength(1);
    expect(items[3].nextElementSibling).toBe(separators[0]);
    expect(separators[0].nextElementSibling).toBe(items[4]);
    // 前面のタブ（ログアウト）は変わらない
    expect(isActive(/ログアウト/)).toBe(true);
  });

  it("他のタブがなければ「他を閉じる」、右側がなければ「右側を閉じる」は押せない", async () => {
    const { user } = await setupWithApi();
    await openBlank(user);
    await rightClick(/新しいリクエスト/);
    disabled("他のタブをすべて閉じる");
    disabled("右側のタブをすべて閉じる");
    enabled("すべてのタブを閉じる");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    await openOp(user, "video-client", /取引先一覧取得/);
    await openOp(user, "video-client", /取引先登録/);
    // タブは 新しいリクエスト・取引先一覧取得・取引先登録 の順
    await rightClick(/取引先登録/);
    enabled("他のタブをすべて閉じる");
    disabled("右側のタブをすべて閉じる");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    await rightClick(/新しいリクエスト/);
    enabled("他のタブをすべて閉じる");
    enabled("右側のタブをすべて閉じる");
  });

  it("「タブを閉じる」は、右クリックしたタブだけを閉じる（前面のタブは変わらない）。未保存なら 1 枚用の確認が出る", async () => {
    const { user } = await setupWithApi();
    await openBlank(user, "{{baseUrl}}/a");
    await openBlank(user);
    await openOp(user, "video-client", /取引先一覧取得/);

    // 2 枚目（空のタブ）を右クリック。前面は 3 枚目
    await rightClick(/新しいリクエスト/, 1);
    await user.click(item("タブを閉じる"));
    await waitFor(() => expect(tabNames()).toHaveLength(2));
    expect(tabsOf(/新しいリクエスト/)).toHaveLength(1); // 未保存の 1 枚目は残っている
    expect(isActive(/取引先一覧取得/)).toBe(true);

    // 未保存のタブ（URL を入れた 1 枚目）は、× と同じ確認が出る
    await rightClick(/新しいリクエスト/);
    await user.click(item("タブを閉じる"));
    const confirm = await screen.findByRole("dialog", { name: "タブを閉じますか？" });
    expect(confirm).toHaveTextContent("「新しいリクエスト」には保存していない変更があります。閉じると変更は失われます。");
    await user.click(within(confirm).getByRole("button", { name: "保存せずに閉じる" }));
    await waitFor(() => expect(screen.queryByRole("tab", { name: /新しいリクエスト/, hidden: true })).not.toBeInTheDocument());
  });

  it("他のタブをすべて閉じる: 対象のタブだけが残り、前面になる。別のプロジェクトのタブは混ぜない", async () => {
    const { user } = await setupThree();
    // 別のプロジェクトのタブ（画面には出ない）
    const stray = useTabsStore.getState().openNew("other-project");
    useTabsStore.getState().activate("other-project", stray);

    await rightClick(/取引先登録/);
    await user.click(item("他のタブをすべて閉じる"));
    await waitFor(() => expect(tabNames()).toHaveLength(1));
    expect(tab(/取引先登録/)).toHaveAttribute("aria-selected", "true");
    expect(useTabsStore.getState().tabs.map((t) => t.id)).toContain(stray);
    expect(useTabsStore.getState().tabs).toHaveLength(2);
  });

  it("右側のタブをすべて閉じる / すべてのタブを閉じる", async () => {
    const { user } = await setupThree();
    await rightClick(/取引先一覧取得/);
    await user.click(item("右側のタブをすべて閉じる"));
    await waitFor(() => expect(tabNames()).toHaveLength(1));
    // 前面だったログアウトが閉じたので、残った対象のタブが前面になる
    expect(isActive(/取引先一覧取得/)).toBe(true);

    await rightClick(/取引先一覧取得/);
    await user.click(item("すべてのタブを閉じる"));
    await waitFor(() => expect(countTabs()).toBe(0));
    expect(screen.getByText("開いているリクエストはありません")).toBeInTheDocument();
  });

  it("未保存のタブを含めてまとめて閉じるときは、確認が 1 回だけ出る。キャンセルなら何も閉じない", async () => {
    const { user } = await setupWithApi();
    await openOp(user, "video-client", /取引先一覧取得/); // 変更なし
    await openBlank(user, "{{baseUrl}}/a"); // 未保存
    await openBlank(user, "{{baseUrl}}/b"); // 未保存
    const before = tabNames();
    expect(before).toHaveLength(3);

    await rightClick(/取引先一覧取得/);
    await user.click(item("他のタブをすべて閉じる"));
    const confirm = await screen.findByRole("dialog", { name: "タブを閉じますか？" });
    expect(confirm).toHaveTextContent("保存していない変更が 2 件あります。閉じると変更は失われます。");
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(within(confirm).getByRole("button", { name: "保存せずに閉じる" })).toHaveClass("btn-danger");

    await user.click(within(confirm).getByRole("button", { name: "キャンセル" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(tabNames()).toEqual(before);
    expect(isActive(/新しいリクエスト/, 1)).toBe(true); // 前面も変わらない（最後に開いたタブ）

    // もう一度、今度は確定する。確認は 1 回で、あとから 1 枚ずつ確認が出ることはない
    await rightClick(/取引先一覧取得/);
    await user.click(item("他のタブをすべて閉じる"));
    const again = await screen.findByRole("dialog", { name: "タブを閉じますか？" });
    await user.click(within(again).getByRole("button", { name: "保存せずに閉じる" }));
    await waitFor(() => expect(tabNames()).toHaveLength(1));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(isActive(/取引先一覧取得/)).toBe(true);
  });

  it("未保存が 1 件のときも、すべてのタブを閉じる前に確認する。未保存がなければ確認なしで閉じる", async () => {
    const { user } = await setupWithApi();
    await openOp(user, "video-client", /取引先一覧取得/);
    await openBlank(user, "{{baseUrl}}/a");
    await rightClick(/取引先一覧取得/);
    await user.click(item("すべてのタブを閉じる"));
    const confirm = await screen.findByRole("dialog", { name: "タブを閉じますか？" });
    expect(confirm).toHaveTextContent("保存していない変更が 1 件あります。");
    await user.click(within(confirm).getByRole("button", { name: "保存せずに閉じる" }));
    await waitFor(() => expect(countTabs()).toBe(0));

    // 変更のないタブだけなら、確認なし
    await openOp(user, "video-client", /取引先一覧取得/);
    await openOp(user, "video-client", /取引先登録/);
    await rightClick(/取引先登録/);
    await user.click(item("すべてのタブを閉じる"));
    await waitFor(() => expect(countTabs()).toBe(0));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("送信中のタブは、まとめて閉じるときに止めてから閉じる", async () => {
    const base = createMockBackend([], { sendDelayMs: 5000 });
    const { user, mock } = await setupWithApi(base, base);
    const cancel = vi.spyOn(mock, "requestCancel");
    await openOp(user, "video-client", /取引先一覧取得/);
    await waitFor(() => expect(screen.getByRole("button", { name: "送信" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "送信" }));
    await screen.findByRole("button", { name: "中止" });
    // 送信 ID が分かるまで少し待つ
    await waitFor(() => expect(useTabsStore.getState().tabs[0].send.sendId).not.toBeNull());
    await openBlank(user);

    await rightClick(/新しいリクエスト/);
    await user.click(item("他のタブをすべて閉じる"));
    await waitFor(() => expect(tabNames()).toHaveLength(1));
    expect(cancel).toHaveBeenCalledTimes(1);
  });
});

describe("タブを複製", () => {
  async function setupAdhoc() {
    return setupWithApi(undefined, undefined, async (p, backend) => {
      await backend.adhocSave({
        id: "",
        projectId: p.id,
        name: "メモ用のリクエスト",
        description: "",
        method: "POST",
        url: "{{baseUrl}}/memo",
        params: emptySpec().params,
        body: { kind: "json", value: '{"a":1}' },
        auth: null,
        createdAt: "",
        updatedAt: "",
      });
    });
  }

  it("同じ入力の未保存のタブを、対象の右隣に開いて前面にする。名前は「<名前> のコピー」で、保存済みのリクエストとは結び付かない", async () => {
    const { user } = await setupAdhoc();
    await user.click(await within(sidebar()).findByRole("treeitem", { name: /メモ用のリクエスト/ }));
    await openOp(user, "video-client", /取引先一覧取得/);
    // 並び: メモ用のリクエスト・取引先一覧取得
    expect(isActive(/取引先一覧取得/)).toBe(true);

    await rightClick(/メモ用のリクエスト/);
    await user.click(item("タブを複製"));
    await waitFor(() => expect(tabNames()).toHaveLength(3));
    expect(tabNames()).toEqual([expect.stringContaining("メモ用のリクエスト"), expect.stringContaining("メモ用のリクエスト のコピー"), expect.stringContaining("取引先一覧取得")]);
    expect(tab(/メモ用のリクエスト のコピー/)).toHaveAttribute("aria-selected", "true");
    // 同じ入力
    expect(screen.getByLabelText("リクエストURL")).toHaveValue("{{baseUrl}}/memo");
    expect(screen.getByLabelText("HTTPメソッド")).toHaveValue("POST");
    await user.click(screen.getByRole("tab", { name: "ボディ" }));
    expect(await screen.findByRole("textbox", { name: "JSON のボディ" })).toHaveTextContent('{"a":1}');
    // 未保存の新しいタブ（保存済みのリクエストとは結び付かない）
    expect(within(tab(/メモ用のリクエスト のコピー/)).getByRole("img", { name: "未保存" })).toBeInTheDocument();
    const copy = useTabsStore.getState().tabs.find((t) => t.name === "メモ用のリクエスト のコピー")!;
    expect(copy.adhocId).toBeNull();
    expect(copy.op).toBeNull();
    // 元のタブは変わらず、未保存でもない
    expect(within(tab(/^POST メモ用のリクエスト$/)).queryByRole("img", { name: "未保存" })).not.toBeInTheDocument();
  });

  it("未保存の新しいタブも複製できる（未保存の入力ごと）。OpenAPI の API のタブは複製できない", async () => {
    const { user } = await setupWithApi();
    await openBlank(user, "{{baseUrl}}/draft");
    await rightClick(/新しいリクエスト/);
    await user.click(item("タブを複製"));
    await waitFor(() => expect(tabNames()).toHaveLength(2));
    expect(tab(/新しいリクエスト のコピー/)).toHaveAttribute("aria-selected", "true");
    expect(screen.getByLabelText("リクエストURL")).toHaveValue("{{baseUrl}}/draft");

    await openOp(user, "video-client", /取引先一覧取得/);
    await rightClick(/取引先一覧取得/);
    disabled("タブを複製");
    await user.click(item("タブを複製"));
    expect(tabNames()).toHaveLength(3);
  });
});

describe("ツリーで表示", () => {
  it("閉じたフォルダを開いて行を示す。すでに前面のタブでも、手で閉じたフォルダでも動く", async () => {
    const { user } = await setupThree();
    const folder = () => within(sidebar()).getByRole("treeitem", { name: /^video-client(\s|$)/ });
    // 前面はログアウト。video-client を手で閉じる
    await user.click(folder());
    expect(folder()).toHaveAttribute("aria-expanded", "false");
    expect(within(sidebar()).queryByRole("treeitem", { name: /取引先登録/ })).not.toBeInTheDocument();

    // 前面でないタブ: 前面になり、フォルダが開く
    await rightClick(/取引先登録/);
    await user.click(item("ツリーで表示"));
    expect(isActive(/取引先登録/)).toBe(true);
    expect(folder()).toHaveAttribute("aria-expanded", "true");
    const row = await within(sidebar()).findByRole("treeitem", { name: /取引先登録/ });
    expect(row).toHaveAttribute("aria-selected", "true");

    // すでに前面のタブで、手でフォルダを閉じたあと: 既存の連動では動かないが、明示的な要求なので開く
    await user.click(folder());
    expect(folder()).toHaveAttribute("aria-expanded", "false");
    await rightClick(/取引先登録/);
    await user.click(item("ツリーで表示"));
    expect(folder()).toHaveAttribute("aria-expanded", "true");
    expect(await within(sidebar()).findByRole("treeitem", { name: /取引先登録/ })).toBeInTheDocument();
  });

  it("「その他」の保存済みリクエストのタブも示す", async () => {
    const { user } = await setupWithApi(undefined, undefined, async (p, backend) => {
      await backend.adhocSave({ id: "", projectId: p.id, name: "メモ用のリクエスト", description: "", method: "GET", url: "{{baseUrl}}/memo", params: emptySpec().params, body: emptySpec().body, auth: null, createdAt: "", updatedAt: "" });
    });
    await user.click(await within(sidebar()).findByRole("treeitem", { name: /メモ用のリクエスト/ }));
    await openOp(user, "auth", /ログアウト/);
    const other = () => within(sidebar()).getByRole("treeitem", { name: /^その他(\s|$)/ });
    await user.click(other());
    expect(other()).toHaveAttribute("aria-expanded", "false");

    await rightClick(/メモ用のリクエスト/);
    await user.click(item("ツリーで表示"));
    expect(other()).toHaveAttribute("aria-expanded", "true");
    expect(await within(sidebar()).findByRole("treeitem", { name: /メモ用のリクエスト/ })).toHaveAttribute("aria-selected", "true");
  });

  it("ツリーにないタブ（新規の未保存タブ）と、検索で行が隠れているときは押せない", async () => {
    const { user } = await setupWithApi();
    await openOp(user, "video-client", /取引先一覧取得/);
    await openBlank(user);
    await rightClick(/新しいリクエスト/);
    disabled("ツリーで表示");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    await rightClick(/取引先一覧取得/);
    enabled("ツリーで表示");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

    await user.type(screen.getByLabelText("API を検索"), "ログアウト");
    await rightClick(/取引先一覧取得/);
    disabled("ツリーで表示");
  });
});

describe("cURL としてコピー", () => {
  it("前面でないタブでも、そのタブの入力を、秘密を伏せてコピーする。そのタブが前面になり、結果の知らせが出る", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const { user, mock } = await setupWithApi(base, base, async (p, b) => {
      const env = (await b.envList(p.id)).find((e) => !e.isGlobal)!;
      const vars = { session: "abc123", xsrfToken: "tok789" };
      await b.envSave({
        ...env,
        vars: [...env.vars, ...Object.entries(vars).map(([name, value]) => ({ name, value: { kind: "plain" as const, value }, description: "", enabled: true }))],
      });
    });
    // user-event が差し込むクリップボードを、描画のあとで差し替える
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const exp = vi.spyOn(mock, "curlExport");
    await openOp(user, "video-client", /取引先登録/);
    await openOp(user, "video-client", /取引先一覧取得/);
    expect(isActive(/取引先一覧取得/)).toBe(true);

    await rightClick(/取引先登録/);
    await user.click(item("cURL としてコピー"));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(exp).toHaveBeenCalledTimes(1);
    expect(exp).toHaveBeenCalledWith(expect.objectContaining({ opKey: { kind: "id", value: "createClient" }, environmentId: null }), "redacted");
    const text = writeText.mock.calls[0][0] as string;
    expect(text).toContain("curl -X POST 'http://localhost:8080/api/v1/video/clients'");
    expect(text).toContain("-H 'Cookie: ••••'");
    expect(text).not.toContain("abc123");
    expect(text).not.toContain("tok789");
    expect(isActive(/取引先登録/)).toBe(true);
    expect(await screen.findByText("cURL をコピーしました（秘密の値は伏せています）")).toBeInTheDocument();

    // ほかのタブに移ると、知らせは消える
    await user.click(tab(/取引先一覧取得/));
    expect(screen.queryByText("cURL をコピーしました（秘密の値は伏せています）")).not.toBeInTheDocument();
  });

  it("展開できない変数があるときは、失敗の知らせを出す", async () => {
    const { user } = await setupWithApi();
    await openBlank(user, "{{nope}}/x");
    await openBlank(user);
    // 1 枚目（URL を入れたタブ）が対象。前面は 2 枚目
    await rightClick(/新しいリクエスト/, 0);
    await user.click(item("cURL としてコピー"));
    expect(await screen.findByText(/cURL にできませんでした: 展開できない変数があります: \{\{nope\}\}/)).toBeInTheDocument();
  });
});

describe("開き方と標準メニュー", () => {
  it("Shift+F10 とコンテキストメニューキーで、フォーカスのあるタブのメニューを開く", async () => {
    const { user } = await setupThree();
    tab(/取引先登録/).focus();
    fireEvent.keyDown(tab(/取引先登録/), { key: "F10", shiftKey: true });
    await screen.findByRole("menu");
    expect(menu()).toHaveAccessibleName("取引先登録 のメニュー");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    // 閉じたら、そのタブにフォーカスが戻る
    expect(tab(/取引先登録/)).toHaveFocus();

    fireEvent.keyDown(tab(/取引先一覧取得/), { key: "ContextMenu" });
    await screen.findByRole("menu");
    expect(menu()).toHaveAccessibleName("取引先一覧取得 のメニュー");
  });

  it("アプリ全体の標準メニューの抑止と両立する: タブでは独自のメニューが開き、ほかの場所では何も出ない（どちらも標準メニューは出ない）", async () => {
    const off = suppressNativeContextMenu();
    try {
      const { user } = await setupThree();
      const onTab = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
      tab(/取引先登録/).dispatchEvent(onTab);
      expect(await screen.findByRole("menu")).toBeInTheDocument();
      expect(onTab.defaultPrevented).toBe(true);
      await user.keyboard("{Escape}");
      await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());

      for (const el of [screen.getByLabelText("リクエストURL"), sidebar(), document.body]) {
        const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
        el.dispatchEvent(ev);
        expect(ev.defaultPrevented).toBe(true);
      }
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    } finally {
      off();
    }
  });
});
