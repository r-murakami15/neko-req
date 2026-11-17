import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { FileDropEvent } from "./api/backend";
import { createMockBackend } from "./mock/backend";
import { renderApp } from "./test/render";
import { footerButton, setupWithApi, sidebar } from "./test/openapi";

/** プロジェクトがない状態から、作成画面の「OpenAPI から取り込む」でダイアログを開く。 */
async function openImport() {
  const backend = createMockBackend();
  const view = renderApp(backend);
  await view.user.click(await screen.findByRole("button", { name: /OpenAPI から取り込む/ }));
  const dialog = await screen.findByRole("dialog", { name: "OpenAPIから取り込み" });
  return { ...view, backend, dialog };
}

async function typePath(user: ReturnType<typeof renderApp>["user"], dialog: HTMLElement, text: string) {
  const input = within(dialog).getByLabelText("パスまたは URL");
  await user.clear(input);
  await user.click(input);
  await user.paste(text);
}

describe("取り込みの 3 段階", () => {
  it("ファイル選択 → 内容の確認 → 取り込みで、新しいプロジェクトと API ツリーができる", async () => {
    const { user, dialog, backend } = await openImport();

    // 1. ファイル選択: 指定方法は 2 通り。入力するまで進めない
    expect(within(dialog).getByRole("radio", { name: /パス・URL を指定/ })).toBeChecked();
    expect(within(dialog).getByRole("radio", { name: /ファイルを直接アップロード/ })).not.toBeChecked();
    expect(within(dialog).getByRole("button", { name: "内容を確認" })).toBeDisabled();
    expect(within(dialog).getByRole("list", { name: "取り込みの手順" })).toHaveTextContent(/ファイル選択.*内容の確認.*取り込み/);

    await user.click(within(dialog).getByRole("button", { name: "ファイルを選択…" }));
    expect(within(dialog).getByLabelText("パスまたは URL")).toHaveValue("/mock/openapi.yaml");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));

    // 2. 内容の確認
    expect(await within(dialog).findByText("ミニ契約管理 API", { selector: "dd" })).toBeInTheDocument();
    const dl = within(dialog);
    expect(dl.getByText("0.1.0")).toBeInTheDocument();
    expect(dl.getByText("3.0.3")).toBeInTheDocument();
    expect(dl.getByText(/分割された yaml（6 ファイルを読みました）/)).toBeInTheDocument();
    expect(dl.getByText("cookieAuth（Cookie: SESSION）")).toBeInTheDocument();
    // servers → 作られる環境
    const servers = dl.getByRole("list", { name: "作られる環境" });
    expect(servers).toHaveTextContent("ローカル (Dev Container)");
    expect(servers).toHaveTextContent("http://localhost:8080");
    // 変換オプション 4 つ（既定はすべて入）
    for (const label of ["タグごとにフォルダを作る", "example からボディとパラメータを埋める", "cookieAuth を認証設定に反映する", "description を設計タブのドキュメントにする"]) {
      expect(dl.getByRole("checkbox", { name: label })).toBeChecked();
    }
    // タグごとの API 一覧
    expect(dl.getByRole("status")).toHaveTextContent("11 / 11 件を選択中 · タグ 4");
    const list = dl.getByRole("group", { name: "取り込む API" });
    expect(within(list).getByRole("button", { name: /^auth/ })).toHaveTextContent("4 件");
    expect(within(list).getByRole("button", { name: /^video-work/ })).toHaveAttribute("aria-expanded", "false");
    expect(within(list).getByText("/api/v1/auth/password-login")).toBeInTheDocument();
    // 検査結果
    const inspection = dl.getByRole("group", { name: "検査結果" });
    expect(inspection).toHaveTextContent("operationId のある 10 件はすべて一意です");
    expect(inspection).toHaveTextContent("security: [] の 3 件は「認証なし」として取り込みます");
    expect(inspection).toHaveTextContent("description に Markdown の表がある 2 件は、設計タブで表として表示します");
    // 取り込み先は新規プロジェクト（名前は yaml の title）
    expect(dl.getByRole("combobox", { name: /取り込み先/ })).toHaveValue("new");
    expect(dl.getByLabelText("プロジェクト名")).toHaveValue("ミニ契約管理 API");

    // 3. 取り込み
    await user.click(dl.getByRole("button", { name: "11 件を取り込む" }));
    expect(await dl.findByText("11 件を取り込みました")).toBeInTheDocument();
    expect(dl.getByText(/作った環境: ローカル \(Dev Container\)/)).toBeInTheDocument();
    await user.click(footerButton(dialog, "閉じる"));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    // 新しいプロジェクトが開き、API ツリーにタグごとに並ぶ
    expect(await screen.findByRole("button", { name: /プロジェクトを切り替え（現在: ミニ契約管理 API）/ })).toBeInTheDocument();
    expect(screen.getByText("v0.1.0 · OpenAPI 3.0.3")).toBeInTheDocument();
    expect(await within(sidebar()).findByText("11 件 · タグ 4")).toBeInTheDocument();
    expect((await backend.projectList())[0].source?.kind).toBe("path");
    expect((await backend.envList((await backend.projectList())[0].id)).map((e) => e.name)).toContain("ローカル (Dev Container)");
  });

  it("選んだ API だけが取り込まれる（グループのチェック・個別のチェック・すべて解除）", async () => {
    const { user, dialog, backend } = await openImport();
    const importSpy = vi.spyOn(backend, "openapiImport");
    await typePath(user, dialog, "/mock/openapi.yaml");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    const list = await within(dialog).findByRole("group", { name: "取り込む API" });

    // すべて解除 → 取り込めない。すべて選択で戻る
    await user.click(within(dialog).getByRole("button", { name: "すべて解除" }));
    expect(within(dialog).getByRole("status")).toHaveTextContent("0 / 11 件を選択中");
    expect(within(dialog).getByRole("button", { name: "0 件を取り込む" })).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: "すべて選択" }));
    expect(within(dialog).getByRole("status")).toHaveTextContent("11 / 11 件を選択中");

    // 「legacy」のグループを外し、auth の中の 1 件を外す
    await user.click(within(list).getByRole("checkbox", { name: "legacy のすべての API を選ぶ" }));
    await user.click(within(list).getByRole("checkbox", { name: "auth のすべての API を選ぶ" })); // 全部外れる
    await user.click(within(list).getByRole("checkbox", { name: "auth のすべての API を選ぶ" })); // 全部入る
    await user.click(within(list).getByRole("checkbox", { name: /GET \/api\/v1\/auth\/session/ }));
    expect(within(dialog).getByRole("status")).toHaveTextContent("9 / 11 件を選択中");
    expect(within(list).getByRole("button", { name: /^auth/ })).toHaveTextContent("3 / 4 件");

    await user.click(within(dialog).getByRole("button", { name: "9 件を取り込む" }));
    expect(await within(dialog).findByText("9 件を取り込みました")).toBeInTheDocument();
    const request = importSpy.mock.calls[0][0];
    expect(request.selected).toHaveLength(9);
    expect(request.projectId).toBeNull();
    expect(JSON.stringify(request.selected)).not.toContain("legacyPing");
    expect(JSON.stringify(request.selected)).not.toContain("getSession");

    await user.click(footerButton(dialog, "閉じる"));
    expect(await within(sidebar()).findByText("9 件 · タグ 3")).toBeInTheDocument();
    const project = (await backend.projectList())[0];
    expect((await backend.operationList(project.id)).map((o) => o.operationId)).not.toContain("legacyPing");
    expect((await backend.operationList(project.id)).map((o) => o.operationId)).not.toContain("getSession");
  });

  it("変換オプションは取り込みに渡る。タグごとにフォルダを作らないとツリーは平らになる", async () => {
    const { user, dialog, backend } = await openImport();
    const importSpy = vi.spyOn(backend, "openapiImport");
    await typePath(user, dialog, "/mock/openapi.yaml");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    await user.click(await within(dialog).findByRole("checkbox", { name: "タグごとにフォルダを作る" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "cookieAuth を認証設定に反映する" }));
    await user.click(within(dialog).getByRole("button", { name: "11 件を取り込む" }));
    await within(dialog).findByText("11 件を取り込みました");
    expect(importSpy.mock.calls[0][0].options).toEqual({ tagFolders: false, fillFromExamples: true, applyCookieAuth: false, descriptionAsDoc: true });
    await user.click(footerButton(dialog, "閉じる"));
    // フォルダなし: タグのフォルダ（auth など）は出ず、API がそのまま並ぶ
    expect(await within(sidebar()).findByRole("treeitem", { name: /ログイン状態取得/ })).toBeInTheDocument();
    expect(within(sidebar()).queryByRole("treeitem", { name: /^auth\s/ })).not.toBeInTheDocument();
  });

  it("ファイルを直接アップロード: ダイアログで選ぶ・ドロップで足す・外す。パスが取り込みに渡る", async () => {
    const base = createMockBackend();
    let drop: ((e: FileDropEvent) => void) | undefined;
    const backend = Object.assign(base, {
      onFileDrop: async (h: (e: FileDropEvent) => void) => {
        drop = h;
        return () => {
          drop = undefined;
        };
      },
    });
    const previewSpy = vi.spyOn(backend, "openapiPreview");
    const { user } = renderApp(backend);
    await user.click(await screen.findByRole("button", { name: /OpenAPI から取り込む/ }));
    const dialog = await screen.findByRole("dialog", { name: "OpenAPIから取り込み" });
    await user.click(within(dialog).getByRole("radio", { name: /ファイルを直接アップロード/ }));

    expect(within(dialog).getByText(/ここにフォルダ・ファイル・zip をドラッグ＆ドロップ/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "内容を確認" })).toBeDisabled();

    await user.click(within(dialog).getByRole("button", { name: "フォルダを選択…" }));
    const chosen = await within(dialog).findByRole("list", { name: "選んだもの" });
    expect(chosen).toHaveTextContent("api-docs");

    // ドロップ: 重ねて足せる（同じものは 1 つ）
    await waitFor(() => expect(drop).toBeDefined());
    act(() => drop?.({ type: "over" }));
    expect(within(dialog).getByLabelText("アップロードするファイルのドロップ先")).toHaveClass("is-over");
    act(() => drop?.({ type: "drop", paths: ["/Users/me/specs/openapi.zip", "/mock/api-docs"] }));
    await waitFor(() => expect(within(dialog).getByRole("list", { name: "選んだもの" }).querySelectorAll("li")).toHaveLength(2));
    expect(within(dialog).getByRole("list", { name: "選んだもの" })).toHaveTextContent("openapi.zip");

    await user.click(within(dialog).getByRole("button", { name: "api-docs を外す" }));
    expect(within(dialog).getByRole("list", { name: "選んだもの" }).querySelectorAll("li")).toHaveLength(1);

    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    expect(await within(dialog).findByText("取り込むリクエスト")).toBeInTheDocument();
    expect(previewSpy).toHaveBeenCalledWith({ kind: "upload", paths: ["/Users/me/specs/openapi.zip"], entry: null }, null);
    expect(within(dialog).getByText(/アップロード（1 件）/)).toBeInTheDocument();
  });
});

describe("入口の候補・別ホスト・取り込めない問題", () => {
  it("入口の候補が複数のときは、選んで続ける（パス）", async () => {
    const { user, dialog, backend } = await openImport();
    const spy = vi.spyOn(backend, "openapiPreview");
    await typePath(user, dialog, "/mock/multiple");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));

    const choice = await within(dialog).findByRole("group", { name: "入口ファイルの選択" });
    expect(choice).toHaveTextContent("入口ファイルが複数あります");
    expect(choice).toHaveTextContent("/mock/multiple/openapi.yaml");
    expect(choice).toHaveTextContent("/mock/multiple/legacy/openapi.yaml");
    await user.click(within(choice).getAllByRole("button", { name: "これを取り込む" })[1]);

    expect(await within(dialog).findByText("取り込むリクエスト")).toBeInTheDocument();
    // 選んだ候補の絶対パスで呼び直している
    expect(spy).toHaveBeenLastCalledWith({ kind: "path", path: "/mock/multiple/legacy/openapi.yaml" }, null);
  });

  it("入口の候補が複数のときは、選んで続ける（アップロードは entry に入れる）", async () => {
    const { user, dialog, backend } = await openImport();
    const spy = vi.spyOn(backend, "openapiPreview");
    await user.click(within(dialog).getByRole("radio", { name: /ファイルを直接アップロード/ }));
    await user.click(within(dialog).getByRole("button", { name: "フォルダを選択…" }));
    // モックは「multiple」を含むパスで入口の候補を返す
    vi.spyOn(backend, "pickPaths").mockResolvedValue(["/mock/multiple-docs"]);
    await user.click(within(dialog).getByRole("button", { name: "フォルダを選択…" }));
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    const choice = await within(dialog).findByRole("group", { name: "入口ファイルの選択" });
    await user.click(within(choice).getAllByRole("button", { name: "これを取り込む" })[0]);
    expect(await within(dialog).findByText("取り込むリクエスト")).toBeInTheDocument();
    expect(spy).toHaveBeenLastCalledWith({ kind: "upload", paths: ["/mock/api-docs", "/mock/multiple-docs"], entry: "openapi.yaml" }, null);
  });

  it("別のホストを指す参照は、確認してから読み直す。URL ならトークンも渡る", async () => {
    const { user, dialog, backend } = await openImport();
    const spy = vi.spyOn(backend, "openapiPreview");
    await typePath(user, dialog, "https://git.example.com/foreign/openapi.yaml");
    // URL のときだけトークン欄が出る
    await user.type(within(dialog).getByLabelText("アクセストークン"), "ghp_secret");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));

    const warn = await within(dialog).findByRole("group", { name: "別のホストの確認" });
    expect(warn).toHaveTextContent("assets.other-host.example");
    expect(warn).toHaveTextContent("openapi.yaml:12");
    expect(spy).toHaveBeenCalledWith(
      { kind: "url", url: "https://git.example.com/foreign/openapi.yaml", token: "ghp_secret", allowedHosts: [] },
      null,
    );

    await user.click(within(warn).getByRole("button", { name: "許可して続ける" }));
    expect(await within(dialog).findByText("取り込むリクエスト")).toBeInTheDocument();
    expect(spy).toHaveBeenLastCalledWith(
      { kind: "url", url: "https://git.example.com/foreign/openapi.yaml", token: "ghp_secret", allowedHosts: ["assets.other-host.example"] },
      null,
    );
  });

  it("参照先のファイルがない・yaml が壊れている・対応していない版は、場所と理由を出して取り込まない", async () => {
    const { user, dialog } = await openImport();
    await typePath(user, dialog, "/mock/missing");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    let alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("足りないファイル: video/client.yaml");
    expect(alert).toHaveTextContent("参照元: openapi.yaml:38openapi.yaml:57");
    expect(within(dialog).queryByText("取り込むリクエスト")).not.toBeInTheDocument();

    await typePath(user, dialog, "/mock/broken");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    await waitFor(() => expect(within(dialog).getByRole("alert")).toHaveTextContent("video/client.yaml:42:7"));
    alert = within(dialog).getByRole("alert");
    expect(alert).toHaveTextContent("42 行目 7 列目");
    expect(alert).toHaveTextContent("mapping values are not allowed in this context");

    await typePath(user, dialog, "/mock/swagger.yaml");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    await waitFor(() => expect(within(dialog).getByRole("alert")).toHaveTextContent("対応していない版です"));
    expect(within(dialog).getByRole("alert")).toHaveTextContent("2.0");

    await typePath(user, dialog, "/mock/notopenapi.yaml");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    await waitFor(() => expect(within(dialog).getByRole("alert")).toHaveTextContent("OpenAPI の文書ではありません"));
  });

  it("指定を変えたら、前の確認結果（別ホストの確認など）は消える", async () => {
    const { user, dialog } = await openImport();
    await typePath(user, dialog, "https://git.example.com/foreign/openapi.yaml");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    expect(await within(dialog).findByRole("group", { name: "別のホストの確認" })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("radio", { name: /ファイルを直接アップロード/ }));
    expect(within(dialog).queryByRole("group", { name: "別のホストの確認" })).not.toBeInTheDocument();
  });

  it("検査結果: 解決できない $ref と operationId の重複は警告として出る", async () => {
    const { user, dialog } = await openImport();
    await typePath(user, dialog, "/mock/refwarn-duplicate.yaml");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    const inspection = await within(dialog).findByRole("group", { name: "検査結果" });
    expect(inspection).toHaveTextContent("解決できない $ref: ./common.yaml#/components/schemas/Missing");
    expect(inspection).toHaveTextContent("video/client.yaml:88");
    expect(inspection).toHaveTextContent("operationId「getSession」が重複しています");
  });
});

describe("開き方と、既存プロジェクトへの統合", () => {
  it("ヘッダーのボタンと、新規作成ダイアログの「OpenAPI から」で開く", async () => {
    const backend = createMockBackend();
    await backend.projectCreate("P");
    const { user } = renderApp(backend);
    const header = await screen.findByRole("banner");
    await user.click(within(header).getByRole("button", { name: /OpenAPI取り込み/ }));
    expect(await screen.findByRole("dialog", { name: "OpenAPIから取り込み" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "キャンセル" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: "新規作成" }));
    await user.click(await screen.findByRole("button", { name: /OpenAPI から/ }));
    expect(await screen.findByRole("dialog", { name: "OpenAPIから取り込み" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "新規作成" })).not.toBeInTheDocument();
  });

  it("既存プロジェクトに統合すると、そのプロジェクトとの差分を付けて確認でき、反映できる", async () => {
    const { user, mock, project } = await setupWithApi();
    // yaml が更新された状態で、同じ yaml をもう一度取り込む（統合）
    mock.mock.touchSource();
    await user.click(within(await screen.findByRole("banner")).getByRole("button", { name: /OpenAPI取り込み/ }));
    const dialog = await screen.findByRole("dialog", { name: "OpenAPIから取り込み" });
    await typePath(user, dialog, "/mock/openapi.yaml");
    await user.click(within(dialog).getByRole("button", { name: "内容を確認" }));
    await within(dialog).findByText("取り込むリクエスト");
    expect(within(dialog).queryByRole("group", { name: "既存プロジェクトとの差分" })).not.toBeInTheDocument();

    await user.selectOptions(within(dialog).getByRole("combobox", { name: /取り込み先/ }), "existing");
    const diff = await within(dialog).findByRole("group", { name: "既存プロジェクトとの差分" });
    expect(diff).toHaveTextContent("追加 1 · 変更 1 · 削除 1");
    expect(within(dialog).getByRole("combobox", { name: "統合先のプロジェクト" })).toHaveValue(project.id);

    await user.click(within(dialog).getByRole("button", { name: /件を取り込む/ }));
    expect(await within(dialog).findByText(/件を取り込みました/)).toBeInTheDocument();
    expect(within(dialog).getByText(/追加 1 · 変更 1 · 削除 1/)).toBeInTheDocument();
    await user.click(footerButton(dialog, "閉じる"));
    expect(await within(sidebar()).findByRole("treeitem", { name: /^video-client\s/ })).toBeInTheDocument();
    expect((await mock.operationList(project.id)).some((o) => o.operationId === "getClient")).toBe(true);
  });
});
