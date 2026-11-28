# mode: bash
# `$'\u…'` is decoded for the locale the shell starts in: outside UTF-8, past ASCII it stays
# written out, as `printf` and `echo -e` leave it.
echo $'é' $'\U1F600' $'\u41' | od -An -c
printf 'é \U1F600 \u41\n' | od -An -c
echo -e 'é \U1F600' | od -An -c
