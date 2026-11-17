import type { ApiKeyIn, AuthOverride, AutoHeader, RequestSpec } from "../bindings";
import type { UnresolvedIndex } from "../lib/vars";
import type { RequestTab } from "../store/tabs";
import { LockIcon } from "./Icons";
import { VarInput } from "./VarInput";

type Kind = "default" | AuthOverride["type"];

const KINDS: { id: Kind; label: string; sub: string; disabled?: boolean }[] = [
  { id: "default", label: "プロジェクトの既定を使う", sub: "M3 で OpenAPI の security が入ると使えます", disabled: true },
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
  onKind: (type: AuthOverride["type"]) => void;
  unresolved: UnresolvedIndex;
  autoHeaders: AutoHeader[];
}

/** 認証のタブ（なし・API キー・Bearer・Basic）。 */
export function AuthPanel({ tab, onSpec, onKind, unresolved, autoHeaders }: Props) {
  const auth = tab.spec.auth ?? { type: "none" as const };
  const setAuth = (next: AuthOverride) => onSpec((s) => ({ ...s, auth: next }));
  const sent = autoHeaders.filter((h) => h.source.includes("認証"));

  return (
    <div className="auth-layout">
      <div className="auth-kinds" role="radiogroup" aria-label="認証方式">
        <span className="auth-kinds-title">認証方式</span>
        {KINDS.map((k) => (
          <label key={k.id} className={`auth-kind${auth.type === k.id ? " is-on" : ""}${k.disabled ? " is-disabled" : ""}`}>
            <input
              type="radio"
              name="auth-kind"
              checked={auth.type === k.id}
              disabled={k.disabled}
              onChange={() => k.id !== "default" && onKind(k.id)}
            />
            <span className="auth-kind-text">
              <span>{k.label}</span>
              <span className="auth-kind-sub">{k.sub}</span>
            </span>
          </label>
        ))}
      </div>

      <div className="auth-fields">
        {auth.type === "none" && <p className="muted-note">認証を付けずに送ります。</p>}

        {auth.type === "apiKey" && (
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

        {auth.type === "bearer" && (
          <div className="field-label">
<span>トークン</span><VarInput className="field" aria-label="トークン" value={auth.token} onChange={(token) => setAuth({ ...auth, token })} unresolved={unresolved.auth("token")} placeholder="{{token}}" />
          </div>
        )}

        {auth.type === "basic" && (
          <div className="field-grid">
            <div className="field-label">
<span>ユーザー名</span><VarInput className="field" aria-label="ユーザー名" value={auth.username} onChange={(username) => setAuth({ ...auth, username })} unresolved={unresolved.auth("username")} placeholder="{{username}}" />
            </div>
            <div className="field-label">
<span>パスワード</span><VarInput className="field" aria-label="パスワード" value={auth.password} onChange={(password) => setAuth({ ...auth, password })} unresolved={unresolved.auth("password")} placeholder="{{password}}" />
            </div>
          </div>
        )}

        {auth.type !== "none" && (
          <div className="field-label">
            送信されるヘッダー
            <div className="code-box">
              {sent.length === 0 ? (
                <span className="muted-note">ヘッダーには付きません（クエリ・Cookie、または値が空です）</span>
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
