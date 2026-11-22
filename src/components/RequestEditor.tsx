import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";

import { errorMessage, useBackend } from "../api";
import { useEnvironments, useOperationDetail, usePreview, useSaveDraft, useSaveTab } from "../api/queries";
import type { Body, Project, RequestSpec } from "../bindings";
import { copyCurlOfTab, requestInputOf } from "../lib/curl";
import { PARAM_IN_LABEL } from "../lib/opkey";
import { countActive, emptyRequiredParams, METHODS, type ParamMeta, specForSend } from "../lib/spec";
import { describeLocation, indexUnresolved, type Section, splitVars } from "../lib/vars";
import { useActionsStore } from "../store/actions";
import { cancelTab, sendTab } from "../store/send";
import { type RequestPane, type RequestTab, useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";
import { ConfirmDialog } from "./Dialog";
import { ArrowRightIcon, ChevronDownIcon, PanelRightIcon } from "./Icons";
import { AuthPanel, type AuthKind, defaultAuth } from "./AuthPanel";
import { BodyPanel, defaultBody } from "./BodyPanel";
import { DesignPanel } from "./DesignPanel";
import { HeadersPanel } from "./HeadersPanel";
import { ParamsPanel } from "./ParamsPanel";
import { ResponsePane } from "./ResponsePane";
import { ResponseResizer, useResponseSplit } from "./ResponseResizer";
import { VarInput } from "./VarInput";

/** 開いているリクエスト 1 つぶんの画面（URL・送信・リクエストの入力・レスポンス）。 */
export function RequestEditor({ project, tab }: { project: Project; tab: RequestTab }) {
  const backend = useBackend();
  const envs = useEnvironments(project.id);
  const updateSpec = useTabsStore((s) => s.updateSpec);
  const patch = useTabsStore((s) => s.patch);
  const setSaveDialogTab = useTabsStore((s) => s.setSaveDialogTab);
  const saveTab = useSaveTab();
  const saveDraft = useSaveDraft();
  const [saveError, setSaveError] = useState<string | null>(null);
  // cURL をコピーした結果の知らせ（タブの右クリックメニューからコピーしたぶんもここに出る）
  const curlMessage = useUiStore((s) => (s.curlNotice?.tabId === tab.id ? s.curlNotice : null));
  const [rawConfirmOpen, setRawConfirmOpen] = useState(false);
  const dismissDropped = useTabsStore((s) => s.dismissDropped);
  const responseOpen = useUiStore((s) => s.responseOpen);
  const setResponseOpen = useUiStore((s) => s.setResponseOpen);
  const unseen = useUiStore((s) => s.unseenResponses[tab.id] === true);
  const urlField = useRef<HTMLDivElement>(null);

  const op = tab.op;
  const opKey = op?.opKey ?? null;
  const detail = useOperationDetail(project.id, opKey);
  // yaml の API: 「yaml にない」パラメータは、有効にしてあっても送らない
  const sendSpec = useMemo(() => specForSend(tab.spec, op?.missingInYaml ?? []), [tab.spec, op?.missingInYaml]);
  const paramMeta: ParamMeta | undefined = useMemo(
    () => (op ? { docs: detail.data?.parameters ?? [], missing: op.missingInYaml, required: op.requiredParams } : undefined),
    [op, detail.data],
  );
  const emptyRequired = useMemo(() => (op ? emptyRequiredParams(tab.spec, op.requiredParams) : []), [op, tab.spec]);
  // スキーマだけから生成した JSON の例（ボディの「スキーマから例を生成」）
  const schemaExample = useMemo(() => {
    const media = detail.data?.requestBody?.contents.find((c) => /json/i.test(c.contentType));
    return media && media.generatedExample !== null ? JSON.stringify(media.generatedExample, null, 2) : null;
  }, [detail.data]);

  // 環境を選び直した・環境変数を保存した・認証の自動化のチェックを変えたときも、付くヘッダーを聞き直す
  const refreshKey = `${project.defaultEnvironmentId ?? ""}:${envs.dataUpdatedAt}:${project.settings?.autoSession ?? true}:${project.settings?.autoCsrf ?? true}`;
  const { preview } = usePreview(project.id, sendSpec, refreshKey, opKey);
  const unresolved = useMemo(() => indexUnresolved(preview?.unresolved ?? []), [preview]);
  const unresolvedNames = useMemo(() => new Set((preview?.unresolved ?? []).map((u) => u.name)), [preview]);

  const sending = tab.send.phase === "sending";
  const canSend = preview !== null && preview.unresolved.length === 0 && preview.urlError === null && !sending;
  const envName = envs.data?.find((e) => e.id === project.defaultEnvironmentId)?.name ?? null;

  const onSpec = (update: (spec: RequestSpec) => RequestSpec) => updateSpec(tab.id, update);

  const send = () => {
    if (!canSend) return;
    void sendTab(backend, tab.id, requestInputOf(tab), envName);
  };

  /** 今のリクエストを cURL にしてコピーする。既定は秘密を伏せる。`raw` は秘密の値がそのまま入る。 */
  const copyCurl = (mode: "redacted" | "raw") => copyCurlOfTab(backend, tab, mode);

  const save = () => {
    setSaveError(null);
    if (op) {
      // yaml の API は、タグでフォルダが決まるので、保存ダイアログは出さずにそのまま保存する
      saveDraft.mutate(tab, { onError: (e) => setSaveError(errorMessage(e)) });
      return;
    }
    if (tab.adhocId === null) {
      setSaveDialogTab(tab.id);
      return;
    }
    saveTab.mutate({ tab }, { onError: (e) => setSaveError(errorMessage(e)) });
  };

  // この画面を離れたら、cURL の知らせは消す
  useEffect(
    () => () => {
      const ui = useUiStore.getState();
      if (ui.curlNotice?.tabId === tab.id) ui.setCurlNotice(null);
    },
    [tab.id],
  );

  // ショートカット（送信・保存）が、前面のリクエストの最新の操作を呼べるようにする
  const latest = useRef({ send, save });
  latest.current = { send, save };
  const setEditorActions = useActionsStore((s) => s.setEditor);
  useEffect(() => {
    setEditorActions({ tabId: tab.id, send: () => latest.current.send(), save: () => latest.current.save() });
    return () => setEditorActions(null);
  }, [tab.id, setEditorActions]);

  // 「＋」・Cmd/Ctrl+T で開いたばかりのタブは、URL 欄にフォーカスを入れる
  useEffect(() => {
    const ui = useUiStore.getState();
    if (ui.focusUrlTabId !== tab.id) return;
    ui.setFocusUrlTabId(null);
    urlField.current?.querySelector("input")?.focus();
  }, [tab.id]);

  const onBodyKind = (kind: Body["kind"]) => {
    const current = tab.spec.body;
    const stash = { ...tab.bodyStash, [current.kind]: current };
    patch(tab.id, { bodyStash: stash });
    updateSpec(tab.id, (s) => ({ ...s, body: stash[kind] ?? defaultBody(kind) }));
  };
  const onAuthKind = (kind: AuthKind) => {
    const current = tab.spec.auth;
    const stash = current ? { ...tab.authStash, [current.type]: current } : tab.authStash;
    patch(tab.id, { authStash: stash });
    // 「プロジェクトの既定を使う」は、認証の上書きなし（null）= yaml の security に従う
    updateSpec(tab.id, (s) => ({ ...s, auth: kind === "default" ? null : (stash[kind] ?? defaultAuth(kind)) }));
  };
  const applyExample = (json: string) => {
    updateSpec(tab.id, (s) => ({ ...s, body: { kind: "json", value: json } }));
    patch(tab.id, { requestPane: "body" });
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

  // yaml の API の設計タブは、画面イメージのとおり幅いっぱいに読ませる（レスポンス欄は隠す）
  const designFull = op !== null && tab.requestPane === "design";
  const showResponse = responseOpen && !designFull;
  const panesRef = useRef<HTMLDivElement>(null);
  const split = useResponseSplit(panesRef);

  // レスポンス欄が見えているタブの「新しい結果」の印は外す
  useEffect(() => {
    if (showResponse && unseen) useUiStore.getState().clearResponseUnseen(tab.id);
  }, [showResponse, unseen, tab.id]);
  const urlBad = unresolved.url.size > 0;
  const showUrlError = preview?.urlError && tab.spec.url.trim() !== "";

  return (
    <>
      <div className="urlbar">
        <div className="urlbar-row">
          <div className={`url-field${urlBad ? " is-invalid" : ""}`} ref={urlField}>
            <select
              className="method-select"
              aria-label="HTTPメソッド"
              data-method={tab.spec.method}
              value={tab.spec.method}
              disabled={op !== null}
              title={op ? "OpenAPI の API の method は yaml で決まります" : undefined}
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
              readOnly={op !== null}
              title={op ? "OpenAPI の API の URL は yaml で決まります（{{baseUrl}} は環境変数）" : undefined}
              onChange={(url) => onSpec((s) => ({ ...s, url }))}
              onKeyDown={(e) => {
                // Ctrl/⌘+Enter は、画面全体のショートカットが送る（ここでも送ると 2 回送ってしまう）
                if (e.key === "Enter" && !e.ctrlKey && !e.metaKey) send();
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
          <button type="button" className="btn btn-save" onClick={save} disabled={saveTab.isPending || saveDraft.isPending}>
            保存
          </button>
          <DropdownMenu.Root>
            <DropdownMenu.Trigger className="btn btn-curl" aria-label="cURL のメニュー" title="今のリクエストを cURL としてコピー">
              cURL
              <ChevronDownIcon />
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content className="menu" align="end" sideOffset={6}>
                <DropdownMenu.Item className="menu-item" onSelect={() => void copyCurl("redacted")}>
                  <span className="menu-item-text">cURL としてコピー（秘密は伏せる）</span>
                </DropdownMenu.Item>
                <DropdownMenu.Item className="menu-item" onSelect={() => setRawConfirmOpen(true)}>
                  <span className="menu-item-text">cURL としてコピー（秘密もそのまま）…</span>
                </DropdownMenu.Item>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
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
        {op?.deleted && (
          <div className="alert alert-warn">この API は yaml から削除されました。入力値は残してあります（サイドバーの「削除された API」から消せます）。</div>
        )}
        {emptyRequired.length > 0 && (
          <div className="alert alert-warn" role="status">
            必須（required）の項目が空か無効です: {emptyRequired.map((r) => `${r.name}（${PARAM_IN_LABEL[r.location]}）`).join("、")}（送信はできます）
          </div>
        )}
        {saveError && (
          <div className="alert alert-danger" role="alert">
            保存できませんでした: {saveError}
          </div>
        )}
        {curlMessage && (
          <div className={`alert ${curlMessage.ok ? "alert-ok" : "alert-danger"}`} role={curlMessage.ok ? "status" : "alert"}>
            {curlMessage.text}
          </div>
        )}
        {tab.droppedNote && (
          <div className="alert alert-warn dropped-note" role="status">
            <span>
              {tab.droppedNote.source === "session"
                ? "手で入力した認証の値は保存していません。入れ直してください"
                : "履歴には伏せ字で残っているため、手で入力した認証の値は外しました。入れ直してください"}
              {tab.droppedNote.fields.length > 0 && `（${tab.droppedNote.fields.join("、")}）`}
            </span>
            <button type="button" className="link-btn" onClick={() => dismissDropped(tab.id)}>
              閉じる
            </button>
          </div>
        )}
      </div>

      <ConfirmDialog
        open={rawConfirmOpen}
        onOpenChange={setRawConfirmOpen}
        title="秘密の値をそのままコピーしますか？"
        description="セッション・トークン・パスワードなどの秘密の値が、平文のまま、クリップボードに入ります。貼り付けた先（チャット・メモ・履歴など）に残らないよう気をつけてください。"
        confirmLabel="そのままコピー"
        danger
        onConfirm={() => {
          setRawConfirmOpen(false);
          void copyCurl("raw");
        }}
      />

      <div
        ref={panesRef}
        className={`panes${designFull ? " is-design" : ""}${!responseOpen && !designFull ? " is-response-closed" : ""}`}
        style={{ "--response-ratio": split.ratio } as CSSProperties}
      >
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
            {!designFull && !responseOpen && (
              <button
                type="button"
                className="icon-btn response-open-btn"
                aria-label={unseen ? "レスポンスを表示（新しい結果があります）" : "レスポンスを表示"}
                title={unseen ? "レスポンスを表示（新しい結果があります）" : "レスポンスを表示"}
                onClick={() => setResponseOpen(true)}
              >
                <PanelRightIcon />
                {unseen && <span className="response-open-dot" aria-hidden="true" />}
              </button>
            )}
          </div>
          <div className="pane-body" role="tabpanel">
            {tab.requestPane === "params" && <ParamsPanel spec={tab.spec} onChange={onSpec} unresolved={unresolved} meta={paramMeta} />}
            {tab.requestPane === "headers" && (
              <HeadersPanel
                spec={tab.spec}
                onChange={onSpec}
                unresolved={unresolved}
                autoHeaders={preview?.autoHeaders ?? []}
                autoNotes={preview?.autoNotes ?? []}
                meta={paramMeta}
              />
            )}
            {tab.requestPane === "body" && (
              <BodyPanel tab={tab} onSpec={onSpec} onKind={onBodyKind} unresolved={unresolved} schemaExample={op ? schemaExample : null} />
            )}
            {tab.requestPane === "auth" && (
              <AuthPanel
                project={project}
                tab={tab}
                onSpec={onSpec}
                onKind={onAuthKind}
                unresolved={unresolved}
                autoHeaders={preview?.autoHeaders ?? []}
                autoNotes={preview?.autoNotes ?? []}
              />
            )}
            {tab.requestPane === "design" && <DesignPanel tab={tab} onApplyBody={applyExample} />}
          </div>
        </section>

        {showResponse && <ResponseResizer total={split.total} ratio={split.ratio} />}
        {showResponse && <ResponsePane tab={tab} onClose={() => setResponseOpen(false)} />}
      </div>
    </>
  );
}
