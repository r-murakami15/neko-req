import { create } from "zustand";

/** 前面のリクエストの操作（ショートカットから呼ぶ）。RequestEditor が登録する。 */
export interface EditorActions {
  tabId: string;
  send: () => void;
  save: () => void;
}

interface ActionsState {
  editor: EditorActions | null;
  setEditor: (editor: EditorActions | null) => void;
}

export const useActionsStore = create<ActionsState>()((set) => ({
  editor: null,
  setEditor: (editor) => set({ editor }),
}));
