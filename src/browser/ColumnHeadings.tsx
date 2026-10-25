import { useRef, type PointerEvent } from "react";
import type { SortKey, Track } from "../api/types";
import { useLibraryViews } from "../preferences/libraryViewStore";
import { columnWidth, type ColumnId, type ColumnLayout } from "../preferences/libraryViews";
import { ALL_COLUMNS, columnLabel, type Column } from "./columns";
import { formatColumnText } from "./columnFormat";
import { SEPARATOR, showMenu } from "./songMenu";

const MIME = "application/x-unsub-column";
interface Props {
  columns: Column[]; layout: ColumnLayout; source: string; regular: boolean; editable: boolean;
  sort: SortKey; descending: boolean; tracks: Track[]; sortBy: (key: SortKey) => void;
  preview: (id: ColumnId, width: number | null) => void;
}
export function ColumnHeadings({ columns, layout, source, regular, editable, sort, descending, tracks, sortBy, preview }: Props) {
  const resizing = useRef<{ id: ColumnId; startX: number; startWidth: number; width: number } | null>(null);
  const actions = useLibraryViews.getState();
  const update = (changes: Partial<ColumnLayout>) => actions.updateLayout(source, changes, regular);
  const resize = (id: ColumnId, width: number) => update({ widths: { ...layout.widths, [id]: columnWidth(id, width) } });
  const move = (from: ColumnId, to: ColumnId) => {
    if (from === to) return;
    const order = [...layout.order], index = order.indexOf(to);
    order.splice(order.indexOf(from), 1); order.splice(index, 0, from); update({ order });
  };
  const menu = () => {
    if (!editable) return;
    void showMenu([
      ...ALL_COLUMNS.map(c => ({ text: columnLabel(c.id), checked: layout.visible.includes(c.id as ColumnId), enabled: c.id !== "name", action: () => {
        const id = c.id as ColumnId;
        update({ visible: layout.visible.includes(id) ? layout.visible.filter(key => key !== id) : [...layout.visible, id] });
      } })), SEPARATOR,
      { text: "Show View Options…", action: () => actions.setOptionsOpen(true) },
      { text: "Restore Defaults", action: () => actions.restore(source) },
    ]);
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
        className={`song-list-header-cell song-heading${sorted ? " song-list-header-cell--sorted" : ""}`} draggable={canEdit}
        onContextMenu={event => { if (editable) { event.preventDefault(); menu(); } }}
        onDragStart={event => { if (!canEdit || resizing.current) { event.preventDefault(); return; } event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData(MIME, id); }}
        onDragOver={event => { if (canEdit && event.dataTransfer.types.includes(MIME)) { event.preventDefault(); event.dataTransfer.dropEffect = "move"; } }}
        onDrop={event => { if (!canEdit || !event.dataTransfer.types.includes(MIME)) return; event.preventDefault(); event.stopPropagation(); const from = event.dataTransfer.getData(MIME) as ColumnId; if (layout.order.includes(from)) move(from, id); }}>
        <button className="song-heading-sort" disabled={!column.sort} aria-label={`${columnLabel(id)}${column.sort ? ", sort" : ""}`}
          title={canEdit ? `${columnLabel(id)} — drag to move; Shift+Alt+Left/Right moves with the keyboard` : columnLabel(id)}
          style={{ justifyContent: column.align === "right" ? "flex-end" : "flex-start" }} onClick={() => column.sort && sortBy(column.sort)}
          onKeyDown={event => {
            if (canEdit && event.shiftKey && event.altKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
              event.preventDefault(); event.stopPropagation();
              const visible = layout.order.filter(key => layout.visible.includes(key)), target = visible[visible.indexOf(id) + (event.key === "ArrowLeft" ? -1 : 1)];
              if (target) move(id, target);
            } else if (editable && (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10"))) { event.preventDefault(); event.stopPropagation(); menu(); }
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
