import { useId, useState } from "react";

import { errorMessage, useBackend } from "../api";
import { useOperationDetail } from "../api/queries";
import type { ErrorCodeEntry, JsonValue, MediaDoc, OperationDetail, ParamDoc, ResponseDoc } from "../bindings";
import { formatDateTime } from "../lib/format";
import { PARAM_IN_LABEL } from "../lib/opkey";
import { methodColor } from "../lib/spec";
import type { RequestTab } from "../store/tabs";
import { MarkdownView } from "./Markdown";
import { ExampleBlock, SchemaTable } from "./SchemaTable";

/** ステータスコードの色分け（2xx 成功、3xx 案内、4xx クライアント、5xx サーバー）。 */
function statusTone(status: string): string {
  switch (status.charAt(0)) {
    case "2":
      return "ok";
    case "3":
      return "info";
    case "4":
      return "warn";
    case "5":
      return "danger";
    default:
      return "plain";
  }
}

const isJsonType = (contentType: string) => /json/i.test(contentType);

/**
 * 設計タブ。yaml の内容を読み取り専用のドキュメントとして見せる（説明・パラメータ・ボディのスキーマ・
 * ステータスごとのレスポンス）。「ボディに反映」で、例を送信タブのボディに入れられる。
 */
export function DesignPanel({ tab, onApplyBody }: { tab: RequestTab; onApplyBody: (json: string) => void }) {
  const detail = useOperationDetail(tab.projectId, tab.op?.opKey ?? null);

  if (!tab.op) {
    return (
      <div className="panel">
        <div className="pane-placeholder">
          このリクエストは OpenAPI の API ではないので、設計はありません。OpenAPI を取り込んだ API のタブで、説明・スキーマ・ステータスごとの応答を読めます。
        </div>
      </div>
    );
  }
  if (detail.isPending) {
    return (
      <div className="panel" role="status">
        <span className="muted-note">設計を読み込んでいます…</span>
      </div>
    );
  }
  if (detail.isError) {
    return (
      <div className="panel">
        <div className="alert alert-danger" role="alert">
          設計を読み込めませんでした: {errorMessage(detail.error)}
        </div>
      </div>
    );
  }
  return <DesignDocument projectId={tab.projectId} detail={detail.data} onApplyBody={onApplyBody} />;
}

function DesignDocument({ projectId, detail, onApplyBody }: { projectId: string; detail: OperationDetail; onApplyBody: (json: string) => void }) {
  const backend = useBackend();
  const [editorError, setEditorError] = useState<string | null>(null);
  const descId = useId();
  const reqId = useId();
  const resId = useId();

  const openSource = async () => {
    if (detail.sourceFile === null) return;
    setEditorError(null);
    try {
      await backend.editorOpen(projectId, detail.sourceFile, detail.sourceLine);
    } catch (e) {
      setEditorError(errorMessage(e));
    }
  };

  const title = detail.summary ?? `${detail.method} ${detail.path}`;
  const sourceText = detail.sourceFile === null ? null : `${detail.sourceFile}${detail.sourceLine !== null ? `:${detail.sourceLine}` : ""}`;
  const body = detail.requestBody;
  const contentTypes = body?.contents.map((c) => c.contentType) ?? [];

  return (
    <article className="design" aria-label="設計">
      <div className="design-head">
        <div className="design-title-row">
          <h1 className="design-title">{title}</h1>
          {detail.tags.map((t) => (
            <span key={t} className="badge">
              tag: {t}
            </span>
          ))}
          {detail.security.badge === "none" ? (
            <span className="badge badge-ok">認証不要（security: []）</span>
          ) : (
            <span className="badge badge-auth">認証: {detail.security.labels.length > 0 ? detail.security.labels.join("、") : "必要"}</span>
          )}
          {detail.deleted && <span className="badge badge-danger">yaml から削除された API</span>}
        </div>
        <div className="design-meta">
          <span>
            <span className="tree-method" style={{ color: methodColor(detail.method) }}>
              {detail.method}
            </span>{" "}
            <code>{detail.path}</code>
          </span>
          <span>
            operationId{" "}
            {detail.operationId ? <code>{detail.operationId}</code> : <span title="operationId がないので「METHOD path」をキーにしています">なし</span>}
          </span>
          {sourceText && (
            <span>
              取り込み元{" "}
              <button type="button" className="link-btn mono" onClick={() => void openSource()} title="取り込んだ yaml を既定のアプリで開きます">
                {sourceText}
              </button>
            </span>
          )}
          {detail.importedAt && <span>最終取り込み {formatDateTime(detail.importedAt)}</span>}
        </div>
        {editorError && (
          <div className="alert alert-danger" role="alert">
            エディタで開けませんでした: {editorError}
          </div>
        )}
        {detail.deleted && (
          <div className="alert alert-warn">この API は yaml から削除されました。入力値は残してあります（サイドバーの「削除された API」から消せます）。</div>
        )}
      </div>

      <section aria-labelledby={descId} className="design-section">
        <h2 id={descId} className="design-h2">
          説明
        </h2>
        <div className="design-card">
          {detail.description && detail.description.trim() !== "" ? <MarkdownView source={detail.description} /> : <p className="muted-note">説明は書かれていません。</p>}
        </div>
      </section>

      <section aria-labelledby={reqId} className="design-section">
        <h2 id={reqId} className="design-h2">
          リクエスト
        </h2>
        <div className="design-cards">
          <div className="design-stat">
            <span className="muted-note">パラメータ</span>
            <span>{detail.parameters.length === 0 ? "なし" : `${detail.parameters.length} 件${requiredNote(detail.parameters)}`}</span>
          </div>
          <div className="design-stat">
            <span className="muted-note">Content-Type</span>
            <span className="mono">{contentTypes.length > 0 ? contentTypes.join("、") : "—"}</span>
          </div>
          <div className="design-stat">
            <span className="muted-note">ボディ</span>
            <span>
              {body ? (
                <>
                  {body.required ? "必須" : "任意"} · <code>{body.contents[0]?.schemaLabel ?? "（スキーマなし）"}</code>
                </>
              ) : (
                "なし"
              )}
            </span>
          </div>
        </div>

        {detail.parameters.length > 0 && <ParamTable params={detail.parameters} />}
        {body?.description && <p className="design-note">{body.description}</p>}
        {body && body.contents.length > 0 && (
          <MediaView
            label="リクエストボディ"
            medias={body.contents}
            onApply={(m, value) => (value !== null && isJsonType(m.contentType) ? () => onApplyBody(JSON.stringify(value, null, 2)) : undefined)}
          />
        )}
      </section>

      <section aria-labelledby={resId} className="design-section">
        <h2 id={resId} className="design-h2">
          レスポンス
        </h2>
        {detail.responses.length === 0 ? <p className="muted-note">レスポンスの定義はありません。</p> : <ResponseTabs responses={detail.responses} all={detail.errorCodes} />}
      </section>
    </article>
  );
}

function requiredNote(params: ParamDoc[]): string {
  const n = params.filter((p) => p.required).length;
  return n > 0 ? `（必須 ${n}）` : "";
}

function ParamTable({ params }: { params: ParamDoc[] }) {
  return (
    <div className="schema-wrap">
      <table className="schema-table" aria-label="パラメータ">
        <thead>
          <tr>
            <th>名前</th>
            <th>場所</th>
            <th>型</th>
            <th>必須</th>
            <th>制約</th>
            <th>例・既定値</th>
            <th>説明</th>
          </tr>
        </thead>
        <tbody>
          {params.map((p) => (
            <tr key={`${p.location}:${p.name}`}>
              <td className="schema-name mono">{p.name}</td>
              <td className="schema-in">{PARAM_IN_LABEL[p.location]}</td>
              <td className="schema-type mono">{p.typeLabel}</td>
              <td className="schema-required">{p.required ? "必須" : ""}</td>
              <td className="schema-constraints">
                {p.constraints.map((c) => (
                  <span key={c} className="chip">
                    {c}
                  </span>
                ))}
              </td>
              <td className="schema-example mono">
                {p.example !== null && <span>例: {p.example}</span>}
                {p.example !== null && p.default !== null && " "}
                {p.default !== null && <span>既定: {p.default}</span>}
              </td>
              <td className="schema-desc">{p.description ?? ""}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Content-Type ごとのスキーマ表と例。Content-Type が複数あれば、タブで切り替える。 */
function MediaView({ label, medias, onApply }: { label: string; medias: MediaDoc[]; onApply: (m: MediaDoc, value: JsonValue | null) => (() => void) | undefined }) {
  const [index, setIndex] = useState(0);
  const media = medias[Math.min(index, medias.length - 1)];
  return (
    <div className="media-view">
      {medias.length > 1 && (
        <div className="segmented" role="tablist" aria-label={`${label}の Content-Type`}>
          {medias.map((m, i) => (
            <button key={m.contentType} type="button" role="tab" aria-selected={i === index} className="segmented-item" onClick={() => setIndex(i)}>
              {m.contentType}
            </button>
          ))}
        </div>
      )}
      <div className="design-block">
        <div className="design-block-head">
          <span>データモデル</span>
          {media.schemaLabel && <code className="design-type">{media.schemaLabel}</code>}
          <span className="muted-note mono">{media.contentType}</span>
        </div>
        {media.rows.length > 0 ? <SchemaTable rows={media.rows} label={`${label}のスキーマ`} /> : <p className="design-note">スキーマはありません。</p>}
      </div>
      <ExampleSection key={media.contentType} label={label} media={media} onApply={onApply} />
    </div>
  );
}

/**
 * 例。名前つきの例（`examples`）が複数あれば、切り替えて見る。「ボディに反映」は、選んでいる例が入る。
 * 名前つきの例がなければ、`example`（yaml の example・スキーマから作った例）を 1 つだけ見せる。
 */
function ExampleSection({ label, media, onApply }: { label: string; media: MediaDoc; onApply: (m: MediaDoc, value: JsonValue | null) => (() => void) | undefined }) {
  const [index, setIndex] = useState(0);
  const named = media.examples;
  const current = named.length > 0 ? named[Math.min(index, named.length - 1)] : null;
  const value = current ? current.value : media.example;
  if (value === null) return null;
  return (
    <>
      {named.length > 1 && (
        <div className="segmented example-switch" role="tablist" aria-label={`${label}の例を切り替え`}>
          {named.map((n, i) => (
            <button key={`${n.name}-${i}`} type="button" role="tab" aria-selected={i === index} className="segmented-item" title={n.summary ?? undefined} onClick={() => setIndex(i)}>
              {n.name}
            </button>
          ))}
        </div>
      )}
      {current && (current.summary || current.description) && <p className="design-note example-summary">{current.summary ?? current.description}</p>}
      <ExampleBlock value={value} source={media.exampleSource} onApply={onApply(media, value)} />
    </>
  );
}

function ResponseTabs({ responses, all }: { responses: ResponseDoc[]; all: ErrorCodeEntry[] }) {
  const [index, setIndex] = useState(0);
  const current = responses[Math.min(index, responses.length - 1)];
  // 返す code は、レスポンスごとの値がなければ、全体の表から同じステータスのものを使う
  const codes = current.errorCodes.length > 0 ? current.errorCodes : all.filter((c) => c.status === current.status);
  const types = current.contents.map((c) => c.contentType);
  return (
    <>
      <div className="status-tabs" role="tablist" aria-label="ステータスコード">
        {responses.map((r, i) => (
          <button key={r.status} type="button" role="tab" aria-selected={i === index} className="status-tab" onClick={() => setIndex(i)}>
            <span className={`status-num is-${statusTone(r.status)}`}>{r.status}</span>
            {r.description ? shorten(r.description) : ""}
          </button>
        ))}
      </div>
      <div className="design-panel" role="tabpanel" aria-label={`ステータス ${current.status}`}>
        <div className="design-strip">
          <span>
            <span className="muted-note">ステータス</span> {current.status}
          </span>
          <span>
            <span className="muted-note">Content-Type</span> <span className="mono">{types.length > 0 ? types.join("、") : "なし"}</span>
          </span>
          <span>
            <span className="muted-note">スキーマ</span> <code>{current.contents[0]?.schemaLabel ?? "—"}</code>
          </span>
        </div>
        {current.description && <p className="design-desc">{current.description}</p>}
        {codes.length > 0 && (
          <div className="error-codes" aria-label="この API で返す code">
            <span className="muted-note">この API で返す code</span>
            {codes.map((c, i) => (
              <span key={`${c.code}-${i}`} className="error-code">
                <span className="chip chip-danger mono">{c.code}</span>
                {c.note && <span className="muted-note">{c.note}</span>}
              </span>
            ))}
          </div>
        )}
        {current.contents.length > 0 ? (
          <div className="design-panel-body">
            <MediaView key={current.status} label={`レスポンス ${current.status}`} medias={current.contents} onApply={() => undefined} />
          </div>
        ) : (
          <p className="design-note design-panel-body">ボディはありません。</p>
        )}
      </div>
    </>
  );
}

function shorten(text: string): string {
  const one = text.replace(/\s+/g, " ").trim();
  const cut = one.split(/[。（(]/)[0];
  return cut.length > 20 ? `${cut.slice(0, 20)}…` : cut;
}
