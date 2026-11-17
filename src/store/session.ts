import { useEffect } from "react";

import type { Backend } from "../api/backend";
import { useBackend } from "../api";
import type { AdHocRequest, SessionState, SessionTab } from "../bindings";
import { emptySpec, NEW_REQUEST_NAME, specOfAdhoc } from "../lib/spec";
import { type BodyMode, isDirty, opInfoOf, opTabName, type RequestPane, type RequestTab, type ResponsePane, type TabSeed, useTabsStore } from "./tabs";

/** タブの変更を保存するまで待つ時間（ミリ秒）。 */
export const SESSION_SAVE_DEBOUNCE_MS = 400;

interface ViewState {
  requestPane?: RequestPane;
  responsePane?: ResponsePane;
  bodyMode?: BodyMode | null;
}

const REQUEST_PANES: RequestPane[] = ["params", "headers", "body", "auth", "design"];
const RESPONSE_PANES: ResponsePane[] = ["body", "headers", "timing"];

function parseView(text: string | null): ViewState {
  if (!text) return {};
  try {
    const v = JSON.parse(text) as ViewState;
    return {
      requestPane: REQUEST_PANES.includes(v.requestPane as RequestPane) ? v.requestPane : undefined,
      responsePane: RESPONSE_PANES.includes(v.responsePane as ResponsePane) ? v.responsePane : undefined,
      bodyMode: v.bodyMode === "json" || v.bodyMode === "raw" ? v.bodyMode : null,
    };
  } catch {
    return {};
  }
}

/** タブ 1 つを保存する形にする。未保存の入力（と「新しい」タブの入力）だけ中身を持つ。 */
export function sessionTabOf(tab: RequestTab): SessionTab {
  const dirty = isDirty(tab);
  const kind = tab.op ? "operation" : tab.adhocId ? "adhoc" : "new";
  const view: ViewState = { requestPane: tab.requestPane, responsePane: tab.responsePane, bodyMode: tab.bodyMode };
  return {
    id: tab.id,
    kind,
    opKey: tab.op?.opKey ?? null,
    adhocId: tab.adhocId,
    title: tab.name,
    request: kind === "new" || dirty ? tab.spec : null,
    dirty,
    viewState: JSON.stringify(view),
    // 復元時の「認証の値を入れ直して」の案内は、閉じるまで次の起動にも引き継ぐ
    droppedFields: tab.droppedNote?.source === "session" ? tab.droppedNote.fields : [],
  };
}

/** プロジェクトの開いているタブと前面のタブ。 */
export function sessionOfProject(projectId: string): SessionState {
  const { tabs, activeByProject } = useTabsStore.getState();
  const mine = tabs.filter((t) => t.projectId === projectId);
  const active = mine.find((t) => t.id === activeByProject[projectId]) ?? mine[0];
  return { tabs: mine.map(sessionTabOf), activeTabId: active?.id ?? null, savedAt: null };
}

async function seedOf(backend: Backend, projectId: string, t: SessionTab, adhocs: AdHocRequest[]): Promise<TabSeed | null> {
  const view = parseView(t.viewState);
  const dropped = t.droppedFields ?? [];
  const droppedNote = dropped.length > 0 ? { source: "session" as const, fields: dropped } : null;
  if (t.kind === "operation" && t.opKey) {
    try {
      const opened = await backend.operationOpen(projectId, t.opKey);
      return { name: opTabName(opened), spec: t.request ?? opened.request, baseSpec: opened.request, op: opInfoOf(opened), ...view, droppedNote };
    } catch {
      return null; // 消えた API のタブは戻さない
    }
  }
  if (t.kind === "adhoc") {
    const a = adhocs.find((x) => x.id === t.adhocId);
    if (!a) return null; // 消した「その他」のタブは戻さない
    return { name: a.name, spec: t.request ?? specOfAdhoc(a), baseSpec: specOfAdhoc(a), adhocId: a.id, description: a.description ?? "", ...view, droppedNote };
  }
  return { name: t.title || NEW_REQUEST_NAME, spec: t.request ?? emptySpec(), baseName: NEW_REQUEST_NAME, baseSpec: emptySpec(), ...view, droppedNote };
}

/** 前回のタブ・未保存の入力・選んでいたタブを戻す。戻したタブの数を返す。 */
export async function restoreSession(backend: Backend, projectId: string, isCancelled: () => boolean): Promise<number> {
  const settings = await backend.appSettingsGet();
  if (!settings.appearance.restoreTabs) return 0;
  const session = await backend.sessionLoad(projectId);
  if (!session || session.tabs.length === 0) return 0;
  const adhocs = session.tabs.some((t) => t.kind === "adhoc") ? await backend.adhocList(projectId) : [];
  const seeds: { seed: TabSeed; id: string }[] = [];
  for (const t of session.tabs) {
    const seed = await seedOf(backend, projectId, t, adhocs);
    if (seed) seeds.push({ seed, id: t.id });
  }
  if (isCancelled()) return 0;
  // 待っている間に、ユーザーがもうタブを開いていたら、そのままにする
  if (useTabsStore.getState().tabs.some((t) => t.projectId === projectId)) return 0;
  const activeIndex = seeds.findIndex((s) => s.id === session.activeTabId);
  useTabsStore.getState().restoreProject(
    projectId,
    seeds.map((s) => s.seed),
    activeIndex >= 0 ? activeIndex : null,
  );
  return seeds.length;
}

/**
 * プロジェクトを開いたとき（切り替えたときも）、前回のタブを戻す。済むまではタブの状態を保存しない
 * （空の状態で前回の保存を上書きしないため）。
 */
export function useSessionRestore(projectId: string) {
  const backend = useBackend();
  const restored = useTabsStore((s) => s.restored[projectId] === true);
  useEffect(() => {
    if (restored) return;
    let cancelled = false;
    void (async () => {
      try {
        await restoreSession(backend, projectId, () => cancelled);
      } catch {
        /* 読めなくても、空の状態で始める */
      }
      if (!cancelled) useTabsStore.getState().markRestored(projectId);
    })();
    return () => {
      cancelled = true;
    };
  }, [backend, projectId, restored]);
}

/** タブ・未保存の入力・前面のタブが変わるたびに（少し待って）保存する。 */
export function useSessionSave(projectId: string) {
  const backend = useBackend();
  const restored = useTabsStore((s) => s.restored[projectId] === true);
  useEffect(() => {
    if (!restored) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let last: string | null = null;
    const flush = () => {
      timer = undefined;
      const state = sessionOfProject(projectId);
      const json = JSON.stringify(state);
      if (json === last) return;
      last = json;
      backend.sessionSave(projectId, state).catch(() => {
        last = null; // 保存できなかったら、次の変更でもう一度
      });
    };
    const unsubscribe = useTabsStore.subscribe(() => {
      if (timer !== undefined) clearTimeout(timer);
      timer = setTimeout(flush, SESSION_SAVE_DEBOUNCE_MS);
    });
    return () => {
      unsubscribe();
      if (timer !== undefined) {
        clearTimeout(timer);
        flush();
      }
    };
  }, [backend, projectId, restored]);
}
