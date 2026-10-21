import * as Menu from "@radix-ui/react-dropdown-menu";
import { Ellipsis, type LucideIcon } from "lucide-react";
import { Button } from "./Button";

export interface MoreMenuItem {
  label: string;
  icon?: LucideIcon;
  onSelect: () => void;
  disabled?: boolean;
  /** Removes or deletes something: shown last, in Signal Red, after a divider. */
  destructive?: boolean;
}

/**
 * Secondary actions for one item behind a "More" button, so rows and panes keep one or two
 * visible actions. `label` names the item for screen readers ("More actions for shop.test").
 */
export function MoreMenu({ label, items }: { label: string; items: MoreMenuItem[] }) {
  const regular = items.filter((i) => !i.destructive);
  const destructive = items.filter((i) => i.destructive);
  return (
    // Non-modal: a modal menu that opens a dialog can leave the page unclickable afterwards.
    <Menu.Root modal={false}>
      <Menu.Trigger asChild>
        <Button variant="ghost" aria-label={label} className="px-2">
          <Ellipsis aria-hidden className="size-4" />
        </Button>
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Content
          align="end"
          sideOffset={4}
          className="z-50 min-w-44 rounded-md border border-border-strong bg-surface-raised p-1 text-sm text-fg shadow-[0_12px_32px_-8px_rgb(0_0_0/0.45)]"
        >
          {regular.map((i) => (
            <Item key={i.label} item={i} />
          ))}
          {regular.length > 0 && destructive.length > 0 && <Menu.Separator className="my-1 h-px bg-border" />}
          {destructive.map((i) => (
            <Item key={i.label} item={i} />
          ))}
        </Menu.Content>
      </Menu.Portal>
    </Menu.Root>
  );
}

function Item({ item }: { item: MoreMenuItem }) {
  const Icon = item.icon;
  return (
    <Menu.Item
      disabled={item.disabled}
      onSelect={item.onSelect}
      className={`flex min-h-8 cursor-default items-center gap-2 rounded px-2 outline-none select-none data-[disabled]:opacity-60 data-[highlighted]:bg-surface-sunken ${
        item.destructive ? "text-danger" : ""
      }`}
    >
      {Icon && <Icon aria-hidden className="size-4" />}
      {item.label}
    </Menu.Item>
  );
}
