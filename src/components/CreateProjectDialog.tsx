import * as Dialog from "@radix-ui/react-dialog";

import { useUiStore } from "../store/ui";
import { CreateProjectForm } from "./CreateProjectForm";

/** ヘッダーのプロジェクト切り替えから開く「新しいプロジェクト」。 */
export function CreateProjectDialog() {
  const open = useUiStore((s) => s.createDialogOpen);
  const setOpen = useUiStore((s) => s.setCreateDialogOpen);

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay" />
        <Dialog.Content className="dialog" aria-describedby="create-project-description">
          <Dialog.Title className="dialog-title">新しいプロジェクト</Dialog.Title>
          <Dialog.Description id="create-project-description" className="dialog-description">
            プロジェクト名を入れて作成します。作成するとそのプロジェクトに切り替わります。
          </Dialog.Description>
          <CreateProjectForm onCreated={() => setOpen(false)} />
          <Dialog.Close className="btn dialog-close" type="button">
            キャンセル
          </Dialog.Close>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
