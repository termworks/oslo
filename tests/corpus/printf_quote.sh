# mode: bash
# `%q` in bash's exact style: a backslash before each special byte (`,`, `{` and `}` included),
# `#` only at the start, `~` only where tilde expansion could see it, and `$'...'` with `\E` for
# anything unprintable. In the C locale a byte above 0x7f is unprintable.
printf '%q\n' 'a b' "it's" '' '~x' 'a~' 'a:~' 'a=~b' '#x' 'x#' '{a,b}' '$x' '\' '^x' 'x^' '!' '%+-./:=@_'
printf '%q\n' $'\e[1m' $'tab\there' $'\r' $'\x01' $'\x7f' $'a\nb' "a'b"$'\n'
printf '%q|%q\n' é 'aé b'
printf '%#q|%#q|%#q|%#q\n' "it's" "'" 'a b' $'a\tb'
printf '%Q|%.3Q|%10.3Q|%-8Q|%.*Q|%.Q|\n' 'a b' 'a b c' 'a b c' 'x y' 2 'a b' 'x y'
printf '%.3q|%8q|%-8q|\n' 'a b' 'a b' 'a b'
