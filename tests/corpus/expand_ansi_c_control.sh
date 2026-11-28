# mode: bash
# `$'\cX'` keeps the low five bits of X upper-cased; `\c\\` is one control-backslash.

printf '%s|' $'\c\\' $'\cZ' $'\c1' $'\ca' $'\c[' $'\c?' | od -An -c
printf '%s|' $'\u41' $'\U000041' $'\x41b' | od -An -c
