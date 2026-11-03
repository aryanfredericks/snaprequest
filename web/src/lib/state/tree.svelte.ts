// Sidebar tree state: expanded nodes and the item being dragged.
import type { ItemKind } from "./app.svelte";

export interface DragItem {
  collection: string;
  kind: ItemKind;
  key: string;
}

export const tree = $state({
  /** `collection` or `collection/folder key` → expanded. */
  expanded: {} as Record<string, boolean>,
  dragging: null as DragItem | null,
});

export const nodeId = (collection: string, key = "") => (key ? `${collection}/${key}` : collection);

/** Where a drop on a row puts the dragged item: into the folder `to`, before sibling `before`. */
export interface DropTarget {
  to: string;
  before: string | null;
}

/** Whether `item` may be dropped into folder `to` (not into itself, same collection). */
export function canDrop(item: DragItem | null, collection: string, to: string): boolean {
  if (!item || item.collection !== collection || item.kind === "flow") return false;
  if (item.kind === "folder" && (to === item.key || to.startsWith(`${item.key}/`))) return false;
  return true;
}
