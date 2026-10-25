import { useRef, useState, type PointerEvent } from "react";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import type { SortKey, Track } from "../api/types";
import { useLibraryViews } from "../preferences/libraryViewStore";
import { columnWidth, type ColumnId, type ColumnLayout } from "../preferences/libraryViews";
import { ALL_COLUMNS, columnLabel, type Column } from "./columns";
import { formatColumnText } from "./columnFormat";
import { useApp } from "../state/store";
import { SEPARATOR, showMenu } from "./songMenu";

interface Props {
  columns: Column[]; layout: ColumnLayout; source: string; regular: boolean; editable: boolean;
  sort: SortKey; descending: boolean; tracks: Track[]; sortBy: (key: SortKey) => void;
  preview: (id: ColumnId, width: number | null) => void;
}
export function ColumnHeadings({ columns, layout, source, regular, editable, sort, descending, tracks, sortBy, preview }: Props) {
  const resizing = useRef<{ id: ColumnId; startX: number; startWidth: number; width: number } | null>(null);
  const moving = useRef<{ id: ColumnId; source: string; pointer: number; x: number; y: number; dragged: boolean; heading: HTMLElement; captor: HTMLElement } | null>(null);
  const suppressSort = useRef<ColumnId | null>(null);
  const [dragging, setDragging] = useState<{ from: ColumnId; to: ColumnId | null } | null>(null);
  const actions = useLibraryViews.getState();
  const update = (changes: Partial<ColumnLayout>) => actions.updateLayout(source, changes, regular);
  const resize = (id: ColumnId, width: number) => update({ widths: { ...layout.widths, [id]: columnWidth(id, width) } });
  const move = (from: ColumnId, to: ColumnId) => {
    if (from === to) return;
    const order = [...layout.order], index = order.indexOf(to);
    order.splice(order.indexOf(from), 1); order.splice(index, 0, from); update({ order });
  };
  const dropTarget = (event: PointerEvent<HTMLDivElement>) => {
    const target = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>("[data-column-id]");
    const id = target?.dataset.columnId as ColumnId | undefined;
    return target?.parentElement === moving.current?.heading.parentElement && id && layout.order.includes(id) ? id : null;
  };
  const endMove = (event: PointerEvent<HTMLDivElement>, commit: boolean) => {
    const active = moving.current;
    if (!active || active.pointer !== event.pointerId) return;
    const target = commit && active.dragged && editable && source === active.source ? dropTarget(event) : null;
    moving.current = null; setDragging(null);
    if (active.dragged) suppressSort.current = active.id;
    if (active.captor.hasPointerCapture(event.pointerId)) active.captor.releasePointerCapture(event.pointerId);
    if (target) move(active.id, target);
  };
  const menu = (at: LogicalPosition) => {
    if (!editable) return;
    void showMenu([
      ...ALL_COLUMNS.map(c => ({ text: columnLabel(c.id), checked: layout.visible.includes(c.id as ColumnId), enabled: c.id !== "name", action: () => {
        const id = c.id as ColumnId;
        update({ visible: layout.visible.includes(id) ? layout.visible.filter(key => key !== id) : [...layout.visible, id] });
      } })), SEPARATOR,
      { text: "Show View Options…", action: () => actions.setOptionsOpen(true) },
      { text: "Restore Defaults", action: () => actions.restore(source) },
    ], at).catch(error => useApp.getState().showNotice(`Couldn't open column options: ${String(error)}`));
  };
  const autosize = (column: Column) => {
    const context = document.createElement("canvas").getContext("2d"); if (!context) return;
    context.font = getComputedStyle(document.querySelector(".song-cell") ?? document.body).font;
    let width = context.measureText(columnLabel(column.id)).width + 28;
    const step = Math.max(1, Math.ceil(tracks.length / 256));
    for (let i = 0; i < tracks.length; i += step) width = Math.max(width, context.measureText(formatColumnText(column.id, tracks[i])).width + 18);
    resize(column.id as ColumnId, width);
  };
  const finish = (event: PointerEvent<HTMLDivElement>) => {
    const active = resizing.current; if (!active) return;
    resize(active.id, active.width); preview(active.id, null); resizing.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
  };
  return <>
    <div className="song-list-header-cell" role="columnheader" aria-label="Status" />
    {columns.map(column => {
      const id = column.id as ColumnId, canEdit = editable && column.id !== "position", sorted = column.sort === sort;
      return <div key={id} role="columnheader" aria-sort={sorted ? (descending ? "descending" : "ascending") : "none"}
        className={`song-list-header-cell song-heading${sorted ? " song-list-header-cell--sorted" : ""}`} data-column-id={id}
        style={{ touchAction: canEdit ? "none" : undefined, boxShadow: dragging?.to === id && dragging.from !== id
          ? `inset ${layout.order.indexOf(dragging.from) < layout.order.indexOf(id) ? "-2px" : "2px"} 0 #347bc5` : undefined }}
        onContextMenu={event => { if (editable) { event.preventDefault(); menu(new LogicalPosition(event.clientX, event.clientY)); } }}
        // Native file-drop support replaces HTML drag/drop on desktop webviews.
        // Pointer capture keeps heading moves independent of file and song drags.
        onPointerDown={event => {
          suppressSort.current = null;
          if (!canEdit || resizing.current || event.button !== 0 || !event.isPrimary) return;
          const captor = (event.target as Element).closest<HTMLElement>("button") ?? event.currentTarget;
          moving.current = { id, source, pointer: event.pointerId, x: event.clientX, y: event.clientY, dragged: false, heading: event.currentTarget, captor };
          captor.setPointerCapture(event.pointerId);
        }}
        onPointerMove={event => {
          const active = moving.current;
          if (!active || active.pointer !== event.pointerId || source !== active.source) return;
          if (!active.dragged && Math.hypot(event.clientX - active.x, event.clientY - active.y) < 5) return;
          active.dragged = true; event.preventDefault();
          const to = dropTarget(event);
          setDragging(previous => previous?.to === to && previous.from === active.id ? previous : { from: active.id, to });
        }}
        onPointerUp={event => endMove(event, true)}
        onPointerCancel={event => endMove(event, false)}
        onLostPointerCapture={event => endMove(event, false)}>
        <button className="song-heading-sort" disabled={!column.sort} aria-label={`${columnLabel(id)}${column.sort ? ", sort" : ""}`}
          title={canEdit ? `${columnLabel(id)} — drag to move; Shift+Alt+Left/Right moves with the keyboard` : columnLabel(id)}
          style={{ justifyContent: column.align === "right" ? "flex-end" : "flex-start" }} onClick={event => {
            const suppress = suppressSort.current === id && event.detail !== 0;
            suppressSort.current = null;
            if (!suppress && column.sort) sortBy(column.sort);
          }}
          onKeyDown={event => {
            if (canEdit && event.shiftKey && event.altKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
              event.preventDefault(); event.stopPropagation();
              const visible = layout.order.filter(key => layout.visible.includes(key)), target = visible[visible.indexOf(id) + (event.key === "ArrowLeft" ? -1 : 1)];
              if (target) move(id, target);
            } else if (editable && (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10"))) {
              event.preventDefault(); event.stopPropagation();
              const heading = event.currentTarget.getBoundingClientRect();
              menu(new LogicalPosition(heading.left, heading.bottom));
            }
          }}>
          <span className="song-list-header-title">{column.title}</span>
          {sorted && <svg className="sort-arrow" width="7" height="5" viewBox="0 0 7 5" aria-hidden="true"><path d={descending ? "M0 0h7L3.5 5z" : "M0 5h7L3.5 0z"} /></svg>}
        </button>
        {canEdit && <div className="song-heading-resizer" role="separator" aria-orientation="vertical" aria-label={`Resize ${columnLabel(id)}`}
          aria-valuemin={id === "enabled" ? 22 : 40} aria-valuemax={800} aria-valuenow={column.width} tabIndex={0}
          title="Drag to resize; double-click to fit. Arrow keys resize, Enter fits."
          onDoubleClick={event => { event.stopPropagation(); autosize(column); }}
          onPointerDown={event => { if (event.button !== 0 || !event.isPrimary) return; event.preventDefault(); event.stopPropagation(); resizing.current = { id, startX: event.clientX, startWidth: column.width, width: column.width }; event.currentTarget.setPointerCapture(event.pointerId); }}
          onPointerMove={event => { const active = resizing.current; if (!active || active.id !== id) return; active.width = columnWidth(id, active.startWidth + event.clientX - active.startX); preview(id, active.width); }}
          onPointerUp={finish}
          onPointerCancel={event => { const active = resizing.current; if (active) preview(active.id, null); resizing.current = null; if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }}
          onLostPointerCapture={() => { const active = resizing.current; if (active) preview(active.id, null); resizing.current = null; }}
          onKeyDown={event => {
            if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); event.stopPropagation(); resize(id, column.width + (event.key === "ArrowLeft" ? -1 : 1) * (event.shiftKey ? 20 : 8)); }
            else if (event.key === "Enter") { event.preventDefault(); event.stopPropagation(); autosize(column); }
            else if (event.key === "Home") { event.preventDefault(); event.stopPropagation(); resize(id, columnWidth(id)); }
          }} />}
      </div>;
    })}
  </>;
}
