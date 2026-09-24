export class AdminMutationDialogCoordinator {
    constructor(mutation, adapter) {
        this.mutation = mutation;
        this.adapter = adapter;
        this.target = "";
        this.context = null;
        this.bodyFingerprint = "";
        this.cleaning = false;
    }
    open(target, context = null, body) {
        this.cleanup(false);
        this.target = target;
        this.context = context;
        this.bodyFingerprint = body ? JSON.stringify(body) : "";
    }
    async submit(kind, body) {
        const fingerprint = JSON.stringify(body);
        if (this.context && fingerprint === this.bodyFingerprint && this.mutation.has(this.target)) {
            await this.mutation.retry(this.target);
            return;
        }
        if (this.context || this.mutation.has(this.target))
            this.mutation.cancel(this.target);
        this.context = this.mutation.start(kind, this.target, body);
        this.bodyFingerprint = fingerprint;
        if (this.context)
            await this.mutation.submit(this.context);
    }
    setPendingContext(context, body) {
        this.context = context;
        this.bodyFingerprint = JSON.stringify(body);
    }
    cancel() { this.cleanup(true); }
    handleCancel(event) {
        event.preventDefault();
        if (this.target && this.mutation.isPending(this.target))
            return;
        this.cleanup(true);
    }
    handleClose() { this.cleanup(true); }
    closeForSessionEnd() { this.cleanup(true); }
    currentTarget() { return this.target; }
    cleanup(closeDialog) {
        if (this.cleaning || (!this.target && !this.context && !this.bodyFingerprint))
            return;
        this.cleaning = true;
        const target = this.target;
        this.target = "";
        this.context = null;
        this.bodyFingerprint = "";
        if (target)
            this.mutation.cancel(target);
        this.adapter.clearSensitive();
        if (closeDialog && this.adapter.isOpen())
            this.adapter.close();
        this.adapter.restoreFocus();
        this.cleaning = false;
    }
}
