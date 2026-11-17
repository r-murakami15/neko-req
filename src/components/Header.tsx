import { useSelectEnvironment, useEnvironments } from "../api/queries";
import type { Project } from "../bindings";
import { useUiStore } from "../store/ui";
import { DownloadIcon, GearIcon, PencilIcon, CatLogo } from "./Icons";
import { ProjectSwitcher } from "./ProjectSwitcher";

/** まだ使えない操作の見た目。押せず、何のマイルストーンで入るかを title と印で示す。 */
function Pending({ milestone }: { milestone: string }) {
  return <span className="pending-badge">{milestone}</span>;
}

export function Header({ projects, current }: { projects: Project[]; current: Project }) {
  const envs = useEnvironments(current.id);
  const select = useSelectEnvironment(current.id);
  const setScreen = useUiStore((s) => s.setScreen);
  const choices = (envs.data ?? []).filter((e) => !e.isGlobal);

  return (
    <header className="app-header">
      <div className="brand">
        <CatLogo />
        <span className="brand-name">NekoReq</span>
        <span className="brand-sep" aria-hidden="true">
          /
        </span>
        <ProjectSwitcher projects={projects} current={current} />
      </div>

      <label className="env-select">
        環境
        <select
          className="select"
          disabled={choices.length === 0 || select.isPending}
          value={current.defaultEnvironmentId ?? ""}
          onChange={(e) => select.mutate(e.target.value === "" ? null : e.target.value)}
        >
          {choices.length === 0 && <option value="">環境がありません</option>}
          {choices.length > 0 && current.defaultEnvironmentId === null && <option value="">（環境なし）</option>}
          {choices.map((e) => (
            <option key={e.id} value={e.id}>
              {e.name}
            </option>
          ))}
        </select>
      </label>
      <button type="button" className="btn btn-icon" aria-label="環境変数を編集" title="環境変数を編集" onClick={() => setScreen("environments")}>
        <PencilIcon />
      </button>
      <button type="button" className="btn" disabled title="OpenAPI の取り込みは M3 で実装">
        <DownloadIcon />
        OpenAPI取り込み
        <Pending milestone="M3 で実装" />
      </button>
      <button type="button" className="btn btn-icon" disabled aria-label="設定（M6 で実装）" title="設定は M6 で実装">
        <GearIcon />
      </button>
    </header>
  );
}
