import type { Backend } from "../api/backend";
import { errorMessage } from "../api";
import type { RequestFinished, RequestInput, SendOutcome } from "../bindings";
import { IDLE_SEND, isDirty, useTabsStore } from "./tabs";

/**
 * 送信の流れ。
 *
 * `request://finished` は `request_send` の戻り値より先に届くことがあるので、
 * 送る前に購読しておき、届いた結果は送信 ID で突き合わせる。送信 ID がまだ分からないうちに
 * 届いたものは `early` に取っておく。
 */
const listening = new WeakMap<Backend, Promise<() => void>>();
const early = new Map<string, RequestFinished>();
/** 送信が終わるたびに呼ぶもの（環境の一覧・履歴の取り直しなど）。 */
const finishedHooks = new Set<(event: RequestFinished) => void>();

/** 送信が終わったときの処理を足す。外す関数を返す。 */
export function onSendFinished(hook: (event: RequestFinished) => void): () => void {
  finishedHooks.add(hook);
  return () => {
    finishedHooks.delete(hook);
  };
}
/** 閉じたタブの結果が溜まり続けないように、取っておく数は少なくする。 */
const MAX_EARLY = 4;

function finish(tabId: string, outcome: SendOutcome, event?: RequestFinished) {
  useTabsStore.getState().setSend(tabId, (s) => ({
    ...s,
    phase: "finished",
    outcome,
    cancelRequested: false,
    autoUpdates: event?.autoUpdates ?? [],
    autoUpdateSkipped: event?.autoUpdateSkipped ?? [],
  }));
}

function onFinished(event: RequestFinished) {
  for (const hook of [...finishedHooks]) hook(event);
  const tab = useTabsStore.getState().tabs.find((t) => t.send.sendId === event.sendId);
  if (tab) {
    finish(tab.id, event.result, event);
    return;
  }
  early.set(event.sendId, event);
  while (early.size > MAX_EARLY) early.delete(early.keys().next().value as string);
}

/** 結果のイベントの購読を（まだなら）始める。 */
export function ensureListening(backend: Backend): Promise<() => void> {
  let p = listening.get(backend);
  if (!p) {
    p = backend.onRequestFinished(onFinished);
    listening.set(backend, p);
  }
  return p;
}

const failure = (message: string): SendOutcome => ({ status: "failure", error: { kind: "other", message } });

export async function sendTab(backend: Backend, tabId: string, input: RequestInput, envName: string | null) {
  const tabs = useTabsStore.getState();
  tabs.setSend(tabId, () => ({ ...IDLE_SEND, phase: "sending", envName }));
  try {
    await ensureListening(backend);
  } catch (e) {
    finish(tabId, failure(errorMessage(e)));
    return;
  }

  let sendId: string;
  try {
    // 設定画面で保存した値（タイムアウト・リダイレクト・証明書の検証）を使わせる
    sendId = await backend.requestSend(input, null);
  } catch (e) {
    if (useTabsStore.getState().tabs.some((t) => t.id === tabId)) finish(tabId, failure(errorMessage(e)));
    return;
  }
  await attachSend(backend, tabId, sendId);
}

/** 送信 ID が分かったあと、タブに結びつける（先に届いた結果があれば、すぐ反映する）。 */
async function attachSend(backend: Backend, tabId: string, sendId: string) {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId);
  if (!tab) {
    // 送っている間にタブが閉じられた
    void backend.requestCancel(sendId);
    return;
  }
  useTabsStore.getState().setSend(tabId, (s) => ({ ...s, sendId }));
  const arrived = early.get(sendId);
  if (arrived) {
    early.delete(sendId);
    finish(tabId, arrived.result, arrived);
  } else if (tab.send.cancelRequested) {
    await backend.requestCancel(sendId);
  }
}

/** 送信中のリクエストを止める。止めると `cancelled` の失敗が届く。 */
export async function cancelTab(backend: Backend, tabId: string) {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId);
  if (!tab || tab.send.phase !== "sending") return;
  if (tab.send.sendId === null) {
    // まだ送信 ID が返っていない。返ったら止める
    useTabsStore.getState().setSend(tabId, (s) => ({ ...s, cancelRequested: true }));
    return;
  }
  await backend.requestCancel(tab.send.sendId);
}

/** タブを閉じるとき、送信中なら止める。 */
export function cancelIfSending(backend: Backend, tabId: string) {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId);
  if (tab?.send.phase === "sending" && tab.send.sendId) void backend.requestCancel(tab.send.sendId);
}

/**
 * タブを閉じる。未保存の変更があれば（`force` でなければ）確認ダイアログを出すだけにする。
 * 送信中なら止めてから閉じる。
 */
export function closeTab(backend: Backend, tabId: string, force = false) {
  const state = useTabsStore.getState();
  const tab = state.tabs.find((t) => t.id === tabId);
  if (!tab) return;
  if (!force && isDirty(tab)) {
    state.setCloseConfirmTab(tabId);
    return;
  }
  cancelIfSending(backend, tabId);
  state.close(tabId);
}

/**
 * 履歴の「再送信」: 元の入力をタブとして開き、そのタブで送信の結果を受け取る。
 * 送る前にタブを作って購読しておくので、結果が先に届いても取りこぼさない。
 */
export async function resendInTab(backend: Backend, tabId: string, envName: string | null, start: () => Promise<string>) {
  useTabsStore.getState().setSend(tabId, () => ({ ...IDLE_SEND, phase: "sending", envName }));
  try {
    await ensureListening(backend);
  } catch (e) {
    finish(tabId, failure(errorMessage(e)));
    return;
  }
  let sendId: string;
  try {
    sendId = await start();
  } catch (e) {
    if (useTabsStore.getState().tabs.some((t) => t.id === tabId)) finish(tabId, failure(errorMessage(e)));
    return;
  }
  await attachSend(backend, tabId, sendId);
}
