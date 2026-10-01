# Changelog

All notable public Chadex releases are summarized here. Detailed notes remain under `docs/releases/`.

## 0.3.1 (Unreleased)

- Added the animated Code Ferret mascot, driven by observed tool activity, task progress, and completion/failure evidence.
- Added mascot visibility and motion controls, macOS Reduce Motion support, and English / Traditional Chinese labels.
- Kept unknown progress indeterminate and reset mascot reactions when switching projects.
- Release validation is pending; see `docs/releases/0.3.1.md` for the planned checks.

## 0.3.0

- Increased the model-facing long-job observation window from the previous short slicing behavior to a 60-second bounded wait while preserving event-driven terminal/failure wakeups.
- Preserved durable-job identity across observation timeout and truncated output so long work is resumed/observed instead of restarted.
- Added bounded terminal handoff/follow-up correlation for diagnosing `local terminal -> next host/model action` stalls without replaying completed local work.
- Kept the existing in-app updater and free ad-hoc-signed macOS release path; no migration is required from 0.2.x.
- Validated the release direction with paired short/medium benchmarks and a 10–20 minute endurance repository workflow including one ~5-minute bursty validation and one ~6-minute silent validation, with no duplicate long-job execution or terminal stall in the formal run.

## 0.2.3

- Fixed duplicate main windows when reopening Chadex from the Dock after the last visible window was closed.

## 0.2.2

- Hardened packaged resource lookup and updater presentation behavior.

## 0.2.1

- Fixed updater shutdown/reopen reliability.

## 0.2.0

- Added the public in-app updater and long-job observation improvements.

See `docs/releases/` for per-release validation and upgrade notes.
