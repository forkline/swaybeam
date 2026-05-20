#!/bin/bash
set -euo pipefail

ensure_full_history() {
    if git rev-parse --is-shallow-repository 2>/dev/null | grep -q "true"; then
        echo "Shallow clone detected. Fetching full history..."
        git fetch --unshallow --quiet
    fi
    echo "Fetching tags..."
    git fetch --tags --quiet
}

ensure_clean_state() {
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "Working tree is not clean. Commit or stash changes first."
        exit 1
    fi

    BRANCH=$(git rev-parse --abbrev-ref HEAD)
    if [ "$BRANCH" != "main" ]; then
        echo "Not on main branch. Current branch: $BRANCH"
        exit 1
    fi

    git pull origin main --quiet

    if [ "$(git rev-list --count origin/main..HEAD 2>/dev/null || echo 1)" -ne 0 ]; then
        echo "There are commits ahead of origin/main. Push or merge them first."
        echo "CHANGELOG template needs main commit ID."
        exit 1
    fi
}

echo "=== Checking repository state ==="
ensure_clean_state
ensure_full_history

echo ""
echo "=== Recent commits since last release ==="
LATEST_TAG=$(git tag --sort=-creatordate | head -1)
if [ -n "$LATEST_TAG" ]; then
    git log "$LATEST_TAG"..HEAD --oneline
else
    git log --oneline -10
fi

echo ""
echo "=== Bumping version ==="
vim ./Cargo.toml

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' ./Cargo.toml | head -n1)
echo "New version: $VERSION"

echo ""
echo "=== Updating dependencies ==="
just update-version

echo ""
echo "=== Updating changelog ==="
just update-changelog

echo ""
echo "=== Committing release ==="
git add .
git commit -m "release: Version $VERSION"

echo ""
echo "=== Creating release branch and pushing ==="
git checkout -b "release/v$VERSION"
git push -u origin "release/v$VERSION"

echo ""
echo "Release v$VERSION prepared successfully."
echo "Create a PR to merge release/v$VERSION into main."
echo "After merge, create and push tag: git tag v$VERSION && git push origin v$VERSION"