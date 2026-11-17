import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ApiError, type Backend } from "./api/backend";
import { createMockBackend } from "./mock/backend";
import { renderApp } from "./test/render";

describe("プロジェクトがないとき", () => {
  it("作成画面が出て、名前を入れて作るとヘッダーに名前が出る", async () => {
    const { user } = renderApp();

    expect(await screen.findByRole("heading", { name: "プロジェクトを作成" })).toBeInTheDocument();
    // まだアプリの外枠（ヘッダー）は出ない
    expect(screen.queryByRole("banner")).not.toBeInTheDocument();

    const create = screen.getByRole("button", { name: "プロジェクトを作成" });
    expect(create).toBeDisabled(); // 名前が空のうちは押せない

    await user.type(screen.getByLabelText("プロジェクト名"), "日本アニメーション API");
    await user.click(create);

    const header = await screen.findByRole("banner");
    expect(within(header).getByText("NekoReq")).toBeInTheDocument();
    expect(within(header).getByRole("button", { name: /日本アニメーション API/ })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "プロジェクトを作成" })).not.toBeInTheDocument();
  });

  it("Enter キーでも作成できる", async () => {
    const { user } = renderApp();
    await user.type(await screen.findByLabelText("プロジェクト名"), "Enter で作る{Enter}");
    expect(await screen.findByRole("button", { name: /Enter で作る/ })).toBeInTheDocument();
  });

  it("バックエンドが検証エラーを返したらその文言を出す", async () => {
    const base = createMockBackend();
    const backend: Backend = {
      ...base,
      projectCreate: async () => {
        throw new ApiError({ kind: "validation", message: "プロジェクト名を入力してください" });
      },
    };
    const { user } = renderApp(backend);
    await user.type(await screen.findByLabelText("プロジェクト名"), "x");
    await user.click(screen.getByRole("button", { name: "プロジェクトを作成" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("プロジェクト名を入力してください");
  });
});

describe("外枠", () => {
  it("ヘッダー・サイドバー・タブ領域が出て、リクエストを開くと入力欄とレスポンス欄が出る", async () => {
    const backend = createMockBackend();
    await backend.projectCreate("P1");
    const { user } = renderApp(backend);

    const header = await screen.findByRole("banner");
    expect(within(header).getByText("NekoReq")).toBeInTheDocument();
    // 環境がまだないので選べない
    expect(within(header).getByLabelText("環境")).toBeDisabled();
    expect(within(header).getByRole("button", { name: "環境変数を編集" })).toBeEnabled();
    // 未実装の操作は押せず、どのマイルストーンで入るかが分かる
    const importButton = within(header).getByRole("button", { name: /OpenAPI取り込み/ });
    expect(importButton).toBeDisabled();
    expect(importButton).toHaveTextContent("M3 で実装");
    expect(within(header).getByRole("button", { name: /設定/ })).toBeDisabled();
    // エクスポートは v1 では出さない
    expect(screen.queryByText("エクスポート")).not.toBeInTheDocument();

    const sidebar = screen.getByRole("navigation", { name: "API ツリー" });
    expect(within(sidebar).getByText("環境変数")).toBeInTheDocument();
    expect(within(sidebar).getByText("履歴")).toBeInTheDocument();
    expect(within(sidebar).getByText("その他")).toBeInTheDocument();

    // まだ何も開いていない
    expect(screen.getByRole("tablist", { name: "開いているリクエスト" })).toBeInTheDocument();
    expect(screen.getByText("開いているリクエストはありません")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "リクエスト" })).not.toBeInTheDocument();
    expect(await screen.findByText(/保存先:/)).toBeInTheDocument();
    expect(screen.getByText("OpenAPI は未取り込み")).toBeInTheDocument();

    await user.click(within(sidebar).getByRole("button", { name: "新規作成" }));
    const dialog = await screen.findByRole("dialog", { name: "新規作成" });
    // OpenAPI・cURL はまだ使えない。フォルダのカードは v1 では出さない
    expect(within(dialog).getByRole("button", { name: /OpenAPI から/ })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: "cURL から作成" })).toBeDisabled();
    expect(within(dialog).queryByText("フォルダ")).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /HTTP リクエスト/ }));

    expect(await screen.findByRole("region", { name: "リクエスト" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "レスポンス" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /新しいリクエスト/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

describe("プロジェクトの切り替え", () => {
  it("ヘッダーのメニューから新しいプロジェクトを作ると切り替わり、元のプロジェクトにも戻せる", async () => {
    const backend = createMockBackend();
    await backend.projectCreate("最初のプロジェクト");
    const { user } = renderApp(backend);

    await user.click(await screen.findByRole("button", { name: /プロジェクトを切り替え/ }));
    await user.click(await screen.findByRole("menuitem", { name: /新しいプロジェクトを作成/ }));

    const dialog = await screen.findByRole("dialog", { name: "新しいプロジェクト" });
    await user.type(within(dialog).getByLabelText("プロジェクト名"), "二つ目");
    await user.click(within(dialog).getByRole("button", { name: "プロジェクトを作成" }));

    // ダイアログが閉じて、ヘッダーが二つ目に切り替わる
    const switcher = await screen.findByRole("button", { name: "プロジェクトを切り替え（現在: 二つ目）" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    await user.click(switcher);
    const items = await screen.findAllByRole("menuitemradio");
    expect(items.map((i) => i.textContent)).toEqual(["最初のプロジェクト", "二つ目"]);
    await user.click(items[0]);

    expect(
      await screen.findByRole("button", { name: "プロジェクトを切り替え（現在: 最初のプロジェクト）" }),
    ).toBeInTheDocument();
  });
});

describe("読み込みの失敗", () => {
  it("プロジェクトを読めないときはエラーと再読み込みを出す", async () => {
    const base = createMockBackend();
    let fail = true;
    const backend: Backend = {
      ...base,
      projectList: async () => {
        if (fail) throw new Error("データベースを開けません");
        return base.projectList();
      },
    };
    const { user } = renderApp(backend);
    expect(await screen.findByRole("alert")).toHaveTextContent("データベースを開けません");

    fail = false;
    await user.click(screen.getByRole("button", { name: "再読み込み" }));
    expect(await screen.findByRole("heading", { name: "プロジェクトを作成" })).toBeInTheDocument();
  });
});
