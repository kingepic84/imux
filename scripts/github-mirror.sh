#!/bin/bash
# Pushes a copy of this repository to the GitHub mirror, without the files that only the CI runner
# uses (the list below).
#
#   scripts/github-mirror.sh                  push main and the v* tags to GitHub
#   scripts/github-mirror.sh --to DIR         push to the bare repository DIR instead (a trial run)
#
# Needs the whole history (a full clone, not a shallow one). GitHub gets the token from the
# environment variable MIRROR_GITHUB_TOKEN; it is sent in a header, never put in a URL or a log.
#
# Each commit is copied with its tree minus the excluded paths, and with the same author,
# committer, dates and message. The copies depend only on the originals, so a commit gets the same
# hash every time, GitHub keeps the commits it already has, and a push adds only what is new. The
# hashes on GitHub differ from the ones here because the trees differ.
set -eu

# Paths left out of the mirror: the CI workflows and the files that only they use.
EXCLUDE=".forgejo"

GITHUB_REPO="https://github.com/kingepic84/imux.git"
BRANCH="main"

dest="$GITHUB_REPO"
trial=0
if [ "${1:-}" = "--to" ]; then
  dest="${2:?--to needs a directory}"
  trial=1
fi

cd "$(git rev-parse --show-toplevel)"
if [ "$(git rev-parse --is-shallow-repository)" = "true" ]; then
  echo "github-mirror: this clone is shallow; fetch the whole history first" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir "$work/map"
export GIT_INDEX_FILE="$work/index"

head_commit="$(git rev-parse --verify "refs/heads/$BRANCH" 2>/dev/null || git rev-parse --verify "refs/remotes/origin/$BRANCH")"

# Oldest first, so a commit's parents are copied before it.
git rev-list --reverse --topo-order "$head_commit" | while read -r c; do
  rm -f "$GIT_INDEX_FILE"
  git read-tree "$c"
  # shellcheck disable=SC2086
  git rm -f -r -q --cached --ignore-unmatch -- $EXCLUDE
  tree="$(git write-tree)"

  parents=""
  for p in $(git rev-list --parents -n 1 "$c" | cut -s -d ' ' -f 2-); do
    parents="$parents -p $(cat "$work/map/$p")"
  done

  # shellcheck disable=SC2046
  new="$(
    GIT_AUTHOR_NAME="$(git log -1 --format=%an "$c")" \
    GIT_AUTHOR_EMAIL="$(git log -1 --format=%ae "$c")" \
    GIT_AUTHOR_DATE="$(git log -1 --format=%ad --date=raw "$c")" \
    GIT_COMMITTER_NAME="$(git log -1 --format=%cn "$c")" \
    GIT_COMMITTER_EMAIL="$(git log -1 --format=%ce "$c")" \
    GIT_COMMITTER_DATE="$(git log -1 --format=%cd --date=raw "$c")" \
    git commit-tree "$tree" $parents -F <(git log -1 --format=%B "$c")
  )"
  echo "$new" > "$work/map/$c"
done
unset GIT_INDEX_FILE

new_head="$(cat "$work/map/$head_commit")"
refspecs="$new_head:refs/heads/$BRANCH"
tags=""
for t in $(git tag --list 'v*'); do
  target="$(git rev-parse --verify "$t^{commit}")"
  if [ -f "$work/map/$target" ]; then
    refspecs="$refspecs $(cat "$work/map/$target"):refs/tags/$t"
    tags="$tags $t"
  fi
done

if [ "$trial" = 1 ]; then
  # shellcheck disable=SC2086
  git push --force "$dest" $refspecs
else
  if [ -z "${MIRROR_GITHUB_TOKEN:-}" ]; then
    echo "github-mirror: MIRROR_GITHUB_TOKEN is not set; nothing pushed" >&2
    exit 0
  fi
  auth="$(printf 'x-access-token:%s' "$MIRROR_GITHUB_TOKEN" | base64 | tr -d '\n')"
  # shellcheck disable=SC2086
  git -c "http.extraheader=Authorization: Basic $auth" push --force "$dest" $refspecs
fi
echo "github-mirror: $BRANCH is $new_head${tags:+, tags:$tags}"
