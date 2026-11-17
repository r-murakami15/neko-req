import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(`${process.cwd()}/src/styles/openapi.css`, "utf8");

describe("設計タブの説明（Markdown）の表", () => {
  it("表の中では overflow-wrap: anywhere を使わない（列が 1 文字幅まで縮んで 404 や code が縦に折り返さないように）", () => {
    expect(css).toMatch(/\.md table\s*\{[^}]*overflow-wrap: break-word;/);
  });

  it("見出しは折り返さない", () => {
    expect(css).toMatch(/\.md th\s*\{[^}]*white-space: nowrap;/);
  });
});
