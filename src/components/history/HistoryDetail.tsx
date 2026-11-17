import { useMemo } from "react";

import type { HistoryEntry } from "../../bindings";
import { formatBytes, formatDateTime, formatMs } from "../../lib/format";
import { statusText, statusTone } from "../../lib/history";
import { prettyResponseJson, tokenizeJson } from "../../lib/json";
import { parseProblem } from "../../lib/problem";
import { methodColor } from "../../lib/spec";
import { ERROR_TITLES } from "../ResponsePane";

export interface HistoryActions {
  onResend: () => void;
  onOpen: () => void;
  onSave: () => void;
  onCopyCurl: () => void;
  onToggleCompare: () => void;
  onDelete: () => void;
}

/** この文字数を超える JSON は、色分けせずにそのまま見せる。 */
const MAX_HIGHLIGHT_CHARS = 300_000;

function JsonOrText({ text }: { text: string }) {
  const pretty = useMemo(() => prettyResponseJson(text), [text]);
  const shown = pretty ?? text;
  const tokens = useMemo(() => (pretty !== null && pretty.length <= MAX_HIGHLIGHT_CHARS ? tokenizeJson(pretty) : null), [pretty]);
  return (
    <>
      {tokens
        ? tokens.map((t, i) => (t.kind === "plain" ? <span key={i}>{t.text}</span> : <span key={i} className={`tok-${t.kind}`}>{t.text}</span>))
        : shown}
    </>
  );
}

/** 送った内容（ヘッダーとボディ）。 */
function SentContent({ entry }: { entry: HistoryEntry }) {
  const hasBody = entry.requestBodyKind !== "none";
  return (
    <pre className="history-pre" aria-label="送信した内容" tabIndex={0}>
      {entry.requestHeaders.map((h, i) => (
        <span key={i}>
          <span className="tok-key">{h.name}</span>: {h.value}
          {"\n"}
        </span>
      ))}
      {hasBody && "\n"}
      {entry.requestBodyKind === "text" && entry.requestBody !== null && <JsonOrText text={entry.requestBody} />}
      {entry.requestBodyKind === "multipart" &&
        entry.requestParts.map((p, i) => (
          <span key={i}>
            <span className="tok-key">{p.name}</span>: {p.isFile ? `@${p.value}` : p.value}
            {p.contentType ? `（${p.contentType}）` : ""}
            {"\n"}
          </span>
        ))}
      {entry.requestBodyKind === "file" && <span className="muted-note">（ファイルをそのまま送信: {entry.requestFile ?? "—"}）</span>}
      {entry.requestBodyKind === "binary" && <span className="muted-note">（バイナリのボディ・{formatBytes(entry.requestBodySize)}。中身は保存していません）</span>}
    </pre>
  );
}

function ReceivedContent({ entry }: { entry: HistoryEntry }) {
  const problem = parseProblem(entry.responseContentType, entry.responseBody);
  if (entry.status === null) {
    const kind = entry.errorKind ?? "other";
    return (
      <div className="failure" role="group" aria-label="失敗の内容">
        <div className="failure-title">{ERROR_TITLES[kind as keyof typeof ERROR_TITLES] ?? ERROR_TITLES.other}</div>
        <div className="failure-message">{entry.error}</div>
      </div>
    );
  }
  return (
    <>
      {problem && (
        <div className="problem" role="group" aria-label="エラーの内容">
          <div className="problem-head">
            <span className="problem-badge">エラー</span>
            {problem.title && <span className="problem-title">{problem.title}</span>}
          </div>
          {problem.code && (
            <div className="problem-row">
              <span className="problem-key">code</span>
              <span className="problem-code mono">{problem.code}</span>
            </div>
          )}
          {problem.detail && (
            <div className="problem-row">
              <span className="problem-key">detail</span>
              <span className="problem-detail">{problem.detail}</span>
            </div>
          )}
        </div>
      )}
      <details className="history-headers">
        <summary>レスポンスヘッダー ({entry.responseHeaders.length})</summary>
        <div className="history-header-list">
          {entry.responseHeaders.map((h, i) => (
            <div key={i}>
              <span className="tok-key">{h.name}</span>: {h.value}
            </div>
          ))}
        </div>
      </details>
      {entry.responseBinary ? (
        <div className="notice" role="status">
          バイナリのボディです（{entry.responseContentType ?? "種類不明"}・{formatBytes(entry.responseSize)}）。履歴には中身を保存していません。
        </div>
      ) : (
        <pre className="history-pre" aria-label="受け取ったボディ" tabIndex={0}>
          {entry.responseBody === null || entry.responseBody === "" ? <span className="muted-note">（ボディなし）</span> : <JsonOrText text={entry.responseBody} />}
        </pre>
      )}
    </>
  );
}

/** 履歴 1 件の詳細（送った内容・受け取ったレスポンス）と、操作のボタン。 */
export function HistoryDetail({
  entry,
  actions,
  comparing,
  message,
  busy,
}: {
  entry: HistoryEntry;
  actions: HistoryActions;
  comparing: boolean;
  message: { ok: boolean; text: string } | null;
  busy: boolean;
}) {
  const failed = entry.status === null;
  const tone = statusTone({ status: entry.status, failed });
  const total = entry.timing?.totalMs ?? null;
  const title = entry.opSummary ?? `${entry.method} ${safePath(entry.url)}`;
  return (
    <div className="history-detail">
      <div className="history-detail-head">
        <span className="history-method is-large" style={{ color: methodColor(entry.method) }}>
          {entry.method}
        </span>
        <h2 className="history-title">{title}</h2>
        <span className={`history-badge is-${tone}`} aria-label="ステータス" title={failed ? (entry.error ?? undefined) : undefined}>
          {statusText({ status: entry.status, failed })}
        </span>
        <span className="muted-note">
          {entry.environmentName ?? "環境なし"} · {formatDateTime(entry.sentAt)}
          {total !== null ? ` · ${formatMs(total)}` : ""}
        </span>
      </div>
      <div className="history-url mono">{entry.url}</div>
      {entry.finalUrl && <div className="muted-note">最終 URL（リダイレクト先）: <span className="mono">{entry.finalUrl}</span></div>}

      <div className="history-actions" role="toolbar" aria-label="履歴の操作">
        <button type="button" className="btn btn-primary" disabled={busy || entry.input === null} onClick={actions.onResend}>
          再送信
        </button>
        <button type="button" className="btn btn-ghost" disabled={busy || entry.input === null} onClick={actions.onOpen}>
          編集して開く
        </button>
        <button type="button" className="btn btn-ghost" disabled={busy || entry.input === null} onClick={actions.onSave}>
          入力値として保存
        </button>
        <button type="button" className="btn btn-ghost" disabled={busy} onClick={actions.onCopyCurl}>
          cURL としてコピー
        </button>
        <button type="button" className="btn btn-ghost" aria-pressed={comparing} disabled={busy} onClick={actions.onToggleCompare}>
          {comparing ? "比較をやめる" : "別の履歴と比較"}
        </button>
        <button type="button" className="btn btn-danger-ghost history-delete" disabled={busy} onClick={actions.onDelete}>
          この履歴を削除
        </button>
      </div>
      {message && (
        <div className={`notice ${message.ok ? "notice-ok" : "notice-bad"}`} role={message.ok ? "status" : "alert"}>
          {message.text}
        </div>
      )}

      {entry.maskedFields.length > 0 && (
        <div className="notice" role="note">
          手で書いた認証の値は伏せ字（••••）で保存しています: {entry.maskedFields.join("、")}。再送信・編集して開くでは外れるので、入れ直してください。
        </div>
      )}
      {entry.overrideNames.length > 0 && (
        <div className="notice" role="note">
          送信のときに指定していた変数の上書き（{entry.overrideNames.join("、")}）は、値を保存していません。再送信では使えません。
        </div>
      )}

      <div className="history-columns">
        <section className="history-col" aria-label="送信した内容の欄">
          <span className="history-col-title">送信した内容</span>
          <SentContent entry={entry} />
          {entry.requestBodyTruncated && (
            <div className="notice" role="status">
              ボディが大きいため、先頭だけ保存しています（全体は {formatBytes(entry.requestBodySize)}）。
            </div>
          )}
        </section>
        <section className="history-col" aria-label="受け取ったレスポンスの欄">
          <span className="history-col-title">受け取ったレスポンス{entry.responseContentType ? `（${entry.responseContentType}）` : ""}</span>
          <ReceivedContent entry={entry} />
          {entry.responseBodyTruncated && !entry.responseBinary && (
            <div className="notice" role="status">
              先頭だけ保存しています（全体は {formatBytes(entry.responseSize)}）。
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function safePath(url: string): string {
  try {
    return new URL(url).pathname;
  } catch {
    return url;
  }
}
