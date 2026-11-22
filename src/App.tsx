import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { errorMessage } from "./api";
import { useAppSettings, useProjects } from "./api/queries";
import type { Project } from "./bindings";
import { CreateProjectDialog } from "./components/CreateProjectDialog";
import { CreateProjectScreen } from "./components/CreateProjectScreen";
import { EnvironmentsScreen } from "./components/EnvironmentsScreen";
import { Header } from "./components/Header";
import { HistoryScreen } from "./components/HistoryScreen";
import { ImportDialog } from "./components/openapi/ImportDialog";
import { SyncDialog } from "./components/openapi/SyncDialog";
import { SettingsDialog } from "./components/settings/SettingsDialog";
import { Sidebar } from "./components/Sidebar";
import { SidebarResizer } from "./components/SidebarResizer";
import { UpdateBanner, useUpdateNotifications } from "./components/UpdateBanner";
import { Workspace } from "./components/Workspace";
import { applyAppearance } from "./lib/appearance";
import { useGlobalShortcuts } from "./lib/shortcuts";
import { onSendFinished } from "./store/send";
import { useSessionRestore, useSessionSave } from "./store/session";
import { useUiStore } from "./store/ui";

/** 設定の外観（テーマ・エディタの文字サイズ）を画面に反映する。 */
function useApplyAppearance() {
  const settings = useAppSettings();
  const appearance = settings.data?.appearance;
  useEffect(() => {
    if (appearance) applyAppearance(appearance);
  }, [appearance]);
}

/** 送信が終わるたびに、履歴と（認証の自動更新で変わった）環境の一覧を取り直す。 */
function useRefreshAfterSend() {
  const queryClient = useQueryClient();
  useEffect(
    () =>
      onSendFinished((event) => {
        void queryClient.invalidateQueries({ queryKey: ["history"] });
        if (event.autoUpdates.length > 0) void queryClient.invalidateQueries({ queryKey: ["environments"] });
      }),
    [queryClient],
  );
}

export default function App() {
  const projects = useProjects();
  const currentProjectId = useUiStore((s) => s.currentProjectId);
  useApplyAppearance();
  useUpdateNotifications();
  useRefreshAfterSend();

  if (projects.isPending) {
    return (
      <div className="center-message" role="status">
        読み込み中…
      </div>
    );
  }

  if (projects.isError) {
    return (
      <div className="center-message" role="alert">
        <p>プロジェクトを読み込めませんでした。</p>
        <p className="center-message-detail">{errorMessage(projects.error)}</p>
        <button type="button" className="btn" onClick={() => void projects.refetch()}>
          再読み込み
        </button>
      </div>
    );
  }

  // 取り込みダイアログは、プロジェクトを作る（画面が切り替わる）取り込みの最中も閉じないように、どの画面でも同じ位置に置く
  const empty = projects.data.length === 0;
  // 覚えていたプロジェクトがもうなければ、先頭を開く
  const current = projects.data.find((p) => p.id === currentProjectId) ?? projects.data[0];
  return (
    <>
      {empty ? <CreateProjectScreen /> : <ProjectShell projects={projects.data} current={current} />}
      <ImportDialog />
    </>
  );
}

/** プロジェクトを開いているときの画面（メイン・環境変数・履歴）。タブの保存と復元・ショートカットもここ。 */
function ProjectShell({ projects, current }: { projects: Project[]; current: Project }) {
  const screen = useUiStore((s) => s.screen);
  useSessionRestore(current.id);
  useSessionSave(current.id);
  useGlobalShortcuts(current.id);

  return (
    <>
      <div className="app">
        {screen === "environments" ? (
          <>
            <UpdateBanner />
            <EnvironmentsScreen project={current} />
          </>
        ) : screen === "history" ? (
          <>
            <UpdateBanner />
            <HistoryScreen project={current} />
          </>
        ) : (
          <>
            <Header projects={projects} current={current} />
            <UpdateBanner />
            <div className="app-body">
              <Sidebar project={current} />
              <SidebarResizer />
              <Workspace project={current} />
            </div>
            <CreateProjectDialog />
            <SyncDialog project={current} />
          </>
        )}
      </div>
      <SettingsDialog />
    </>
  );
}
