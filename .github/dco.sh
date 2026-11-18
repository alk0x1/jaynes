#!/bin/sh
# The Developer Certificate of Origin (https://developercertificate.org), for a pull request from outside: every
# commit in BASE..HEAD, merges aside, carries a line "Signed-off-by: Name <email>" with its own author's name and
# email (git commit -s writes it). Says each commit that lacks it, and fails if any does.
#
#   .github/dco.sh BASE HEAD
set -eu
base=${1:?the base commit}
head=${2:?the head commit}
missing=0
for c in $(git rev-list --no-merges "$base..$head"); do
  author=$(git log -1 --format='%an <%ae>' "$c")
  if ! git log -1 --format='%(trailers:key=Signed-off-by,valueonly)' "$c" | grep -qixF "$author"; then
    echo "$(git log -1 --format='%h %s' "$c" | cut -c1-80): no Signed-off-by: $author"
    missing=$((missing + 1))
  fi
done
if [ "$missing" -gt 0 ]; then
  echo
  echo "$missing commit(s) lack the sign-off. Sign each with git commit -s (git rebase --signoff $(git rev-parse --short "$base") adds it to"
  echo "those already made), then push again. CONTRIBUTING.md says what signing off certifies."
  exit 1
fi
echo "every commit is signed off by its author"
