import * as Dialog from "@radix-ui/react-dialog";
import type { ReactNode } from "react";

import { CloseIcon } from "./Icons";

interface FrameProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  /** 画面イメージ（新規作成・保存）の幅広のダイアログ。 */
  wide?: boolean;
  children: ReactNode;
  footer?: ReactNode;
}

/** タイトル・本文・フッターのあるダイアログ。 */
export function DialogFrame({ open, onOpenChange, title, description, wide, children, footer }: FrameProps) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay" />
        <Dialog.Content className={`dialog dialog-frame${wide ? " dialog-wide" : ""}`} {...(description ? {} : { "aria-describedby": undefined })}>
          <header className="dialog-header">
            <Dialog.Title className="dialog-heading">{title}</Dialog.Title>
            <Dialog.Close className="icon-btn" aria-label="閉じる">
              <CloseIcon />
            </Dialog.Close>
          </header>
          {description && <Dialog.Description className="dialog-description dialog-pad">{description}</Dialog.Description>}
          <div className="dialog-body">{children}</div>
          {footer && <footer className="dialog-footer">{footer}</footer>}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

interface ConfirmProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
}

/** 「よろしいですか？」の確認。 */
export function ConfirmDialog({ open, onOpenChange, title, description, confirmLabel, danger, onConfirm }: ConfirmProps) {
  return (
    <DialogFrame
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      description={description}
      footer={
        <>
          <Dialog.Close className="btn" type="button">
            キャンセル
          </Dialog.Close>
          <button type="button" className={`btn ${danger ? "btn-danger" : "btn-primary"}`} onClick={onConfirm}>
            {confirmLabel}
          </button>
        </>
      }
    >
      {null}
    </DialogFrame>
  );
}
