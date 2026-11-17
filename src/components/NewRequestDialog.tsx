import * as Dialog from "@radix-ui/react-dialog";

import { useTabsStore } from "../store/tabs";
import { useUiStore } from "../store/ui";
import { DialogFrame } from "./Dialog";

/** タブの「＋」・サイドバーの「＋」から開く「新規作成」。 */
export function NewRequestDialog({ projectId }: { projectId: string }) {
  const open = useUiStore((s) => s.newRequestDialogOpen);
  const setOpen = useUiStore((s) => s.setNewRequestDialogOpen);
  const setCreateDialogOpen = useUiStore((s) => s.setCreateDialogOpen);
  const setImportDialogOpen = useUiStore((s) => s.setImportDialogOpen);
  const openNew = useTabsStore((s) => s.openNew);

  return (
    <DialogFrame
      open={open}
      onOpenChange={setOpen}
      title="新規作成"
      wide
      footer={
        <>
          <Dialog.Close className="btn" type="button">
            キャンセル
          </Dialog.Close>
          <button type="button" className="btn btn-primary" disabled>
            cURL から作成
          </button>
        </>
      }
    >
      <div className="card-grid">
        <button
          type="button"
          className="card is-primary"
          onClick={() => {
            openNew(projectId);
            setOpen(false);
          }}
        >
          <span className="card-title">HTTP リクエスト</span>
          <span className="card-sub">空のリクエストを新しいタブで開く</span>
        </button>
        <button
          type="button"
          className="card"
          onClick={() => {
            setOpen(false);
            setCreateDialogOpen(true);
          }}
        >
          <span className="card-title">プロジェクト</span>
          <span className="card-sub">新しいプロジェクトを作る</span>
        </button>
        <button
          type="button"
          className="card"
          onClick={() => {
            setOpen(false);
            setImportDialogOpen(true);
          }}
        >
          <span className="card-title">OpenAPI から</span>
          <span className="card-sub">YAML / JSON を取り込む</span>
        </button>
      </div>

      <div className="field-label">
        <span>
          cURL コマンドから作成<span className="pending-badge">M6 で実装</span>
        </span>
        <textarea
          className="curl-input"
          rows={4}
          disabled
          aria-label="cURL コマンド"
          placeholder={"curl -X POST http://localhost:8080/api/v1/auth/password-login \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"email\":\"user@example.com\"}'"}
        />
      </div>
    </DialogFrame>
  );
}
