# mode: posix
# oslo's `rm` is a builtin; bash runs /bin/rm. A script cannot tell them apart, so they must agree —
# including on the cases that cost data. Each of these was a divergence:
#   a directory removed without -r, an unreadable-but-empty directory left on disk with status 1,
#   and an unreadable directory with contents wrongly reported as removed.
mkdir -p tree/inner
: > tree/inner/file
# A directory needs -r. Status and the tree must both survive.
rm tree 2>/dev/null
echo "no -r: rc=$? still-there=$([ -d tree ] && echo yes || echo no)"
# -f on something absent is not an error.
rm -f definitely-not-here
echo "force-missing: rc=$?"
# -r takes the whole tree.
rm -r tree
echo "recursive: rc=$? gone=$([ -d tree ] && echo no || echo yes)"
# An unreadable but empty directory still unlinks from a writable parent.
mkdir -p shut && chmod 111 shut
rm -rf shut
echo "unreadable-empty: rc=$? gone=$([ -d shut ] && echo no || echo yes)"
# One with contents in it does not, and says so.
mkdir -p full && : > full/f && chmod 555 full
rm -rf full 2>/dev/null
echo "unreadable-full: rc=$? still-there=$([ -d full ] && echo yes || echo no)"
chmod 755 full 2>/dev/null
# A trailing operand list: every one is attempted, and one failure shows.
: > a; : > b
rm a missing b 2>/dev/null
echo "several: rc=$? a=$([ -e a ] && echo yes || echo no) b=$([ -e b ] && echo yes || echo no)"
