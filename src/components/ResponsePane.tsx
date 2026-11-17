import { useMemo, useState } from "react";

import { errorMessage, useBackend } from "../api";
import { useProjects } from "../api/queries";
import type { ResponseData, SendError } from "../bindings";
import { useCopy } from "../lib/clipboard";
import { formatBytes, formatMs } from "../lib/format";
import { prettyResponseJson, tokenizeJson } from "../lib/json";
import { parseProblem } from "../lib/problem";
import { describeLocation } from "../lib/vars";
import { type BodyMode, type RequestTab, type ResponsePane as Pane, useTabsStore } from "../store/tabs";
import { CloseIcon, SearchIcon } from "./Icons";

/** この文字数を超える JSON は、色分けせずにそのまま見せる（重くならないように）。 */
const MAX_HIGHLIGHT_CHARS = 300_000;

export const ERROR_TITLES: Record<SendError["kind"], string> = {
  unresolved: "展開できない変数があるため、送信していません",
  invalidRequest: "リクエストの形が正しくないため、送信していません",
  bodyFile: "ボディのファイルを読めないため、送信していません",
  connect: "接続できません",
  tls: "TLS（証明書）のエラー",
  timeout: "タイムアウトしました",
  cancelled: "送信を中止しました",
  tooLarge: "ボディが大きすぎます",
  other: "送信に失敗しました",
};

function statusClass(status: number): string {
  if (status >= 500) return "status-5xx";
  if (status >= 400) return "status-4xx";
  if (status >= 300) return "status-3xx";
  return "status-2xx";
}

function fileNameFor(contentType: string | null): string {
  const ct = (contentType ?? "").toLowerCase();
  if (ct.includes("json")) return "response.json";
  if (ct.includes("html")) return "response.html";
  if (ct.includes("xml")) return "response.xml";
  if (ct.includes("pdf")) return "response.pdf";
  if (ct.startsWith("text/")) return "response.txt";
  return "response.bin";
}

/** レスポンス欄を閉じるボタン。 */
function CloseResponseButton({ onClose }: { onClose: () => void }) {
  return (
    <button type="button" className="icon-btn response-close-btn" aria-label="レスポンスを閉じる" title="レスポンスを閉じる" onClick={onClose}>
      <CloseIcon />
    </button>
  );
}

/** 送信前・送信中・失敗のときの見出し（成功のときは、ステータスなどの行にボタンを置く）。 */
function ResponseHead({ onClose }: { onClose: () => void }) {
  return (
    <div className="response-bar response-head">
      <span className="response-title">レスポンス</span>
      <CloseResponseButton onClose={onClose} />
    </div>
  );
}

/** レスポンス欄。送信前・送信中・失敗・成功（ボディ / ヘッダー / タイミング）を出し分ける。`onClose` で欄ごと閉じる。 */
export function ResponsePane({ tab, onClose }: { tab: RequestTab; onClose: () => void }) {
  const { send } = tab;

  if (send.phase === "idle") {
    return (
      <section className="pane pane-response" aria-label="レスポンス">
        <ResponseHead onClose={onClose} />
        <div className="pane-placeholder">
          送信すると、ステータス・時間・サイズとレスポンスの 3 タブ（ボディ・ヘッダー・タイミング）がここに出ます。
        </div>
      </section>
    );
  }
  if (send.phase === "sending" || send.outcome === null) {
    return (
      <section className="pane pane-response" aria-label="レスポンス">
        <ResponseHead onClose={onClose} />
        <div className="pane-placeholder" role="status">
          {send.cancelRequested ? "中止しています…" : "送信しています…"}
        </div>
      </section>
    );
  }
  if (send.outcome.status === "failure") {
    const { error } = send.outcome;
    return (
      <section className="pane pane-response" aria-label="レスポンス">
        <ResponseHead onClose={onClose} />
        <div className={`failure${error.kind === "cancelled" ? " failure-quiet" : ""}`} role="alert">
          <div className="failure-title">{ERROR_TITLES[error.kind]}</div>
          <div className="failure-message">{error.message}</div>
          {error.kind === "unresolved" && (
            <ul className="failure-list">
              {error.vars.map((v, i) => (
                <li key={i}>
                  <span className="mono">{`{{${v.name}}}`}</span>（{describeLocation(v.location)}）
                </li>
              ))}
            </ul>
          )}
        </div>
      </section>
    );
  }
  return <SuccessView tab={tab} response={send.outcome.response} onClose={onClose} />;
}

/** 送信の結果で自動更新した変数（session・xsrfToken）と、更新できなかった理由。 */
function AutoUpdateNotices({ tab }: { tab: RequestTab }) {
  const { autoUpdates, autoUpdateSkipped } = tab.send;
  if (autoUpdates.length === 0 && autoUpdateSkipped.length === 0) return null;
  return (
    <div className="auto-updates" role="status" aria-label="認証の自動更新">
      {autoUpdates.map((u) => (
        <div key={`${u.name}-${u.environmentId}-${u.action}`} className="auto-update is-ok">
          <span aria-hidden="true">◆</span>
          <span>
            <span className="mono">{u.name}</span> を{u.action === "cleared" ? "消しました" : "更新しました"}（{u.environmentName}）
          </span>
        </div>
      ))}
      {autoUpdateSkipped.map((u) => (
        <div key={`${u.name}-${u.reason}`} className="auto-update is-warn">
          <span aria-hidden="true">!</span>
          <span>更新できませんでした: {u.reason}</span>
        </div>
      ))}
    </div>
  );
}

function SuccessView({ tab, response, onClose }: { tab: RequestTab; response: ResponseData; onClose: () => void }) {
  const patch = useTabsStore((s) => s.patch);
  const projects = useProjects();
  const autoCsrf = projects.data?.find((p) => p.id === tab.projectId)?.settings?.autoCsrf ?? true;
  const pane = tab.responsePane;
  const tabs: { id: Pane; label: string }[] = [
    { id: "body", label: "ボディ" },
    { id: "headers", label: `ヘッダー (${response.headers.length})` },
    { id: "timing", label: "タイミング" },
  ];

  return (
    <section className="pane pane-response" aria-label="レスポンス">
      <AutoUpdateNotices tab={tab} />
      <div className="response-bar">
        <span className={`status-chip ${statusClass(response.status)}`} aria-label="ステータス">
          {`${response.status} ${response.statusText}`.trim()}
        </span>
        <span className="response-meta" aria-label="時間">
          {formatMs(response.timing.totalMs)}
        </span>
        <span className="response-meta" aria-label="サイズ">
          {formatBytes(response.body.size)}
        </span>
        <div className="response-tabs" role="tablist" aria-label="レスポンス表示">
          {tabs.map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              aria-selected={pane === t.id}
              className="pane-tab"
              onClick={() => patch(tab.id, { responsePane: t.id })}
            >
              {t.label}
            </button>
          ))}
        </div>
        <CloseResponseButton onClose={onClose} />
      </div>
      <div className="response-content" role="tabpanel">
        {pane === "body" && <BodyView tab={tab} response={response} />}
        {pane === "headers" && <HeadersView response={response} xsrfNote={autoCsrf} />}
        {pane === "timing" && <TimingView tab={tab} response={response} />}
      </div>
    </section>
  );
}

function BodyView({ tab, response }: { tab: RequestTab; response: ResponseData }) {
  const backend = useBackend();
  const patch = useTabsStore((s) => s.patch);
  const [copied, copy] = useCopy();
  const [saveMessage, setSaveMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const body = response.body;
  const binary = body.encoding === "base64";

  const pretty = useMemo(() => (binary ? null : prettyResponseJson(body.content)), [binary, body.content]);
  const mode: BodyMode = tab.bodyMode ?? (pretty !== null ? "json" : "raw");
  const shown = mode === "json" && pretty !== null ? pretty : body.content;
  const tokens = useMemo(
    () => (mode === "json" && pretty !== null && pretty.length <= MAX_HIGHLIGHT_CHARS ? tokenizeJson(pretty) : null),
    [mode, pretty],
  );
  const problem = useMemo(() => parseProblem(body.contentType, body.content), [body.contentType, body.content]);

  const saveToFile = async () => {
    setSaveMessage(null);
    try {
      const path = await backend.pickSavePath(fileNameFor(body.contentType));
      if (!path || tab.send.sendId === null) return;
      await backend.responseSaveBody(tab.send.sendId, path);
      setSaveMessage({ ok: true, text: `保存しました: ${path}` });
    } catch (e) {
      setSaveMessage({ ok: false, text: errorMessage(e) });
    }
  };

  return (
    <div className="body-view">
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

      {(body.truncated || binary) && (
        <div className="notice" role="status">
          <span>
            {binary
              ? `バイナリのボディです（${body.contentType ?? "種類不明"}・${formatBytes(body.size)}）。画面には表示せず、ファイルに保存できます。`
              : `先頭だけ表示しています（全体は ${formatBytes(body.size)}）`}
          </span>
          <button type="button" className="btn btn-small" onClick={() => void saveToFile()}>
            ファイルに保存
          </button>
        </div>
      )}
      {saveMessage && (
        <div className={`notice ${saveMessage.ok ? "notice-ok" : "notice-bad"}`} role={saveMessage.ok ? "status" : "alert"}>
          {saveMessage.text}
        </div>
      )}

      {!binary && (
        <>
          <div className="body-controls">
            <div className="mode-switch" role="group" aria-label="表示形式">
              <button
                type="button"
                className="mode-btn"
                aria-pressed={mode === "json"}
                disabled={pretty === null}
                title={pretty === null ? "JSON として読めないため、Raw だけ表示できます" : undefined}
                onClick={() => patch(tab.id, { bodyMode: "json" })}
              >
                JSON
              </button>
              <button type="button" className="mode-btn" aria-pressed={mode === "raw"} onClick={() => patch(tab.id, { bodyMode: "raw" })}>
                Raw
              </button>
            </div>
            <button type="button" className="mode-btn mode-copy" onClick={() => void copy(shown)}>
              {copied ? "コピーしました" : "コピー"}
            </button>
          </div>
          <pre className={`code-view${mode === "raw" ? " is-raw" : ""}`} aria-label="レスポンスのボディ" tabIndex={0}>
            {tokens
              ? tokens.map((t, i) =>
                  t.kind === "plain" ? <span key={i}>{t.text}</span> : <span key={i} className={`tok-${t.kind}`}>{t.text}</span>,
                )
              : shown}
          </pre>
        </>
      )}
    </div>
  );
}

function HeadersView({ response, xsrfNote }: { response: ResponseData; xsrfNote: boolean }) {
  const [filter, setFilter] = useState("");
  // Set-Cookie で XSRF-TOKEN を受け取った（値は伏せ字で届く）
  const setsXsrf = response.headers.some((h) => h.name.toLowerCase() === "set-cookie" && h.value.trimStart().startsWith("XSRF-TOKEN="));
  const [copied, copy] = useCopy();
  const q = filter.trim().toLowerCase();
  const rows = response.headers.filter((h) => q === "" || h.name.toLowerCase().includes(q) || h.value.toLowerCase().includes(q));

  return (
    <div className="headers-view">
      <div className="headers-bar">
        <label className="search-field">
          <SearchIcon />
          <input aria-label="ヘッダーを絞り込み" placeholder="ヘッダーを絞り込み" value={filter} onChange={(e) => setFilter(e.target.value)} />
        </label>
        <button
          type="button"
          className="btn btn-ghost"
          onClick={() => void copy(response.headers.map((h) => `${h.name}: ${h.value}`).join("\n"))}
        >
          {copied ? "コピーしました" : "すべてコピー"}
        </button>
      </div>
      <div className="kv-wrap">
        <table className="kv-table headers-table">
          <thead>
            <tr>
              <th className="headers-name">名前</th>
              <th>値</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((h, i) => (
              <tr key={i}>
                <td className="mono tok-key">{h.name}</td>
                <td className="mono headers-value">{h.value}</td>
              </tr>
            ))}
            {rows.length === 0 && (
              <tr>
                <td colSpan={2} className="headers-empty">
                  一致するヘッダーはありません
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {xsrfNote && setsXsrf && (
        <div className="headers-note" role="note">
          <span aria-hidden="true" className="headers-note-mark">
            ◆
          </span>
          XSRF-TOKEN は保持され、次の GET 以外のリクエストで X-XSRF-TOKEN ヘッダーに自動で入ります
        </div>
      )}
    </div>
  );
}

const PHASES: { key: "dnsMs" | "connectMs" | "tlsMs" | "sendMs" | "waitMs" | "downloadMs"; label: string; accent?: boolean }[] = [
  { key: "dnsMs", label: "DNS 解決" },
  { key: "connectMs", label: "TCP 接続" },
  { key: "tlsMs", label: "TLS ハンドシェイク" },
  { key: "sendMs", label: "リクエスト送信" },
  { key: "waitMs", label: "応答待ち（TTFB）", accent: true },
  { key: "downloadMs", label: "ダウンロード" },
];

function hostOf(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}

function TimingView({ tab, response }: { tab: RequestTab; response: ResponseData }) {
  const { timing, connection } = response;
  const total = Math.max(timing.totalMs, 0.0001);
  let at = 0;
  const bars = PHASES.map((p) => {
    const ms = timing[p.key];
    const left = (at / total) * 100;
    const width = ms === null || ms === 0 ? 0 : Math.max(0.7, (ms / total) * 100);
    at += ms ?? 0;
    return { ...p, ms, left, width };
  });
  const host = hostOf(response.finalUrl);

  return (
    <div className="timing-view">
      <div className="timing-total">
        <span className="timing-total-value">{formatMs(timing.totalMs)}</span>
        <span className="muted-note">合計（{host ? `${host}・` : ""}新規接続）</span>
      </div>

      <div className="timing-rows">
        {bars.map((b) => (
          <div key={b.key} className="timing-row">
            <span className="timing-label">{b.label}</span>
            <span className="timing-track" aria-hidden="true">
              <span className={`timing-bar${b.accent ? " is-accent" : ""}`} style={{ left: `${b.left}%`, width: `${b.width}%` }} />
            </span>
            <span className="timing-ms mono">{b.ms === null ? "—" : formatMs(b.ms)}</span>
          </div>
        ))}
      </div>

      <dl className="info-list">
        <dt>接続先</dt>
        <dd className="mono">{connection.remoteAddr ? `${connection.remoteAddr}${host ? `（${host}）` : ""}` : "—"}</dd>
        <dt>HTTP バージョン</dt>
        <dd>{connection.httpVersion}</dd>
        <dt>TLS</dt>
        <dd>{connection.tls ? "あり（https）" : "なし（http）"}</dd>
        {tab.send.envName && (
          <>
            <dt>環境</dt>
            <dd>{tab.send.envName}</dd>
          </>
        )}
        {response.redirectCount > 0 && (
          <>
            <dt>最終 URL</dt>
            <dd className="mono">
              {response.finalUrl}（リダイレクト {response.redirectCount} 回）
            </dd>
          </>
        )}
        <dt>送信 / 受信</dt>
        <dd>
          {formatBytes(response.sentBytes)} / {formatBytes(response.receivedBytes)}
        </dd>
      </dl>
    </div>
  );
}
