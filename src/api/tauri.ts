import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";

import { commands, events } from "../bindings";
import { type Backend, unwrap } from "./backend";

/** Tauri のコマンド（Rust）を呼ぶ実装。型とコマンドは tauri-specta が生成した bindings.ts に従う。 */
export function createTauriBackend(): Backend {
  return {
    appInfo: () => unwrap(commands.appInfo()),
    projectList: () => unwrap(commands.projectList()),
    projectCreate: (name) => unwrap(commands.projectCreate(name)),
    projectRename: (id, name) => unwrap(commands.projectRename(id, name)),
    projectDelete: async (id) => {
      await unwrap(commands.projectDelete(id));
    },

    requestSend: (input, settings) => unwrap(commands.requestSend(input, settings)),
    requestCancel: (sendId) => commands.requestCancel(sendId),
    requestPreview: (input) => unwrap(commands.requestPreview(input)),
    onRequestFinished: (handler) => events.requestFinished.listen((e) => handler(e.payload)),
    responseSaveBody: async (sendId, path) => {
      await unwrap(commands.responseSaveBody(sendId, path));
    },

    pickSavePath: (defaultName) => save({ defaultPath: defaultName }),
    pickFile: async () => {
      const picked = await open({ multiple: false, directory: false });
      return typeof picked === "string" ? picked : null;
    },

    envList: (projectId) => unwrap(commands.envList(projectId)),
    envSave: (environment) => unwrap(commands.envSave(environment)),
    envCreate: (projectId, name) => unwrap(commands.envCreate(projectId, name)),
    envDuplicate: (id) => unwrap(commands.envDuplicate(id)),
    envDelete: async (id) => {
      await unwrap(commands.envDelete(id));
    },
    envSelect: (projectId, environmentId) => unwrap(commands.envSelect(projectId, environmentId)),
    secretSet: async (projectId, environmentId, name, value) => {
      await unwrap(commands.secretSet(projectId, environmentId, name, value));
    },
    secretDelete: async (projectId, environmentId, name) => {
      await unwrap(commands.secretDelete(projectId, environmentId, name));
    },
    secretReveal: (projectId, environmentId, name) => unwrap(commands.secretReveal(projectId, environmentId, name)),

    adhocList: (projectId) => unwrap(commands.adhocList(projectId)),
    adhocSave: (request) => unwrap(commands.adhocSave(request)),
    adhocDelete: async (id) => {
      await unwrap(commands.adhocDelete(id));
    },

    openapiPreview: (source, projectId) => unwrap(commands.openapiPreview(source, projectId)),
    openapiImport: (request) => unwrap(commands.openapiImport(request)),
    openapiSyncCheck: (projectId, upload) => unwrap(commands.openapiSyncCheck(projectId, upload)),
    openapiSyncApply: (projectId, upload) => unwrap(commands.openapiSyncApply(projectId, upload)),
    openapiSourceChanged: (projectId) => unwrap(commands.openapiSourceChanged(projectId)),
    operationList: (projectId) => unwrap(commands.operationList(projectId)),
    operationGet: (projectId, opKey) => unwrap(commands.operationGet(projectId, opKey)),
    operationOpen: (projectId, opKey) => unwrap(commands.operationOpen(projectId, opKey)),
    operationForget: async (projectId, opKey) => {
      await unwrap(commands.operationForget(projectId, opKey));
    },
    sourceTokenSet: (projectId, token) => unwrap(commands.sourceTokenSet(projectId, token)),
    editorOpen: async (projectId, file, line) => {
      await unwrap(commands.editorOpen(projectId, file, line));
    },
    draftSave: (draft) => unwrap(commands.draftSave(draft)),
    draftGet: (projectId, opKey) => unwrap(commands.draftGet(projectId, opKey)),

    pickPaths: async (kind) => {
      const picked =
        kind === "folder"
          ? await open({ multiple: false, directory: true })
          : kind === "openapi-file"
            ? await open({ multiple: false, directory: false, filters: [{ name: "OpenAPI（yaml / json）", extensions: ["yaml", "yml", "json"] }] })
            : await open({ multiple: true, directory: false, filters: [{ name: "OpenAPI・zip", extensions: ["yaml", "yml", "json", "zip"] }] });
      if (picked === null) return [];
      return Array.isArray(picked) ? picked : [picked];
    },
    onFileDrop: (handler) =>
      getCurrentWebview().onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "drop") handler({ type: "drop", paths: p.paths });
        else if (p.type === "leave") handler({ type: "leave" });
        else handler({ type: "over" });
      }),
    openExternal: (url) => openUrl(url),
  };
}
