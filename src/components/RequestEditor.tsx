import { useMemo, useState } from "react";

import { errorMessage, useBackend } from "../api";
import { useEnvironments, usePreview, useSaveTab } from "../api/queries";
import type { AuthOverride, Body, Project, RequestSpec } from "../bindings";
import { countActive, METHODS } from "../lib/spec";
import { describeLocation, indexUnresolved, type Section, splitVars } from "../lib/vars";
import { cancelTab, sendTab } from "../store/send";
import { type RequestPane, type RequestTab, useTabsStore } from "../store/tabs";
import { ArrowRightIcon } from "./Icons";
import { AuthPanel, defaultAuth } from "./AuthPanel";
import { BodyPanel, defaultBody } from "./BodyPanel";
import { HeadersPanel } from "./HeadersPanel";
import { ParamsPanel } from "./ParamsPanel";
import { ResponsePane } from "./ResponsePane";
import { VarInput } from "./VarInput";

/** 開いているリクエスト 1 つぶんの画面（URL・送信・リクエストの入力・レスポンス）。 */
export function RequestEditor({ project, tab }: { project: Project; tab: RequestTab }) {
  const backend = useBackend();
  const envs = useEnvironments(project.id);
  const updateSpec = useTabsStore((s) => s.updateSpec);
  const patch = useTabsStore((s) => s.patch);
  const setSaveDialogTab = useTabsStore((s) => s.setSaveDialogTab);
  const saveTab = useSaveTab();
  const [saveError, setSaveError] = useState<string | null>(null);

  const refreshKey = `${project.defaultEnvironmentId ?? ""}:${envs.dataUpdatedAt}`;
  const { preview } = usePreview(project.id, tab.spec, refreshKey);
  const unresolved = useMemo(() => indexUnresolved(preview?.unresolved ?? []), [preview]);
  const unresolvedNames = useMemo(() => new Set((preview?.unresolved ?? []).map((u) => u.name)), [preview]);

  const sending = tab.send.phase === "sending";
  const canSend = preview !== null && preview.unresolved.length === 0 && preview.urlError === null && !sending;
  const envName = envs.data?.find((e) => e.id === project.defaultEnvironmentId)?.name ?? null;

  const onSpec = (update: (spec: RequestSpec) => RequestSpec) => updateSpec(tab.id, update);

  const send = () => {
    if (!canSend) return;
    void sendTab(backend, tab.id, { projectId: project.id, environmentId: null, request: tab.spec, overrides: [] }, envName);
  };

  const save = () => {
    setSaveError(null);
    if (tab.adhocId === null) {
      setSaveDialogTab(tab.id);
      return;
    }
    saveTab.mutate({ tab }, { onError: (e) => setSaveError(errorMessage(e)) });
  };

  const onBodyKind = (kind: Body["kind"]) => {
    const current = tab.spec.body;
    const stash = { ...tab.bodyStash, [current.kind]: current };
    patch(tab.id, { bodyStash: stash });
    updateSpec(tab.id, (s) => ({ ...s, body: stash[kind] ?? defaultBody(kind) }));
  };
  const onAuthKind = (type: AuthOverride["type"]) => {
    const current = tab.spec.auth;
    const stash = current ? { ...tab.authStash, [current.type]: current } : tab.authStash;
    patch(tab.id, { authStash: stash });
    updateSpec(tab.id, (s) => ({ ...s, auth: stash[type] ?? defaultAuth(type) }));
  };

  const paramCount = countActive(tab.spec.params, ["query", "path", "cookie"]);
  const headerCount = countActive(tab.spec.params, ["header"]);
  const tabs: { id: RequestPane; label: string; section?: Section }[] = [
    { id: "params", label: paramCount > 0 ? `パラメータ (${paramCount})` : "パラメータ", section: "params" },
    { id: "headers", label: headerCount > 0 ? `ヘッダー (${headerCount})` : "ヘッダー", section: "headers" },
    { id: "body", label: "ボディ", section: "body" },
    { id: "auth", label: "認証", section: "auth" },
    { id: "design", label: "設計" },
  ];

  const urlBad = unresolved.url.size > 0;
  const showUrlError = preview?.urlError && tab.spec.url.trim() !== "";

  return (
    <>
      <div className="urlbar">
        <div className="urlbar-row">
          <div className={`url-field${urlBad ? " is-invalid" : ""}`}>
            <select
              className="method-select"
              aria-label="HTTPメソッド"
              data-method={tab.spec.method}
              value={tab.spec.method}
              onChange={(e) => onSpec((s) => ({ ...s, method: e.target.value }))}
            >
              {METHODS.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
            <VarInput
              className="url-input"
              aria-label="リクエストURL"
              placeholder="{{baseUrl}}/api/v1/..."
              value={tab.spec.url}
              unresolved={unresolved.url}
              onChange={(url) => onSpec((s) => ({ ...s, url }))}
              onKeyDown={(e) => {
                if (e.key === "Enter") send();
              }}
            />
          </div>
          {sending ? (
            <button type="button" className="btn btn-send btn-danger" onClick={() => void cancelTab(backend, tab.id)} disabled={tab.send.cancelRequested}>
              中止
            </button>
          ) : (
            <button type="button" className="btn btn-primary btn-send" onClick={send} disabled={!canSend}>
              <ArrowRightIcon />
              送信
            </button>
          )}
          <button type="button" className="btn btn-save" onClick={save} disabled={saveTab.isPending}>
            保存
          </button>
        </div>

        <div className="expanded" role="status" aria-label="展開後の URL">
          展開後:{" "}
          <span className="expanded-url">
            {preview ? (
              preview.url === "" ? (
                "（URL が空です）"
              ) : (
                splitVars(preview.url).map((seg, i) =>
                  seg.name !== null && unresolvedNames.has(seg.name) ? (
                    <span key={i} className="var-token-bad">
                      {seg.text}
                    </span>
                  ) : (
                    <span key={i}>{seg.text}</span>
                  ),
                )
              )
            ) : (
              "…"
            )}
          </span>
        </div>

        {preview && preview.unresolved.length > 0 && (
          <div className="alert alert-danger" role="alert">
            <div>展開できない変数があるため、送信できません。環境変数に追加してください。</div>
            <ul className="alert-list">
              {preview.unresolved.map((u, i) => (
                <li key={i}>
                  <span className="mono">{`{{${u.name}}}`}</span>（{describeLocation(u.location)}）
                </li>
              ))}
            </ul>
          </div>
        )}
        {showUrlError && (
          <div className="alert alert-danger" role="alert">
            {preview.urlError}
          </div>
        )}
        {preview && preview.missingPathParams.length > 0 && (
          <div className="alert alert-warn">
            パスパラメータの値が空です: {preview.missingPathParams.map((n) => `{${n}}`).join("、")}（そのまま送られます）
          </div>
        )}
        {saveError && (
          <div className="alert alert-danger" role="alert">
            保存できませんでした: {saveError}
          </div>
        )}
      </div>

      <div className="panes">
        <section className="pane pane-request" aria-label="リクエスト">
          <div className="pane-tabs" role="tablist" aria-label="リクエスト設定">
            {tabs.map((t) => (
              <button
                key={t.id}
                type="button"
                role="tab"
                aria-selected={tab.requestPane === t.id}
                className={`pane-tab${t.section && unresolved.has(t.section) ? " is-bad" : ""}`}
                onClick={() => patch(tab.id, { requestPane: t.id })}
              >
                {t.label}
                {t.section && unresolved.has(t.section) && (
                  <span className="pane-tab-dot" aria-hidden="true">
                    ●
                  </span>
                )}
              </button>
            ))}
          </div>
          <div className="pane-body" role="tabpanel">
            {tab.requestPane === "params" && <ParamsPanel spec={tab.spec} onChange={onSpec} unresolved={unresolved} />}
            {tab.requestPane === "headers" && (
              <HeadersPanel spec={tab.spec} onChange={onSpec} unresolved={unresolved} autoHeaders={preview?.autoHeaders ?? []} />
            )}
            {tab.requestPane === "body" && <BodyPanel tab={tab} onSpec={onSpec} onKind={onBodyKind} unresolved={unresolved} />}
            {tab.requestPane === "auth" && (
              <AuthPanel tab={tab} onSpec={onSpec} onKind={onAuthKind} unresolved={unresolved} autoHeaders={preview?.autoHeaders ?? []} />
            )}
            {tab.requestPane === "design" && (
              <div className="panel">
                <div className="pane-placeholder">
                  設計タブ（OpenAPI の説明・スキーマ・ステータスごとの応答）は M4 で実装します。OpenAPI を取り込んだ API で、ここに表示されます。
                </div>
              </div>
            )}
          </div>
        </section>

        <ResponsePane tab={tab} />
      </div>
    </>
  );
}
