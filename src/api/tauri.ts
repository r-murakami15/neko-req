import { open, save } from "@tauri-apps/plugin-dialog";

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
  };
}
