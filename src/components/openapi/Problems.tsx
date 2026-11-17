import type { ForeignRef, LoadProblem, RefSite } from "../../bindings";

const site = (s: RefSite) => (s.line === null ? s.file : `${s.file}:${s.line}`);

/** 取り込み・同期を止めた問題（足りないファイル・壊れた yaml など）の一覧。 */
export function LoadProblems({ problems, upload = false, title }: { problems: LoadProblem[]; upload?: boolean; title: string }) {
  return (
    <div className="alert alert-danger problems" role="alert">
      <div className="problems-title">{title}</div>
      <ul className="problems-list">
        {problems.map((p, i) => (
          <li key={i}>
            {p.kind === "missingFile" && (
              <>
                <div>
                  足りないファイル: <span className="mono">{p.file}</span>
                </div>
                <div className="problems-sub">{p.reason}</div>
                <div className="problems-sub">
                  参照元:{" "}
                  {p.referencedFrom.map((r, j) => (
                    <span key={j} className="mono problems-ref">
                      {site(r)}
                    </span>
                  ))}
                </div>
                {upload && <div className="problems-sub">このファイルを追加して、もう一度アップロードしてください。</div>}
              </>
            )}
            {p.kind === "yamlError" && (
              <>
                <div>
                  yaml を読めません:{" "}
                  <span className="mono">
                    {p.file}
                    {p.line !== null ? `:${p.line}` : ""}
                    {p.line !== null && p.column !== null ? `:${p.column}` : ""}
                  </span>
                  {p.line !== null && <span className="problems-sub"> （{p.line} 行目{p.column !== null ? ` ${p.column} 列目` : ""}）</span>}
                </div>
                <div className="problems-sub">{p.reason}</div>
              </>
            )}
            {p.kind === "notOpenApi" && (
              <>
                <div>
                  OpenAPI の文書ではありません: <span className="mono">{p.file}</span>
                </div>
                <div className="problems-sub">{p.reason}</div>
              </>
            )}
            {p.kind === "unsupportedVersion" && (
              <>
                <div>
                  対応していない版です: <span className="mono">{p.file}</span>（{p.version}）
                </div>
                <div className="problems-sub">取り込めるのは OpenAPI 3.0 / 3.1 / 3.2 です（Swagger 2.0 は扱いません）。</div>
              </>
            )}
            {p.kind === "unreadable" && (
              <>
                <div>
                  読み込めません: <span className="mono">{p.file}</span>
                </div>
                <div className="problems-sub">{p.reason}</div>
              </>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** 入口と別のホストを指す参照（読む前に確認が要る）。 */
export function ForeignHosts({ foreign }: { foreign: ForeignRef[] }) {
  return (
    <ul className="problems-list">
      {foreign.map((f, i) => (
        <li key={i}>
          <div>
            <span className="mono">{f.host}</span> を読みに行きます
          </div>
          <div className="problems-sub">
            <span className="mono">{f.url}</span>（参照元: <span className="mono">{site(f.referencedFrom)}</span>）
          </div>
        </li>
      ))}
    </ul>
  );
}
