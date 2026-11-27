# mode: bash
# An `(( ))` command ends at the `))` that closes its own `((`, and what is inside is arithmetic:
# a `$( )` there is expanded, not run as a command, and `<<` is a shift, not a here-document.
((1 || (1 && 1))) && echo 1
((x = 1 + (2))); echo "2 $x"
((($(echo 7) & 3) == 3)) && echo 3
if (( $(echo 3) > 5 )); then echo big; else echo small; fi
i=0
while (( $(echo $i) < 2 )); do i=$((i + 1)); done
echo "4 $i"
(($(echo 1))) && echo 5
((y = 1 << 2)); echo "6 $y"
for ((k = 0; k < 1 << 1; k++)); do echo "7 $k"; done
for ((k = (1); k < 3; k += (1))); do echo "8 $k"; done
echo *
