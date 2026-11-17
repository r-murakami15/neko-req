import { useBackend } from "../api";
import { methodColor } from "../lib/spec";
import { openBlankTab } from "../store/ops";
import { closeTab } from "../store/send";
import { isDirty, type RequestTab, useTabsStore } from "../store/tabs";
import { CloseIcon } from "./Icons";

/** 開いているリクエストのタブ列。 */
export function TabBar({ projectId, tabs, activeId }: { projectId: string; tabs: RequestTab[]; activeId: string | undefined }) {
  const backend = useBackend();
  const activate = useTabsStore((s) => s.activate);

  return (
    <div className="tabbar" role="tablist" aria-label="開いているリクエスト">
      {tabs.length === 0 && <span className="tabbar-empty">開いているリクエストはありません</span>}
      {tabs.map((t) => (
        <div key={t.id} className={`tab${t.id === activeId ? " is-active" : ""}`} role="presentation">
          <button type="button" role="tab" aria-selected={t.id === activeId} className="tab-main" onClick={() => activate(projectId, t.id)}>
            <span className="tab-method" style={{ color: methodColor(t.spec.method) }}>
              {t.spec.method}
            </span>
            <span className="tab-name">{t.name}</span>
            {isDirty(t) && (
              <span className="tab-dirty" role="img" aria-label="未保存">
                ●
              </span>
            )}
          </button>
          <button type="button" className="tab-close" aria-label={`${t.name} を閉じる`} onClick={() => closeTab(backend, t.id)}>
            <CloseIcon />
          </button>
        </div>
      ))}
      <button type="button" className="tab-add" aria-label="新しいタブ" title="空のリクエストを新しいタブで開く" onClick={() => openBlankTab(projectId)}>
        +
      </button>
    </div>
  );
}
