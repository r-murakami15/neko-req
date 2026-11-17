import type { ApiKeyIn, AuthOverride, AutoHeader, DefaultAuth, RequestSpec } from "../bindings";
import type { UnresolvedIndex } from "../lib/vars";
import type { RequestTab } from "../store/tabs";
import { InfoIcon, LockIcon } from "./Icons";
import { VarInput } from "./VarInput";

export type AuthKind = "default" | AuthOverride["type"];

const KINDS: { id: AuthKind; label: string; sub: string }[] = [
  { id: "default", label: "プロジェクトの既定を使う", sub: "" },
  { id: "none", label: "なし", sub: "認証を付けずに送る" },
  { id: "apiKey", label: "API キー", sub: "ヘッダー / クエリ / Cookie" },
  { id: "bearer", label: "Bearer トークン", sub: "Authorization ヘッダー" },
  { id: "basic", label: "Basic 認証", sub: "ユーザー名とパスワード" },
];

export function defaultAuth(type: AuthOverride["type"]): AuthOverride {
  switch (type) {
    case "none":
      return { type: "none" };
    case "apiKey":
      return { type: "apiKey", location: "header", name: "", value: "" };
    case "bearer":
      return { type: "bearer", token: "" };
    case "basic":
      return { type: "basic", username: "", password: "" };
  }
}

const API_KEY_IN: { value: ApiKeyIn; label: string }[] = [
  { value: "header", label: "ヘッダー" },
  { value: "query", label: "クエリ" },
  { value: "cookie", label: "Cookie" },
];

interface Props {
  tab: RequestTab;
  onSpec: (update: (spec: RequestSpec) => RequestSpec) => void;
  onKind: (kind: AuthKind) => void;
  unresolved: UnresolvedIndex;
  autoHeaders: AutoHeader[];
}

const API_KEY_IN_LABEL: Record<ApiKeyIn, string> = { header: "ヘッダー", query: "クエリ", cookie: "Cookie" };

/** yaml の security から決まる既定の認証の、選択肢の説明。 */
function defaultSub(auth: DefaultAuth | null): string {
  if (auth === null) return "security: []（この API は認証なし）";
  return `security: ${auth.scheme}`;
}

/** 「プロジェクトの既定を使う」のときの、yaml の security の内容（表示専用）。 */
function DefaultAuthView({ auth }: { auth: DefaultAuth | null }) {
  if (auth === null) {
    return <p className="muted-note">この API は security: []（認証なし）なので、認証を付けずに送ります。</p>;
  }
  return (
    <>
      <div className="auth-source" role="status">
        <InfoIcon />
        OpenAPI の securitySchemes「{auth.scheme}」から設定されました
      </div>
      {auth.type === "apiKey" && (
        <>
          <div className="field-grid">
            <div className="field-label">
              <span>送り先</span>
              <input className="field" aria-label="送り先" value={API_KEY_IN_LABEL[auth.location]} readOnly />
            </div>
            <div className="field-label">
              <span>名前</span>
              <input className="field mono" aria-label="名前" value={auth.name} readOnly />
            </div>
          </div>
          <div className="field-label">
            <span>値</span>
            <input className="field mono" aria-label="値" value={`{{${auth.variable}}}`} readOnly />
          </div>
        </>
      )}
      {auth.type === "bearer" && (
        <div className="field-label">
          <span>トークン</span>
          <input className="field mono" aria-label="トークン" value={`{{${auth.variable}}}`} readOnly />
        </div>
      )}
      {auth.type === "basic" && (
        <div className="field-grid">
          <div className="field-label">
            <span>ユーザー名</span>
            <input className="field mono" aria-label="ユーザー名" value={`{{${auth.usernameVariable}}}`} readOnly />
          </div>
          <div className="field-label">
            <span>パスワード</span>
            <input className="field mono" aria-label="パスワード" value={`{{${auth.passwordVariable}}}`} readOnly />
          </div>
        </div>
      )}
      <p className="muted-note">
        値は環境変数から読みます。変数が展開できないときは、認証なしで送ります。上書きするときは、左のほかの認証方式を選んでください。
      </p>
    </>
  );
}

/** 認証のタブ（プロジェクトの既定・なし・API キー・Bearer・Basic）。 */
export function AuthPanel({ tab, onSpec, onKind, unresolved, autoHeaders }: Props) {
  const op = tab.op;
  // yaml の API では、`auth` が空（null）なら「プロジェクトの既定を使う」。そうでないタブでは、空は「なし」
  const kind: AuthKind = op && tab.spec.auth === null ? "default" : (tab.spec.auth?.type ?? "none");
  const auth = tab.spec.auth ?? { type: "none" as const };
  const setAuth = (next: AuthOverride) => onSpec((s) => ({ ...s, auth: next }));
  const sent = autoHeaders.filter((h) => h.source.includes("認証"));

  return (
    <div className="auth-layout">
      <div className="auth-kinds" role="radiogroup" aria-label="認証方式">
        <span className="auth-kinds-title">認証方式</span>
        {KINDS.map((k) => {
          const disabled = k.id === "default" && !op;
          const sub = k.id === "default" ? (op ? defaultSub(op.defaultAuth) : "OpenAPI の API のタブで使えます") : k.sub;
          return (
            <label key={k.id} className={`auth-kind${kind === k.id ? " is-on" : ""}${disabled ? " is-disabled" : ""}`}>
              <input type="radio" name="auth-kind" checked={kind === k.id} disabled={disabled} onChange={() => onKind(k.id)} />
              <span className="auth-kind-text">
                <span>{k.label}</span>
                <span className="auth-kind-sub">{sub}</span>
              </span>
            </label>
          );
        })}
      </div>

      <div className="auth-fields">
        {kind === "default" && op && <DefaultAuthView auth={op.defaultAuth} />}

        {kind === "none" && <p className="muted-note">認証を付けずに送ります。</p>}

        {kind !== "default" && auth.type === "apiKey" && (
          <>
            <div className="field-grid">
              <div className="field-label">
<span>送り先</span><select
                  aria-label="送り先"
                  className="select select-tall"
                  value={auth.location}
                  onChange={(e) => setAuth({ ...auth, location: e.target.value as ApiKeyIn })}
                >
                  {API_KEY_IN.map((o) => (
                    <option key={o.value} value={o.value}>
                      {o.label}
                    </option>
                  ))}
                </select>
              </div>
              <div className="field-label">
<span>名前</span><VarInput className="field" aria-label="名前" value={auth.name} onChange={(name) => setAuth({ ...auth, name })} unresolved={unresolved.auth("name")} placeholder="X-API-Key" />
              </div>
            </div>
            <div className="field-label">
<span>値</span><VarInput className="field" aria-label="値" value={auth.value} onChange={(value) => setAuth({ ...auth, value })} unresolved={unresolved.auth("value")} placeholder="{{apiKey}}" />
            </div>
          </>
        )}

        {kind !== "default" && auth.type === "bearer" && (
          <div className="field-label">
<span>トークン</span><VarInput className="field" aria-label="トークン" value={auth.token} onChange={(token) => setAuth({ ...auth, token })} unresolved={unresolved.auth("token")} placeholder="{{token}}" />
          </div>
        )}

        {kind !== "default" && auth.type === "basic" && (
          <div className="field-grid">
            <div className="field-label">
<span>ユーザー名</span><VarInput className="field" aria-label="ユーザー名" value={auth.username} onChange={(username) => setAuth({ ...auth, username })} unresolved={unresolved.auth("username")} placeholder="{{username}}" />
            </div>
            <div className="field-label">
<span>パスワード</span><VarInput className="field" aria-label="パスワード" value={auth.password} onChange={(password) => setAuth({ ...auth, password })} unresolved={unresolved.auth("password")} placeholder="{{password}}" />
            </div>
          </div>
        )}

        {(kind === "default" ? op?.defaultAuth != null : kind !== "none") && (
          <div className="field-label">
            送信されるヘッダー
            <div className="code-box">
              {sent.length === 0 ? (
                <span className="muted-note">
                  {kind === "default" ? "変数が展開できないので、認証なしで送ります" : "ヘッダーには付きません（クエリ・Cookie、または値が空です）"}
                </span>
              ) : (
                sent.map((h, i) => (
                  <div key={`${h.name}-${i}`}>
                    {h.name}: {h.value}
                  </div>
                ))
              )}
            </div>
          </div>
        )}

        <div className="lock-note">
          <LockIcon />
          値は {"{{変数}}"} で書きます。シークレットの値は OS のキーチェーンに保存され、保存するリクエストには書き込まれません
        </div>
      </div>
    </div>
  );
}
