# mode: bash
# `read -i` is accepted and ignored off a terminal, `getopts` takes `?` as an option when the
# optstring names it, and `mapfile -C` runs its callback every `-c` records with the index and the
# record.
read -i default x <<< "a b"
echo "read rc=$? [$x]"
getopts 'a?' o '-?'
echo "getopts rc=$? o=[$o] OPTARG=[${OPTARG-unset}]"
printf '1\n2\n3\n4\n5\n' | {
    mapfile -t -C 'echo cb' -c 2 arr
    echo "${arr[@]}"
}
printf '1\n2\n3\n' | mapfile -t -C 'printf "<%s|%s>\n"' -c 1 arr
printf '1\n2\n3\n' | {
    mapfile -t -c 0 -C echo r 2>/dev/null
    echo "quantum rc=$?"
}
