import type { Backend } from "../api/backend";
import type { HistoryOpen, OpKey } from "../bindings";
import { emptySpec, NEW_REQUEST_NAME, suggestName } from "../lib/spec";
import { opInfoOf, opTabName, useTabsStore } from "./tabs";

/**
 * yaml の API を開く。同じ API のタブが開いていればそのタブを前に出し、なければ `operation_open` で初期値を作って
 * 新しいタブで開く（入力値が保存されていればそれ、なければ example・default から）。
 */
export async function openOperation(backend: Backend, projectId: string, opKey: OpKey): Promise<void> {
  const tabs = useTabsStore.getState();
  const existing = tabs.findOpTab(projectId, opKey);
  if (existing) {
    tabs.activate(projectId, existing.id);
    return;
  }
  const opened = await backend.operationOpen(projectId, opKey);
  useTabsStore.getState().openOp(projectId, opened);
}

/**
 * 履歴の「編集して開く」: 送ったときの入力を、新しいタブに未保存の入力として入れる。
 * yaml の API ならその API のタブ（API が見つからなければ「その他」の新しいタブ）。
 * 伏せ字で残っていて外した値は、タブの上で知らせる。
 */
export async function openFromHistory(backend: Backend, projectId: string, h: HistoryOpen): Promise<string> {
  const droppedNote = h.droppedFields.length > 0 ? { source: "history" as const, fields: h.droppedFields } : null;
  const tabs = useTabsStore.getState();
  if (h.opKey) {
    try {
      const opened = await backend.operationOpen(projectId, h.opKey);
      return tabs.openSeed(projectId, { name: opTabName(opened), spec: h.request, baseSpec: opened.request, op: opInfoOf(opened), droppedNote });
    } catch {
      // API が見つからなければ、「その他」として開く
    }
  }
  const name = h.summary ?? (suggestName(h.request) || NEW_REQUEST_NAME);
  return tabs.openSeed(projectId, { name, spec: h.request, baseName: NEW_REQUEST_NAME, baseSpec: emptySpec(), droppedNote });
}
