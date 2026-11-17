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

interface UiState {
  /** いま開いているプロジェクト。存在しない ID のときは画面側で先頭のプロジェクトに読み替える。 */
  currentProjectId: string | null;
  setCurrentProjectId: (id: string | null) => void;
  /** 「新しいプロジェクト」のダイアログ。 */
  createDialogOpen: boolean;
  setCreateDialogOpen: (open: boolean) => void;
  /** メインの画面か、環境変数の画面か。 */
  screen: "main" | "environments";
  setScreen: (screen: "main" | "environments") => void;
  /** 「新規作成」のダイアログ。 */
  newRequestDialogOpen: boolean;
  setNewRequestDialogOpen: (open: boolean) => void;
  /** 「OpenAPI 取り込み」のダイアログ。 */
  importDialogOpen: boolean;
  setImportDialogOpen: (open: boolean) => void;
  /** 同期の確認ダイアログ。 */
  syncDialogOpen: boolean;
  setSyncDialogOpen: (open: boolean) => void;
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
      newRequestDialogOpen: false,
      setNewRequestDialogOpen: (open) => set({ newRequestDialogOpen: open }),
      importDialogOpen: false,
      setImportDialogOpen: (open) => set({ importDialogOpen: open }),
      syncDialogOpen: false,
      setSyncDialogOpen: (open) => set({ syncDialogOpen: open }),
    }),
    {
      name: "nekoreq-ui",
      storage: createJSONStorage(() => safeStorage),
      // 覚えておくのは開いていたプロジェクトだけ
      partialize: (state) => ({ currentProjectId: state.currentProjectId }),
    },
  ),
);
