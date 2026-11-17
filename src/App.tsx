import { errorMessage } from "./api";
import { useProjects } from "./api/queries";
import { CreateProjectDialog } from "./components/CreateProjectDialog";
import { CreateProjectScreen } from "./components/CreateProjectScreen";
import { Header } from "./components/Header";
import { Sidebar } from "./components/Sidebar";
import { EnvironmentsScreen } from "./components/EnvironmentsScreen";
import { ImportDialog } from "./components/openapi/ImportDialog";
import { SyncDialog } from "./components/openapi/SyncDialog";
import { Workspace } from "./components/Workspace";
import { useUiStore } from "./store/ui";

export default function App() {
  const projects = useProjects();
  const currentProjectId = useUiStore((s) => s.currentProjectId);
  const screen = useUiStore((s) => s.screen);

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
  if (projects.data.length === 0) {
    return (
      <>
        <CreateProjectScreen />
        <ImportDialog />
      </>
    );
  }

  // 覚えていたプロジェクトがもうなければ、先頭を開く
  const current = projects.data.find((p) => p.id === currentProjectId) ?? projects.data[0];

  if (screen === "environments") {
    return (
      <>
        <div className="app">
          <EnvironmentsScreen project={current} />
        </div>
        <ImportDialog />
      </>
    );
  }

  return (
    <>
      <div className="app">
        <Header projects={projects.data} current={current} />
        <div className="app-body">
          <Sidebar project={current} />
          <Workspace project={current} />
        </div>
        <CreateProjectDialog />
        <SyncDialog project={current} />
      </div>
      <ImportDialog />
    </>
  );
}
