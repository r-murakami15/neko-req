import { create } from "zustand";

import type { AdHocRequest, AuthOverride, AutoUpdate, AutoUpdateSkip, Body, DefaultAuth, OpKey, OperationOpen, ParamRef, RequestDraft, RequestSpec, SendOutcome } from "../bindings";
import { opKeyString } from "../lib/opkey";
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
  /** 受け取った Set-Cookie で自動更新した変数（`session`・`xsrfToken`）。 */
  autoUpdates: AutoUpdate[];
  /** 更新できなかった変数と、その理由。 */
  autoUpdateSkipped: AutoUpdateSkip[];
}

export const IDLE_SEND: SendState = {
  phase: "idle",
  sendId: null,
  cancelRequested: false,
  outcome: null,
  envName: null,
  autoUpdates: [],
  autoUpdateSkipped: [],
};

/** 開いたとき入力から外した値の案内（履歴から開いた・前回のタブを復元した）。 */
export interface DroppedNote {
  source: "history" | "session";
  fields: string[];
}

/** yaml の API（OpenAPI の Operation）として開いたタブの、yaml 側の情報。 */
export interface OpTabInfo {
  opKey: OpKey;
  method: string;
  path: string;
  summary: string | null;
  /** yaml から消えたパラメータ（「yaml にない」と表示して、送信から外す）。 */
  missingInYaml: ParamRef[];
  /** yaml の必須パラメータ（空なら警告）。 */
  requiredParams: ParamRef[];
  /** yaml の security から決まる既定の認証（表示用）。 */
  defaultAuth: DefaultAuth | null;
  /** yaml から消えた API（「削除された API」）。 */
  deleted: boolean;
  bodyContentType: string | null;
}

/** 開いているリクエストのタブ。入力は保存するまでここに持つ。 */
export interface RequestTab {
  id: string;
  projectId: string;
  /** 保存済みの AdHocRequest の ID。新しいリクエストは `null`。 */
  adhocId: string | null;
  /** yaml の API のタブなら、その情報（保存は RequestDraft として、ダイアログなしで行う）。 */
  op: OpTabInfo | null;
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
  /** 外した値があるときの案内（閉じるまで出す）。 */
  droppedNote: DroppedNote | null;
}

/** タブを作るための材料（履歴・cURL・前回の復元から開くとき）。 */
export interface TabSeed {
  name: string;
  /** いま入っている入力。 */
  spec: RequestSpec;
  /** 保存済みの内容（これと違えば「未保存」）。 */
  baseName?: string;
  baseSpec?: RequestSpec;
  adhocId?: string | null;
  description?: string;
  op?: OpTabInfo | null;
  requestPane?: RequestPane;
  responsePane?: ResponsePane;
  bodyMode?: BodyMode | null;
  droppedNote?: DroppedNote | null;
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

/** 開いたとき（`operation_open`）の戻り値から、タブの yaml 側の情報を作る。 */
export function opInfoOf(opened: OperationOpen): OpTabInfo {
  return {
    opKey: opened.opKey,
    method: opened.method,
    path: opened.path,
    summary: opened.summary,
    missingInYaml: opened.missingInYaml,
    requiredParams: opened.requiredParams,
    defaultAuth: opened.defaultAuth,
    deleted: opened.deleted,
    bodyContentType: opened.bodyContentType,
  };
}

/** yaml の API のタブの表示名。 */
export const opTabName = (opened: OperationOpen) => opened.summary ?? `${opened.method} ${opened.path}`;

export const isDirty = (tab: RequestTab) => serializeTab(tab.name, tab.spec) !== tab.baseline;

let counter = 0;
const nextTabId = () => `tab-${(counter += 1)}`;

function makeTab(
  projectId: string,
  name: string,
  spec: RequestSpec,
  adhocId: string | null,
  description = "",
  op: OpTabInfo | null = null,
): RequestTab {
  return {
    id: nextTabId(),
    projectId,
    adhocId,
    op,
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
    droppedNote: null,
  };
}

function tabFromSeed(projectId: string, seed: TabSeed): RequestTab {
  const base = makeTab(projectId, seed.name, seed.spec, seed.adhocId ?? null, seed.description ?? "", seed.op ?? null);
  return {
    ...base,
    baseline: serializeTab(seed.baseName ?? seed.name, seed.baseSpec ?? seed.spec),
    requestPane: seed.requestPane ?? base.requestPane,
    responsePane: seed.responsePane ?? base.responsePane,
    bodyMode: seed.bodyMode === undefined ? base.bodyMode : seed.bodyMode,
    droppedNote: seed.droppedNote ?? null,
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
  /** 「まとめて閉じる」で、未保存のタブがあるので確認中のタブ（閉じる対象の全部。確認が要らなければ `null`）。 */
  closeManyConfirm: string[] | null;
  /** 前回のタブの復元が済んだプロジェクト（済むまでは、タブの状態を保存しない）。 */
  restored: Record<string, boolean>;

  openNew: (projectId: string) => string;
  /** 入力が入った状態で新しいタブを開く（履歴の「編集して開く」・cURL から作成）。必ず新しいタブにする。 */
  openSeed: (projectId: string, seed: TabSeed) => string;
  /** プロジェクトのタブを `seeds` に置き換える（前回の復元）。`activeIndex` のタブを前に出す。 */
  restoreProject: (projectId: string, seeds: TabSeed[], activeIndex: number | null) => string[];
  markRestored: (projectId: string) => void;
  dismissDropped: (tabId: string) => void;
  openAdhoc: (adhoc: AdHocRequest) => string;
  /** yaml の API を開く。同じ API のタブが開いていれば、そのタブを前に出す。 */
  openOp: (projectId: string, opened: OperationOpen) => string;
  /** 開いている API のタブを探す。 */
  findOpTab: (projectId: string, opKey: OpKey) => RequestTab | undefined;
  /** 同期のあとなどに、タブの yaml 側の情報だけを更新する（入力した内容は変えない）。 */
  setOpInfo: (tabId: string, op: OpTabInfo) => void;
  markDraftSaved: (tabId: string, saved: RequestDraft, base: { name: string; spec: RequestSpec }) => void;
  /**
   * タブを複製する: 同じ入力の「未保存の新しいタブ」（`adhocId` は `null`・名前は「<名前> のコピー」）を、元の右隣に開いて前面にする。
   * yaml の API のタブは複製しない（`null` を返す。理由は docs/02-implementation-notes.md）。
   */
  duplicate: (tabId: string) => string | null;
  activate: (projectId: string, tabId: string) => void;
  close: (tabId: string) => void;
  patch: (tabId: string, patch: TabPatch) => void;
  updateSpec: (tabId: string, update: (spec: RequestSpec) => RequestSpec) => void;
  markSaved: (tabId: string, saved: AdHocRequest) => void;
  setSaveDialogTab: (tabId: string | null) => void;
  setCloseConfirmTab: (tabId: string | null) => void;
  setCloseManyConfirm: (tabIds: string[] | null) => void;
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
  closeManyConfirm: null,
  restored: {},

  openSeed: (projectId, seed) => {
    const tab = tabFromSeed(projectId, seed);
    set((s) => ({ tabs: [...s.tabs, tab], activeByProject: { ...s.activeByProject, [projectId]: tab.id } }));
    return tab.id;
  },

  restoreProject: (projectId, seeds, activeIndex) => {
    const made = seeds.map((seed) => tabFromSeed(projectId, seed));
    set((s) => ({
      tabs: [...s.tabs.filter((t) => t.projectId !== projectId), ...made],
      activeByProject: { ...s.activeByProject, [projectId]: activeIndex === null ? made[0]?.id : made[activeIndex]?.id },
      restored: { ...s.restored, [projectId]: true },
    }));
    return made.map((t) => t.id);
  },

  markRestored: (projectId) => set((s) => ({ restored: { ...s.restored, [projectId]: true } })),

  dismissDropped: (tabId) => set((s) => ({ tabs: mapTab(s.tabs, tabId, (t) => ({ ...t, droppedNote: null })) })),

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

  openOp: (projectId, opened) => {
    const existing = get().tabs.find((t) => t.projectId === projectId && t.op && opKeyString(t.op.opKey) === opKeyString(opened.opKey));
    if (existing) {
      set((s) => ({ activeByProject: { ...s.activeByProject, [projectId]: existing.id } }));
      return existing.id;
    }
    const tab = makeTab(projectId, opTabName(opened), opened.request, null, "", opInfoOf(opened));
    set((s) => ({ tabs: [...s.tabs, tab], activeByProject: { ...s.activeByProject, [projectId]: tab.id } }));
    return tab.id;
  },

  findOpTab: (projectId, opKey) =>
    get().tabs.find((t) => t.projectId === projectId && t.op && opKeyString(t.op.opKey) === opKeyString(opKey)),

  setOpInfo: (tabId, op) => set((s) => ({ tabs: mapTab(s.tabs, tabId, (t) => ({ ...t, op })) })),

  // 保存している間にも入力は続けられるので、タブの内容はそのままにして「保存した時点の内容」だけ更新する
  markDraftSaved: (tabId, saved, base) =>
    set((s) => ({
      tabs: mapTab(s.tabs, tabId, (t) => ({
        ...t,
        baseline: serializeTab(base.name, { ...base.spec, params: saved.params, body: saved.body, auth: saved.auth }),
        // 保存すると、画面で消した「yaml にない」パラメータの印も外れる
        op: t.op ? { ...t.op, missingInYaml: t.op.missingInYaml.filter((m) => saved.params.some((p) => p.location === m.location && p.name === m.name)) } : t.op,
      })),
    })),

  duplicate: (tabId) => {
    const src = get().tabs.find((t) => t.id === tabId);
    if (!src || src.op) return null;
    // 保存していない新しいタブ: 保存した時点の内容は空のリクエストにして、閉じるときは確認が出るようにする
    const copy = tabFromSeed(src.projectId, {
      name: `${src.name} のコピー`,
      spec: JSON.parse(JSON.stringify(src.spec)) as RequestSpec,
      baseName: NEW_REQUEST_NAME,
      baseSpec: emptySpec(),
      requestPane: src.requestPane,
      responsePane: src.responsePane,
      bodyMode: src.bodyMode,
    });
    copy.bodyStash = { ...src.bodyStash };
    copy.authStash = { ...src.authStash };
    set((s) => {
      const at = s.tabs.findIndex((t) => t.id === tabId);
      const tabs = [...s.tabs];
      tabs.splice(at + 1, 0, copy);
      return { tabs, activeByProject: { ...s.activeByProject, [src.projectId]: copy.id } };
    });
    return copy.id;
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
  setCloseManyConfirm: (tabIds) => set({ closeManyConfirm: tabIds }),

  setSend: (tabId, update) => set((s) => ({ tabs: mapTab(s.tabs, tabId, (t) => ({ ...t, send: update(t.send) })) })),

  reset: () => set({ tabs: [], activeByProject: {}, saveDialogTabId: null, closeConfirmTabId: null, closeManyConfirm: null, restored: {} }),
}));

/** プロジェクトの前面のタブ（なければ先頭）。 */
export function activeTabOf(tabs: RequestTab[], activeByProject: Record<string, string | undefined>, projectId: string) {
  const mine = tabs.filter((t) => t.projectId === projectId);
  return mine.find((t) => t.id === activeByProject[projectId]) ?? mine[0];
}

export type { SendOutcome };
