export class AdminHttpError extends Error {
    constructor(status, message) {
        super(message);
        this.name = "AdminHttpError";
        this.status = status;
    }
}
function isAbortError(error) {
    return error instanceof DOMException
        ? error.name === "AbortError"
        : Boolean(error && typeof error === "object" && "name" in error && error.name === "AbortError");
}
export class AdminRefreshController {
    constructor(dependencies) {
        this.generation = 0;
        this.requestId = 0;
        this.token = "";
        this.active = null;
        this.timer = null;
        this.dependencies = dependencies;
    }
    beginSession(token) {
        this.invalidateRequests();
        this.token = token;
        this.dependencies.clearError();
        return this.refresh();
    }
    lock(message = "") {
        this.invalidateRequests();
        this.token = "";
        this.stopAutoRefresh();
        this.dependencies.showLocked(message);
    }
    refresh() {
        return this.refreshInternal();
    }
    invalidateAndRefresh() {
        if (!this.token)
            return Promise.resolve();
        this.invalidateRequests();
        return this.refreshInternal();
    }
    refreshInternal() {
        if (!this.token)
            return Promise.resolve();
        if (this.active &&
            this.active.generation === this.generation &&
            this.active.token === this.token) {
            return this.active.promise;
        }
        const generation = this.generation;
        const token = this.token;
        const id = ++this.requestId;
        const controller = new AbortController();
        this.dependencies.clearError();
        const promise = this.dependencies
            .request(token, controller.signal)
            .then((data) => {
            if (!this.isCurrent(generation, token, id))
                return;
            this.dependencies.render(data);
            this.dependencies.showAuthenticated();
            this.dependencies.setStatus(`Updated ${new Date().toLocaleTimeString()}`);
        })
            .catch((error) => {
            if (!this.isCurrent(generation, token, id) || isAbortError(error))
                return;
            if (error instanceof AdminHttpError && (error.status === 401 || error.status === 403)) {
                if (this.dependencies.onUnauthorized)
                    this.dependencies.onUnauthorized();
                else
                    this.lock("Administrator authentication required.");
                return;
            }
            this.dependencies.showError("Dashboard refresh failed");
            this.dependencies.setStatus("Refresh failed; showing last successful data.");
        })
            .finally(() => {
            if (this.active?.id === id)
                this.active = null;
        });
        this.active = { generation, id, token, controller, promise };
        return promise;
    }
    startAutoRefresh(milliseconds) {
        this.stopAutoRefresh();
        if (!this.token)
            return;
        const schedule = this.dependencies.setInterval || setInterval;
        this.timer = schedule(() => {
            void this.refresh();
        }, milliseconds);
    }
    stopAutoRefresh() {
        if (this.timer === null)
            return;
        const cancel = this.dependencies.clearInterval || clearInterval;
        cancel(this.timer);
        this.timer = null;
    }
    dispose() {
        this.invalidateRequests();
        this.stopAutoRefresh();
    }
    invalidateRequests() {
        this.generation += 1;
        this.active?.controller.abort();
        this.active = null;
    }
    isCurrent(generation, token, id) {
        return (this.generation === generation &&
            this.token === token &&
            this.active?.id === id);
    }
}
