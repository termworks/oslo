# mode: bash
# needs-bash: 5.3
# a function ending a substitution or a job replaces that process with its last command
lvl() { grep '^SHLVL='; }
one() { env; }
two() { :; env; }
both() { true && env; }
SHLVL=5
x=$(one); lvl <<< "$x"
x=$(two); lvl <<< "$x"
x=$(both); lvl <<< "$x"
lvl < <(one)
lvl < <(two)
outer() { one; }
x=$(outer); lvl <<< "$x"
x=$(one; :); lvl <<< "$x"
piped() { env | cat; }
x=$(piped); lvl <<< "$x"
trap '' USR1
x=$(one); lvl <<< "$x"
two > early & wait
one > single & wait; two > joined & wait; lvl < early; lvl < single; lvl < joined
