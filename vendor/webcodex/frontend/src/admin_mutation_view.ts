import { AdminMutationController, MutationKind } from "./admin_mutation_controller.js";

export type DialogContext = ReturnType<AdminMutationController["start"]>;

type Adapter = {
  close(): void;
  isOpen(): boolean;
  clearSensitive(): void;
  restoreFocus(): void;
};

export class AdminMutationDialogCoordinator {
  private target = "";
  private context: DialogContext = null;
  private bodyFingerprint = "";
  private cleaning = false;

  constructor(private readonly mutation: AdminMutationController, private readonly adapter: Adapter) {}

  open(target: string, context: DialogContext = null, body?: Record<string, unknown>): void {
    this.cleanup(false);
    this.target = target;
    this.context = context;
    this.bodyFingerprint = body ? JSON.stringify(body) : "";
  }

  async submit(kind: MutationKind, body: Record<string, unknown>): Promise<void> {
    const fingerprint = JSON.stringify(body);
    if (this.context && fingerprint === this.bodyFingerprint && this.mutation.has(this.target)) {
      await this.mutation.retry(this.target);
      return;
    }
    if (this.context || this.mutation.has(this.target)) this.mutation.cancel(this.target);
    this.context = this.mutation.start(kind, this.target, body);
    this.bodyFingerprint = fingerprint;
    if (this.context) await this.mutation.submit(this.context);
  }

  setPendingContext(context: DialogContext, body: Record<string, unknown>): void {
    this.context = context;
    this.bodyFingerprint = JSON.stringify(body);
  }

  cancel(): void { this.cleanup(true); }

  handleCancel(event: { preventDefault(): void }): void {
    event.preventDefault();
    if (this.target && this.mutation.isPending(this.target)) return;
    this.cleanup(true);
  }

  handleClose(): void { this.cleanup(true); }

  closeForSessionEnd(): void { this.cleanup(true); }

  currentTarget(): string { return this.target; }

  private cleanup(closeDialog: boolean): void {
    if (this.cleaning || (!this.target && !this.context && !this.bodyFingerprint)) return;
    this.cleaning = true;
    const target = this.target;
    this.target = "";
    this.context = null;
    this.bodyFingerprint = "";
    if (target) this.mutation.cancel(target);
    this.adapter.clearSensitive();
    if (closeDialog && this.adapter.isOpen()) this.adapter.close();
    this.adapter.restoreFocus();
    this.cleaning = false;
  }
}
