# mode: posix
# `$(<file)` is the file, not a command. There is nothing to run, so the ordinary path forked a
# child that ran nothing and the substitution became the empty string with status 0 — a script
# doing `version=$(<VERSION)` got an empty version and no sign anything was wrong.
printf 'content\n' > f
x=$(<f)
echo "[$x] rc=$?"
printf 'a\nb\n\n\n' > multi
printf '[%s]\n' "$(<multi)"
echo "spaced [$( < f )]"
echo "not the special form [$(cat <f)] [$(<f; echo x)]"
y=$(<missing) 2>/dev/null
echo "missing rc=$? [$y]"
