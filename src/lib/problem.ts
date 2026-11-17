/** `application/problem+json` のボディから、目立たせる項目（title・code・detail）を取り出す。 */
export function parseProblem(contentType: string | null, content: string | null): { title?: string; code?: string; detail?: string } | null {
  if (content === null || !/problem\+json/i.test(contentType ?? "")) return null;
  try {
    const v: unknown = JSON.parse(content);
    if (v === null || typeof v !== "object") return null;
    const o = v as Record<string, unknown>;
    const str = (x: unknown) => (typeof x === "string" && x !== "" ? x : undefined);
    const result = { title: str(o.title), code: str(o.code), detail: str(o.detail) };
    return result.title || result.code || result.detail ? result : null;
  } catch {
    return null;
  }
}
