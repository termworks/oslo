#!/usr/bin/env bash
# Catch completion entries that name a flag no spec declares.
#
# A `completion.flag` key is matched against the flag's longhand — `-u, --euid=` is keyed `euid`,
# and a short-only `-U=` is keyed `U`. A key matching nothing is not an error anywhere: the reader
# applies what it can and drops the rest, so the flag simply completes to nothing and no one finds
# out until they press Tab.
#
# The corpus is generated from Fig and from argc, so nobody reads these files. This is the only
# thing that would notice a hand-written entry keyed on the wrong spelling.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
specs="${1:-$here/share/completion}"

[ -d "$specs" ] || { echo "no specs at $specs — run scripts/completion.sh" >&2; exit 0; }

dead=$(
    for file in "$specs"/*.yaml; do
        awk '
        # The key of a flag declaration: its longhand without dashes, or its only name.
        function keyof(decl,   n, i, parts, name) {
            n = split(decl, parts, ", ")
            name = parts[1]
            for (i = 1; i <= n; i++) if (parts[i] ~ /^--/) { name = parts[i]; break }
            sub(/[=*?].*$/, "", name)
            sub(/^-+/, "", name)
            return name
        }
        /^(flags|persistentflags):$/ { mode = "flags"; next }
        /^completion:$/ { mode = "completion"; next }
        # Any other top-level key ends the block — `commands:` most of all, whose nested flags are
        # indented and belong to a subcommand rather than to this spec.
        /^[a-z]/ { mode = ""; inflag = 0; next }
        mode == "flags" && /^  "/ {
            decl = $0
            sub(/^  "/, "", decl); sub(/":.*$/, "", decl)
            valid[keyof(decl)] = 1
            next
        }
        mode == "completion" && /^  flag:$/ { inflag = 1; next }
        mode == "completion" && /^  [a-z]/ { inflag = 0 }
        inflag && /^    [A-Za-z0-9_-]+:/ {
            key = $0; sub(/^    /, "", key); sub(/:.*$/, "", key)
            used[key] = 1
        }
        END {
            for (key in used) if (!(key in valid))
                printf "%s: `completion.flag.%s` names no flag\n", FILENAME, key
        }
        ' "$file"
    done
)

if [ -n "$dead" ]; then
    printf '%s\n' "$dead" >&2
    printf '\n%s dead completion key(s). Each is a flag that silently completes to nothing.\n' \
        "$(printf '%s\n' "$dead" | wc -l | tr -d ' ')" >&2
    exit 1
fi

printf 'specs: %s files, every completion key names a flag that exists\n' \
    "$(find "$specs" -name '*.yaml' | wc -l | tr -d ' ')"
