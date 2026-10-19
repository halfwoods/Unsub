import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState, type DragEvent, type KeyboardEvent } from "react";
import { libraryAddToPlaylist, libraryCreatePlaylist, libraryDeletePlaylist, libraryMovePlaylist, libraryRenamePlaylist } from "../api/commands";
import type { Playlist } from "../api/types";
import { playlistSource, useApp } from "../state/store";
import { SourceIcon } from "./icons";

/** Song rows put their ids here when dragged; playlist rows put their own id. */
export const TRACKS_MIME = "application/x-antistreaming-tracks";
const PLAYLIST_MIME = "application/x-antistreaming-playlist";

function RenameField({ playlist }: { playlist: Playlist }) {
  const [name, setName] = useState(playlist.name);
  const setRenaming = useApp((s) => s.setRenamingPlaylist);
  const refreshPlaylists = useApp((s) => s.refreshPlaylists);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.select(), []);
  const commit = () => {
    setRenaming(null);
    const trimmed = name.trim();
    if (trimmed && trimmed !== playlist.name) void libraryRenamePlaylist(playlist.id, trimmed).then(refreshPlaylists);
  };
  return (
    <input
      ref={input}
      className="sidebar-rename"
      value={name}
      onChange={(e) => setName(e.target.value)}
      onMouseDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") setRenaming(null);
        e.stopPropagation();
      }}
      onBlur={commit}
    />
  );
}

function PlaylistRow({ playlist, depth, byParent }: { playlist: Playlist; depth: number; byParent: Map<number | null, Playlist[]> }) {
  const source = playlistSource(playlist.id);
  const selected = useApp((s) => s.source === source);
  const renaming = useApp((s) => s.renamingPlaylist === playlist.id);
  const expanded = useApp((s) => s.expandedFolders.has(playlist.id));
  const { setSource, toggleFolder, setRenamingPlaylist, refreshPlaylists, refresh, openSmartEditor } = useApp.getState();
  const [dropTarget, setDropTarget] = useState(false);
  const folder = playlist.kind === "folder";
  const children = byParent.get(playlist.id) ?? [];

  // Songs drop onto regular playlists; playlists drop onto folders.
  const accepts = (e: DragEvent) =>
    (playlist.kind === "regular" && e.dataTransfer.types.includes(TRACKS_MIME)) ||
    (folder && e.dataTransfer.types.includes(PLAYLIST_MIME));

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setDropTarget(false);
    const tracks = e.dataTransfer.getData(TRACKS_MIME);
    const moved = e.dataTransfer.getData(PLAYLIST_MIME);
    if (tracks) {
      void libraryAddToPlaylist(playlist.id, JSON.parse(tracks) as number[]).then(async () => {
        await refreshPlaylists();
        // Dropping onto the playlist being viewed should show the new rows straight away.
        if (useApp.getState().source === source) await refresh();
      });
    }
    if (!tracks && moved && Number(moved) !== playlist.id) void libraryMovePlaylist(Number(moved), playlist.id, children.length).then(refreshPlaylists);
  };

  const onKeyDown = async (e: KeyboardEvent) => {
    if (e.key !== "Backspace" && e.key !== "Delete") return;
    const confirmed = await ask(`Are you sure you want to delete the ${folder ? "folder" : "playlist"} “${playlist.name}”?`, {
      title: "AntiStreaming",
      kind: "warning",
    });
    if (!confirmed) return;
    await libraryDeletePlaylist(playlist.id);
    await refreshPlaylists();
    setSource("music");
  };

  return (
    <>
      <li
        role="option"
        tabIndex={selected ? 0 : -1}
        aria-selected={selected}
        className={["sidebar-item", depth > 0 ? "sidebar-item--child" : "", dropTarget ? "sidebar-item--drop" : ""].join(" ")}
        style={depth > 1 ? { paddingLeft: 21 + depth * 16 } : undefined}
        draggable={!renaming}
        onDragStart={(e) => e.dataTransfer.setData(PLAYLIST_MIME, String(playlist.id))}
        onDragOver={(e) => {
          if (!accepts(e)) return;
          e.preventDefault();
          setDropTarget(true);
        }}
        onDragLeave={() => setDropTarget(false)}
        onDrop={onDrop}
        onMouseDown={() => setSource(source)}
        onDoubleClick={() =>
          playlist.kind === "smart" && playlist.smart
            ? openSmartEditor({ id: playlist.id, name: playlist.name, rules: playlist.smart })
            : setRenamingPlaylist(playlist.id)
        }
        onKeyDown={(e) => void onKeyDown(e)}
      >
        {folder && (
          <button
            className={expanded ? "sidebar-disclosure sidebar-disclosure--open" : "sidebar-disclosure"}
            aria-label={expanded ? `Collapse ${playlist.name}` : `Expand ${playlist.name}`}
            aria-expanded={expanded}
            style={depth > 0 ? { left: 7 + depth * 16 } : undefined}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => toggleFolder(playlist.id)}
          />
        )}
        <SourceIcon name={folder ? "folder" : playlist.kind === "smart" ? "smart" : "playlist"} />
        {renaming ? <RenameField playlist={playlist} /> : <span className="sidebar-label">{playlist.name}</span>}
      </li>
      {folder && expanded && children.map((child) => <PlaylistRow key={child.id} playlist={child} depth={depth + 1} byParent={byParent} />)}
    </>
  );
}

/** The library's playlists as iTunes' source-list tree; dropping songs on empty space makes a new one. */
export function PlaylistRows() {
  const playlists = useApp((s) => s.playlists);
  const refreshPlaylists = useApp((s) => s.refreshPlaylists);
  const byParent = new Map<number | null, Playlist[]>();
  for (const playlist of playlists) byParent.set(playlist.parentId, [...(byParent.get(playlist.parentId) ?? []), playlist]);

  const onDropNew = (e: DragEvent) => {
    const tracks = e.dataTransfer.getData(TRACKS_MIME);
    if (!tracks) return;
    e.preventDefault();
    void libraryCreatePlaylist("untitled playlist", "regular", null, JSON.parse(tracks) as number[]).then(async (created) => {
      await refreshPlaylists();
      useApp.getState().setSource(playlistSource(created.id));
      useApp.getState().setRenamingPlaylist(created.id);
    });
  };

  return (
    <>
      {(byParent.get(null) ?? []).map((playlist) => (
        <PlaylistRow key={playlist.id} playlist={playlist} depth={0} byParent={byParent} />
      ))}
      <li
        className="sidebar-drop-new"
        aria-hidden="true"
        onDragOver={(e) => e.dataTransfer.types.includes(TRACKS_MIME) && e.preventDefault()}
        onDrop={onDropNew}
      />
    </>
  );
}
