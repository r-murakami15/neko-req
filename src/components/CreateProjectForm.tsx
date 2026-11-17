import { useId, useState, type FormEvent } from "react";

import { errorMessage } from "../api";
import { useCreateProject } from "../api/queries";
import type { Project } from "../bindings";

/** プロジェクト名を入れて作るフォーム。作成画面とヘッダーのダイアログで共通。 */
export function CreateProjectForm({
  onCreated,
  autoFocus = true,
}: {
  onCreated?: (project: Project) => void;
  autoFocus?: boolean;
}) {
  const inputId = useId();
  const errorId = useId();
  const [name, setName] = useState("");
  const create = useCreateProject();

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (create.isPending) return;
    create.mutate(name, {
      onSuccess: (project) => {
        setName("");
        onCreated?.(project);
      },
    });
  };

  return (
    <form className="form" onSubmit={submit} noValidate>
      <label className="form-label" htmlFor={inputId}>
        プロジェクト名
      </label>
      <input
        id={inputId}
        className="input"
        value={name}
        onChange={(e) => {
          setName(e.target.value);
          if (create.isError) create.reset();
        }}
        placeholder="例: 日本アニメーション ライセンス契約管理システム API"
        autoComplete="off"
        autoFocus={autoFocus}
        aria-invalid={create.isError}
        aria-describedby={create.isError ? errorId : undefined}
      />
      {create.isError && (
        <p id={errorId} className="form-error" role="alert">
          {errorMessage(create.error)}
        </p>
      )}
      <button type="submit" className="btn btn-primary" disabled={create.isPending || name.trim() === ""}>
        {create.isPending ? "作成しています…" : "プロジェクトを作成"}
      </button>
    </form>
  );
}
