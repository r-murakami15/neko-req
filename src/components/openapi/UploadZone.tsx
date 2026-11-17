import { useEffect, useRef, useState } from "react";

import { useBackend } from "../../api";
import { CloseIcon, UploadIcon } from "../Icons";

interface Props {
  paths: string[];
  onChange: (paths: string[]) => void;
}

const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

/** 「ファイルを直接アップロード」。ドラッグ＆ドロップかダイアログで、フォルダ・複数のファイル・zip の絶対パスを集める。 */
export function UploadZone({ paths, onChange }: Props) {
  const backend = useBackend();
  const [over, setOver] = useState(false);
  // ドロップの購読は 1 回だけ張り、最新の値は ref から読む
  const latest = useRef({ paths, onChange });
  latest.current = { paths, onChange };

  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend
      .onFileDrop((e) => {
        if (e.type === "over") setOver(true);
        else if (e.type === "leave") setOver(false);
        else {
          setOver(false);
          latest.current.onChange([...new Set([...latest.current.paths, ...e.paths])]);
        }
      })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else off = unlisten;
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend]);

  const pick = async (kind: "folder" | "files") => {
    const picked = await backend.pickPaths(kind);
    if (picked.length > 0) onChange([...new Set([...paths, ...picked])]);
  };

  return (
    <div className="upload">
      <div className={`dropzone${over ? " is-over" : ""}`} aria-label="アップロードするファイルのドロップ先">
        <UploadIcon />
        <p className="dropzone-title">ここにフォルダ・ファイル・zip をドラッグ＆ドロップ</p>
        <p className="muted-note">分割された yaml はフォルダごと入れてください。yaml / yml / json だけを読みます（zip は展開して読みます）。</p>
        <div className="dropzone-actions">
          <button type="button" className="btn" onClick={() => void pick("files")}>
            ファイルを選択…
          </button>
          <button type="button" className="btn" onClick={() => void pick("folder")}>
            フォルダを選択…
          </button>
        </div>
      </div>
      {paths.length > 0 && (
        <ul className="upload-list" aria-label="選んだもの">
          {paths.map((p) => (
            <li key={p}>
              <span className="mono upload-name">{baseName(p)}</span>
              <span className="muted-note upload-path" title={p}>
                {p}
              </span>
              <button type="button" className="icon-btn" aria-label={`${baseName(p)} を外す`} onClick={() => onChange(paths.filter((x) => x !== p))}>
                <CloseIcon />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
