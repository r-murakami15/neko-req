import { create } from "zustand";

import type { AdHocRequest, AuthOverride, Body, RequestSpec, SendOutcome } from "../bindings";
import { emptySpec, NEW_REQUEST_NAME, specOfAdhoc } from "../lib/spec";

export type RequestPane = "params" | "headers" | "body" | "auth" | "design";
export type ResponsePane = "body" | "headers" | "timing";
export type BodyMode = "json" | "raw";

/** 送信の状態。 */
export interface SendState {
  phase: "idle" | "sending" | "finished";
  /** `request_send` が返した送信 ID（返るまでは `null`）。 */
  sendId: string | null;
  /** 送信 ID が分かる前に「中止」が押された。 */
  cancelRequested: boolean;
  outcome: SendOutcome | null;
  /** 送ったときに使っていた環境の名前。 */
  envName: string | null;
}

export const IDLE_SEND: SendState = { phase: "idle", sendId: null, cancelRequested: false, outcome: null, envName: null };

/** 開いているリクエストのタブ。入力は保存するまでここに持つ。 */
export interface RequestTab {
  id: string;
  projectId: string;
  /** 保存済みの AdHocRequest の ID。新しいリクエストは `null`。 */
  adhocId: string | null;
  name: string;
  /** 保存済みの説明（「説明（任意）」）。上書き保存のときに引き継ぐ。 */
  description: string;
  spec: RequestSpec;
  /** 開いた（保存した）時点の内容。これと違えば「未保存」。 */
  baseline: string;
  /** 種類を切り替えても、前に入れた内容を取り戻せるように持っておく。 */
  bodyStash: Partial<Record<Body["kind"], Body>>;
  authStash: Partial<Record<AuthOverride["type"], AuthOverride>>;
  requestPane: RequestPane;
  responsePane: ResponsePane;
  /** `null` は自動（JSON として読めれば JSON）。 */
  bodyMode: BodyMode | null;
  send: SendState;
}

/** キーの順番に左右されない文字列にする（Rust が返す JSON と画面で作った JSON を比べるため）。 */
function stable(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (value !== null && typeof value === "object") {
    const o = value as Record<string, unknown>;
    return `{${Object.keys(o)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${stable(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}

export const serializeTab = (name: string, spec: RequestSpec) => stable([name, spec]);

export const isDirty = (tab: RequestTab) => serializeTab(tab.name, tab.spec) !== tab.baseline;

let counter = 0;
const nextTabId = () => `tab-${(counter += 1)}`;

function makeTab(projectId: string, name: string, spec: RequestSpec, adhocId: string | null, description = ""): RequestTab {
  return {
    id: nextTabId(),
    projectId,
    adhocId,
    name,
    description,
    spec,
    baseline: serializeTab(name, spec),
    bodyStash: {},
    authStash: {},
    requestPane: "params",
    responsePane: "body",
    bodyMode: null,
    send: IDLE_SEND,
  };
}

type TabPatch = Partial<Pick<RequestTab, "requestPane" | "responsePane" | "bodyMode" | "bodyStash" | "authStash">>;

interface TabsState {
  tabs: RequestTab[];
  /** プロジェクトごとに前面のタブ。 */
  activeByProject: Record<string, string | undefined>;
  /** 「保存」ダイアログを開いているタブ。 */
  saveDialogTabId: string | null;
  /** 「未保存のまま閉じる？」を確認中のタブ。 */
  closeConfirmTabId: string | null;

  openNew: (projectId: string) => string;
  openAdhoc: (adhoc: AdHocRequest) => string;
  activate: (projectId: string, tabId: string) => void;
  close: (tabId: string) => void;
  patch: (tabId: string, patch: TabPatch) => void;
  updateSpec: (tabId: string, update: (spec: RequestSpec) => RequestSpec) => void;
  markSaved: (tabId: string, saved: AdHocRequest) => void;
  setSaveDialogTab: (tabId: string | null) => void;
  setCloseConfirmTab: (tabId: string | null) => void;
  setSend: (tabId: string, update: (s: SendState) => SendState) => void;
  reset: () => void;
}

const mapTab = (tabs: RequestTab[], tabId: string, f: (t: RequestTab) => RequestTab) =>
  tabs.map((t) => (t.id === tabId ? f(t) : t));

export const useTabsStore = create<TabsState>()((set, get) => ({
  tabs: [],
  activeByProject: {},
  saveDialogTabId: null,
  closeConfirmTabId: null,

  openNew: (projectId) => {
    const tab = makeTab(projectId, NEW_REQUEST_NAME, emptySpec(), null);
    set((s) => ({ tabs: [...s.tabs, tab], activeByProject: { ...s.activeByProject, [projectId]: tab.id } }));
    return tab.id;
  },

  openAdhoc: (adhoc) => {
    const existing = get().tabs.find((t) => t.adhocId === adhoc.id);
    if (existing) {
      set((s) => ({ activeByProject: { ...s.activeByProject, [adhoc.projectId]: existing.id } }));
      return existing.id;
    }
    const tab = makeTab(adhoc.projectId, adhoc.name, specOfAdhoc(adhoc), adhoc.id, adhoc.description ?? "");
    set((s) => ({ tabs: [...s.tabs, tab], activeByProject: { ...s.activeByProject, [adhoc.projectId]: tab.id } }));
    return tab.id;
  },

  activate: (projectId, tabId) => set((s) => ({ activeByProject: { ...s.activeByProject, [projectId]: tabId } })),

  close: (tabId) =>
    set((s) => {
      const tab = s.tabs.find((t) => t.id === tabId);
      if (!tab) return s;
      const sameProject = s.tabs.filter((t) => t.projectId === tab.projectId);
      const index = sameProject.findIndex((t) => t.id === tabId);
      const activeByProject = { ...s.activeByProject };
      if (activeByProject[tab.projectId] === tabId) {
        // 閉じたタブの右隣（なければ左隣）を前に出す
        activeByProject[tab.projectId] = (sameProject[index + 1] ?? sameProject[index - 1])?.id;
      }
      return {
        tabs: s.tabs.filter((t) => t.id !== tabId),
        activeByProject,
        closeConfirmTabId: s.closeConfirmTabId === tabId ? null : s.closeConfirmTabId,
        saveDialogTabId: s.saveDialogTabId === tabId ? null : s.saveDialogTabId,
      };
    }),

  patch: (tabId, patch) => set((s) => ({ tabs: mapTab(s.tabs, tabId, (t) => ({ ...t, ...patch })) })),

  updateSpec: (tabId, update) => set((s) => ({ tabs: mapTab(s.tabs, tabId, (t) => ({ ...t, spec: update(t.spec) })) })),

  // 保存している間にも入力は続けられるので、タブの内容はそのままにして「保存した時点の内容」だけ更新する
  markSaved: (tabId, saved) =>
    set((s) => ({
      tabs: mapTab(s.tabs, tabId, (t) => ({
        ...t,
        adhocId: saved.id,
        name: saved.name,
        description: saved.description ?? "",
        baseline: serializeTab(saved.name, specOfAdhoc(saved)),
      })),
    })),

  setSaveDialogTab: (tabId) => set({ saveDialogTabId: tabId }),
  setCloseConfirmTab: (tabId) => set({ closeConfirmTabId: tabId }),

  setSend: (tabId, update) => set((s) => ({ tabs: mapTab(s.tabs, tabId, (t) => ({ ...t, send: update(t.send) })) })),

  reset: () => set({ tabs: [], activeByProject: {}, saveDialogTabId: null, closeConfirmTabId: null }),
}));

/** プロジェクトの前面のタブ（なければ先頭）。 */
export function activeTabOf(tabs: RequestTab[], activeByProject: Record<string, string | undefined>, projectId: string) {
  const mine = tabs.filter((t) => t.projectId === projectId);
  return mine.find((t) => t.id === activeByProject[projectId]) ?? mine[0];
}

export type { SendOutcome };
