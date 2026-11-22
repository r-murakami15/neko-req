import { useBackend } from "../api";
import { useAppInfo } from "../api/queries";
import type { Project } from "../bindings";
import { closeTab, closeTabs } from "../store/send";
import { activeTabOf, isDirty, useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";
import { ConfirmDialog } from "./Dialog";
import { NewRequestDialog } from "./NewRequestDialog";
import { RequestEditor } from "./RequestEditor";
import { SaveRequestDialog } from "./SaveRequestDialog";
import { TabBar } from "./TabBar";

/** 中央の領域。開いているタブ・URL と送信・リクエスト／レスポンス。 */
export function Workspace({ project }: { project: Project }) {
  const backend = useBackend();
  const appInfo = useAppInfo();
  const allTabs = useTabsStore((s) => s.tabs);
  const activeByProject = useTabsStore((s) => s.activeByProject);
  const closeConfirmTabId = useTabsStore((s) => s.closeConfirmTabId);
  const setCloseConfirmTab = useTabsStore((s) => s.setCloseConfirmTab);
  const closeManyConfirm = useTabsStore((s) => s.closeManyConfirm);
  const setCloseManyConfirm = useTabsStore((s) => s.setCloseManyConfirm);
  const setNewRequestDialogOpen = useUiStore((s) => s.setNewRequestDialogOpen);

  const tabs = allTabs.filter((t) => t.projectId === project.id);
  const active = activeTabOf(allTabs, activeByProject, project.id);
  const closing = allTabs.find((t) => t.id === closeConfirmTabId);
  const dirtyCount = closeManyConfirm ? allTabs.filter((t) => closeManyConfirm.includes(t.id) && isDirty(t)).length : 0;

  return (
    <main className="workspace">
      <TabBar projectId={project.id} tabs={tabs} activeId={active?.id} />

      {active ? (
        <RequestEditor key={active.id} project={project} tab={active} />
      ) : (
        <div className="empty-workspace">
          <p>リクエストを開いていません。</p>
          <p className="muted-note">新しいリクエストを作るか、サイドバーの API ツリー（OpenAPI の API・「その他」）から開いてください。</p>
          <button type="button" className="btn btn-primary" onClick={() => setNewRequestDialogOpen(true)}>
            新しいリクエスト
          </button>
        </div>
      )}

      <footer className="statusbar">
        <span className="statusbar-right">{appInfo.data ? `${appInfo.data.name} v${appInfo.data.version}` : ""}</span>
      </footer>

      <NewRequestDialog projectId={project.id} />
      <SaveRequestDialog />
      <ConfirmDialog
        open={closing !== undefined}
        onOpenChange={(open) => !open && setCloseConfirmTab(null)}
        title="タブを閉じますか？"
        description={closing ? `「${closing.name}」には保存していない変更があります。閉じると変更は失われます。` : ""}
        confirmLabel="保存せずに閉じる"
        danger
        onConfirm={() => closing && closeTab(backend, closing.id, true)}
      />
      {/* タブをまとめて閉じるとき（未保存が 1 つ以上）の確認。1 回だけ出す */}
      <ConfirmDialog
        open={closeManyConfirm !== null}
        onOpenChange={(open) => !open && setCloseManyConfirm(null)}
        title="タブを閉じますか？"
        description={`保存していない変更が ${dirtyCount} 件あります。閉じると変更は失われます。`}
        confirmLabel="保存せずに閉じる"
        danger
        onConfirm={() => {
          const ids = closeManyConfirm;
          setCloseManyConfirm(null);
          if (ids) closeTabs(backend, ids, true);
        }}
      />
    </main>
  );
}
