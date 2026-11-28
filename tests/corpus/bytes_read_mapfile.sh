# mode: bash
# `read` and `mapfile` store the bytes they read, whatever they are, and a delimiter or an IFS
# character can itself be a byte that is not UTF-8.
printf '\xff\xfe\n' | { read -r l; printf %s "$l" | od -An -tx1; }
printf '\xff\xfe\n' | { mapfile -t a; printf %s "${a[0]}" | od -An -tx1; }
printf '\xff\nb\xfe\n' | { mapfile a; declare -p a; }
read -r l <<< $'\xff'
printf %s "$l" | od -An -tx1
printf 'a\xffb\n' | { IFS=$'\xff' read p q; echo "$p|$q"; }
printf 'a\xffb' | { read -d $'\xff' l; echo "$l"; }
printf 'a\xffb\xffc' | { mapfile -d $'\xff' -t a; declare -p a; }
printf '%s\n' $'\xff' | { IFS= read -r -a arr; printf %s "${arr[0]}" | od -An -tx1; }
mapfile -t a < <(printf 'a\xff\nb\xfe\n')
declare -p a
read -r l < <(printf '\xe2\x94\x80\n')
echo "${#l}"
