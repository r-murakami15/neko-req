import { errorMessage } from "./api";
import { useProjects } from "./api/queries";
import { CreateProjectDialog } from "./components/CreateProjectDialog";
import { CreateProjectScreen } from "./components/CreateProjectScreen";
import { Header } from "./components/Header";
import { Sidebar } from "./components/Sidebar";
import { EnvironmentsScreen } from "./components/EnvironmentsScreen";
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

  if (projects.data.length === 0) {
    return <CreateProjectScreen />;
  }

  // 覚えていたプロジェクトがもうなければ、先頭を開く
  const current = projects.data.find((p) => p.id === currentProjectId) ?? projects.data[0];

  if (screen === "environments") {
    return (
      <div className="app">
        <EnvironmentsScreen project={current} />
      </div>
    );
  }

  return (
    <div className="app">
      <Header projects={projects.data} current={current} />
      <div className="app-body">
        <Sidebar project={current} />
        <Workspace project={current} />
      </div>
      <CreateProjectDialog />
    </div>
  );
}
