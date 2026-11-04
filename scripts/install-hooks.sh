#!/bin/sh
# Install this repo's git hooks (currently: pre-push shell drift check).
#
# Writes a small stub into the shared hooks dir (.git/hooks, common to every
# worktree). The stub runs scripts/hooks/<name> from whichever worktree is
# pushing, so branches without the script are simply skipped. Works with a
# global core.hooksPath only if that global hook chains to .git/hooks (as
# ~/.git-hooks/_chain does); otherwise git never runs .git/hooks.
# Refuses to overwrite a different existing hook. Re-running is safe.
set -e

hooks_dir="$(git rev-parse --git-common-dir)/hooks"
mkdir -p "$hooks_dir"

name=pre-push
{
    target="$hooks_dir/$name"
    stub="#!/bin/sh
# Installed by scripts/install-hooks.sh — runs the tracked hook from the
# current worktree, if it has one.
hook=\"\$(git rev-parse --show-toplevel)/scripts/hooks/$name\"
[ -x \"\$hook\" ] && exec \"\$hook\" \"\$@\"
exit 0"
    if [ -e "$target" ] && [ "$(cat "$target")" != "$stub" ]; then
        echo "install-hooks: $target already exists and differs; not overwriting." >&2
        echo "  Add a call to scripts/hooks/$name to it by hand." >&2
        exit 1
    fi
    printf '%s\n' "$stub" > "$target"
    chmod +x "$target"
    echo "install-hooks: installed $name -> $target"
}

hooks_path="$(git config --get core.hooksPath || true)"
if [ -n "$hooks_path" ]; then
    echo "install-hooks: note: core.hooksPath=$hooks_path. These hooks run only if" >&2
    echo "  that directory's $name chains to .git/hooks." >&2
fi
