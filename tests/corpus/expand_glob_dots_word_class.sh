# mode: bash
# A leading dot is matched only explicitly: by a `.` of the pattern's own, one opening an extglob
# alternative, or one after a `*(…)` or `?(…)` that matched nothing. And `[[:word:]]` is a class.

shopt -s extglob
touch a.txt b.txt c.log .hid .x
echo *(a|c).*
echo ?(x).*
echo @(a|).*
echo +(a).*
echo *(a).hid
echo !(a).*
echo *(.)hid
echo @(.)*
echo .*
shopt -s dotglob
echo *(a|c).*
shopt -u dotglob

rm -f ./*.txt ./*.log
touch a b_ c- d1
echo [[:word:]]*
echo [[:alnum:]_]*
echo *[[:word:]]
x=a_1
[[ $x == [[:word:]]* ]] && echo "word class in [["
[[ - == [[:word:]] ]] || echo "dash is not a word character"
