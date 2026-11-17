import { useState } from "react";

import { useBackend } from "../api";
import type { Body, KeyValue, MultipartPart, RequestSpec } from "../bindings";
import { checkJson, formatJson } from "../lib/json";
import type { UnresolvedIndex } from "../lib/vars";
import type { RequestTab } from "../store/tabs";
import { CloseIcon } from "./Icons";
import { JsonEditor } from "./JsonEditor";
import { KeyValueTable } from "./KeyValueTable";
import { VarInput } from "./VarInput";

const KINDS: { kind: Body["kind"]; label: string }[] = [
  { kind: "none", label: "なし" },
  { kind: "json", label: "JSON" },
  { kind: "form", label: "フォーム" },
  { kind: "multipart", label: "multipart" },
  { kind: "text", label: "テキスト" },
  { kind: "binary", label: "バイナリ" },
];

function defaultBody(kind: Body["kind"]): Body {
  switch (kind) {
    case "none":
      return { kind: "none" };
    case "json":
      return { kind: "json", value: "" };
    case "form":
      return { kind: "form", value: [] };
    case "multipart":
      return { kind: "multipart", value: [] };
    case "text":
      return { kind: "text", value: "" };
    case "binary":
      return { kind: "binary", value: "" };
  }
}

const CONTENT_TYPE: Record<Body["kind"], string | null> = {
  none: null,
  json: "application/json",
  form: "application/x-www-form-urlencoded",
  multipart: "multipart/form-data",
  text: "text/plain; charset=utf-8",
  binary: "application/octet-stream",
};

interface Props {
  tab: RequestTab;
  onSpec: (update: (spec: RequestSpec) => RequestSpec) => void;
  onKind: (kind: Body["kind"]) => void;
  unresolved: UnresolvedIndex;
}

/** ボディのタブ。 */
export function BodyPanel({ tab, onSpec, onKind, unresolved }: Props) {
  const body = tab.spec.body;
  const setBody = (next: Body) => onSpec((s) => ({ ...s, body: next }));
  const contentType = CONTENT_TYPE[body.kind];

  return (
    <div className="panel panel-body">
      <div className="segmented" role="radiogroup" aria-label="ボディの種類">
        {KINDS.map(({ kind, label }) => (
          <button
            key={kind}
            type="button"
            role="radio"
            aria-checked={body.kind === kind}
            className="segmented-item"
            onClick={() => body.kind !== kind && onKind(kind)}
          >
            {label}
          </button>
        ))}
      </div>

      {body.kind === "none" && <p className="muted-note">このリクエストにはボディがありません。</p>}

      {body.kind === "json" && <JsonBody value={body.value} onChange={(v) => setBody({ kind: "json", value: v })} unresolved={unresolved.body} />}

      {body.kind === "form" && (
        <>
          <div className="body-toolbar">
            <span className="muted-note">Content-Type: {contentType}</span>
          </div>
          <KeyValueTable
            label="フォーム"
            location="query"
            items={body.value}
            onChange={(items: KeyValue[]) => setBody({ kind: "form", value: items })}
            unresolved={(row, part) => unresolved.bodyForm(row, part)}
          />
        </>
      )}

      {body.kind === "multipart" && (
        <>
          <div className="body-toolbar">
            <span className="muted-note">Content-Type: {contentType}（境界の文字列は自動で付きます）</span>
          </div>
          <MultipartTable parts={body.value} onChange={(parts) => setBody({ kind: "multipart", value: parts })} unresolved={unresolved} />
        </>
      )}

      {body.kind === "text" && (
        <>
          <div className="body-toolbar">
            <span className="muted-note">Content-Type: {contentType}</span>
          </div>
          <textarea
            className="text-body"
            aria-label="テキストのボディ"
            value={body.value}
            onChange={(e) => setBody({ kind: "text", value: e.target.value })}
            spellCheck={false}
          />
        </>
      )}

      {body.kind === "binary" && <BinaryBody value={body.value} onChange={(v) => setBody({ kind: "binary", value: v })} unresolved={unresolved.body} />}

      {unresolved.body.size > 0 && body.kind !== "form" && body.kind !== "multipart" && (
        <div className="alert alert-danger" role="alert">
          ボディに展開できない変数があります: {[...unresolved.body].map((n) => `{{${n}}}`).join("、")}
        </div>
      )}
    </div>
  );
}

function JsonBody({ value, onChange, unresolved }: { value: string; onChange: (v: string) => void; unresolved: ReadonlySet<string> }) {
  const [formatError, setFormatError] = useState<string | null>(null);
  const check = checkJson(value);

  const format = () => {
    const r = formatJson(value);
    if (r.ok) {
      setFormatError(null);
      onChange(r.text);
    } else {
      setFormatError(r.error);
    }
  };

  return (
    <>
      <div className="body-toolbar">
        <span className="muted-note">Content-Type: application/json</span>
        <button type="button" className="btn btn-small btn-ghost" onClick={format}>
          整形
        </button>
      </div>
      <JsonEditor
        value={value}
        onChange={(v) => {
          setFormatError(null);
          onChange(v);
        }}
        unresolved={unresolved}
        ariaLabel="JSON のボディ"
      />
      <div className="body-status">
        {formatError && (
          <div className="status-line status-bad" role="alert">
            <span aria-hidden="true">✗</span>
            {formatError}
          </div>
        )}
        {!formatError && check?.ok === true && (
          <div className="status-line status-ok">
            <span aria-hidden="true">✓</span>JSON として正しい形式です
          </div>
        )}
        {!formatError && check?.ok === false && (
          <div className="status-line status-bad">
            <span aria-hidden="true">✗</span>
            {check.error}
          </div>
        )}
        <div className="status-line">
          <span aria-hidden="true" className="var-mark">
            {"{ }"}
          </span>
          紫の値は送信時に環境変数で置き換えられます
        </div>
      </div>
    </>
  );
}

function BinaryBody({ value, onChange, unresolved }: { value: string; onChange: (v: string) => void; unresolved: ReadonlySet<string> }) {
  const backend = useBackend();
  return (
    <>
      <div className="body-toolbar">
        <span className="muted-note">Content-Type: application/octet-stream（選んだファイルの中身をそのまま送ります）</span>
      </div>
      <div className="file-row">
        <VarInput className="field" value={value} onChange={onChange} unresolved={unresolved} aria-label="送るファイルのパス" placeholder="ファイルのパス" />
        <button
          type="button"
          className="btn"
          onClick={async () => {
            const picked = await backend.pickFile();
            if (picked) onChange(picked);
          }}
        >
          ファイルを選択…
        </button>
      </div>
    </>
  );
}

function MultipartTable({
  parts,
  onChange,
  unresolved,
}: {
  parts: MultipartPart[];
  onChange: (parts: MultipartPart[]) => void;
  unresolved: UnresolvedIndex;
}) {
  const backend = useBackend();
  const update = (i: number, patch: Partial<MultipartPart>) => onChange(parts.map((p, j) => (j === i ? { ...p, ...patch } : p)));

  return (
    <div className="kv-wrap">
      <table className="kv-table">
        <thead>
          <tr>
            <th className="kv-check">
              <span className="sr-only">有効</span>
            </th>
            <th>名前</th>
            <th className="kv-kind">種類</th>
            <th>値</th>
            <th className="kv-remove">
              <span className="sr-only">削除</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {parts.map((p, i) => (
            <tr key={i} className={p.enabled ? undefined : "kv-off"}>
              <td className="kv-check">
                <input
                  type="checkbox"
                  checked={p.enabled}
                  onChange={(e) => update(i, { enabled: e.target.checked })}
                  aria-label={`multipart ${i + 1} 行目を有効にする`}
                />
              </td>
              <td>
                <VarInput
                  className="kv-input"
                  value={p.name}
                  onChange={(name) => update(i, { name })}
                  unresolved={unresolved.bodyMultipart(i + 1, "name")}
                  aria-label={`multipart ${i + 1} 行目の名前`}
                  placeholder="名前"
                />
              </td>
              <td className="kv-kind">
                <select
                  className="select select-small"
                  value={p.value.kind}
                  aria-label={`multipart ${i + 1} 行目の種類`}
                  onChange={(e) => update(i, { value: { kind: e.target.value === "file" ? "file" : "text", value: "" } })}
                >
                  <option value="text">テキスト</option>
                  <option value="file">ファイル</option>
                </select>
              </td>
              <td>
                <div className="file-row">
                  <VarInput
                    className="kv-input"
                    value={p.value.value}
                    onChange={(v) => update(i, { value: { kind: p.value.kind, value: v } })}
                    unresolved={unresolved.bodyMultipart(i + 1, "value")}
                    aria-label={`multipart ${i + 1} 行目の値`}
                    placeholder={p.value.kind === "file" ? "ファイルのパス" : "値"}
                  />
                  {p.value.kind === "file" && (
                    <button
                      type="button"
                      className="btn btn-small"
                      onClick={async () => {
                        const picked = await backend.pickFile();
                        if (picked) update(i, { value: { kind: "file", value: picked } });
                      }}
                    >
                      選択…
                    </button>
                  )}
                </div>
              </td>
              <td className="kv-remove">
                <button type="button" className="icon-btn" aria-label={`multipart ${i + 1} 行目を削除`} onClick={() => onChange(parts.filter((_, j) => j !== i))}>
                  <CloseIcon />
                </button>
              </td>
            </tr>
          ))}
          <tr className="kv-add-row">
            <td />
            <td colSpan={4}>
              <button
                type="button"
                className="kv-add"
                aria-label="multipart を追加"
                onClick={() => onChange([...parts, { name: "", value: { kind: "text", value: "" }, enabled: true }])}
              >
                ＋ 名前を追加
              </button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  );
}

export { defaultBody };
