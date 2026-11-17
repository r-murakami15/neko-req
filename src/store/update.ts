import { create } from "zustand";

import type { Backend } from "../api/backend";
import { errorMessage } from "../api";
import type { UpdateInfo, UpdateProgress } from "../bindings";

/** 自動更新の状態。勝手には入れ替えず、ユーザーが選んだときだけ `install` する。 */
export type UpdatePhase = "idle" | "downloading" | "installing" | "installed" | "restarting" | "error";

interface UpdateState {
  /** 見つかっている新しい版。 */
  info: UpdateInfo | null;
  phase: UpdatePhase;
  progress: UpdateProgress | null;
  error: string | null;
  /** 「あとで」を押した（バナーを隠す。設定画面からはまた入れ替えられる）。 */
  dismissed: boolean;

  found: (info: UpdateInfo) => void;
  progressed: (progress: UpdateProgress) => void;
  dismiss: () => void;
  install: (backend: Backend) => Promise<void>;
  restart: (backend: Backend) => Promise<void>;
  reset: () => void;
}

export const useUpdateStore = create<UpdateState>()((set, get) => ({
  info: null,
  phase: "idle",
  progress: null,
  error: null,
  dismissed: false,

  found: (info) =>
    set((s) => {
      // 入れ替え中・済みのときは、同じ知らせで状態を戻さない
      if (s.phase === "downloading" || s.phase === "installing" || s.phase === "installed" || s.phase === "restarting") return s;
      return { info, phase: "idle", error: null, dismissed: s.info?.version === info.version ? s.dismissed : false };
    }),

  progressed: (progress) =>
    set((s) => {
      if (s.phase === "restarting" || s.phase === "error") return s;
      return { progress, phase: progress.phase };
    }),

  dismiss: () => set({ dismissed: true }),

  install: async (backend) => {
    const { phase } = get();
    if (phase === "downloading" || phase === "installing" || phase === "installed" || phase === "restarting") return;
    set({ phase: "downloading", error: null, progress: { phase: "downloading", downloaded: 0, total: null } });
    try {
      await backend.updateInstall();
      set((s) => (s.phase === "restarting" ? s : { phase: "installed" }));
    } catch (e) {
      set({ phase: "error", error: errorMessage(e) });
    }
  },

  restart: async (backend) => {
    set({ phase: "restarting" });
    try {
      await backend.updateRestart();
    } catch (e) {
      set({ phase: "error", error: errorMessage(e) });
    }
  },

  reset: () => set({ info: null, phase: "idle", progress: null, error: null, dismissed: false }),
}));
