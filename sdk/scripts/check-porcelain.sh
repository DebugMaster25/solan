#!/usr/bin/env bash

set -eo pipefail
here="$(dirname "$0")"
src_root="$(readlink -f "${here}/..")"
cd "${src_root}"

(
  if [[ -n "$GITHUB_BASE_REF" ]]; then
    branch="$GITHUB_BASE_REF"
    remote=origin
  else
    IFS='/' read -r remote branch < <(git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null) || true
    if [[ -z "$branch" ]]; then
      branch="$remote"
      remote=
    fi
  fi

  if [[ -n "$remote" ]] && ! git remote | grep --quiet "^$remote\$" 2>/dev/null; then
    echo "WARNING: Remote \`$remote\` not configured for this working directory. Assuming it is actually part of the branch name"
    branch="$remote"/"$branch"
    remote=
  fi

  if [[ -z "$branch" || -z "$remote" ]]; then
    msg="Cannot determine remote target branch. Set one with \`git branch --set-upstream-to=TARGET\`"
    if [[ -n "$CI" ]]; then
      echo "ERROR: $msg" 1>&2
      exit 1
    else
      echo "WARNING: $msg" 1>&2
    fi
  fi

  # Look for failed mergify.io backports by searching leftover conflict markers
  # Also check for any trailing whitespaces!
  if [[ -n "$remote" ]]; then
    echo "Checking remote \`$remote\` for updates to target branch \`$branch\`"
    git fetch --quiet "$remote" "$branch"
    target="$remote"/"$branch"
  else
    echo "WARNING: Target branch \`$branch\` appears to be local. No remote updates will be considered."
    target="$branch"
  fi
  set -x
  git diff "$target" --check --oneline
)

# Disallow uncommitted Cargo.lock changes
(
  cargo tree >/dev/null
  set +e
  if ! git diff --exit-code; then
    cat <<EOF 1>&2

Error: Uncommitted Cargo.lock changes.
Run 'cargo tree' and commit the result.
EOF
    exit 1
  fi
)
