# mode: bash
# Here-strings and here-documents hand a command the bytes their text stands for, whether the
# byte came from an expansion or sits literally in the script.
x=$'\xff'
cat <<< "$x$x" | od -An -tx1
cat <<< $'\xff\xfe' | od -An -tx1
cat <<E | od -An -tx1
$x
E
printf 'cat <<E\na\377b\nE\necho "\376"\n' > s.sh
. ./s.sh | od -An -tx1
printf 'x=\377\376\necho "${#x}"\nprintf %%s "$x"\n' > t.sh
. ./t.sh | od -An -tx1
printf 'f() { echo \377; }\nf\ndeclare -f f\n' > u.sh
. ./u.sh | od -An -tx1
