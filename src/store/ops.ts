import type { Backend } from "../api/backend";
import type { OpKey } from "../bindings";
import { useTabsStore } from "./tabs";

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
