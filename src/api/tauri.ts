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
    openapiSyncCheck: (projectId, upload, allowedHosts) => unwrap(commands.openapiSyncCheck(projectId, upload, allowedHosts ?? null)),
    openapiSyncApply: (projectId, upload, allowedHosts) => unwrap(commands.openapiSyncApply(projectId, upload, allowedHosts ?? null)),
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

    projectSettingsSet: (projectId, settings) => unwrap(commands.projectSettingsSet(projectId, settings)),
    appSettingsGet: () => unwrap(commands.appSettingsGet()),
    appSettingsSet: (settings) => unwrap(commands.appSettingsSet(settings)),
    proxyPasswordSet: (password) => unwrap(commands.proxyPasswordSet(password)),
    clientCertPasswordSet: (id, password) => unwrap(commands.clientCertPasswordSet(id, password)),
    pickCertFile: async (kind) => {
      const picked = await open({
        multiple: false,
        directory: false,
        filters:
          kind === "ca"
            ? [{ name: "CA 証明書（PEM）", extensions: ["pem", "crt", "cer"] }]
            : [{ name: "クライアント証明書（PKCS#12・PEM）", extensions: ["p12", "pfx", "pem"] }],
      });
      return typeof picked === "string" ? picked : null;
    },

    historyList: (projectId, query) => unwrap(commands.historyList(projectId, query)),
    historyGet: (id) => unwrap(commands.historyGet(id)),
    historyDelete: async (id) => {
      await unwrap(commands.historyDelete(id));
    },
    historyClear: (projectId) => unwrap(commands.historyClear(projectId)),
    historyOpen: (id) => unwrap(commands.historyOpen(id)),
    historyResend: (id, settings) => unwrap(commands.historyResend(id, settings)),
    historySaveAsInput: (id, name) => unwrap(commands.historySaveAsInput(id, name)),
    historyCurl: (id) => unwrap(commands.historyCurl(id)),
    historyCompare: (leftId, rightId) => unwrap(commands.historyCompare(leftId, rightId)),
    curlParse: (projectId, text, environmentId) => unwrap(commands.curlParse(projectId, text, environmentId)),
    curlExport: (input, mode) => unwrap(commands.curlExport(input, mode)),
    sessionSave: (projectId, session) => unwrap(commands.sessionSave(projectId, session)),
    sessionLoad: (projectId) => unwrap(commands.sessionLoad(projectId)),

    updateCheck: () => unwrap(commands.updateCheck()),
    updatePending: () => commands.updatePending(),
    updateInstall: async () => {
      await unwrap(commands.updateInstall());
    },
    updateRestart: () => commands.updateRestart(),
    onUpdateAvailable: (handler) => events.updateAvailable.listen((e) => handler(e.payload.info)),
    onUpdateProgress: (handler) => events.updateProgress.listen((e) => handler(e.payload)),

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
