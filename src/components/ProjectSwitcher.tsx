import * as DropdownMenu from "@radix-ui/react-dropdown-menu";

import type { Project } from "../bindings";
import { useUiStore } from "../store/ui";
import { CheckIcon, ChevronDownIcon } from "./Icons";

/** ヘッダーのプロジェクト名。押すとプロジェクトを切り替えたり、新しく作ったりできる。 */
export function ProjectSwitcher({ projects, current }: { projects: Project[]; current: Project }) {
  const setCurrentProjectId = useUiStore((s) => s.setCurrentProjectId);
  const setCreateDialogOpen = useUiStore((s) => s.setCreateDialogOpen);

  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger className="project-switcher" aria-label={`プロジェクトを切り替え（現在: ${current.name}）`}>
        <span className="project-name">{current.name}</span>
        <ChevronDownIcon />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="menu" align="start" sideOffset={6}>
          <DropdownMenu.Label className="menu-label">プロジェクト</DropdownMenu.Label>
          <DropdownMenu.RadioGroup value={current.id} onValueChange={setCurrentProjectId}>
            {projects.map((p) => (
              <DropdownMenu.RadioItem key={p.id} value={p.id} className="menu-item">
                <span className="menu-item-check">
                  <DropdownMenu.ItemIndicator>
                    <CheckIcon />
                  </DropdownMenu.ItemIndicator>
                </span>
                <span className="menu-item-text">{p.name}</span>
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
          <DropdownMenu.Separator className="menu-separator" />
          <DropdownMenu.Item className="menu-item" onSelect={() => setCreateDialogOpen(true)}>
            <span className="menu-item-check" aria-hidden="true">
              +
            </span>
            <span className="menu-item-text">新しいプロジェクトを作成…</span>
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
