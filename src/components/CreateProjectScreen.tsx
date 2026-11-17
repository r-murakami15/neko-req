import { useUiStore } from "../store/ui";
import { CatLogo, DownloadIcon } from "./Icons";
import { CreateProjectForm } from "./CreateProjectForm";

/** プロジェクトが 1 つもないときに出す画面。 */
export function CreateProjectScreen() {
  const setImportDialogOpen = useUiStore((s) => s.setImportDialogOpen);
  return (
    <main className="welcome">
      <section className="welcome-card" aria-labelledby="welcome-title">
        <div className="welcome-brand">
          <CatLogo size={36} />
          <span className="brand-name">NekoReq</span>
        </div>
        <h1 id="welcome-title" className="welcome-title">
          プロジェクトを作成
        </h1>
        <p className="welcome-lead">
          API ごとにプロジェクトを作ります。OpenAPI の取り込みや環境変数は、作成したあとに設定できます。
        </p>
        <CreateProjectForm />
        <div className="welcome-or">
          <span>または</span>
          <button type="button" className="btn" onClick={() => setImportDialogOpen(true)}>
            <DownloadIcon />
            OpenAPI から取り込む
          </button>
        </div>
      </section>
    </main>
  );
}
