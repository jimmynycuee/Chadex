export class AdminHttpError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "AdminHttpError";
    this.status = status;
  }
}

type TimerHandle = ReturnType<typeof setInterval>;

type ControllerDependencies<T> = {
  request(token: string, signal: AbortSignal): Promise<T>;
  render(data: T): void;
  showAuthenticated(): void;
  showLocked(message: string): void;
  setStatus(message: string): void;
  showError(message: string): void;
  clearError(): void;
  onUnauthorized?(): void;
  setInterval?(callback: () => void, milliseconds: number): TimerHandle;
  clearInterval?(handle: TimerHandle): void;
};

type ActiveRequest = {
  generation: number;
  id: number;
  token: string;
  controller: AbortController;
  promise: Promise<void>;
};

function isAbortError(error: unknown): boolean {
  return error instanceof DOMException
    ? error.name === "AbortError"
    : Boolean(error && typeof error === "object" && "name" in error && error.name === "AbortError");
}

export class AdminRefreshController<T> {
  private readonly dependencies: ControllerDependencies<T>;
  private generation = 0;
  private requestId = 0;
  private token = "";
  private active: ActiveRequest | null = null;
  private timer: TimerHandle | null = null;

  constructor(dependencies: ControllerDependencies<T>) {
    this.dependencies = dependencies;
  }

  beginSession(token: string): Promise<void> {
    this.invalidateRequests();
    this.token = token;
    this.dependencies.clearError();
    return this.refresh();
  }

  lock(message = ""): void {
    this.invalidateRequests();
    this.token = "";
    this.stopAutoRefresh();
    this.dependencies.showLocked(message);
  }

  refresh(): Promise<void> {
    return this.refreshInternal();
  }

  invalidateAndRefresh(): Promise<void> {
    if (!this.token) return Promise.resolve();
    this.invalidateRequests();
    return this.refreshInternal();
  }

  private refreshInternal(): Promise<void> {
    if (!this.token) return Promise.resolve();
    if (
      this.active &&
      this.active.generation === this.generation &&
      this.active.token === this.token
    ) {
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
        if (!this.isCurrent(generation, token, id)) return;
        this.dependencies.render(data);
        this.dependencies.showAuthenticated();
        this.dependencies.setStatus(`Updated ${new Date().toLocaleTimeString()}`);
      })
      .catch((error: unknown) => {
        if (!this.isCurrent(generation, token, id) || isAbortError(error)) return;
        if (error instanceof AdminHttpError && (error.status === 401 || error.status === 403)) {
          if (this.dependencies.onUnauthorized) this.dependencies.onUnauthorized();
          else this.lock("Administrator authentication required.");
          return;
        }
        this.dependencies.showError("Dashboard refresh failed");
        this.dependencies.setStatus("Refresh failed; showing last successful data.");
      })
      .finally(() => {
        if (this.active?.id === id) this.active = null;
      });

    this.active = { generation, id, token, controller, promise };
    return promise;
  }

  startAutoRefresh(milliseconds: number): void {
    this.stopAutoRefresh();
    if (!this.token) return;
    const schedule = this.dependencies.setInterval || setInterval;
    this.timer = schedule(() => {
      void this.refresh();
    }, milliseconds);
  }

  stopAutoRefresh(): void {
    if (this.timer === null) return;
    const cancel = this.dependencies.clearInterval || clearInterval;
    cancel(this.timer);
    this.timer = null;
  }

  dispose(): void {
    this.invalidateRequests();
    this.stopAutoRefresh();
  }

  private invalidateRequests(): void {
    this.generation += 1;
    this.active?.controller.abort();
    this.active = null;
  }

  private isCurrent(generation: number, token: string, id: number): boolean {
    return (
      this.generation === generation &&
      this.token === token &&
      this.active?.id === id
    );
  }
}
