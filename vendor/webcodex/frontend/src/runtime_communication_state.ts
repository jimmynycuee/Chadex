export class RuntimeCommunicationRefreshCoordinator {
  private generation = 0;
  private inFlight: { includeData: boolean; generation: number; promise: Promise<boolean> } | null = null;

  constructor(private readonly runRefresh: (includeData: boolean) => Promise<boolean>) {}

  refresh(includeData = true): Promise<boolean> {
    const generation = this.generation;
    const current = this.inFlight;
    if (current && current.generation === generation) {
      if (!includeData || current.includeData) return current.promise;
      return current.promise.then(
        () => this.generation === generation ? this.refresh(true) : false,
        () => this.generation === generation ? this.refresh(true) : false,
      );
    }

    const promise = Promise.resolve().then(() => this.runRefresh(includeData));
    const started = { includeData, generation, promise };
    this.inFlight = started;
    const clear = () => {
      if (this.inFlight === started) this.inFlight = null;
    };
    void promise.then(clear, clear);
    return promise;
  }

  reset(): void {
    this.generation += 1;
    this.inFlight = null;
  }
}

export function runtimeCommunicationTranscriptAfterSeq(lastSeq: unknown, limit = 100): number {
  const normalizedLastSeq = typeof lastSeq === "number" && Number.isSafeInteger(lastSeq)
    ? Math.max(0, lastSeq)
    : 0;
  const normalizedLimit = Number.isSafeInteger(limit) && limit > 0 ? limit : 100;
  return Math.max(0, normalizedLastSeq - normalizedLimit);
}
