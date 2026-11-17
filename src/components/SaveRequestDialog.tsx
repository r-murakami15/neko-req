import * as Dialog from "@radix-ui/react-dialog";
import { type FormEvent, useId, useState } from "react";

import { errorMessage } from "../api";
import { useSaveTab } from "../api/queries";
import { suggestName } from "../lib/spec";
import { NEW_REQUEST_NAME } from "../lib/spec";
import { type RequestTab, useTabsStore } from "../store/tabs";
import { DialogFrame } from "./Dialog";
import { FolderIcon } from "./Icons";

/** 新しい AdHocRequest を保存するときだけ出す、名前を付けるダイアログ（保存先は「その他」）。 */
export function SaveRequestDialog() {
  const tab = useTabsStore((s) => s.tabs.find((t) => t.id === s.saveDialogTabId));
  const setSaveDialogTab = useTabsStore((s) => s.setSaveDialogTab);
  return (
    <DialogFrame open={tab !== undefined} onOpenChange={(open) => !open && setSaveDialogTab(null)} title="リクエストを保存" wide>
      {tab && <SaveForm key={tab.id} tab={tab} onDone={() => setSaveDialogTab(null)} />}
    </DialogFrame>
  );
}

function SaveForm({ tab, onDone }: { tab: RequestTab; onDone: () => void }) {
  const nameId = useId();
  const descriptionId = useId();
  const save = useSaveTab();
  const [name, setName] = useState(tab.name === NEW_REQUEST_NAME ? suggestName(tab.spec) : tab.name);
  const [description, setDescription] = useState(tab.description);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (save.isPending || name.trim() === "") return;
    save.mutate({ tab, name, description: description.trim() }, { onSuccess: onDone });
  };

  return (
    <form className="save-form" onSubmit={submit} noValidate>
      <div className="field-label">
        <label htmlFor={nameId}>名前</label>
        <input
          id={nameId}
          className="input input-accent"
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            if (save.isError) save.reset();
          }}
          placeholder="例: 取引先一覧取得"
          autoFocus
          autoComplete="off"
          aria-invalid={save.isError}
        />
      </div>

      <div className="field-label">
        <label htmlFor={descriptionId}>説明（任意）</label>
        <textarea
          id={descriptionId}
          className="input"
          rows={3}
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder="例: ログイン状態を確かめる"
        />
      </div>

      <div className="field-label">
        <span>保存先フォルダ</span>
        <div className="folder-list" role="listbox" aria-label="保存先フォルダ">
          <div className="folder-option is-selected" role="option" aria-selected="true">
            <FolderIcon />
            その他
          </div>
        </div>
        <span className="muted-note">手で作ったリクエストは「その他」に保存されます。</span>
      </div>

      {save.isError && (
        <p className="form-error" role="alert">
          {errorMessage(save.error)}
        </p>
      )}

      <div className="dialog-actions">
        <Dialog.Close className="btn" type="button">
          キャンセル
        </Dialog.Close>
        <button type="submit" className="btn btn-primary" disabled={save.isPending || name.trim() === ""}>
          {save.isPending ? "保存しています…" : "保存"}
        </button>
      </div>
    </form>
  );
}
