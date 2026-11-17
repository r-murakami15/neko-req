import { useState } from "react";

import { errorMessage, useBackend } from "../../api";
import type { AppSettings, ClientCertSetting, ProxyMode } from "../../bindings";
import { CommitText, Row } from "./controls";

const MODES: { id: ProxyMode; label: string }[] = [
  { id: "system", label: "OSの設定を使う" },
  { id: "none", label: "使わない" },
  { id: "manual", label: "手動" },
];

const fileName = (path: string) => path.split(/[\\/]/).pop() || path;

interface Props {
  settings: AppSettings;
  /** 設定を変えて保存する（保存した設定を返す。保存できなければ `null`）。 */
  save: (change: (s: AppSettings) => AppSettings) => Promise<AppSettings | null>;
  /** パスワードを保存した結果（`hasPassword` の印）を反映する。 */
  onSaved: (settings: AppSettings) => void;
}

/** 「プロキシ・証明書」。プロキシ（OS の設定・使わない・手動）、CA 証明書、クライアント証明書。 */
export function NetworkSection({ settings, save, onSaved }: Props) {
  const backend = useBackend();
  const { proxy, caCertPath, clientCerts } = settings.network;
  // 「手動」を選んだが、URL をまだ入れていない（空の URL は保存できないので、入力するまで保存しない）
  const [manualPending, setManualPending] = useState(false);
  const mode: ProxyMode = manualPending ? "manual" : proxy.mode;
  const [proxyPassword, setProxyPassword] = useState("");
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  const setProxy = (patch: Partial<AppSettings["network"]["proxy"]>) =>
    save((s) => ({ ...s, network: { ...s.network, proxy: { ...s.network.proxy, ...patch } } }));

  const pickMode = (next: ProxyMode) => {
    if (next === "manual" && proxy.url.trim() === "") {
      setManualPending(true);
      return;
    }
    setManualPending(false);
    void setProxy({ mode: next });
  };

  const run = async (action: () => Promise<void>) => {
    setMessage(null);
    try {
      await action();
    } catch (e) {
      setMessage({ ok: false, text: errorMessage(e) });
    }
  };

  const savePassword = (password: string) =>
    run(async () => {
      const saved = await backend.proxyPasswordSet(password);
      onSaved(saved);
      setProxyPassword("");
      setMessage({ ok: true, text: password === "" ? "プロキシのパスワードを消しました" : "プロキシのパスワードを OS のキーチェーンに保存しました" });
    });

  const pickCa = () =>
    run(async () => {
      const path = await backend.pickCertFile("ca");
      if (path !== null) await save((s) => ({ ...s, network: { ...s.network, caCertPath: path } }));
    });

  return (
    <div className="settings-grid">
      <Row label="プロキシ">
        <div role="radiogroup" aria-label="プロキシ" className="settings-radios">
          {MODES.map((m) => (
            <label key={m.id} className="settings-radio">
              <input type="radio" name="proxy-mode" checked={mode === m.id} onChange={() => pickMode(m.id)} />
              {m.label}
            </label>
          ))}
        </div>
      </Row>

      {mode === "manual" && (
        <>
          <CommitText
            label="プロキシの URL"
            value={proxy.url}
            mono
            width={320}
            placeholder="http://proxy.example:8080"
            hint="http:// か https:// から。user:pass@ は書かず、下の欄に入力します"
            onCommit={(url) => {
              setManualPending(false);
              void setProxy({ mode: "manual", url });
            }}
          />
          <CommitText label="ユーザー名" value={proxy.username} width={200} onCommit={(username) => void setProxy({ username })} />
          <Row label="パスワード" hint={proxy.hasPassword ? "保存済み（OS のキーチェーン）" : "未設定"}>
            <span className="settings-inline">
              <input
                className="input input-small"
                type="password"
                aria-label="プロキシのパスワード"
                autoComplete="off"
                value={proxyPassword}
                onChange={(e) => setProxyPassword(e.target.value)}
              />
              <button type="button" className="btn btn-small" disabled={proxyPassword === ""} onClick={() => void savePassword(proxyPassword)}>
                パスワードを保存
              </button>
              {proxy.hasPassword && (
                <button type="button" className="btn btn-small btn-danger-ghost" onClick={() => void savePassword("")}>
                  消す
                </button>
              )}
            </span>
          </Row>
          <CommitText
            label="プロキシを通さないホスト"
            value={proxy.noProxy.join(", ")}
            mono
            width={320}
            placeholder="localhost, *.internal.example"
            hint="カンマ区切り"
            onCommit={(text) => void setProxy({ noProxy: text.split(/[,\n]/).map((s) => s.trim()).filter((s) => s !== "") })}
          />
        </>
      )}

      <Row label="CA 証明書">
        <span className="settings-inline">
          <span className={caCertPath ? "mono settings-path" : "muted-note"}>{caCertPath ?? "未設定"}</span>
          <button type="button" className="btn btn-small" onClick={() => void pickCa()}>
            ファイルを選択
          </button>
          {caCertPath && (
            <button type="button" className="btn btn-small btn-ghost" onClick={() => void save((s) => ({ ...s, network: { ...s.network, caCertPath: null } }))}>
              解除
            </button>
          )}
        </span>
      </Row>

      <Row label="クライアント証明書">
        <ClientCerts certs={clientCerts} save={save} onSaved={onSaved} />
      </Row>

      {message && (
        <p className={message.ok ? "muted-note settings-message" : "form-error settings-message"} role={message.ok ? "status" : "alert"}>
          {message.text}
        </p>
      )}
    </div>
  );
}

function ClientCerts({ certs, save, onSaved }: { certs: ClientCertSetting[]; save: Props["save"]; onSaved: Props["onSaved"] }) {
  const backend = useBackend();
  const [adding, setAdding] = useState(false);
  const [host, setHost] = useState("");
  const [path, setPath] = useState("");
  const [password, setPassword] = useState("");
  const [passwordFor, setPasswordFor] = useState<string | null>(null);
  const [rowPassword, setRowPassword] = useState("");
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  const fail = (e: unknown) => setMessage({ ok: false, text: errorMessage(e) });

  const pick = async () => {
    try {
      const picked = await backend.pickCertFile("client");
      if (picked !== null) setPath(picked);
    } catch (e) {
      fail(e);
    }
  };

  const add = async () => {
    setMessage(null);
    const before = new Set(certs.map((c) => c.id));
    const saved = await save((s) => ({
      ...s,
      network: { ...s.network, clientCerts: [...s.network.clientCerts, { id: "", hostPattern: host.trim(), path, hasPassword: false }] },
    }));
    if (!saved) return; // 検証エラーは設定画面の上に出る
    const created = saved.network.clientCerts.find((c) => !before.has(c.id));
    try {
      if (created && password !== "") onSaved(await backend.clientCertPasswordSet(created.id, password));
    } catch (e) {
      fail(e);
    }
    setAdding(false);
    setHost("");
    setPath("");
    setPassword("");
  };

  const savePassword = async (id: string, value: string) => {
    setMessage(null);
    try {
      onSaved(await backend.clientCertPasswordSet(id, value));
      setPasswordFor(null);
      setRowPassword("");
    } catch (e) {
      fail(e);
    }
  };

  return (
    <div className="client-certs">
      {certs.length === 0 && !adding && <span className="muted-note">未設定</span>}
      {certs.map((c) => (
        <div key={c.id} className="client-cert" role="group" aria-label={`クライアント証明書 ${c.hostPattern}`}>
          <span className="mono">
            {c.hostPattern} → {fileName(c.path)}
          </span>
          <span className="muted-note">パスワード: {c.hasPassword ? "保存済み" : "なし"}</span>
          {passwordFor === c.id ? (
            <span className="settings-inline">
              <input
                className="input input-small"
                type="password"
                aria-label={`${c.hostPattern} の証明書のパスワード`}
                autoComplete="off"
                value={rowPassword}
                onChange={(e) => setRowPassword(e.target.value)}
              />
              <button type="button" className="btn btn-small" onClick={() => void savePassword(c.id, rowPassword)}>
                保存
              </button>
              {c.hasPassword && (
                <button type="button" className="btn btn-small btn-ghost" onClick={() => void savePassword(c.id, "")}>
                  消す
                </button>
              )}
            </span>
          ) : (
            <button type="button" className="btn btn-small btn-ghost" onClick={() => setPasswordFor(c.id)}>
              パスワード
            </button>
          )}
          <button
            type="button"
            className="btn btn-small btn-danger-ghost"
            aria-label={`${c.hostPattern} の証明書を削除`}
            onClick={() => void save((s) => ({ ...s, network: { ...s.network, clientCerts: s.network.clientCerts.filter((x) => x.id !== c.id) } }))}
          >
            削除
          </button>
        </div>
      ))}

      {adding ? (
        <div className="client-cert-form" role="group" aria-label="クライアント証明書を追加">
          <label className="client-cert-field">
            <span className="muted-note">ホストのパターン（* で何文字でも）</span>
            <input className="input input-small mono" aria-label="ホストのパターン" placeholder="*.internal.example" value={host} onChange={(e) => setHost(e.target.value)} />
          </label>
          <div className="client-cert-field">
            <span className="muted-note">証明書のファイル（.p12 / .pfx / PEM）</span>
            <span className="settings-inline">
              <span className={path ? "mono settings-path" : "muted-note"}>{path || "未選択"}</span>
              <button type="button" className="btn btn-small" onClick={() => void pick()}>
                ファイルを選択
              </button>
            </span>
          </div>
          <label className="client-cert-field">
            <span className="muted-note">ファイルのパスワード（任意。OS のキーチェーンに保存）</span>
            <input className="input input-small" type="password" aria-label="証明書のパスワード" autoComplete="off" value={password} onChange={(e) => setPassword(e.target.value)} />
          </label>
          <span className="settings-inline">
            <button type="button" className="btn btn-small btn-primary" disabled={host.trim() === "" || path === ""} onClick={() => void add()}>
              追加する
            </button>
            <button type="button" className="btn btn-small btn-ghost" onClick={() => setAdding(false)}>
              やめる
            </button>
          </span>
        </div>
      ) : (
        <button type="button" className="btn btn-small btn-ghost client-cert-add" onClick={() => setAdding(true)}>
          + 追加
        </button>
      )}
      {message && (
        <p className="form-error" role="alert">
          {message.text}
        </p>
      )}
    </div>
  );
}
