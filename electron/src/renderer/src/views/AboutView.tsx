// About panel (AboutView.swift port): version, the copyright/ffmpeg line, and
// the way into Acknowledgements (docs/LICENSING.md §1 obligation 2). Opened
// from the app menu's About item (main process → 'couple:show-about'). A
// modal overlay rather than a window: the shell has one window.
import React, { useEffect } from 'react';
import { env } from '../kit/native';
import { licensesFolder } from '../app/Licenses';
import './about.css';

const { ipcRenderer } = (window as unknown as { require: NodeRequire }).require(
  'electron'
) as typeof import('electron');

/** Capture-phase key handling for a modal overlay: Escape closes it, every
 * other key stops at the overlay so the queue keymap (j/k/v/space…) in
 * App.tsx does not fire underneath. */
export function useModalKeys(onClose: () => void): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
      e.stopPropagation();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onClose]);
}

export function AboutView({
  onClose,
  onAcknowledgements,
}: {
  onClose: () => void;
  onAcknowledgements: () => void;
}): React.JSX.Element {
  useModalKeys(onClose);
  const version = env.COUPLE_APP_VERSION;
  const ffmpegBundled = !!env.COUPLE_FFMPEG_LICENSE_DIR;
  const folder = licensesFolder();
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="about" onClick={(e) => e.stopPropagation()}>
        <div className="about-name">Couple</div>
        <div className="about-version">{version ? `Version ${version}` : 'Development run'}</div>
        <p className="about-copyright">
          © 2026 Tim Beccue.{' '}
          {ffmpegBundled
            ? 'ffmpeg and ffprobe (LGPL v2.1 or later) are shipped as separate executables; their licenses and source information are in the application’s resources folder.'
            : 'This build does not bundle ffmpeg; Couple uses the ffmpeg installed on this computer.'}
        </p>
        <div className="about-buttons">
          <button className="primary" onClick={onAcknowledgements}>
            Acknowledgements
          </button>
          {folder && (
            <button onClick={() => void ipcRenderer.invoke('couple:reveal', folder)}>Show Licenses Folder</button>
          )}
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
