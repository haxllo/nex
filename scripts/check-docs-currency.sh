#!/usr/bin/env bash
# Docs currency gate.
#
# Usage:
#   scripts/check-docs-currency.sh <changed files...>
#   git diff --name-only origin/master... | scripts/check-docs-currency.sh
#   scripts/check-docs-currency.sh --list
#
# For every source file that cannot change without making a document wrong, the
# matching document must also appear in the changed set.
#
# Bypass, when a change genuinely does not affect the document (a refactor, a
# comment, a private helper):
#
#   NEX_ALLOW_STALE_DOCS=1 ...
#
# The bypass is deliberately per run and noisy in the log, so that using it is
# visible rather than silent.
set -euo pipefail

# source pattern | required document
# Keep this list narrow. A rule here fires on every touch of that file, so a
# false positive trains people to use the bypass, which costs more than the rule
# is worth. Add a rule only when the file cannot change without changing what
# the document says.
RULES=(
  "apps/core/src/config.rs|docs/user/settings.md"
  "apps/core/assets/settings.html|docs/user/settings.md"
  "apps/core/assets/settings.js|docs/user/settings.md"
  "apps/core/src/query_dsl.rs|docs/user/search-syntax.md"
  "apps/core/src/chat_tools.rs|docs/user/assistant.md"
  "apps/core/src/runtime.rs|docs/user/install.md"
  "apps/core/src/overlay/refraction.rs|docs/development/overlay-and-glass.md"
  "apps/core/src/overlay/glass_ps.rs|docs/development/overlay-and-glass.md"
  "apps/core/src/overlay/glass_ps.hlsl|docs/development/overlay-and-glass.md"
  "apps/core/src/mft_discovery.rs|docs/development/index-and-search.md"
  "apps/core/src/discovery.rs|docs/development/index-and-search.md"
  "apps/core/src/search.rs|docs/development/index-and-search.md"
  "apps/core/src/ipc.rs|docs/development/ipc-and-privileges.md"
  "apps/core/src/mft_protocol.rs|docs/development/ipc-and-privileges.md"
)

if [ "${1:-}" = "--list" ]; then
  printf '%-46s %s\n' "SOURCE" "REQUIRED DOCUMENT"
  for rule in "${RULES[@]}"; do
    printf '%-46s %s\n' "${rule%%|*}" "${rule#*|}"
  done
  exit 0
fi

if [ "$#" -gt 0 ]; then
  changed="$(printf '%s\n' "$@")"
else
  changed="$(cat)"
fi

changed="$(printf '%s\n' "$changed" | sed '/^$/d')"

if [ -z "$changed" ]; then
  echo "No changed files given; nothing to check."
  exit 0
fi

annotate() {
  if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
    echo "::error::$1"
  fi
}

allowed=0
case "${NEX_ALLOW_STALE_DOCS:-0}" in
  1|true|TRUE|yes|YES) allowed=1 ;;
esac

missing=0
fired=0
for rule in "${RULES[@]}"; do
  source="${rule%%|*}"
  doc="${rule#*|}"

  printf '%s\n' "$changed" | grep -qxF "$source" || continue
  fired=$((fired + 1))
  printf '%s\n' "$changed" | grep -qxF "$doc" && continue

  missing=1
  if [ "$allowed" -eq 1 ]; then
    echo "SKIP (NEX_ALLOW_STALE_DOCS): $source changed, $doc not updated."
    echo "  Reason given in the PR body is the record for this."
  else
    echo "FAIL: $source changed, but $doc was not updated in the same change."
    annotate "$source changed but $doc was not updated."
  fi
done

if [ "$missing" -eq 0 ]; then
  if [ "$fired" -eq 0 ]; then
    echo "Docs currency: no document-linked source changed."
  else
    echo "Docs currency: all $fired document-linked source(s) changed alongside their document."
  fi
  exit 0
fi

if [ "$allowed" -eq 1 ]; then
  echo
  echo "Bypassed by NEX_ALLOW_STALE_DOCS. Say in the PR body why the document"
  echo "is unaffected, so the next reader can judge whether you were right."
  exit 0
fi

echo
echo "Docs currency: a document tied to a changed source was not updated."
echo "Update it in this change, or, if the change genuinely cannot affect it,"
echo "re-run with NEX_ALLOW_STALE_DOCS=1 and explain why in the PR body."
echo "Mapping: docs/development/maintaining-docs.md"
exit 1
