import { screen, within } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";

import type { Backend } from "../api/backend";
import type { ImportOptions, Project } from "../bindings";
import { createMockBackend, type MockBackend } from "../mock/backend";
import { renderApp } from "./render";

export const OPTIONS: ImportOptions = { tagFolders: true, fillFromExamples: true, applyCookieAuth: true, descriptionAsDoc: true };

/** yaml（モックの小さな仕様）を取り込み済みのプロジェクトを作って、アプリを描画する。 */
export async function setupWithApi(
  backend: MockBackend = createMockBackend([], { sendDelayMs: 10 }),
  render: Backend = backend,
  /** 描画の前に、モックに入力値を入れたり yaml を更新したりする。 */
  prepare?: (project: Project, backend: MockBackend) => Promise<void>,
) {
  const r = await backend.openapiImport({
    source: { kind: "path", path: "/mock/openapi.yaml" },
    projectId: null,
    projectName: "ミニ契約管理 API",
    selected: null,
    options: OPTIONS,
  });
  if (r.status !== "imported") throw new Error("取り込めませんでした");
  await prepare?.(r.project, backend);
  const view = renderApp(render);
  await screen.findByRole("banner");
  return { ...view, mock: backend, project: r.project };
}

export const sidebar = () => screen.getByRole("navigation", { name: "API ツリー" });

/** タグのフォルダを開く（開いていれば何もしない）。 */
export async function openFolder(user: UserEvent, name: string) {
  const folder = await within(sidebar()).findByRole("button", { name: new RegExp(`^${name}\\s`) });
  if (folder.getAttribute("aria-expanded") !== "true") await user.click(folder);
}

/** ツリーから API を開く。 */
export async function openOp(user: UserEvent, folder: string, name: RegExp | string) {
  await openFolder(user, folder);
  await user.click(await within(sidebar()).findByRole("button", { name }));
}

export const requestRegion = () => screen.getByRole("region", { name: "リクエスト" });

/** ダイアログのフッターの「閉じる」（ヘッダーの × も「閉じる」という名前なので、フッターのほうを押す）。 */
export function footerButton(dialog: HTMLElement, name: string | RegExp) {
  const footer = dialog.querySelector("footer");
  if (!footer) throw new Error("フッターがありません");
  return within(footer as HTMLElement).getByRole("button", { name });
}
