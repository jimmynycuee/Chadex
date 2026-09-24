export const RUNTIME_API_BASE = "/api/runtime-console/";
export function isAbortError(error) {
    return error instanceof DOMException
        ? error.name === "AbortError"
        : Boolean(error && typeof error === "object" && "name" in error && error.name === "AbortError");
}
export function abortController(controller) {
    if (controller)
        controller.abort();
}
export async function writeClipboardText(value, clipboard) {
    if (!value)
        return false;
    try {
        const cb = clipboard || (typeof navigator !== "undefined" ? navigator.clipboard : null);
        if (!cb || typeof cb.writeText !== "function")
            return false;
        await cb.writeText(value);
        return true;
    }
    catch {
        return false;
    }
}
export class RuntimeApiClient {
    constructor(apiBase = RUNTIME_API_BASE) {
        this.apiBase = apiBase;
        this.token = "";
    }
    setToken(token) {
        this.token = token;
    }
    getToken() {
        return this.token;
    }
    clearToken() {
        this.token = "";
    }
    async post(path, payload, signal) {
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
            let data = null;
            try {
                data = await response.json();
            }
            catch {
                data = null;
            }
            return { ok: response.ok, status: response.status, data };
        }
        catch (error) {
            if (isAbortError(error))
                return null;
            return { ok: false, status: 0, data: null };
        }
    }
}
