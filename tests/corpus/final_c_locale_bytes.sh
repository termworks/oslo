# mode: bash
# In the C locale every byte is a character: substrings, `?`, bracket expressions, `${x//?/-}` and
# a multibyte IFS all count bytes, and `$'\u…'` past ASCII stays written out.
export LC_ALL=C
x='héllo' z=é
echo "1 ${#x} ${x:1:2} ${x:2}" | od -An -c
case é in ?) echo "2 one" ;; ??) echo "2 two" ;; esac
case é in [é]) echo "3 bracket" ;; *) echo "3 no" ;; esac
case é in [[:alpha:]]) echo "4 alpha" ;; *) echo "4 no" ;; esac
[[ é == ?? ]] && echo "5 two bytes"
y='aéb'; IFS='é'; set -- $y; echo "6 $# [$1] [$2] [$3]"; unset IFS
echo "7 ${x//?/-} ${z#?}" | od -An -c
echo "8 ${x/é/E} ${x%l*}"
a=(héllo é); echo "9 ${a[@]#?}" | od -An -c
