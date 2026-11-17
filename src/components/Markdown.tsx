import Markdown from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";

import { useBackend } from "../api";

const OPENABLE = /^(https?:|mailto:|tel:)/i;

/**
 * yaml の description を GitHub 形式の Markdown（表を含む）で表示する。
 * HTML は実行せず（生の HTML は読み飛ばし、さらに rehype-sanitize で無害化）、リンクは画面を遷移させずに
 * 既定のブラウザで開く（http・https・mailto・tel のみ）。
 */
export function MarkdownView({ source }: { source: string }) {
  const backend = useBackend();
  return (
    <div className="md">
      <Markdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeSanitize]}
        components={{
          a: ({ href, children }) => (
            <a
              href={href}
              title={href}
              rel="noreferrer noopener"
              onClick={(e) => {
                e.preventDefault();
                if (href && OPENABLE.test(href)) void backend.openExternal(href).catch(() => {});
              }}
            >
              {children}
            </a>
          ),
          table: ({ children }) => (
            <div className="md-table-wrap">
              <table>{children}</table>
            </div>
          ),
        }}
      >
        {source}
      </Markdown>
    </div>
  );
}
