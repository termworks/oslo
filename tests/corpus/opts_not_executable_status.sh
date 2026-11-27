# mode: bash
# 126 for a file that cannot run — not executable on PATH, a directory, a binary that is not a
# program — and 127 for nothing at all. A text file with no `#!` still runs as a script.
d=$(mktemp -d)
mkdir "$d/bin" "$d/dir"
printf '#!/bin/sh\necho never\n' > "$d/bin/tool"
PATH=$d/bin:$PATH
tool 2>/dev/null
echo "not executable on PATH: $?"
"$d/dir" 2>/dev/null
echo "directory: $?"
printf '\177ELF\000\000garbage' > "$d/elf"
chmod +x "$d/elf"
"$d/elf" 2>/dev/null
echo "bad binary: $?"
printf 'echo ran as a script\n' > "$d/script"
chmod +x "$d/script"
"$d/script"
echo "script: $?"
"$d/nosuch" 2>/dev/null
echo "missing: $?"
rm -rf "$d"
