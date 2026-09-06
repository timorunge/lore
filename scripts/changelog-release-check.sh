#!/usr/bin/env bash
# changelog-release-check: CHANGELOG.md must document the version being released.
#
# The gate is RELEASE-TIME by design (tendir 596d14, owner: "Follow option 2").
# A per-commit rule fails every refactor and gets satisfied with a whitespace
# line, which converts a drift problem into a noise problem. At a version bump
# the question is answerable: does CHANGELOG.md have a "## [<version>]" section
# with at least one non-empty body line? Both halves are mechanical.
#
# What this must NOT catch: ordinary commits (it is not wired into pre-commit
# or `make check`), and an [Unreleased] section is not required to be empty.
set -euo pipefail

cd "$(dirname "$0")/.."

# An argument overrides the version, which is how the check is self-tested:
# `scripts/changelog-release-check.sh 9.9.9` must fail.
version=${1:-$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')}
[ -n "$version" ] || { echo "FAIL [CHANGELOG-RELEASE] could not read version from Cargo.toml"; exit 1; }

if ! grep -q "^## \[$version\]" CHANGELOG.md; then
    echo "FAIL [CHANGELOG-RELEASE] CHANGELOG.md has no '## [$version]' section for the version being released"
    echo "  Move the [Unreleased] entries under a '## [$version] - $(date +%Y-%m-%d)' heading first."
    exit 1
fi

# The section must have at least one non-empty, non-heading body line before
# the next "## [" heading.
body=$(awk -v v="$version" '
    $0 ~ "^## \\[" v "\\]" { in_section=1; next }
    in_section && /^## \[/ { exit }
    in_section && NF && !/^###/ { count++ }
    END { print count+0 }
' CHANGELOG.md)

if [ "$body" -eq 0 ]; then
    echo "FAIL [CHANGELOG-RELEASE] the '## [$version]' section is empty -- a heading with no entries documents nothing"
    exit 1
fi

echo "PASS [CHANGELOG-RELEASE] CHANGELOG.md documents $version ($body body lines)"
