# mode: bash
# Case mapping goes one character at a time, as the C library maps them, and `~` toggles. The
# locale decides what a character is: set inside the script, it takes effect on the next line.

x=hello; y=HeLLo; z=abc
echo ${x^} ${x^^} ${x,} ${x,,} ${x~} ${x~~}
echo ${y^} ${y^^} ${y,} ${y,,} ${y~} ${y~~}
echo ${z~~[ab]} ${z~[ab]} ${z~b}
a=(foo bar)
echo ${a[@]^^} ${a[@]~} ${a[*],,[O]} ${a[@]~~}
set -- one Two
echo ${@~} ${*~~}

LC_ALL=C.UTF-8
s='Straße' f='ﬁ' i='İ' e='ÉCOLE' d='ǆ'
echo ${s,,} ${s^^} ${f^^} ${e,,} ${e~~} ${d^} ${s@U}
printf '%s' "${i,,}" | od -An -tx1
echo ${#s} ${#e}

LC_ALL=C
echo ${#s} ${#e}
printf '%s\n' "${e,,}" "${s^^}"
