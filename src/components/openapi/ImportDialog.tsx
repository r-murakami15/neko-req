import * as Dialog from "@radix-ui/react-dialog";
import { useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { errorMessage, useBackend } from "../../api";
import { queryKeys, useProjects } from "../../api/queries";
import type {
  ImportOptions,
  ImportPreview,
  ImportRequest,
  ImportResult,
  PreviewOperation,
  PreviewResult,
  SourceInput,
} from "../../bindings";
import { formatBytes } from "../../lib/format";
import { opKeyString } from "../../lib/opkey";
import { methodColor } from "../../lib/spec";
import { useUiStore } from "../../store/ui";
import { CheckIcon, CloseIcon, FileIcon, UploadIcon } from "../Icons";
import { DiffList, diffIsEmpty } from "./DiffList";
import { ForeignHosts, LoadProblems } from "./Problems";
import { UploadZone } from "./UploadZone";

const DEFAULT_OPTIONS: ImportOptions = { tagFolders: true, fillFromExamples: true, applyCookieAuth: true, descriptionAsDoc: true };

type Step = "source" | "confirm" | "importing" | "done";
type Issue = Exclude<PreviewResult, { status: "ready" }>;
type Imported = Extract<ImportResult, { status: "imported" }>;

interface SourceState {
  method: "path" | "upload";
  /** パスまたは URL。 */
  pathText: string;
  token: string;
  /** 確認済みの別ホスト。 */
  allowedHosts: string[];
  uploadPaths: string[];
}

const isUrl = (text: string) => /^https?:\/\//i.test(text.trim());

/** 画面の指定から、Rust に渡す `SourceInput` を作る。 */
function buildSource(s: SourceState, allowedHosts: string[], entry: string | null = null): SourceInput | null {
  if (s.method === "upload") return s.uploadPaths.length === 0 ? null : { kind: "upload", paths: s.uploadPaths, entry };
  const text = s.pathText.trim();
  if (text === "") return null;
  if (isUrl(text)) return { kind: "url", url: text, token: s.token.trim() === "" ? null : s.token.trim(), allowedHosts };
  return { kind: "path", path: text };
}

const STEPS: { id: Step[]; label: string }[] = [
  { id: ["source"], label: "ファイル選択" },
  { id: ["confirm"], label: "内容の確認" },
  { id: ["importing", "done"], label: "取り込み" },
];

function Steps({ step }: { step: Step }) {
  const index = STEPS.findIndex((s) => s.id.includes(step));
  return (
    <ol className="import-steps" aria-label="取り込みの手順">
      {STEPS.map((s, i) => {
        const done = i < index || step === "done";
        const current = i === index && step !== "done";
        return (
          <li key={s.label} className={`import-step${current ? " is-current" : ""}${done ? " is-done" : ""}`} aria-current={current ? "step" : undefined}>
            {i > 0 && <span className="import-step-line" aria-hidden="true" />}
            <span className="import-step-dot">{done ? "✓" : i + 1}</span>
            {s.label}
          </li>
        );
      })}
    </ol>
  );
}

/** 「OpenAPI から取り込み」。ファイル選択 → 内容の確認 → 取り込み の 3 段階。 */
export function ImportDialog() {
  const open = useUiStore((s) => s.importDialogOpen);
  const setOpen = useUiStore((s) => s.setImportDialogOpen);
  // 取り込んでいる最中は、Esc で閉じて結果を見失わないようにする
  const [busy, setBusy] = useState(false);
  return (
    <Dialog.Root open={open} onOpenChange={(next) => (next || !busy) && setOpen(next)}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay" />
        <Dialog.Content className="dialog import-dialog" aria-describedby={undefined} onInteractOutside={(e) => e.preventDefault()}>
          <ImportFlow onClose={() => setOpen(false)} onBusyChange={setBusy} />
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function ImportFlow({ onClose, onBusyChange }: { onClose: () => void; onBusyChange: (busy: boolean) => void }) {
  const backend = useBackend();
  const queryClient = useQueryClient();
  const projects = useProjects();
  const currentProjectId = useUiStore((s) => s.currentProjectId);
  const setCurrentProjectId = useUiStore((s) => s.setCurrentProjectId);
  const setScreen = useUiStore((s) => s.setScreen);

  const [step, setStep] = useState<Step>("source");
  const [source, setSource] = useState<SourceState>({ method: "path", pathText: "", token: "", allowedHosts: [], uploadPaths: [] });
  const [checking, setChecking] = useState(false);
  const [issue, setIssue] = useState<Issue | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [used, setUsed] = useState<SourceInput | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [openGroups, setOpenGroups] = useState<Set<string>>(new Set());
  const [options, setOptions] = useState<ImportOptions>(DEFAULT_OPTIONS);
  const [target, setTarget] = useState<"new" | "existing">("new");
  const [targetProjectId, setTargetProjectId] = useState<string | null>(null);
  const [projectName, setProjectName] = useState<string | null>(null);
  const [result, setResult] = useState<Imported | null>(null);

  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  useEffect(() => {
    onBusyChange(step === "importing");
    return () => onBusyChange(false);
  }, [step, onBusyChange]);

  const projectList = useMemo(() => projects.data ?? [], [projects.data]);
  const existingId = targetProjectId ?? (projectList.find((p) => p.id === currentProjectId) ?? projectList[0])?.id ?? null;

  /** yaml を読んで確認画面のデータを作る。確認画面の選択は、`keep` のときは残す（取り込み先を変えたときの再読み込み）。 */
  const check = useCallback(
    async (src: SourceInput, projectId: string | null, keep = false) => {
      setChecking(true);
      setError(null);
      setIssue(null);
      try {
        const r = await backend.openapiPreview(src, projectId);
        if (!alive.current) return;
        if (r.status === "ready") {
          setPreview(r.preview);
          setUsed(src);
          if (!keep) {
            const tags = r.preview.tags;
            setSelected(new Set(tags.flatMap((t) => t.operations.map((o) => opKeyString(o.opKey)))));
            setOpenGroups(new Set(tags.slice(0, 2).map((t) => t.name ?? "")));
            setProjectName(null);
          }
          setStep("confirm");
        } else {
          setIssue(r);
          setStep("source");
        }
      } catch (e) {
        if (alive.current) setError(errorMessage(e));
      } finally {
        if (alive.current) setChecking(false);
      }
    },
    [backend],
  );

  const startCheck = (entry: string | null = null, hosts: string[] = source.allowedHosts) => {
    const src = buildSource(source, hosts, entry);
    if (src) void check(src, target === "existing" ? existingId : null);
  };

  const chooseEntry = (candidate: string) => {
    if (source.method === "upload") {
      startCheck(candidate);
    } else {
      setSource((s) => ({ ...s, pathText: candidate }));
      void check({ kind: "path", path: candidate }, target === "existing" ? existingId : null);
    }
  };

  const allowForeign = () => {
    if (issue?.status !== "needsConfirmation") return;
    const hosts = [...new Set([...source.allowedHosts, ...issue.foreign.map((f) => f.host)])];
    setSource((s) => ({ ...s, allowedHosts: hosts }));
    startCheck(null, hosts);
  };

  const allOps: PreviewOperation[] = useMemo(() => preview?.tags.flatMap((t) => t.operations) ?? [], [preview]);
  const selectedOps = allOps.filter((o) => selected.has(opKeyString(o.opKey)));

  const changeTarget = (next: "new" | "existing", projectId: string | null) => {
    setTarget(next);
    setTargetProjectId(projectId);
    // 統合のときは、既存のプロジェクトとの差分を付けて読み直す
    if (used) void check(used, next === "existing" ? (projectId ?? existingId) : null, true);
  };

  const doImport = async () => {
    if (!preview || !used) return;
    const projectId = target === "existing" ? existingId : null;
    const request: ImportRequest = {
      source: used,
      projectId,
      projectName: projectId === null ? (projectName ?? preview.title).trim() || null : null,
      selected: selectedOps.length === allOps.length ? null : selectedOps.map((o) => o.opKey),
      options,
    };
    setStep("importing");
    setError(null);
    try {
      const r = await backend.openapiImport(request);
      if (!alive.current) return;
      if (r.status === "imported") {
        setResult(r);
        setCurrentProjectId(r.project.id);
        setScreen("main");
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: queryKeys.projects }),
          queryClient.invalidateQueries({ queryKey: queryKeys.operations(r.project.id) }),
          queryClient.invalidateQueries({ queryKey: queryKeys.environments(r.project.id) }),
          queryClient.invalidateQueries({ queryKey: ["operation"] }),
          queryClient.invalidateQueries({ queryKey: queryKeys.sourceChanged(r.project.id) }),
        ]);
        if (alive.current) setStep("done");
      } else {
        setIssue(r);
        setStep("source");
      }
    } catch (e) {
      if (alive.current) {
        setError(errorMessage(e));
        setStep("confirm");
      }
    }
  };

  const canCheck = buildSource(source, source.allowedHosts) !== null && !checking;

  return (
    <>
      <header className="import-header">
        <Dialog.Title className="import-title">OpenAPIから取り込み</Dialog.Title>
        <Steps step={step} />
        <Dialog.Close className="icon-btn" aria-label="閉じる" disabled={step === "importing"}>
          <CloseIcon />
        </Dialog.Close>
      </header>

      {step === "source" && (
        <SourceStep
          source={source}
          setSource={(update) => {
            // 指定を変えたら、前の確認結果（入口の選択・別ホスト・エラー）は消す
            setSource(update);
            setIssue(null);
            setError(null);
          }}
          checking={checking}
          issue={issue}
          error={error}
          onChooseEntry={chooseEntry}
          onAllowForeign={allowForeign}
          onDismissIssue={() => setIssue(null)}
        />
      )}
      {step === "confirm" && preview && (
        <ConfirmStep
          preview={preview}
          used={used}
          options={options}
          setOptions={setOptions}
          selected={selected}
          setSelected={setSelected}
          openGroups={openGroups}
          setOpenGroups={setOpenGroups}
          error={error}
          onBack={() => {
            setStep("source");
            setError(null);
          }}
        />
      )}
      {step === "importing" && (
        <div className="import-center" role="status">
          <span className="spinner" aria-hidden="true" />
          取り込んでいます…
        </div>
      )}
      {step === "done" && result && <DoneStep result={result} merged={target === "existing"} />}

      <footer className="import-footer">
        {step === "source" && (
          <>
            <Dialog.Close className="btn import-footer-right" type="button">
              キャンセル
            </Dialog.Close>
            <button type="button" className="btn btn-primary" disabled={!canCheck} onClick={() => startCheck()}>
              {checking ? "読み込み中…" : "内容を確認"}
            </button>
          </>
        )}
        {step === "confirm" && preview && (
          <>
            <div className="import-target">
              <label className="import-target-field">
                取り込み先
                <select
                  className="select select-tall"
                  value={target}
                  disabled={checking}
                  onChange={(e) => changeTarget(e.target.value === "existing" ? "existing" : "new", targetProjectId)}
                >
                  <option value="new">新規プロジェクトを作成</option>
                  <option value="existing" disabled={projectList.length === 0}>
                    既存プロジェクトに統合（差分更新）
                  </option>
                </select>
              </label>
              {target === "new" ? (
                <label className="import-target-field">
                  プロジェクト名
                  <input className="input import-name" value={projectName ?? preview.title} onChange={(e) => setProjectName(e.target.value)} />
                </label>
              ) : (
                <label className="import-target-field">
                  統合先
                  <select
                    className="select select-tall"
                    aria-label="統合先のプロジェクト"
                    value={existingId ?? ""}
                    disabled={checking}
                    onChange={(e) => changeTarget("existing", e.target.value)}
                  >
                    {projectList.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name}
                      </option>
                    ))}
                  </select>
                </label>
              )}
            </div>
            <button type="button" className="btn import-footer-right" onClick={() => setStep("source")}>
              戻る
            </button>
            <button
              type="button"
              className="btn btn-primary"
              disabled={selectedOps.length === 0 || checking || (target === "new" && (projectName ?? preview.title).trim() === "")}
              onClick={() => void doImport()}
            >
              {selectedOps.length} 件を取り込む
            </button>
          </>
        )}
        {step === "done" && (
          <button type="button" className="btn btn-primary import-footer-right" onClick={onClose}>
            閉じる
          </button>
        )}
      </footer>
    </>
  );
}

// ---- 1. ファイル選択 ----

function SourceStep({
  source,
  setSource,
  checking,
  issue,
  error,
  onChooseEntry,
  onAllowForeign,
  onDismissIssue,
}: {
  source: SourceState;
  setSource: (update: (s: SourceState) => SourceState) => void;
  checking: boolean;
  issue: Issue | null;
  error: string | null;
  onChooseEntry: (candidate: string) => void;
  onAllowForeign: () => void;
  onDismissIssue: () => void;
}) {
  const backend = useBackend();
  const url = source.method === "path" && isUrl(source.pathText);

  const pickPath = async (kind: "openapi-file" | "folder") => {
    const picked = await backend.pickPaths(kind);
    if (picked[0]) setSource((s) => ({ ...s, pathText: picked[0] }));
  };

  return (
    <div className="import-source">
      <div className="import-methods" role="radiogroup" aria-label="指定方法">
        <MethodCard
          checked={source.method === "path"}
          onSelect={() => setSource((s) => ({ ...s, method: "path" }))}
          title="パス・URL を指定"
          sub="場所を覚えておき、同期ボタンを押したときに読み直します"
          icon={<FileIcon />}
        />
        <MethodCard
          checked={source.method === "upload"}
          onSelect={() => setSource((s) => ({ ...s, method: "upload" }))}
          title="ファイルを直接アップロード"
          sub="アプリのデータフォルダにコピーして取り込みます"
          icon={<UploadIcon />}
        />
      </div>

      {source.method === "path" ? (
        <div className="import-fields">
          <label className="field-label">
            パスまたは URL
            <input
              className="input input-accent mono"
              aria-label="パスまたは URL"
              value={source.pathText}
              onChange={(e) => setSource((s) => ({ ...s, pathText: e.target.value, allowedHosts: [] }))}
              onKeyDown={(e) => e.key === "Enter" && e.preventDefault()}
              placeholder="~/project/docs/20_design/api/openapi.yaml または https://example.com/openapi.yaml"
              autoComplete="off"
              spellCheck={false}
              autoFocus
            />
          </label>
          <div className="import-pick-row">
            <button type="button" className="btn" onClick={() => void pickPath("openapi-file")}>
              ファイルを選択…
            </button>
            <button type="button" className="btn" onClick={() => void pickPath("folder")}>
              フォルダを選択…
            </button>
            <span className="muted-note">フォルダを指定すると、中の openapi: を持つファイルを探します。分割された yaml も bundle 済みの yaml も読めます。</span>
          </div>
          {url && (
            <label className="field-label">
              アクセストークン（任意）
              <input
                className="input"
                type="password"
                aria-label="アクセストークン"
                value={source.token}
                onChange={(e) => setSource((s) => ({ ...s, token: e.target.value }))}
                autoComplete="off"
                placeholder="非公開のリポジトリを読むときだけ"
              />
              <span className="muted-note">取り込むときに OS のキーチェーンへ保存し、入口と同じサイトにだけ Authorization ヘッダーで送ります。</span>
            </label>
          )}
        </div>
      ) : (
        <UploadZone paths={source.uploadPaths} onChange={(uploadPaths) => setSource((s) => ({ ...s, uploadPaths }))} />
      )}

      {checking && (
        <div className="import-checking" role="status">
          <span className="spinner" aria-hidden="true" />
          yaml を読んでいます…
        </div>
      )}
      {error && (
        <div className="alert alert-danger" role="alert">
          {error}
        </div>
      )}
      {issue?.status === "needsEntry" && (
        <div className="import-issue" role="group" aria-label="入口ファイルの選択">
          <div className="import-issue-title">入口ファイルが複数あります。取り込む入口を選んでください。</div>
          <ul className="entry-list">
            {issue.candidates.map((c) => (
              <li key={c}>
                <span className="mono entry-name">{c}</span>
                <button type="button" className="btn btn-small" disabled={checking} onClick={() => onChooseEntry(c)}>
                  これを取り込む
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {issue?.status === "needsConfirmation" && (
        <div className="import-issue is-warn" role="group" aria-label="別のホストの確認">
          <div className="import-issue-title">入口と別のサイトを指す参照があります。読みに行ってよいか確認してください。</div>
          <ForeignHosts foreign={issue.foreign} />
          <div className="import-issue-actions">
            <button type="button" className="btn btn-primary btn-small" disabled={checking} onClick={onAllowForeign}>
              許可して続ける
            </button>
            <button type="button" className="btn btn-small" onClick={onDismissIssue}>
              やめる
            </button>
          </div>
        </div>
      )}
      {issue?.status === "blocked" && (
        <LoadProblems problems={issue.problems} upload={source.method === "upload"} title="取り込めません。次の問題を直してから、もう一度確認してください。" />
      )}
    </div>
  );
}

function MethodCard({ checked, onSelect, title, sub, icon }: { checked: boolean; onSelect: () => void; title: string; sub: string; icon: ReactNode }) {
  return (
    <label className={`method-card${checked ? " is-on" : ""}`}>
      <input type="radio" name="import-method" checked={checked} onChange={onSelect} />
      <span className="method-card-icon">{icon}</span>
      <span className="method-card-text">
        <span className="method-card-title">{title}</span>
        <span className="muted-note">{sub}</span>
      </span>
    </label>
  );
}

// ---- 2. 内容の確認 ----

const OPTION_LABELS: { key: keyof ImportOptions; label: string }[] = [
  { key: "tagFolders", label: "タグごとにフォルダを作る" },
  { key: "fillFromExamples", label: "example からボディとパラメータを埋める" },
  { key: "applyCookieAuth", label: "cookieAuth を認証設定に反映する" },
  { key: "descriptionAsDoc", label: "description を設計タブのドキュメントにする" },
];

function ConfirmStep({
  preview,
  used,
  options,
  setOptions,
  selected,
  setSelected,
  openGroups,
  setOpenGroups,
  error,
  onBack,
}: {
  preview: ImportPreview;
  used: SourceInput | null;
  options: ImportOptions;
  setOptions: (o: ImportOptions) => void;
  selected: Set<string>;
  setSelected: (s: Set<string>) => void;
  openGroups: Set<string>;
  setOpenGroups: (s: Set<string>) => void;
  error: string | null;
  onBack: () => void;
}) {
  const all = preview.tags.flatMap((t) => t.operations);
  const total = all.length;
  const tagCount = preview.tags.filter((t) => t.name !== null).length;
  const where = used === null ? "" : used.kind === "path" ? used.path : used.kind === "url" ? used.url : `アップロード（${used.paths.length} 件）`;
  const withId = all.filter((o) => o.operationId !== null).length;
  const { inspection } = preview;

  const toggleOp = (key: string, on: boolean) => {
    const next = new Set(selected);
    if (on) next.add(key);
    else next.delete(key);
    setSelected(next);
  };
  const toggleGroup = (ops: PreviewOperation[], on: boolean) => {
    const next = new Set(selected);
    for (const o of ops) {
      if (on) next.add(opKeyString(o.opKey));
      else next.delete(opKeyString(o.opKey));
    }
    setSelected(next);
  };
  const toggleOpen = (name: string) => {
    const next = new Set(openGroups);
    if (next.has(name)) next.delete(name);
    else next.add(name);
    setOpenGroups(next);
  };

  return (
    <div className="import-body">
      <div className="import-left">
        <div className="import-file">
          <FileIcon />
          <div className="import-file-text">
            <span className="mono">{preview.entry}</span>
            <span className="muted-note import-file-sub" title={where}>
              {formatBytes(preview.totalBytes)} · {where}
            </span>
          </div>
          <button type="button" className="btn btn-small btn-ghost" onClick={onBack}>
            変更
          </button>
        </div>

        <dl className="import-dl">
          <dt>タイトル</dt>
          <dd>{preview.title}</dd>
          <dt>APIバージョン</dt>
          <dd>{preview.version}</dd>
          <dt>OpenAPI</dt>
          <dd>
            <span className="badge badge-version mono">{preview.openapiVersion}</span>
          </dd>
          <dt>参照</dt>
          <dd>
            {preview.reference === "bundled"
              ? "bundle 済み（外部ファイルの $ref なし）"
              : `分割された yaml（${preview.fileCount} ファイルを読みました）`}
          </dd>
          <dt>既定の認証</dt>
          <dd className="mono import-auth">{preview.defaultAuth.length > 0 ? preview.defaultAuth.join("、") : "なし"}</dd>
        </dl>

        <fieldset className="import-fieldset">
          <legend>servers → 環境として作成</legend>
          {preview.servers.length === 0 ? (
            <p className="muted-note">servers がないので、環境は作りません（環境変数の画面で手で追加できます）。</p>
          ) : (
            <ul className="import-servers" aria-label="作られる環境">
              {preview.servers.map((s) => (
                <li key={s.url} className="import-server">
                  <CheckIcon />
                  <span className="import-server-text">
                    <span>{s.description ?? s.url}</span>
                    <span className="mono muted-note">{s.url}</span>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </fieldset>

        <fieldset className="import-fieldset">
          <legend>変換オプション</legend>
          {OPTION_LABELS.map(({ key, label }) => (
            <label key={key} className="import-option">
              <input type="checkbox" checked={options[key]} onChange={(e) => setOptions({ ...options, [key]: e.target.checked })} />
              {label}
            </label>
          ))}
        </fieldset>
      </div>

      <div className="import-right">
        <div className="import-list-head">
          <h2 className="import-list-title">取り込むリクエスト</h2>
          <span className="muted-note" role="status">
            {selected.size} / {total} 件を選択中 · タグ {tagCount}
          </span>
          <button
            type="button"
            className="btn btn-small btn-ghost import-footer-right"
            onClick={() => setSelected(selected.size === total ? new Set() : new Set(all.map((o) => opKeyString(o.opKey))))}
          >
            {selected.size === total ? "すべて解除" : "すべて選択"}
          </button>
        </div>

        <div className="import-list" role="group" aria-label="取り込む API">
          {preview.tags.map((tag) => {
            const name = tag.name ?? "";
            const label = tag.name ?? "（タグなし）";
            const picked = tag.operations.filter((o) => selected.has(opKeyString(o.opKey))).length;
            const isOpen = openGroups.has(name);
            return (
              <div key={name} className="import-group">
                <div className="import-group-head">
                  <GroupCheckbox
                    checked={picked === tag.operations.length}
                    indeterminate={picked > 0 && picked < tag.operations.length}
                    label={`${label} のすべての API を選ぶ`}
                    onChange={(on) => toggleGroup(tag.operations, on)}
                  />
                  <button type="button" className="import-group-toggle" aria-expanded={isOpen} onClick={() => toggleOpen(name)}>
                    <span aria-hidden="true" className="import-chevron">
                      {isOpen ? "▾" : "▸"}
                    </span>
                    {label}
                    <span className="muted-note">{picked === tag.operations.length ? `${tag.operations.length} 件` : `${picked} / ${tag.operations.length} 件`}</span>
                  </button>
                </div>
                {isOpen &&
                  tag.operations.map((o) => {
                    const key = opKeyString(o.opKey);
                    return (
                      <label key={key} className="import-op">
                        <input type="checkbox" checked={selected.has(key)} onChange={(e) => toggleOp(key, e.target.checked)} />
                        <span className="tree-method import-op-method" style={{ color: methodColor(o.method) }}>
                          {o.method}
                        </span>
                        <span className="mono import-op-path">{o.path}</span>
                        <span className="muted-note import-op-summary">{o.summary}</span>
                        {o.security === "none" && <span className="badge badge-ok import-op-badge">認証なし</span>}
                      </label>
                    );
                  })}
              </div>
            );
          })}
        </div>

        <div className="import-inspection" role="group" aria-label="検査結果">
          {inspection.duplicateOperationIds.length === 0 ? (
            <InspectLine tone="ok">
              {withId === total ? `operationId ${total} 件はすべて一意です` : `operationId のある ${withId} 件はすべて一意です（ない ${total - withId} 件は「METHOD path」をキーにします）`}
            </InspectLine>
          ) : (
            inspection.duplicateOperationIds.map((d) => (
              <InspectLine key={d.operationId} tone="warn">
                operationId「{d.operationId}」が重複しています（{d.operations.join("、")}）。重複したものは「METHOD path」をキーにします
              </InspectLine>
            ))
          )}
          {inspection.unresolvedRefs.map((u, i) => (
            <InspectLine key={i} tone="warn">
              解決できない $ref: <span className="mono">{u.reference}</span>（<span className="mono">{u.from.file}{u.from.line !== null ? `:${u.from.line}` : ""}</span>）{u.reason}
            </InspectLine>
          ))}
          {inspection.noSecurityCount > 0 && (
            <InspectLine tone="ok">
              <span className="mono">security: []</span> の {inspection.noSecurityCount} 件は「認証なし」として取り込みます
            </InspectLine>
          )}
          {inspection.markdownTableCount > 0 && (
            <InspectLine tone="info">description に Markdown の表がある {inspection.markdownTableCount} 件は、設計タブで表として表示します</InspectLine>
          )}
        </div>

        {preview.diff && (
          <div className="import-diff" role="group" aria-label="既存プロジェクトとの差分">
            <div className="import-diff-head">
              既存プロジェクトとの差分:
              {diffIsEmpty(preview.diff)
                ? " 変更はありません"
                : ` 追加 ${preview.diff.added.length} · 変更 ${preview.diff.changed.length} · 削除 ${preview.diff.removed.length}`}
            </div>
            {!diffIsEmpty(preview.diff) && (
              <details className="diff-details">
                <summary>差分の中身を見る</summary>
                <DiffList diff={preview.diff} />
              </details>
            )}
          </div>
        )}
        {error && (
          <div className="alert alert-danger" role="alert">
            {error}
          </div>
        )}
      </div>
    </div>
  );
}

function GroupCheckbox({ checked, indeterminate, label, onChange }: { checked: boolean; indeterminate: boolean; label: string; onChange: (on: boolean) => void }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = indeterminate;
  }, [indeterminate]);
  return <input ref={ref} type="checkbox" aria-label={label} checked={checked} onChange={(e) => onChange(e.target.checked)} />;
}

function InspectLine({ tone, children }: { tone: "ok" | "warn" | "info"; children: ReactNode }) {
  return (
    <div className={`inspect-line is-${tone}`}>
      <span aria-hidden="true" className="inspect-mark">
        {tone === "ok" ? "✓" : tone === "warn" ? "!" : "i"}
      </span>
      <span>{children}</span>
    </div>
  );
}

// ---- 3. 取り込み ----

function DoneStep({ result, merged }: { result: Imported; merged: boolean }) {
  const { report } = result;
  return (
    <div className="import-center import-done" role="status">
      <span className="import-done-mark" aria-hidden="true">
        ✓
      </span>
      <h2 className="import-done-title">{result.operationCount} 件を取り込みました</h2>
      <p className="muted-note">プロジェクト「{result.project.name}」を開きました。</p>
      <ul className="import-done-list">
        {merged && (
          <li>
            追加 {report.added} · 変更 {report.changed} · 削除 {report.removed}
            {report.keptAsDeleted > 0 && `（入力値があるので「削除された API」に残したもの ${report.keptAsDeleted} 件）`}
            {report.renamed > 0 && ` · operationId の付け替えで入力値を引き継いだもの ${report.renamed} 件`}
          </li>
        )}
        {result.createdEnvironments.length > 0 && <li>作った環境: {result.createdEnvironments.join("、")}</li>}
        {result.createdEnvironments.length === 0 && <li>環境は作りませんでした（servers がない、または同名の環境があります）</li>}
      </ul>
    </div>
  );
}
