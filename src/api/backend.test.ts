import { describe, expect, it } from "vitest";

import type { AppError, Result } from "../bindings";
import { ApiError, errorMessage, unwrap } from "./backend";

describe("unwrap", () => {
  it("成功なら値を返す", async () => {
    const ok: Result<number, AppError> = { status: "ok", data: 42 };
    await expect(unwrap(Promise.resolve(ok))).resolves.toBe(42);
  });

  it("失敗なら ApiError を投げ、kind と message を保つ", async () => {
    const err: Result<number, AppError> = { status: "error", error: { kind: "notFound", message: "ない" } };
    const thrown = await unwrap(Promise.resolve(err)).catch((e: unknown) => e);
    expect(thrown).toBeInstanceOf(ApiError);
    expect((thrown as ApiError).kind).toBe("notFound");
    expect(errorMessage(thrown)).toBe("ない");
  });
});
