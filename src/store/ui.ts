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

export const SIDEBAR_DEFAULT_WIDTH = 280;
export const SIDEBAR_MIN_WIDTH = 200;
export const SIDEBAR_MAX_WIDTH = 600;
/** サイドバーを広げても、右側（タブ・リクエスト欄）に残す幅。 */
export const SIDEBAR_RESERVED_WIDTH = 400;

/** サイドバーの幅の上限。最大 600px、かつウィンドウ幅から右側の 400px を引いた幅まで（下限の 200px は割らない）。 */
export function sidebarMaxWidth(windowWidth: number): number {
  return Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, windowWidth - SIDEBAR_RESERVED_WIDTH));
}

/** サイドバーの幅を、最小〜（ウィンドウ幅に応じた）最大に収める。 */
export function clampSidebarWidth(width: number, windowWidth: number): number {
  const n = Number.isFinite(width) ? Math.round(width) : SIDEBAR_DEFAULT_WIDTH;
  return Math.min(sidebarMaxWidth(windowWidth), Math.max(SIDEBAR_MIN_WIDTH, n));
}

/** リクエストとレスポンスを横に並べたとき、レスポンスが占める割合の既定（半々）。 */
export const RESPONSE_DEFAULT_RATIO = 0.5;
/** リクエスト・レスポンスそれぞれに残す最小の幅（px）。 */
export const PANE_MIN_WIDTH = 280;

/** 並べた幅 `total`（px）のときの、片側の最小幅。狭くて 2 つぶん取れないときは 3 分の 1 まで縮める。 */
export function paneMinWidth(total: number): number {
  return Math.min(PANE_MIN_WIDTH, Math.floor(total / 3));
}

/** レスポンスの割合を、両側が最小幅を割らない範囲に収める。幅がまだ分からない（0）ときは 2〜8 割。 */
export function clampResponseRatio(ratio: number, total: number): number {
  const r = Number.isFinite(ratio) ? ratio : RESPONSE_DEFAULT_RATIO;
  const margin = total > 0 ? paneMinWidth(total) / total : 0.2;
  return Math.min(1 - margin, Math.max(margin, r));
}

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
  /** サイドバーの幅（px。アプリ全体で 1 つ。再起動しても覚える）。 */
  sidebarWidth: number;
  setSidebarWidth: (width: number) => void;
  /** リクエストとレスポンスを並べたときの、レスポンスの割合（0〜1。アプリ全体で 1 つ。再起動しても覚える）。 */
  responseRatio: number;
  setResponseRatio: (ratio: number) => void;
  /** 閉じている間に結果が届いたタブ（開くか、そのタブのレスポンス欄が見えたら外す）。覚えない。 */
  unseenResponses: Record<string, true | undefined>;
  markResponseUnseen: (tabId: string) => void;
  clearResponseUnseen: (tabId: string) => void;
  /** 開いたばかりの空のタブで、URL 欄にフォーカスを入れる（そのタブの画面が出たら使う）。覚えない。 */
  focusUrlTabId: string | null;
  setFocusUrlTabId: (tabId: string | null) => void;
  /** サイドバーの検索欄の入力（タブの右クリックメニューが「ツリーで表示」を出せるか調べるために共有する）。覚えない。 */
  treeQuery: string;
  setTreeQuery: (query: string) => void;
  /** 「ツリーで表示」の頼み。`nonce` が変わるたびに、サイドバーが行を含むフォルダを開いて行までスクロールする。覚えない。 */
  revealRequest: { key: string; nonce: number } | null;
  requestReveal: (key: string) => void;
  /** cURL をコピーした結果の知らせ（そのタブの画面の上に出す。画面を離れたら消える）。覚えない。 */
  curlNotice: { tabId: string; ok: boolean; text: string } | null;
  setCurlNotice: (notice: { tabId: string; ok: boolean; text: string } | null) => void;
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
      sidebarWidth: SIDEBAR_DEFAULT_WIDTH,
      setSidebarWidth: (width) => set({ sidebarWidth: Math.round(width) }),
      responseRatio: RESPONSE_DEFAULT_RATIO,
      setResponseRatio: (ratio) => set({ responseRatio: Math.round(ratio * 10000) / 10000 }),
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
      treeQuery: "",
      setTreeQuery: (query) => set({ treeQuery: query }),
      revealRequest: null,
      requestReveal: (key) => set((s) => ({ revealRequest: { key, nonce: (s.revealRequest?.nonce ?? 0) + 1 } })),
      curlNotice: null,
      setCurlNotice: (notice) => set({ curlNotice: notice }),
    }),
    {
      name: "nekoreq-ui",
      storage: createJSONStorage(() => safeStorage),
      // 覚えておくのは、開いていたプロジェクトとレスポンス欄の開閉、サイドバーの幅、レスポンスの幅の割合
      partialize: (state) => ({
        currentProjectId: state.currentProjectId,
        responseOpen: state.responseOpen,
        sidebarWidth: state.sidebarWidth,
        responseRatio: state.responseRatio,
      }),
    },
  ),
);
