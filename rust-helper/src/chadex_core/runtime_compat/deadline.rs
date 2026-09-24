use std::time::Duration;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Deadline {
    deadline_at: Instant,
}

impl Deadline {
    pub(crate) fn after(duration: Duration) -> Self {
        Self::at(Instant::now() + duration)
    }

    pub(crate) fn at(deadline_at: Instant) -> Self {
        Self { deadline_at }
    }

    pub(crate) fn instant(self) -> Instant {
        self.deadline_at
    }

    pub(crate) fn is_elapsed(self) -> bool {
        Instant::now() >= self.deadline_at
    }

    pub(crate) fn cleanup_deadline(self, slack: Duration) -> Instant {
        let now = Instant::now();
        if self.deadline_at > now {
            self.deadline_at.min(now + slack)
        } else {
            now + slack
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nested_work_reuses_the_original_absolute_deadline() {
        let outer = Deadline::after(Duration::from_millis(80));
        tokio::time::sleep(Duration::from_millis(30)).await;
        let nested = Deadline::at(outer.instant());

        assert!(
            nested.instant().saturating_duration_since(Instant::now()) <= Duration::from_millis(60)
        );
        tokio::time::sleep_until(nested.instant()).await;
        assert!(outer.is_elapsed());
    }
}
