#!/usr/bin/env bash
# Every fork in oslo's own code goes through `oslo_base::forksafe::quiesced`.
#
# `fork` keeps only the calling thread, so a lock another thread held at that instant stays locked
# in the child. oslo has detached threads for the `$PATH` scan, the prediction model and the daily
# prune, and its subshell children run the evaluator rather than `exec`ing, so the two meet. The
# gate in `oslo-base/src/forksafe.rs` is what keeps them apart.
#
# A bare `fork()` is reported even where the child looks exec-only: that reading is what went wrong
# before, when a site claimed in a comment that "oslo is single threaded".
#
# Test code is not checked. A test forks under libtest's own threads, which the gate cannot speak
# for, and the suite has its own arrangements.
set -euo pipefail

status=0

while IFS= read -r file; do
    # Everything from `#[cfg(test)]` onwards is test code.
    body_end=$(awk '/#\[cfg\(test\)\]/{print NR-1; found=1; exit} END{if (!found) print NR}' "$file")

    while IFS=: read -r line text; do
        [ "$line" -gt "$body_end" ] && continue
        case "$text" in
            *//*fork\(\)*) continue ;;
        esac
        window=$(sed -n "$((line > 2 ? line - 2 : 1)),${line}p" "$file")
        case "$window" in
            *quiesced*) continue ;;
        esac
        printf '%s:%s: bare fork(); use oslo_base::forksafe::quiesced\n' "$file" "$line" >&2
        status=1
    done < <(grep -n 'fork()' "$file" || true)
done < <(grep -rl 'fork()' --include='*.rs' src crates tools 2>/dev/null \
         | grep -v 'forksafe\.rs$' | grep -v '/tests\?/' | grep -v 'tests\?\.rs$' | grep -v '^crates/[^/]*/tests_')

if [ "$status" -ne 0 ]; then
    echo >&2
    echo "See crates/oslo-base/src/forksafe.rs for what the gate does and why." >&2
fi

exit "$status"
