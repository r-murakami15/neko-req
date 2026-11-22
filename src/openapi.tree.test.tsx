import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { OperationListItem } from "./bindings";
import { createMockBackend } from "./mock/backend";
import { buildTree, matchesOp, summarize } from "./lib/tree";
import { renderApp } from "./test/render";
import { footerButton, openFolder, openOp, setupWithApi, sidebar } from "./test/openapi";

const item = (over: Partial<OperationListItem> & { n: number }): OperationListItem => ({
  opKey: { kind: "id", value: `op${over.n}` },
  operationId: `op${over.n}`,
  method: "GET",
  path: `/api/v1/items/${over.n}`,
  summary: `API ${over.n}`,
  folder: "items",
  tags: ["items"],
  requiresAuth: true,
  deleted: false,
  hasDraft: false,
  ...over,
});

describe("API ツリーの組み立て", () => {
  it("検索は概要・operationId・path・method・タグに当たり、空白区切りは AND", () => {
    const op = item({ n: 1, summary: "取引先一覧取得", operationId: "listClients", path: "/api/v1/video/clients", tags: ["video-client"] });
    expect(matchesOp(op, "")).toBe(true);
    expect(matchesOp(op, "取引先")).toBe(true);
    expect(matchesOp(op, "LISTCLIENTS")).toBe(true);
    expect(matchesOp(op, "/video/clients")).toBe(true);
    expect(matchesOp(op, "get video-client")).toBe(true);
    expect(matchesOp(op, "post")).toBe(false);
    expect(matchesOp(op, "取引先 作品")).toBe(false);
  });

  it("タグのフォルダ・削除された API・その他を平らな行にする。既定で開くのは最初のタグと「その他」", () => {
    const ops = [item({ n: 1, folder: "a" }), item({ n: 2, folder: "a" }), item({ n: 3, folder: "b" }), item({ n: 4, folder: "b", deleted: true })];
    const rows = buildTree({ ops, adhocs: [], adhocLoaded: true, query: "", expanded: {} });
    expect(rows.map((r) => (r.type === "folder" ? `[${r.label} ${r.count}${r.open ? "+" : "-"}]` : r.type === "op" ? r.item.summary : r.type))).toEqual([
      "[a 2+]",
      "API 1",
      "API 2",
      "[b 1-]",
      "[削除された API 1-]",
      "[その他 0+]",
      "hint",
    ]);
    expect(summarize(ops)).toEqual({ count: 3, tags: 2 });
    // 開閉は上書きできる。検索中は当たったフォルダがすべて開く
    const toggled = buildTree({ ops, adhocs: [], adhocLoaded: true, query: "", expanded: { "tag:a": false, "tag:b": true } });
    expect(toggled.filter((r) => r.type === "op")).toHaveLength(1);
    const found = buildTree({ ops, adhocs: [], adhocLoaded: true, query: "API 3", expanded: {} });
    expect(found.map((r) => (r.type === "folder" ? r.label : r.type === "op" ? r.item.summary : r.type))).toEqual(["b", "API 3"]);
  });
});

describe("API ツリー", () => {
  it("件数・タグのフォルダ（開閉・件数）・method の色と概要を出し、クリックでタブに開く（開いていればそのタブへ）", async () => {
    const { user } = await setupWithApi();
    const nav = within(sidebar());
    expect(await nav.findByText("11 件 · タグ 4")).toBeInTheDocument();

    // 最初のタグ（auth）だけ開いている
    const auth = nav.getByRole("treeitem", { name: /^auth(\s|$)/ });
    expect(auth).toHaveAttribute("aria-expanded", "true");
    // フォルダ内の件数は出さない
    expect(auth).not.toHaveTextContent("4");
    expect(nav.getByRole("treeitem", { name: /ログイン状態取得/ })).toBeInTheDocument();
    const clientFolder = nav.getByRole("treeitem", { name: /^video-client(\s|$)/ });
    expect(clientFolder).toHaveAttribute("aria-expanded", "false");
    expect(nav.queryByRole("treeitem", { name: /取引先一覧取得/ })).not.toBeInTheDocument();
    // タグなしの API は「（タグなし）」に入る
    expect(nav.getByRole("treeitem", { name: /^（タグなし）(\s|$)/ })).toBeInTheDocument();

    await user.click(clientFolder);
    expect(clientFolder).toHaveAttribute("aria-expanded", "true");
    const list = nav.getByRole("treeitem", { name: /取引先一覧取得/ });
    expect(within(list).getByText("GET")).toHaveStyle({ color: "var(--method-get)" });
    expect(within(nav.getByRole("treeitem", { name: /取引先登録/ })).getByText("POST")).toHaveStyle({ color: "var(--method-post)" });

    // クリックでタブに開く。URL は {{baseUrl}}/path
    await user.click(list);
    expect(await screen.findByRole("tab", { name: /取引先一覧取得/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByLabelText("リクエストURL")).toHaveValue("{{baseUrl}}/api/v1/video/clients");
    expect(screen.getByLabelText("HTTPメソッド")).toBeDisabled();
    await waitFor(() => expect(screen.getByRole("status", { name: "展開後の URL" })).toHaveTextContent("http://localhost:8080/api/v1/video/clients"));

    // 別の API を開いてから、もう一度クリックすると、新しいタブは作らず元のタブに戻る
    await user.click(nav.getByRole("treeitem", { name: /取引先登録/ }));
    expect(await screen.findByRole("tab", { name: /取引先登録/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getAllByRole("tab", { name: /^(GET|POST) / }).length).toBeGreaterThanOrEqual(2);
    const before = screen.getAllByRole("tablist", { name: "開いているリクエスト" })[0].querySelectorAll('[role="tab"]').length;
    await user.click(nav.getByRole("treeitem", { name: /取引先一覧取得/ }));
    await waitFor(() => expect(screen.getByRole("tab", { name: /取引先一覧取得/ })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("tablist", { name: "開いているリクエスト" }).querySelectorAll('[role="tab"]')).toHaveLength(before);
  });

  it("検索で絞り込める（概要・operationId・path）。当たったタグは自動で開く", async () => {
    const { user } = await setupWithApi();
    const nav = within(sidebar());
    const search = nav.getByLabelText("API を検索");

    await user.type(search, "取引先");
    expect(await nav.findByRole("treeitem", { name: /取引先一覧取得/ })).toBeInTheDocument();
    expect(nav.getByRole("treeitem", { name: /取引先登録/ })).toBeInTheDocument();
    expect(nav.queryByRole("treeitem", { name: /ログイン状態取得/ })).not.toBeInTheDocument();
    expect(nav.getByText("検索: 3 件")).toBeInTheDocument();

    await user.clear(search);
    await user.type(search, "createPasswordLogin");
    expect(await nav.findByRole("treeitem", { name: /パスワードログイン/ })).toBeInTheDocument();
    expect(nav.queryByRole("treeitem", { name: /取引先一覧取得/ })).not.toBeInTheDocument();

    await user.clear(search);
    await user.type(search, "/works/");
    expect(await nav.findByRole("treeitem", { name: /作品取得/ })).toBeInTheDocument();

    await user.clear(search);
    await user.type(search, "zzzz");
    expect(await nav.findByText("該当する API がありません。")).toBeInTheDocument();

    await user.clear(search);
    expect(await nav.findByText("11 件 · タグ 4")).toBeInTheDocument();
  });

  it("API が 1,000 件あっても、見えている行だけを描く（仮想スクロール）", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const many = Array.from({ length: 1000 }, (_, i) => item({ n: i + 1, folder: "all" }));
    const backend = { ...base, operationList: async () => many };
    await setupWithApi(base, backend);
    const nav = within(sidebar());

    expect(await nav.findByText("1000 件 · タグ 1")).toBeInTheDocument();
    // 1 つのタグに 1,000 件。見えている分（と少しの余白）だけが DOM にある
    const slots = () => sidebar().querySelectorAll(".tree-slot");
    await waitFor(() => expect(slots().length).toBeGreaterThan(10));
    expect(slots().length).toBeLessThan(60);
    expect(nav.getByText("API 1")).toBeInTheDocument();
    expect(nav.queryByText("API 100")).not.toBeInTheDocument();
    // 全体の高さは 1,000 件ぶん（フォルダ 1 行 + 1,000 件 + 「その他」と案内の 2 行）
    expect(sidebar().querySelector(".tree-sizer")).toHaveStyle({ height: `${(1 + 1000 + 2) * 28}px` });

    // スクロールすると、そこにある行に入れ替わる
    const scroller = sidebar().querySelector(".tree-scroll") as HTMLElement;
    act(() => {
      scroller.scrollTop = 28 * 520;
      scroller.dispatchEvent(new Event("scroll"));
    });
    await waitFor(() => expect(nav.getByText("API 521")).toBeInTheDocument());
    expect(slots().length).toBeLessThan(60);
    expect(nav.queryByText("API 1")).not.toBeInTheDocument();
  });
});

describe("削除された API", () => {
  it("同期で yaml から消えても、入力値があれば「削除された API」に残り、自分で消せる", async () => {
    const { user, mock, project } = await setupWithApi();
    await openOp(user, "legacy", /旧ヘルスチェック/);
    await user.click(await screen.findByRole("button", { name: "保存" })); // 入力値（RequestDraft）を作る
    await waitFor(() => expect(mock.draftGet(project.id, { kind: "id", value: "legacyPing" })).resolves.not.toBeNull());

    mock.mock.touchSource();
    await user.click(within(sidebar()).getByRole("button", { name: /OpenAPI を同期/ }));
    const dialog = await screen.findByRole("dialog", { name: "OpenAPI の同期" });
    await user.click(await within(dialog).findByRole("button", { name: "反映する" }));
    await within(dialog).findByText(/反映しました/);
    await user.click(footerButton(dialog, "閉じる"));

    const nav = within(sidebar());
    const gone = await nav.findByRole("treeitem", { name: /^削除された API(\s|$)/ });
    await user.click(gone);
    expect(await nav.findByRole("treeitem", { name: /旧ヘルスチェック$/ })).toBeInTheDocument();
    // 開いていたタブには「yaml から削除」の知らせが出る
    expect(await screen.findByText(/この API は yaml から削除されました/, { selector: ".alert" })).toBeInTheDocument();

    await user.click(nav.getByRole("button", { name: /旧ヘルスチェック を消す/ }));
    const confirm = await screen.findByRole("dialog", { name: "削除された API を消しますか？" });
    expect(confirm).toHaveTextContent("入力値も一緒に消えます");
    await user.click(within(confirm).getByRole("button", { name: "消す" }));
    await waitFor(() => expect(nav.queryByRole("treeitem", { name: /^削除された API(\s|$)/ })).not.toBeInTheDocument());
    expect(screen.queryByRole("tab", { name: /旧ヘルスチェック/ })).not.toBeInTheDocument();
    expect(await mock.draftGet(project.id, { kind: "id", value: "legacyPing" })).toBeNull();
  });
});

describe("同期", () => {
  const syncButton = () => within(sidebar()).getByRole("button", { name: /OpenAPI を同期/ });

  it("yaml が変わると、起動時とウィンドウに戻ったときに同期ボタンに印が付き、差分の確認ダイアログから反映できる", async () => {
    const { user, mock, project } = await setupWithApi();
    expect(syncButton()).toHaveAccessibleName("OpenAPI を同期");
    expect(within(syncButton()).queryByRole("img", { name: "変更あり" })).not.toBeInTheDocument();

    // yaml が更新された → ウィンドウにフォーカスが戻ると印が付く（読み直しはしない）
    const check = vi.spyOn(mock, "openapiSyncCheck");
    mock.mock.touchSource();
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await waitFor(() => expect(syncButton()).toHaveAccessibleName("OpenAPI を同期（yaml に変更があります）"));
    expect(within(syncButton()).getByRole("img", { name: "変更あり" })).toBeInTheDocument();
    expect(check).not.toHaveBeenCalled();

    await user.click(syncButton());
    const dialog = await screen.findByRole("dialog", { name: "OpenAPI の同期" });
    // 追加・変更・削除の一覧。変更は何が変わったか
    expect(await within(dialog).findByText(/追加 1 · 変更 1 · 削除 1/)).toBeInTheDocument();
    const added = within(dialog).getByRole("region", { name: "追加" });
    expect(added).toHaveTextContent("/api/v1/video/clients/{clientId}");
    expect(added).toHaveTextContent("取引先詳細取得");
    const changed = within(dialog).getByRole("region", { name: "変更" });
    expect(changed).toHaveTextContent("/api/v1/video/clients");
    expect(changed).toHaveTextContent("説明: 説明が変わりました");
    expect(changed).toHaveTextContent("パラメータ: 削除: contractType");
    const removed = within(dialog).getByRole("region", { name: "削除" });
    expect(removed).toHaveTextContent("/api/v1/legacy/ping");
    expect(removed).toHaveTextContent("入力値がないので、ツリーから消えます");

    // 反映するまでは、ツリーはそのまま
    expect(await mock.operationList(project.id)).toHaveLength(11);
    await user.click(within(dialog).getByRole("button", { name: "反映する" }));
    await within(dialog).findByText(/反映しました（追加 1 · 変更 1 · 削除 1）/);
    await user.click(footerButton(dialog, "閉じる"));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    // ツリーが取り直され、印が消える
    expect(await within(sidebar()).findByText("11 件 · タグ 3")).toBeInTheDocument();
    await openFolder(user, "video-client");
    expect(await within(sidebar()).findByRole("treeitem", { name: /取引先詳細取得/ })).toBeInTheDocument();
    expect(within(sidebar()).queryByRole("treeitem", { name: /^legacy(\s|$)/ })).not.toBeInTheDocument();
    await waitFor(() => expect(within(syncButton()).queryByRole("img", { name: "変更あり" })).not.toBeInTheDocument());
  });

  it("差分がなければ「変更はありません」とだけ出す", async () => {
    const { user, mock } = await setupWithApi();
    const apply = vi.spyOn(mock, "openapiSyncApply");
    await user.click(syncButton());
    const dialog = await screen.findByRole("dialog", { name: "OpenAPI の同期" });
    expect(await within(dialog).findByText("変更はありません")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "反映する" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("region", { name: "追加" })).not.toBeInTheDocument();
    await user.click(footerButton(dialog, "閉じる"));
    expect(apply).not.toHaveBeenCalled();
  });

  it("yaml が壊れていたら、エラーの行と理由を出して同期を中止し、前回の API を使い続ける", async () => {
    const { user, mock, project } = await setupWithApi();
    mock.mock.touchSource();
    mock.mock.breakSource();
    await user.click(syncButton());
    const dialog = await screen.findByRole("dialog", { name: "OpenAPI の同期" });
    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("yaml を読めないため、同期を中止しました");
    expect(alert).toHaveTextContent("video/client.yaml:42:7");
    expect(alert).toHaveTextContent("42 行目 7 列目");
    expect(alert).toHaveTextContent("mapping values are not allowed in this context");
    expect(within(dialog).getByText(/前回取り込んだ API をそのまま使い続けます/)).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "反映する" })).not.toBeInTheDocument();

    // 直してからもう一度確認できる
    mock.mock.breakSource(false);
    await user.click(within(dialog).getByRole("button", { name: "もう一度確認" }));
    expect(await within(dialog).findByText(/追加 1 · 変更 1 · 削除 1/)).toBeInTheDocument();
    await user.click(footerButton(dialog, "キャンセル"));
    expect(await mock.operationList(project.id)).toHaveLength(11);
  });

  it("コピー方式（アップロード）は、同期のたびにもう一度ファイルを選んでもらう", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const r = await base.openapiImport({
      source: { kind: "upload", paths: ["/mock/api-docs"], entry: null },
      projectId: null,
      projectName: "コピー方式",
      selected: null,
      options: { tagFolders: true, fillFromExamples: true, applyCookieAuth: true, descriptionAsDoc: true },
    });
    expect(r.status).toBe("imported");
    const { user } = renderApp(base);
    await screen.findByRole("banner");
    // コピー方式は、起動時の更新日時の確認をしない
    expect(vi.isMockFunction(base.openapiSourceChanged)).toBe(false);

    base.mock.touchSource();
    const sync = vi.spyOn(base, "openapiSyncCheck");
    await user.click(await within(sidebar()).findByRole("button", { name: /OpenAPI を同期/ }));
    const dialog = await screen.findByRole("dialog", { name: "OpenAPI の同期" });
    const ask = await within(dialog).findByRole("group", { name: "もう一度アップロード" });
    expect(ask).toHaveTextContent("コピー方式で取り込んだ yaml です");
    expect(within(dialog).getByRole("button", { name: "差分を確認" })).toBeDisabled();

    await user.click(within(ask).getByRole("button", { name: "ファイルを選択…" }));
    await user.click(within(dialog).getByRole("button", { name: "差分を確認" }));
    expect(await within(dialog).findByText(/追加 1 · 変更 1 · 削除 1/)).toBeInTheDocument();
    expect(sync).toHaveBeenLastCalledWith(expect.any(String), { paths: ["/mock/api/openapi.yaml", "/mock/api/common.yaml"], entry: null }, null);
  });

  it("URL 方式のプロジェクトでは、同期ダイアログからアクセストークンを保存・消せる", async () => {
    const base = createMockBackend([], { sendDelayMs: 10 });
    const r = await base.openapiImport({
      source: { kind: "url", url: "https://git.example.com/api/openapi.yaml", token: "ghp_old", allowedHosts: [] },
      projectId: null,
      projectName: "URL 方式",
      selected: null,
      options: { tagFolders: true, fillFromExamples: true, applyCookieAuth: true, descriptionAsDoc: true },
    });
    expect(r.status).toBe("imported");
    const { user } = renderApp(base);
    const tokenSet = vi.spyOn(base, "sourceTokenSet");
    await screen.findByRole("banner");
    await user.click(await within(sidebar()).findByRole("button", { name: /OpenAPI を同期/ }));
    const dialog = await screen.findByRole("dialog", { name: "OpenAPI の同期" });
    expect(await within(dialog).findByText(/アクセストークン（保存済み）/)).toBeInTheDocument();

    await user.type(within(dialog).getByLabelText("新しいアクセストークン"), "ghp_new");
    await user.click(within(dialog).getByRole("button", { name: "トークンを保存" }));
    expect(await within(dialog).findByText("トークンを保存しました。")).toBeInTheDocument();
    expect(tokenSet).toHaveBeenCalledWith(expect.any(String), "ghp_new");
    // 入力欄に値は残らない
    expect(within(dialog).getByLabelText("新しいアクセストークン")).toHaveValue("");

    // 空のまま保存すると消える
    await user.click(within(dialog).getByRole("button", { name: "トークンを消す" }));
    expect(await within(dialog).findByText("トークンを消しました。")).toBeInTheDocument();
    expect(tokenSet).toHaveBeenLastCalledWith(expect.any(String), "");
    await waitFor(() => expect(within(dialog).getByText(/アクセストークン（未設定）/)).toBeInTheDocument());
  });
});
