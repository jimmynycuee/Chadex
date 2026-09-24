#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

REF=${1:-HEAD}
REQUIRE_GITLEAKS=${CHADEX_REQUIRE_GITLEAKS:-0}
GITLEAKS_BIN=${GITLEAKS_BIN:-$(command -v gitleaks || true)}
REQUIRE_SINGLE_ROOT=${CHADEX_PUBLIC_REQUIRE_SINGLE_ROOT:-0}
EXPECTED_EMAIL=${CHADEX_PUBLIC_EXPECT_EMAIL:-}
EXPECTED_GITLEAKS_VERSION=${CHADEX_GITLEAKS_VERSION:-8.29.1}
PRIVATE_USER_PATH='/Users/'"jimmy"
PRIVATE_DEVICE_PATTERN='agent:'"device-"'[0-9a-f]{12,}'
PRIVACY_PATTERN="$PRIVATE_USER_PATH|$PRIVATE_DEVICE_PATTERN"

git rev-parse --verify "$REF^{commit}" >/dev/null

if [ "$REQUIRE_SINGLE_ROOT" = "1" ]; then
  commit_count=$(git rev-list --count "$REF")
  if [ "$commit_count" -ne 1 ]; then
    echo "error: initial public candidate must contain exactly one reachable commit; found $commit_count" >&2
    exit 1
  fi
  parent_count=$(git rev-list --parents -n 1 "$REF" | /usr/bin/awk '{print NF - 1}')
  if [ "$parent_count" -ne 0 ]; then
    echo "error: initial public candidate commit unexpectedly has $parent_count parent(s)" >&2
    exit 1
  fi
fi

if [ "$REF" = "HEAD" ] && [ -n "$(git status --porcelain=v1 --untracked-files=all)" ]; then
  echo "error: public release check requires a clean worktree" >&2
  exit 2
fi

echo "==> Public commit identity hygiene"
if git log --format='%ae%n%ce' "$REF" | grep -Fq 'local@chadex.invalid'; then
  echo "error: placeholder Git identity remains in public history" >&2
  exit 1
fi
if [ -n "$EXPECTED_EMAIL" ]; then
  unexpected=$(git log --format='%ae%n%ce' "$REF" | grep -Fvx "$EXPECTED_EMAIL" || true)
  if [ -n "$unexpected" ]; then
    echo "error: public history contains an unexpected author/committer email" >&2
    exit 1
  fi
fi

echo "==> Public history machine/device privacy scan"
privacy_hits=$(mktemp "${TMPDIR:-/tmp}/chadex-public-privacy.XXXXXX")
trap 'rm -f "$privacy_hits"' EXIT HUP INT TERM
for commit in $(git rev-list "$REF"); do
  git grep -n -I -E "$PRIVACY_PATTERN" "$commit" -- ':!vendor/webcodex' >>"$privacy_hits" 2>/dev/null || true
done
if [ -s "$privacy_hits" ]; then
  cat "$privacy_hits" >&2
  echo "error: machine-specific path or device identifier found in public history" >&2
  exit 1
fi

if [ -n "$GITLEAKS_BIN" ]; then
  actual_gitleaks_version=$("$GITLEAKS_BIN" version | tail -n 1 | tr -d '[:space:]')
  if [ "$actual_gitleaks_version" != "$EXPECTED_GITLEAKS_VERSION" ]; then
    echo "error: gitleaks version mismatch: got $actual_gitleaks_version, expected $EXPECTED_GITLEAKS_VERSION" >&2
    exit 2
  fi
  echo "==> Gitleaks history scan"
  "$GITLEAKS_BIN" git "$ROOT" --log-opts="$REF" --redact --no-banner --exit-code 1
elif [ "$REQUIRE_GITLEAKS" = "1" ]; then
  echo "error: gitleaks is required but was not found" >&2
  exit 2
else
  echo "warning: gitleaks not installed; dedicated secret scan skipped" >&2
fi

rm -f "$privacy_hits"
trap - EXIT HUP INT TERM

echo "Public release history check passed."
