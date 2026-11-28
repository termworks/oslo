# mode: bash
# A malformed conversion stops printf with status 1 after what came before it. A field too wide
# for C's int is the same for the conversions C pads, and only a warning for the ones bash pads.
printf 'a%yb\n'; echo " rc=$?"
printf 'x%5'; echo " rc=$?"
printf 'a%9999999999s|b' x; echo " rc=$?"
printf 'a%9999999999b|b' x; echo " rc=$?"
printf 'a%.9999999999b|b' x; echo " rc=$?"
printf '%(%Y)X|' 0; echo " rc=$?"
