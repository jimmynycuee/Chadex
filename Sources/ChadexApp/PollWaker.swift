import Foundation

/// Sleeps until a deadline, but can be woken early. The status poll loop uses
/// it to sleep exactly until the next poll is due (instead of ticking at a fixed
/// rate) while still reacting at once when a state change makes polling more
/// frequent.
@MainActor
final class PollWaker {
    private var continuation: CheckedContinuation<Void, Never>?
    private var signalPending = false

    /// Suspends until `signal()` is called, `seconds` elapse, or the calling
    /// task is cancelled. A signal sent while nobody is waiting makes the next
    /// wait return immediately.
    func wait(seconds: TimeInterval) async {
        if signalPending {
            signalPending = false
            return
        }
        guard seconds > 0 else { return }
        let timer = Task { [weak self] in
            // A cancelled timer must not wake a later wait.
            guard (try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))) != nil else {
                return
            }
            self?.resume()
        }
        await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                if Task.isCancelled {
                    continuation.resume()
                } else {
                    self.continuation = continuation
                }
            }
        } onCancel: {
            Task { @MainActor [weak self] in self?.resume() }
        }
        timer.cancel()
    }

    func signal() {
        if continuation != nil {
            resume()
        } else {
            signalPending = true
        }
    }

    private func resume() {
        let waiting = continuation
        continuation = nil
        waiting?.resume()
    }
}
