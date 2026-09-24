export class AdminMutationError extends Error {
    constructor(status, code, activeJobs) {
        super(code);
        this.status = status;
        this.code = code;
        this.activeJobs = activeJobs;
        this.name = "AdminMutationError";
    }
}
function aborted(error) {
    return Boolean(error && typeof error === "object" && "name" in error && error.name === "AbortError");
}
export class AdminMutationController {
    constructor(deps) {
        this.deps = deps;
        this.generation = 0;
        this.token = "";
        this.contexts = new Map();
    }
    beginSession(token) { this.invalidate(); this.token = token; }
    lock() { this.invalidate(); this.token = ""; }
    dispose() { this.invalidate(); this.token = ""; }
    start(kind, target, body) {
        const existing = this.contexts.get(target);
        if (!this.token || existing)
            return null;
        const context = {
            kind, target, body: { ...body }, key: this.deps.keyFactory(), generation: this.generation,
            token: this.token, controller: new AbortController(), pending: false,
        };
        this.contexts.set(target, context);
        return context;
    }
    retry(target) {
        const context = this.contexts.get(target);
        return context ? this.submit(context) : Promise.resolve();
    }
    cancel(target) {
        const context = this.contexts.get(target);
        if (!context?.pending)
            this.contexts.delete(target);
    }
    has(target) { return this.contexts.has(target); }
    isPending(target) { return this.contexts.get(target)?.pending === true; }
    async submit(context) {
        if (!this.current(context) || context.pending)
            return;
        context.pending = true;
        this.deps.pending(context.target, true);
        try {
            await this.deps.request(context.kind, context.token, { ...context.body, idempotency_key: context.key }, context.controller.signal);
            if (!this.current(context))
                return;
            this.deps.outcome(`${context.kind[0].toUpperCase()}${context.kind.slice(1)} completed.`);
            this.contexts.delete(context.target);
            await this.deps.refresh();
        }
        catch (error) {
            if (!this.current(context) || aborted(error))
                return;
            const classified = error instanceof AdminMutationError
                ? error
                : new AdminMutationError(0, "network_error");
            if (classified.code === "unauthorized") {
                this.deps.lock("Administrator authentication required.");
                return;
            }
            this.deps.error(classified.code, context);
            if (classified.code === "revision_conflict" || classified.code === "active_jobs_conflict") {
                await this.deps.refresh();
            }
            if (classified.code === "revision_conflict")
                this.contexts.delete(context.target);
        }
        finally {
            if (this.generation === context.generation && this.token === context.token) {
                context.pending = false;
                this.deps.pending(context.target, false);
            }
        }
    }
    current(context) {
        return this.generation === context.generation && this.token === context.token && this.contexts.get(context.target) === context;
    }
    invalidate() {
        this.generation += 1;
        for (const context of this.contexts.values())
            context.controller.abort();
        this.contexts.clear();
    }
}
