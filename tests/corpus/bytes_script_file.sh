# mode: bash
# A script file, a script on stdin, a `-c` string and a positional parameter are all bytes: one
# that is not UTF-8 stays itself, in a here-document body as anywhere else.
printf 'cat <<E\na\377b\nE\nx=\376; echo "${#x}"\n' > s.sh
"$BASH" ./s.sh | od -An -tx1
printf 'printf %%s "\377"\n' | "$BASH" | od -An -tx1
"$BASH" -c "$(printf 'printf %%s "\375"')" | od -An -tx1
"$BASH" -c 'printf %s "$1"' _ $'\xfc' | od -An -tx1
