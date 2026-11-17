import * as Dialog from "@radix-ui/react-dialog";
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useRef, useState } from "react";

import { errorMessage, useBackend } from "../../api";
import { queryKeys, useAppSettings } from "../../api/queries";
import type { AppSettings, ResponseLimit, Theme } from "../../bindings";
import { isMac, SHORTCUTS, shortcutText } from "../../lib/platform";
import { useUiStore } from "../../store/ui";
import { CloseIcon } from "../Icons";
import { AboutSection } from "./AboutSection";
import { NetworkSection } from "./NetworkSection";
import { CommitText, Row, Select, Switch } from "./controls";

const THEMES: { id: Theme; label: string }[] = [
  { id: "dark", label: "ダーク" },
  { id: "light", label: "ライト" },
  { id: "system", label: "OSに合わせる" },
];

const SECTIONS = [
  { id: "appearance", label: "外観" },
  { id: "request", label: "リクエスト" },
  { id: "proxy", label: "プロキシ・証明書" },
  { id: "history", label: "履歴とデータ" },
  { id: "editor", label: "エディタ" },
  { id: "keys", label: "ショートカット" },
  { id: "about", label: "このアプリについて" },
];

/** ヘッダーの歯車から開く設定。 */
export function SettingsDialog() {
  const open = useUiStore((s) => s.settingsDialogOpen);
  const setOpen = useUiStore((s) => s.setSettingsDialogOpen);
  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay" />
        <Dialog.Content className="dialog settings-dialog" aria-describedby={undefined}>
          <header className="settings-header">
            <Dialog.Title className="dialog-heading">設定</Dialog.Title>
            <Dialog.Close className="icon-btn" aria-label="閉じる">
              <CloseIcon />
            </Dialog.Close>
          </header>
          <SettingsBody />
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function SettingsBody() {
  const settings = useAppSettings();
  if (settings.isPending) {
    return (
      <div className="settings-loading" role="status">
        読み込んでいます…
      </div>
    );
  }
  if (settings.isError) {
    return (
      <div className="alert alert-danger settings-loading" role="alert">
        設定を読み込めませんでした: {errorMessage(settings.error)}
      </div>
    );
  }
  return <SettingsForm initial={settings.data} />;
}

const LIMITS: { value: ResponseLimit; label: string }[] = [
  { value: "mb10", label: "10 MB" },
  { value: "mb50", label: "50 MB" },
  { value: "unlimited", label: "無制限" },
];

/**
 * 設定の入力。変えるたびに（文字の入力は欄を離れたときに）保存する。検証エラーは Rust のメッセージをそのまま出し、
 * 保存されなかった項目は前の値のまま。
 */
function SettingsForm({ initial }: { initial: AppSettings }) {
  const backend = useBackend();
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState<AppSettings>(initial);
  const [error, setError] = useState<string | null>(null);
  const [active, setActive] = useState("appearance");
  const draftRef = useRef(initial);
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const pending = useRef(0);

  const apply = useCallback(
    (next: AppSettings) => {
      draftRef.current = next;
      setDraft(next);
      queryClient.setQueryData(queryKeys.appSettings, next);
    },
    [queryClient],
  );

  /** 設定を変えて保存する。順番に保存し、保存した設定を返す（保存できなければ `null`）。 */
  const save = useCallback(
    (change: (s: AppSettings) => AppSettings): Promise<AppSettings | null> => {
      const next = change(draftRef.current);
      draftRef.current = next;
      setDraft(next);
      pending.current += 1;
      const job = queue.current.then(async () => {
        try {
          const saved = await backend.appSettingsSet(next);
          setError(null);
          pending.current -= 1;
          // あとの変更が控えていなければ、Rust が整えた値（前後の空白・割り振った ID など）に合わせる
          if (pending.current === 0) apply(saved);
          else queryClient.setQueryData(queryKeys.appSettings, saved);
          return saved;
        } catch (e) {
          pending.current -= 1;
          setError(errorMessage(e));
          return null;
        }
      });
      queue.current = job;
      return job;
    },
    [backend, apply, queryClient],
  );

  /** パスワードを保存した結果（`hasPassword` の印）だけを反映する。 */
  const onPasswordSaved = useCallback(
    (saved: AppSettings) => {
      const merged: AppSettings = { ...draftRef.current, network: { ...draftRef.current.network, proxy: { ...draftRef.current.network.proxy, hasPassword: saved.network.proxy.hasPassword }, clientCerts: draftRef.current.network.clientCerts.map((c) => ({ ...c, hasPassword: saved.network.clientCerts.find((x) => x.id === c.id)?.hasPassword ?? c.hasPassword })) } };
      apply(merged);
    },
    [apply],
  );

  const go = (id: string) => {
    setActive(id);
    document.getElementById(`settings-${id}`)?.scrollIntoView({ block: "start" });
  };

  const a = draft.appearance;
  const mac = isMac();
  return (
    <div className="settings-layout">
      <nav className="settings-nav" aria-label="設定の項目">
        {SECTIONS.map((s) => (
          <button key={s.id} type="button" className={`settings-nav-item${active === s.id ? " is-active" : ""}`} aria-current={active === s.id ? "true" : undefined} onClick={() => go(s.id)}>
            {s.label}
          </button>
        ))}
      </nav>

      <div className="settings-content">
        {error && (
          <div className="alert alert-danger settings-error" role="alert">
            保存できませんでした: {error}（前の設定のままです）
          </div>
        )}

        <section id="settings-appearance" className="settings-section" aria-labelledby="settings-h-appearance">
          <h2 id="settings-h-appearance" className="settings-h">
            外観
          </h2>
          <div className="settings-grid">
            <Row label="テーマ">
              <div role="radiogroup" aria-label="テーマ" className="segmented settings-theme">
                {THEMES.map((t) => (
                  <button
                    key={t.id}
                    type="button"
                    role="radio"
                    aria-checked={a.theme === t.id}
                    className="segmented-item"
                    onClick={() => void save((s) => ({ ...s, appearance: { ...s.appearance, theme: t.id } }))}
                  >
                    {t.label}
                  </button>
                ))}
              </div>
            </Row>
            <Select
              label="エディタの文字サイズ"
              value={a.editorFontSize}
              options={[13, 14, 15].map((n) => ({ value: n, label: `${n} px` }))}
              onChange={(editorFontSize) => void save((s) => ({ ...s, appearance: { ...s.appearance, editorFontSize } }))}
            />
            <Switch label="起動時に前回のタブを復元" checked={a.restoreTabs} onChange={(restoreTabs) => void save((s) => ({ ...s, appearance: { ...s.appearance, restoreTabs } }))} />
          </div>
        </section>

        <section id="settings-request" className="settings-section" aria-labelledby="settings-h-request">
          <h2 id="settings-h-request" className="settings-h">
            リクエスト
          </h2>
          <div className="settings-grid">
            <CommitText
              label="タイムアウト"
              value={String(draft.request.timeoutSecs)}
              width={80}
              suffix={<span className="muted-note">秒（0 で無制限）</span>}
              validate={(v) => (/^\d+$/.test(v.trim()) ? null : "タイムアウトは 0 以上の整数（秒）で入力してください")}
              onCommit={(v) => void save((s) => ({ ...s, request: { ...s.request, timeoutSecs: Number.parseInt(v.trim(), 10) } }))}
            />
            <Switch label="リダイレクトを自動で追う" checked={draft.request.followRedirects} onChange={(followRedirects) => void save((s) => ({ ...s, request: { ...s.request, followRedirects } }))} />
            <Switch label="SSL 証明書を検証する" checked={draft.request.verifyCertificates} onChange={(verifyCertificates) => void save((s) => ({ ...s, request: { ...s.request, verifyCertificates } }))} />
            <Select label="表示するレスポンスの上限" value={draft.request.responseLimit} options={LIMITS} onChange={(responseLimit) => void save((s) => ({ ...s, request: { ...s.request, responseLimit } }))} />
          </div>
        </section>

        <section id="settings-proxy" className="settings-section" aria-labelledby="settings-h-proxy">
          <h2 id="settings-h-proxy" className="settings-h">
            プロキシ・証明書
          </h2>
          <NetworkSection settings={draft} save={save} onSaved={onPasswordSaved} />
        </section>

        <section id="settings-history" className="settings-section" aria-labelledby="settings-h-history">
          <h2 id="settings-h-history" className="settings-h">
            履歴とデータ
          </h2>
          <div className="settings-grid">
            <Select
              label="保存する件数"
              value={draft.history.limit}
              options={[
                { value: 1000, label: "1,000 件" },
                { value: 5000, label: "5,000 件" },
                { value: 0, label: "保存しない" },
              ]}
              onChange={(limit) => void save((s) => ({ ...s, history: { ...s.history, limit } }))}
            />
            <Switch label="1 MB を超えるボディを省略" checked={draft.history.omitLargeBodies} onChange={(omitLargeBodies) => void save((s) => ({ ...s, history: { ...s.history, omitLargeBodies } }))} />
          </div>
          <p className="muted-note settings-note">履歴はプロジェクトごとに、古いものから消えます。シークレットの値は、保存前に ••••（伏せ字）に置き換えます。</p>
        </section>

        <section id="settings-editor" className="settings-section" aria-labelledby="settings-h-editor">
          <h2 id="settings-h-editor" className="settings-h">
            エディタ
          </h2>
          <div className="settings-grid">
            <CommitText
              label="yaml を開くエディタのコマンド"
              value={draft.editorCommand ?? ""}
              mono
              width={360}
              placeholder="code -g {file}:{line}"
              hint={
                <>
                  設計タブのファイル名から開くときに使います。<span className="mono">{"{file}"}</span> は絶対パス、<span className="mono">{"{line}"}</span> は行番号に置き換わります。空なら、VS Code・Cursor などを自動で探します
                </>
              }
              onCommit={(v) => void save((s) => ({ ...s, editorCommand: v.trim() === "" ? null : v.trim() }))}
            />
          </div>
        </section>

        <section id="settings-keys" className="settings-section" aria-labelledby="settings-h-keys">
          <h2 id="settings-h-keys" className="settings-h">
            ショートカット
          </h2>
          <div className="settings-grid">
            {SHORTCUTS.map((s) => (
              <Row key={s.id} label={s.label}>
                <kbd className="kbd">{shortcutText(s, mac)}</kbd>
              </Row>
            ))}
          </div>
          <p className="muted-note settings-note">{mac ? "macOS では ⌘ キー" : "Windows では Ctrl キー"}を押しながら使います。</p>
        </section>

        <section id="settings-about" className="settings-section" aria-labelledby="settings-h-about">
          <h2 id="settings-h-about" className="settings-h">
            このアプリについて
          </h2>
          <AboutSection />
        </section>
      </div>
    </div>
  );
}
