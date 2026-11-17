import type { Backend } from "../api/backend";
import { errorMessage } from "../api";
import type { RequestFinished, RequestInput, SendOutcome } from "../bindings";
import { DEFAULT_SEND_SETTINGS } from "../lib/spec";
import { isDirty, useTabsStore } from "./tabs";

/**
 * 送信の流れ。
 *
 * `request://finished` は `request_send` の戻り値より先に届くことがあるので、
 * 送る前に購読しておき、届いた結果は送信 ID で突き合わせる。送信 ID がまだ分からないうちに
 * 届いたものは `early` に取っておく。
 */
const listening = new WeakMap<Backend, Promise<() => void>>();
const early = new Map<string, SendOutcome>();
/** 閉じたタブの結果が溜まり続けないように、取っておく数は少なくする。 */
const MAX_EARLY = 4;

function finish(tabId: string, outcome: SendOutcome) {
  useTabsStore.getState().setSend(tabId, (s) => ({ ...s, phase: "finished", outcome, cancelRequested: false }));
}

function onFinished(event: RequestFinished) {
  const tab = useTabsStore.getState().tabs.find((t) => t.send.sendId === event.sendId);
  if (tab) {
    finish(tab.id, event.result);
    return;
  }
  early.set(event.sendId, event.result);
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
  tabs.setSend(tabId, () => ({ phase: "sending", sendId: null, cancelRequested: false, outcome: null, envName }));
  try {
    await ensureListening(backend);
  } catch (e) {
    finish(tabId, failure(errorMessage(e)));
    return;
  }

  let sendId: string;
  try {
    sendId = await backend.requestSend(input, DEFAULT_SEND_SETTINGS);
  } catch (e) {
    if (useTabsStore.getState().tabs.some((t) => t.id === tabId)) finish(tabId, failure(errorMessage(e)));
    return;
  }

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
    finish(tabId, arrived);
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
