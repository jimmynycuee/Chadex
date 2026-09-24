export type RuntimeApiResponse<T = any> = {
  ok: boolean;
  status: number;
  data: T | null;
} | null;

export const RUNTIME_API_BASE = "/api/runtime-console/";

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException
    ? error.name === "AbortError"
    : Boolean(error && typeof error === "object" && "name" in error && (error as any).name === "AbortError");
}

export function abortController(controller: AbortController | null): void {
  if (controller) controller.abort();
}

export async function writeClipboardText(
  value: string,
  clipboard?: { writeText(text: string): Promise<void> }
): Promise<boolean> {
  if (!value) return false;
  try {
    const cb = clipboard || (typeof navigator !== "undefined" ? navigator.clipboard : null);
    if (!cb || typeof cb.writeText !== "function") return false;
    await cb.writeText(value);
    return true;
  } catch {
    return false;
  }
}

export class RuntimeApiClient {
  private token = "";

  constructor(private readonly apiBase: string = RUNTIME_API_BASE) {}

  setToken(token: string): void {
    this.token = token;
  }

  getToken(): string {
    return this.token;
  }

  clearToken(): void {
    this.token = "";
  }

  async post<T = any>(
    path: string,
    payload: any,
    signal?: AbortSignal
  ): Promise<RuntimeApiResponse<T>> {
    try {
      const response = await fetch(this.apiBase + path, {
        method: "POST",
        headers: {
          Authorization: "Bearer " + this.token,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(payload),
        signal,
      });
      let data: any = null;
      try {
        data = await response.json();
      } catch {
        data = null;
      }
      return { ok: response.ok, status: response.status, data };
    } catch (error) {
      if (isAbortError(error)) return null;
      return { ok: false, status: 0, data: null };
    }
  }
}
