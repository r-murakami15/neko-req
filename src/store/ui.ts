import { create } from "zustand";
import { createJSONStorage, persist, type StateStorage } from "zustand/middleware";

/** localStorage が使えない環境（プライベートウィンドウなど）でも落ちないようにする。 */
const safeStorage: StateStorage = {
  getItem: (key) => {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  setItem: (key, value) => {
    try {
      localStorage.setItem(key, value);
    } catch {
      /* 保存できなくても動く */
    }
  },
  removeItem: (key) => {
    try {
      localStorage.removeItem(key);
    } catch {
      /* 同上 */
    }
  },
};

export type Screen = "main" | "environments" | "history";

interface UiState {
  /** いま開いているプロジェクト。存在しない ID のときは画面側で先頭のプロジェクトに読み替える。 */
  currentProjectId: string | null;
  setCurrentProjectId: (id: string | null) => void;
  /** 「新しいプロジェクト」のダイアログ。 */
  createDialogOpen: boolean;
  setCreateDialogOpen: (open: boolean) => void;
  /** メインの画面か、環境変数の画面か、履歴の画面か。 */
  screen: Screen;
  setScreen: (screen: Screen) => void;
  /** 設定のダイアログ。 */
  settingsDialogOpen: boolean;
  setSettingsDialogOpen: (open: boolean) => void;
  /** 「新規作成」のダイアログ。 */
  newRequestDialogOpen: boolean;
  setNewRequestDialogOpen: (open: boolean) => void;
  /** 「OpenAPI 取り込み」のダイアログ。 */
  importDialogOpen: boolean;
  setImportDialogOpen: (open: boolean) => void;
  /** 同期の確認ダイアログ。 */
  syncDialogOpen: boolean;
  setSyncDialogOpen: (open: boolean) => void;
  /** レスポンス欄を開いているか（アプリ全体で 1 つ。再起動しても覚える）。 */
  responseOpen: boolean;
  setResponseOpen: (open: boolean) => void;
  /** 閉じている間に結果が届いたタブ（開くか、そのタブのレスポンス欄が見えたら外す）。覚えない。 */
  unseenResponses: Record<string, true | undefined>;
  markResponseUnseen: (tabId: string) => void;
  clearResponseUnseen: (tabId: string) => void;
  /** 開いたばかりの空のタブで、URL 欄にフォーカスを入れる（そのタブの画面が出たら使う）。覚えない。 */
  focusUrlTabId: string | null;
  setFocusUrlTabId: (tabId: string | null) => void;
}

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      currentProjectId: null,
      setCurrentProjectId: (id) => set({ currentProjectId: id }),
      createDialogOpen: false,
      setCreateDialogOpen: (open) => set({ createDialogOpen: open }),
      screen: "main",
      setScreen: (screen) => set({ screen }),
      settingsDialogOpen: false,
      setSettingsDialogOpen: (open) => set({ settingsDialogOpen: open }),
      newRequestDialogOpen: false,
      setNewRequestDialogOpen: (open) => set({ newRequestDialogOpen: open }),
      importDialogOpen: false,
      setImportDialogOpen: (open) => set({ importDialogOpen: open }),
      syncDialogOpen: false,
      setSyncDialogOpen: (open) => set({ syncDialogOpen: open }),
      responseOpen: true,
      setResponseOpen: (open) => set({ responseOpen: open }),
      unseenResponses: {},
      markResponseUnseen: (tabId) => set((s) => ({ unseenResponses: { ...s.unseenResponses, [tabId]: true } })),
      clearResponseUnseen: (tabId) =>
        set((s) => {
          if (!s.unseenResponses[tabId]) return s;
          const { [tabId]: _removed, ...rest } = s.unseenResponses;
          return { unseenResponses: rest };
        }),
      focusUrlTabId: null,
      setFocusUrlTabId: (tabId) => set({ focusUrlTabId: tabId }),
    }),
    {
      name: "nekoreq-ui",
      storage: createJSONStorage(() => safeStorage),
      // 覚えておくのは、開いていたプロジェクトとレスポンス欄の開閉
      partialize: (state) => ({ currentProjectId: state.currentProjectId, responseOpen: state.responseOpen }),
    },
  ),
);
