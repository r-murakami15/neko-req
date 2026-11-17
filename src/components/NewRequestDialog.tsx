import * as Dialog from "@radix-ui/react-dialog";
import { useEffect, useRef, useState } from "react";

import { errorMessage, useBackend } from "../api";
import type { CurlImport, CurlWarning, WarningKind } from "../bindings";
import { emptySpec, NEW_REQUEST_NAME } from "../lib/spec";
import { opInfoOf, opTabName, useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";
import { DialogFrame } from "./Dialog";

type CurlState = { kind: "idle" } | { kind: "parsing" } | { kind: "ok"; result: CurlImport } | { kind: "error"; message: string };

const PARSE_DEBOUNCE_MS = 300;

const WARNING_LABEL: Record<WarningKind, string> = {
  ignored: "無視",
  unknown: "不明",
  unsupported: "未対応",
  adjusted: "読み替え",
};

function bodyLabel(kind: CurlImport["request"]["body"]["kind"]): string {
  switch (kind) {
    case "json":
      return "JSON ボディ";
    case "form":
      return "フォームのボディ";
    case "multipart":
      return "multipart のボディ";
    case "text":
      return "テキストのボディ";
    case "binary":
      return "バイナリのボディ";
    default:
      return "ボディなし";
  }
}

/** 入力のたびに（少し待って）cURL を解析する。何も保存しない。 */
function useCurlParse(projectId: string, text: string, enabled: boolean): CurlState {
  const backend = useBackend();
  const [state, setState] = useState<CurlState>({ kind: "idle" });
  const seq = useRef(0);
  useEffect(() => {
    const mine = (seq.current += 1);
    if (!enabled || text.trim() === "") {
      setState({ kind: "idle" });
      return;
    }
    setState({ kind: "parsing" });
    const timer = setTimeout(() => {
      backend
        .curlParse(projectId, text, null)
        .then((result) => {
          if (mine === seq.current) setState({ kind: "ok", result });
        })
        .catch((e: unknown) => {
          if (mine === seq.current) setState({ kind: "error", message: errorMessage(e) });
        });
    }, PARSE_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [backend, projectId, text, enabled]);
  return state;
}

/** 解析の結果（どの API に当てはまったか・「その他」になるか）と警告。 */
function CurlResult({ result }: { result: CurlImport }) {
  const { request } = result;
  const key = result.opKey ? (result.opKey.kind === "id" ? result.opKey.value : `${result.opKey.value.method} ${result.opKey.value.path}`) : null;
  return (
    <>
      <div className="curl-result is-ok" role="status" aria-label="cURL の解析結果">
        <span aria-hidden="true">✓</span>
        <span>
          {result.target === "operation" ? (
            <>
              {request.method} {result.matchedPath} · {bodyLabel(request.body.kind)} · OpenAPI の <span className="mono">{key}</span> と一致
            </>
          ) : (
            <>
              「その他」の新しいリクエストとして開きます · {request.method} <span className="mono curl-url">{request.url}</span> · {bodyLabel(request.body.kind)}
            </>
          )}
          {result.baseReplaced && <span className="muted-note">（ホストを {"{{baseUrl}}"} に置き換えました）</span>}
        </span>
      </div>
      {result.warnings.length > 0 && (
        <ul className="curl-warnings" aria-label="cURL の警告">
          {result.warnings.map((w: CurlWarning, i) => (
            <li key={`${w.option}-${i}`}>
              <span className="badge">{WARNING_LABEL[w.kind]}</span>
              <span>{w.message}</span>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

/** タブの「＋」・サイドバーの「＋」から開く「新規作成」。 */
export function NewRequestDialog({ projectId }: { projectId: string }) {
  const backend = useBackend();
  const open = useUiStore((s) => s.newRequestDialogOpen);
  const setOpen = useUiStore((s) => s.setNewRequestDialogOpen);
  const setCreateDialogOpen = useUiStore((s) => s.setCreateDialogOpen);
  const setImportDialogOpen = useUiStore((s) => s.setImportDialogOpen);
  const openNew = useTabsStore((s) => s.openNew);
  const [text, setText] = useState("");
  const [creating, setCreating] = useState(false);
  const curl = useCurlParse(projectId, text, open);

  useEffect(() => {
    if (!open) setText("");
  }, [open]);

  /** 当てはまった API のタブ（未保存の入力として）か、新しい「その他」のタブで開く。 */
  const createFromCurl = async (result: CurlImport) => {
    setCreating(true);
    try {
      const tabs = useTabsStore.getState();
      if (result.target === "operation" && result.opKey) {
        try {
          const opened = await backend.operationOpen(projectId, result.opKey);
          tabs.openSeed(projectId, { name: opTabName(opened), spec: result.request, baseSpec: opened.request, op: opInfoOf(opened) });
          setOpen(false);
          return;
        } catch {
          // API が見つからなければ、「その他」として開く
        }
      }
      tabs.openSeed(projectId, { name: result.suggestedName || NEW_REQUEST_NAME, spec: result.request, baseName: NEW_REQUEST_NAME, baseSpec: emptySpec() });
      setOpen(false);
    } finally {
      setCreating(false);
    }
  };

  return (
    <DialogFrame
      open={open}
      onOpenChange={setOpen}
      title="新規作成"
      wide
      footer={
        <>
          <Dialog.Close className="btn" type="button">
            キャンセル
          </Dialog.Close>
          <button
            type="button"
            className="btn btn-primary"
            disabled={curl.kind !== "ok" || creating}
            onClick={() => curl.kind === "ok" && void createFromCurl(curl.result)}
          >
            cURL から作成
          </button>
        </>
      }
    >
      <div className="card-grid">
        <button
          type="button"
          className="card is-primary"
          onClick={() => {
            openNew(projectId);
            setOpen(false);
          }}
        >
          <span className="card-title">HTTP リクエスト</span>
          <span className="card-sub">空のリクエストを新しいタブで開く</span>
        </button>
        <button
          type="button"
          className="card"
          onClick={() => {
            setOpen(false);
            setCreateDialogOpen(true);
          }}
        >
          <span className="card-title">プロジェクト</span>
          <span className="card-sub">新しいプロジェクトを作る</span>
        </button>
        <button
          type="button"
          className="card"
          onClick={() => {
            setOpen(false);
            setImportDialogOpen(true);
          }}
        >
          <span className="card-title">OpenAPI から</span>
          <span className="card-sub">YAML / JSON を取り込む</span>
        </button>
      </div>

      <label className="field-label">
        <span>cURL コマンドから作成</span>
        <textarea
          className="curl-input"
          rows={5}
          aria-label="cURL コマンド"
          spellCheck={false}
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={"curl -X POST http://localhost:8080/api/v1/auth/password-login \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"email\":\"user@example.com\"}'"}
        />
      </label>
      {curl.kind === "parsing" && (
        <div className="curl-result" role="status">
          解析しています…
        </div>
      )}
      {curl.kind === "error" && (
        <div className="curl-result is-bad" role="alert">
          <span aria-hidden="true">!</span>
          <span>{curl.message}</span>
        </div>
      )}
      {curl.kind === "ok" && <CurlResult result={curl.result} />}
    </DialogFrame>
  );
}
