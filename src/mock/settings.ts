import { ApiError } from "../api/backend";
import type { AppSettings, RequestSpec, SessionState, UpdateInfo, UpdateProgress } from "../bindings";
import { scrubManualSecrets } from "./scrub";

/** ブラウザやテストから、設定・タブの復元・自動更新の状態を変える操作。 */
export interface SettingsControls {
  /** 更新の知らせ（`update://available`）を届ける。 */
  emitUpdateAvailable(info?: Partial<UpdateInfo>): void;
  /** 起動時の確認で見つかっていて、まだ入れ替えていない更新（`update_pending` が返す）。 */
  setPendingUpdate(info: UpdateInfo | null): void;
  /** 「アップデートを確認」の結果。`null` は最新、`Error` は確認できない。 */
  setUpdateCheckResult(result: UpdateInfo | null | Error): void;
  /** 入れ替えを失敗させる（署名の検証エラーなど）。 */
  failInstall(message?: string | false): void;
  /** `update_restart` を呼んだ回数。 */
  restartCount(): number;
  /** 保存されている設定（確認用）。 */
  appSettings(): AppSettings;
  /** 保存されているタブの状態（確認用）。 */
  savedSession(projectId: string): SessionState | null;
  /** 設定したパスワードの有無（確認用）。 */
  passwords(): { proxy: boolean; clientCerts: string[] };
}

export const DEFAULT_APP_SETTINGS: AppSettings = {
  editorCommand: null,
  appearance: { theme: "system", editorFontSize: 13, restoreTabs: true },
  request: { timeoutSecs: 30, followRedirects: true, verifyCertificates: true, responseLimit: "mb10" },
  network: { proxy: { mode: "system", url: "", username: "", hasPassword: false, noProxy: [] }, caCertPath: null, clientCerts: [] },
  history: { limit: 1000, omitLargeBodies: true },
};

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;
const validation = (message: string) => new ApiError({ kind: "validation", message });

export function createSettingsMock(host: { now(): string; newId(prefix: string): string }) {
  let settings: AppSettings = clone(DEFAULT_APP_SETTINGS);
  let proxyPassword = false;
  const certPasswords = new Set<string>();
  const sessions = new Map<string, SessionState>();

  const updateListeners = new Set<(info: UpdateInfo) => void>();
  const progressListeners = new Set<(p: UpdateProgress) => void>();
  let pending: UpdateInfo | null = null;
  let checkResult: UpdateInfo | null | Error = null;
  let installFailure: string | null = null;
  let restarts = 0;
  /** 入れ替えの途中経過を届ける間隔。 */
  const STEP_MS = 5;

  const infoOf = (partial: Partial<UpdateInfo> = {}): UpdateInfo => ({
    version: "0.2.0",
    currentVersion: "0.1.0",
    notes: "- 履歴の検索が速くなりました\n- 設定画面を追加しました",
    date: "2026-11-17T00:00:00Z",
    ...partial,
  });

  function validate(next: AppSettings) {
    if (![13, 14, 15].includes(next.appearance.editorFontSize)) throw validation("エディタの文字サイズは 13・14・15 のどれかにしてください");
    if (![0, 1000, 5000].includes(next.history.limit)) throw validation("履歴の件数は 1000・5000・0（保存しない）のどれかにしてください");
    if (next.request.timeoutSecs < 0 || next.request.timeoutSecs > 100_000) throw validation("タイムアウトは 100000 秒以内にしてください");
    const proxy = next.network.proxy;
    if (proxy.mode === "manual") {
      const url = proxy.url.trim();
      if (url === "") throw validation("手動のプロキシには URL を入力してください");
      if (!/^https?:\/\//i.test(url)) throw validation("プロキシの URL は http:// か https:// で始めてください");
      if (/^https?:\/\/[^/]*@/i.test(url)) throw validation("プロキシの URL に user:pass@ は書けません。ユーザー名とパスワードは別の欄に入力してください");
    }
    if (next.network.caCertPath !== null && /invalid/.test(next.network.caCertPath)) throw validation(`CA 証明書を読めません（PEM の証明書がありません）: ${next.network.caCertPath}`);
  }

  const methods = {
    async appSettingsGet(): Promise<AppSettings> {
      return clone(settings);
    },

    async appSettingsSet(next: AppSettings): Promise<AppSettings> {
      const candidate = clone(next);
      candidate.editorCommand = candidate.editorCommand?.trim() ? candidate.editorCommand.trim() : null;
      candidate.network.proxy.url = candidate.network.proxy.url.trim();
      candidate.network.proxy.noProxy = candidate.network.proxy.noProxy.map((s) => s.trim()).filter((s) => s !== "");
      validate(candidate);
      // `hasPassword` は画面から書き換えられない（保存済みの値を引き継ぐ）
      candidate.network.proxy.hasPassword = proxyPassword;
      candidate.network.clientCerts = candidate.network.clientCerts
        .map((c) => ({ ...c, hostPattern: c.hostPattern.trim() }))
        .filter((c) => c.hostPattern !== "")
        .map((c) => {
          const id = c.id === "" ? host.newId("mock-cert") : c.id;
          return { ...c, id, hasPassword: certPasswords.has(id) };
        });
      for (const id of [...certPasswords]) if (!candidate.network.clientCerts.some((c) => c.id === id)) certPasswords.delete(id);
      settings = candidate;
      return clone(settings);
    },

    async proxyPasswordSet(password: string): Promise<AppSettings> {
      proxyPassword = password !== "";
      settings = { ...settings, network: { ...settings.network, proxy: { ...settings.network.proxy, hasPassword: proxyPassword } } };
      return clone(settings);
    },

    async clientCertPasswordSet(id: string, password: string): Promise<AppSettings> {
      if (!settings.network.clientCerts.some((c) => c.id === id)) throw validation(`クライアント証明書が設定にありません: ${id}`);
      if (password === "") certPasswords.delete(id);
      else certPasswords.add(id);
      settings = {
        ...settings,
        network: { ...settings.network, clientCerts: settings.network.clientCerts.map((c) => (c.id === id ? { ...c, hasPassword: certPasswords.has(id) } : c)) },
      };
      return clone(settings);
    },

    async pickCertFile(kind: "ca" | "client"): Promise<string | null> {
      return kind === "ca" ? "/mock/certs/ca.pem" : "/mock/certs/client.p12";
    },

    async sessionSave(projectId: string, session: SessionState): Promise<SessionState> {
      if (session.tabs.length > 200) throw validation("タブは 200 個までです");
      const tabs = session.tabs.map((t) => {
        if (!t.request) return { ...clone(t), droppedFields: [] as string[] };
        const scrubbed = scrubManualSecrets(t.request as RequestSpec, "");
        return { ...clone(t), request: scrubbed.spec, droppedFields: scrubbed.fields };
      });
      const saved: SessionState = { tabs, activeTabId: session.activeTabId, savedAt: host.now() };
      sessions.set(projectId, saved);
      return clone(saved);
    },

    async sessionLoad(projectId: string): Promise<SessionState | null> {
      const s = sessions.get(projectId);
      return s ? clone(s) : null;
    },

    async updateCheck(): Promise<UpdateInfo | null> {
      if (checkResult instanceof Error) throw new ApiError({ kind: "update", message: checkResult.message });
      if (checkResult) pending = checkResult;
      return checkResult ? clone(checkResult) : null;
    },

    async updatePending(): Promise<UpdateInfo | null> {
      return pending ? clone(pending) : null;
    },

    async updateInstall(): Promise<void> {
      if (!pending) throw new ApiError({ kind: "update", message: "入れ替える更新がありません" });
      const emit = (p: UpdateProgress) => {
        for (const l of [...progressListeners]) l(p);
      };
      const total = 4_000_000;
      emit({ phase: "downloading", downloaded: 0, total });
      await new Promise((r) => setTimeout(r, STEP_MS));
      emit({ phase: "downloading", downloaded: total / 2, total });
      if (installFailure) throw new ApiError({ kind: "update", message: installFailure });
      await new Promise((r) => setTimeout(r, STEP_MS));
      emit({ phase: "downloading", downloaded: total, total });
      emit({ phase: "installing", downloaded: total, total });
      await new Promise((r) => setTimeout(r, STEP_MS));
      emit({ phase: "installed", downloaded: total, total });
    },

    async updateRestart(): Promise<void> {
      restarts += 1;
    },

    async onUpdateAvailable(handler: (info: UpdateInfo) => void): Promise<() => void> {
      updateListeners.add(handler);
      return () => {
        updateListeners.delete(handler);
      };
    },

    async onUpdateProgress(handler: (p: UpdateProgress) => void): Promise<() => void> {
      progressListeners.add(handler);
      return () => {
        progressListeners.delete(handler);
      };
    },
  };

  const controls: SettingsControls = {
    emitUpdateAvailable(partial) {
      const info = infoOf(partial);
      pending = info;
      for (const l of [...updateListeners]) l(info);
    },
    setPendingUpdate(info) {
      pending = info;
    },
    setUpdateCheckResult(result) {
      checkResult = result;
    },
    failInstall(message = "署名の検証に失敗しました") {
      installFailure = message === false ? null : message;
    },
    restartCount: () => restarts,
    appSettings: () => clone(settings),
    savedSession: (projectId) => (sessions.has(projectId) ? clone(sessions.get(projectId)!) : null),
    passwords: () => ({ proxy: proxyPassword, clientCerts: [...certPasswords] }),
  };

  return { methods, controls, settings: () => settings };
}
