import { useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useMemo, useState } from "react";

import { errorMessage, useBackend } from "../api";
import { queryKeys, useEnvironments, useSelectEnvironment } from "../api/queries";
import type { EnvVar, Environment, Project } from "../bindings";
import { useUiStore } from "../store/ui";
import { ConfirmDialog } from "./Dialog";
import { ChevronLeftIcon, CloseIcon, EyeIcon, EyeOffIcon, LockIcon } from "./Icons";

/** 画面で編集している変数 1 行。シークレットの値は、入力するまで画面には持たない。 */
interface VarRow {
  key: string;
  name: string;
  enabled: boolean;
  kind: "plain" | "secret";
  /** 通常の変数の値。 */
  value: string;
  description: string;
  /** 保存済みの行のときの、保存されている名前と種類（キーチェーンの値の引き継ぎ・削除に使う）。 */
  origName: string | null;
  origKind: "plain" | "secret" | null;
  /** シークレットの値を入力したとき。`null` は「変えない」。 */
  secretInput: string | null;
  /** 目のアイコンで値を見せている。 */
  shown: boolean;
  /** 見せるために読み出したシークレットの値。 */
  revealed: string | null;
}

interface EnvDraft {
  name: string;
  rows: VarRow[];
}

let rowSeq = 0;
const nextKey = () => `row-${(rowSeq += 1)}`;

function rowFromVar(v: EnvVar): VarRow {
  return {
    key: nextKey(),
    name: v.name,
    enabled: v.enabled,
    kind: v.value.kind,
    value: v.value.kind === "plain" ? v.value.value : "",
    description: v.description,
    origName: v.name,
    origKind: v.value.kind,
    secretInput: null,
    shown: false,
    revealed: null,
  };
}

const draftFromEnv = (env: Environment): EnvDraft => ({ name: env.name, rows: env.vars.map(rowFromVar) });

const newRow = (): VarRow => ({
  key: nextKey(),
  name: "",
  enabled: true,
  kind: "plain",
  value: "",
  description: "",
  origName: null,
  origKind: null,
  secretInput: null,
  shown: false,
  revealed: null,
});

/** 編集前後を比べるための文字列（見せているかどうかは含めない）。 */
const signatureOfDraft = (d: EnvDraft) =>
  JSON.stringify([d.name, d.rows.map((r) => [r.name, r.enabled, r.kind, r.kind === "plain" ? r.value : "", r.description, r.secretInput])]);

const isBlankRow = (r: VarRow) =>
  r.name.trim() === "" && r.description.trim() === "" && (r.kind === "plain" ? r.value === "" : (r.secretInput ?? "") === "");

/** 名前の入力欄の幅（全角は半角 2 文字ぶん）。 */
const nameWidth = (name: string) => Math.max(8, [...name].reduce((w, c) => w + (c.charCodeAt(0) > 0x7f ? 1.7 : 1), 0) + 2);

/** 環境変数の画面（環境の一覧と、変数の編集）。 */
export function EnvironmentsScreen({ project }: { project: Project }) {
  const backend = useBackend();
  const queryClient = useQueryClient();
  const envs = useEnvironments(project.id);
  const selectEnv = useSelectEnvironment(project.id);
  const setScreen = useUiStore((s) => s.setScreen);

  const list = envs.data ?? [];
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, EnvDraft>>({});
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<"delete" | "leave" | null>(null);

  const env: Environment | undefined =
    list.find((e) => e.id === selectedId) ??
    list.find((e) => e.id === project.defaultEnvironmentId) ??
    list.find((e) => !e.isGlobal) ??
    list[0];

  const base = useMemo(() => (env ? draftFromEnv(env) : null), [env]);
  const draft = env ? (drafts[env.id] ?? base) : null;

  const dirtyIds = useMemo(() => {
    const ids = new Set<string>();
    for (const e of list) {
      const d = drafts[e.id];
      if (d && signatureOfDraft(d) !== signatureOfDraft(draftFromEnv(e))) ids.add(e.id);
    }
    return ids;
  }, [drafts, list]);
  const dirty = env ? dirtyIds.has(env.id) : false;
  const inUse = env !== undefined && env.id === project.defaultEnvironmentId;

  const refreshEnvs = () => queryClient.invalidateQueries({ queryKey: queryKeys.environments(project.id) });
  const refreshProjects = () => queryClient.invalidateQueries({ queryKey: queryKeys.projects });

  const edit = (fn: (d: EnvDraft) => EnvDraft) => {
    if (!env || !base) return;
    setDrafts((ds) => ({ ...ds, [env.id]: fn(ds[env.id] ?? base) }));
  };
  const editRow = (key: string, patch: Partial<VarRow>) =>
    edit((d) => ({ ...d, rows: d.rows.map((r) => (r.key === key ? { ...r, ...patch } : r)) }));

  const run = async (task: () => Promise<void>) => {
    setError(null);
    setBusy(true);
    try {
      await task();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const toggleShow = (row: VarRow) =>
    run(async () => {
      if (!env) return;
      if (row.shown) {
        editRow(row.key, { shown: false, revealed: null });
        return;
      }
      if (row.secretInput === null && row.origKind === "secret" && row.origName !== null) {
        const value = await backend.secretReveal(project.id, env.id, row.origName);
        editRow(row.key, { shown: true, revealed: value ?? "" });
      } else {
        editRow(row.key, { shown: true });
      }
    });

  const changeKind = (row: VarRow, kind: "plain" | "secret") =>
    run(async () => {
      if (!env || kind === row.kind) return;
      if (kind === "secret") {
        editRow(row.key, { kind, secretInput: row.value, value: "", shown: false, revealed: null });
        return;
      }
      let value = row.secretInput ?? "";
      if (row.secretInput === null && row.origKind === "secret" && row.origName !== null) {
        value = (await backend.secretReveal(project.id, env.id, row.origName)) ?? "";
      }
      editRow(row.key, { kind, value, secretInput: null, shown: false, revealed: null });
    });

  const save = () =>
    run(async () => {
      if (!env || !draft) return;
      const rows = draft.rows.filter((r) => !isBlankRow(r));
      // 名前を変えたシークレットは、キーチェーンの値が引き継がれないので、読み出して付け直す
      const carried = new Map<string, string>();
      for (const r of rows) {
        if (r.kind === "secret" && r.secretInput === null && r.origKind === "secret" && r.origName !== null && r.origName !== r.name.trim()) {
          const v = await backend.secretReveal(project.id, env.id, r.origName);
          if (v !== null) carried.set(r.key, v);
        }
      }
      const vars: EnvVar[] = rows.map((r) => ({
        name: r.name,
        value: r.kind === "secret" ? { kind: "secret" } : { kind: "plain", value: r.value },
        description: r.description,
        enabled: r.enabled,
      }));
      await backend.envSave({ ...env, name: draft.name, vars });
      for (const r of rows) {
        if (r.kind !== "secret") continue;
        const name = r.name.trim();
        const value = r.secretInput ?? carried.get(r.key);
        if (value === undefined) continue;
        if (value === "") {
          if (r.origKind === "secret" && r.origName === name) await backend.secretDelete(project.id, env.id, name);
        } else {
          await backend.secretSet(project.id, env.id, name, value);
        }
      }
      setDrafts(({ [env.id]: _saved, ...rest }) => rest);
      await refreshEnvs();
    });

  const discard = () => {
    if (!env) return;
    setError(null);
    setDrafts(({ [env.id]: _discarded, ...rest }) => rest);
  };

  const create = (e: FormEvent) => {
    e.preventDefault();
    void run(async () => {
      const created = await backend.envCreate(project.id, newName);
      setAdding(false);
      setNewName("");
      await Promise.all([refreshEnvs(), refreshProjects()]);
      setSelectedId(created.id);
    });
  };

  const duplicate = () =>
    run(async () => {
      if (!env) return;
      const copy = await backend.envDuplicate(env.id);
      await refreshEnvs();
      setSelectedId(copy.id);
    });

  const remove = () =>
    run(async () => {
      if (!env) return;
      setConfirm(null);
      await backend.envDelete(env.id);
      setDrafts(({ [env.id]: _removed, ...rest }) => rest);
      setSelectedId(null);
      await Promise.all([refreshEnvs(), refreshProjects()]);
    });

  const global = list.find((e) => e.isGlobal);
  const others = list.filter((e) => !e.isGlobal);
  const leave = () => (dirtyIds.size > 0 ? setConfirm("leave") : setScreen("main"));

  const envButton = (e: Environment) => (
    <button
      key={e.id}
      type="button"
      className={`env-item${env?.id === e.id ? " is-selected" : ""}`}
      aria-current={env?.id === e.id ? "true" : undefined}
      onClick={() => {
        setSelectedId(e.id);
        setError(null);
      }}
    >
      {!e.isGlobal && <span className={`env-dot${e.id === project.defaultEnvironmentId ? " is-on" : ""}`} aria-hidden="true" />}
      <span className="env-item-name">{e.name}</span>
      {dirtyIds.has(e.id) && (
        <span className="tab-dirty" role="img" aria-label="未保存">
          ●
        </span>
      )}
      <span className="env-item-count">{e.vars.length}</span>
    </button>
  );

  return (
    <div className="env-screen">
      <header className="env-header">
        <button type="button" className="btn btn-ghost" onClick={leave}>
          <ChevronLeftIcon />
          戻る
        </button>
        <h1 className="env-title">環境変数</h1>
        <span className="env-project">{project.name}</span>
      </header>

      <div className="env-body">
        <nav className="env-list" aria-label="環境の一覧">
          <span className="env-group-label">共通</span>
          {global && envButton(global)}
          <span className="env-group-label env-group-gap">プロジェクトの環境</span>
          {others.map(envButton)}
          {adding ? (
            <form className="env-add-form" onSubmit={create}>
              <input
                className="input env-add-input"
                aria-label="新しい環境の名前"
                placeholder="環境名（例: ローカル）"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    setAdding(false);
                    setNewName("");
                  }
                }}
                autoFocus
                autoComplete="off"
              />
              <button type="submit" className="btn btn-primary btn-small" disabled={busy || newName.trim() === ""}>
                作成
              </button>
            </form>
          ) : (
            <button type="button" className="env-add" onClick={() => setAdding(true)}>
              + 環境を追加
            </button>
          )}
          <div className="env-hint">環境には、接続先の URL（baseUrl など）や認証用の値を入れておきます。</div>
        </nav>

        <section className="env-detail" aria-label="環境の設定">
          {env && draft ? (
            <>
              <div className="env-detail-head">
                {env.isGlobal ? (
                  <h2 className="env-detail-name">{env.name}</h2>
                ) : (
                  <input
                    className="env-name-input"
                    style={{ width: `${nameWidth(draft.name)}ch` }}
                    aria-label="環境名"
                    value={draft.name}
                    onChange={(e) => edit((d) => ({ ...d, name: e.target.value }))}
                    autoComplete="off"
                  />
                )}
                {env.isGlobal && <span className="badge">どの環境を選んでいても使われます</span>}
                {inUse && <span className="badge badge-accent">使用中</span>}
                <div className="env-actions">
                  {!env.isGlobal && !inUse && (
                    <button type="button" className="btn btn-small" disabled={busy} onClick={() => selectEnv.mutate(env.id)}>
                      この環境を使う
                    </button>
                  )}
                  {!env.isGlobal && (
                    <>
                      <button
                        type="button"
                        className="btn btn-small"
                        disabled={busy || dirty}
                        title={dirty ? "変更を保存してから複製してください" : undefined}
                        onClick={() => void duplicate()}
                      >
                        複製
                      </button>
                      <button type="button" className="btn btn-small btn-danger-ghost" disabled={busy} onClick={() => setConfirm("delete")}>
                        削除
                      </button>
                    </>
                  )}
                </div>
              </div>

              <div className="kv-wrap">
                <table className="kv-table env-table">
                  <thead>
                    <tr>
                      <th className="kv-check">
                        <span className="sr-only">有効</span>
                      </th>
                      <th className="env-col-name">変数名</th>
                      <th>値</th>
                      <th className="env-col-kind">種類</th>
                      <th className="env-col-desc">説明</th>
                      <th className="kv-remove">
                        <span className="sr-only">削除</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {draft.rows.map((r, i) => (
                      <tr key={r.key} className={r.enabled ? undefined : "kv-off"}>
                        <td className="kv-check">
                          <input
                            type="checkbox"
                            checked={r.enabled}
                            onChange={(e) => editRow(r.key, { enabled: e.target.checked })}
                            aria-label={`変数 ${i + 1} を有効にする`}
                          />
                        </td>
                        <td>
                          <input
                            className="cell-input mono env-var-name"
                            value={r.name}
                            onChange={(e) => editRow(r.key, { name: e.target.value })}
                            aria-label={`変数 ${i + 1} の名前`}
                            placeholder="変数名"
                            spellCheck={false}
                            autoComplete="off"
                          />
                        </td>
                        <td>
                          {r.kind === "plain" ? (
                            <input
                              className="cell-input mono"
                              value={r.value}
                              onChange={(e) => editRow(r.key, { value: e.target.value })}
                              aria-label={`変数 ${i + 1} の値`}
                              placeholder="値"
                              spellCheck={false}
                              autoComplete="off"
                            />
                          ) : (
                            <div className="secret-cell">
                              <input
                                className="cell-input mono"
                                type={r.shown ? "text" : "password"}
                                value={r.shown ? (r.secretInput ?? r.revealed ?? "") : (r.secretInput ?? "")}
                                onChange={(e) => editRow(r.key, { secretInput: e.target.value })}
                                aria-label={`変数 ${i + 1} の値`}
                                placeholder={r.shown ? "未設定" : r.origKind === "secret" && r.secretInput === null ? "••••••••" : "値を入力"}
                                spellCheck={false}
                                autoComplete="off"
                              />
                              <button
                                type="button"
                                className="icon-btn"
                                aria-label={r.shown ? `変数 ${i + 1} の値を隠す` : `変数 ${i + 1} の値を表示`}
                                aria-pressed={r.shown}
                                onClick={() => void toggleShow(r)}
                              >
                                {r.shown ? <EyeOffIcon /> : <EyeIcon />}
                              </button>
                            </div>
                          )}
                        </td>
                        <td>
                          <select
                            className={`kind-select${r.kind === "secret" ? " is-secret" : ""}`}
                            value={r.kind}
                            onChange={(e) => void changeKind(r, e.target.value === "secret" ? "secret" : "plain")}
                            aria-label={`変数 ${i + 1} の種類`}
                          >
                            <option value="plain">通常</option>
                            <option value="secret">シークレット</option>
                          </select>
                        </td>
                        <td>
                          <input
                            className="cell-input"
                            value={r.description}
                            onChange={(e) => editRow(r.key, { description: e.target.value })}
                            aria-label={`変数 ${i + 1} の説明`}
                            placeholder="説明"
                            autoComplete="off"
                          />
                        </td>
                        <td className="kv-remove">
                          <button
                            type="button"
                            className="icon-btn"
                            aria-label={`変数 ${i + 1} を削除`}
                            onClick={() => edit((d) => ({ ...d, rows: d.rows.filter((x) => x.key !== r.key) }))}
                          >
                            <CloseIcon />
                          </button>
                        </td>
                      </tr>
                    ))}
                    <tr className="kv-add-row">
                      <td />
                      <td colSpan={5}>
                        <button type="button" className="kv-add" onClick={() => edit((d) => ({ ...d, rows: [...d.rows, newRow()] }))}>
                          ＋ 変数を追加
                        </button>
                      </td>
                    </tr>
                  </tbody>
                </table>
              </div>

              <div className="lock-note">
                <LockIcon />
                シークレットの値は OS のキーチェーンに保存されます。アプリのデータベースには変数名だけが残ります。
              </div>
              <div className="priority">
                <span>優先順位:</span>
                <span>リクエスト内の上書き</span>
                <span aria-hidden="true">›</span>
                <span className="priority-on">選んでいる環境</span>
                <span aria-hidden="true">›</span>
                <span>グローバル</span>
              </div>

              {error && (
                <div className="alert alert-danger" role="alert">
                  {error}
                </div>
              )}

              <div className="env-footer">
                <button type="button" className="btn" disabled={!dirty || busy} onClick={discard}>
                  破棄
                </button>
                <button type="button" className="btn btn-primary" disabled={!dirty || busy} onClick={() => void save()}>
                  保存
                </button>
              </div>
            </>
          ) : (
            <p className="muted-note">{envs.isPending ? "読み込み中…" : "環境がありません。"}</p>
          )}
        </section>
      </div>

      <ConfirmDialog
        open={confirm === "delete"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title="環境を削除しますか？"
        description={env ? `「${env.name}」と、その変数（シークレットの値を含む）を削除します。この操作は取り消せません。` : ""}
        confirmLabel="削除する"
        danger
        onConfirm={() => void remove()}
      />
      <ConfirmDialog
        open={confirm === "leave"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title="変更を破棄して戻りますか？"
        description="保存していない環境変数の変更があります。戻ると変更は失われます。"
        confirmLabel="破棄して戻る"
        danger
        onConfirm={() => {
          setConfirm(null);
          setScreen("main");
        }}
      />
    </div>
  );
}
