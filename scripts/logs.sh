#!/usr/bin/env bash
# logs.sh — the terminal window into GUI For Duplicity's journald records.
#
# Usage:
#   logs.sh                              # all backup units, today
#   logs.sh <profile>                    # one profile's runs, today
#   logs.sh <profile> -f                 # follow that unit live (Ctrl-C stops)
#   logs.sh <profile> --since "2026-11-08"
#   logs.sh --failures                   # warnings + errors only, all units
#
# Profile names are used exactly as in the GUI (e.g. "Test on 15"); they are
# sanitised the same way the systemd units were named when the schedule was
# created.
set -euo pipefail

stem() {
    local s
    s=$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/-/g; s/--*/-/g; s/^-//; s/-$//')
    [ -z "$s" ] && s="profile"
    printf 'gfd-%s' "$s"
}

profile=""
follow=0
failures=0
since="today"
declare -a extra=()

while [ $# -gt 0 ]; do
    case "$1" in
        -f|--follow)   follow=1 ;;
        --failures)    failures=1 ;;
        --since)       shift; since="$1" ;;
        -h|--help)     sed -n '2,10p' "$0"; exit 0 ;;
        *)             profile="$1" ;;
    esac
    shift
done

declare -a args=(--user --no-pager)

if [ -n "$profile" ]; then
    args+=(-u "$(stem "$profile").service")
else
    args+=(-u 'gfd-*.service')
fi

[ "$failures" = 1 ] && args+=(-p warning)
args+=("--since=$since")
[ "$follow" = 1 ] && args+=(-f)

exec journalctl "${args[@]}"
