# mode: bash
# Where a tilde expands: the start of a word, the value of an assignment, and the value of an
# argument shaped like `name=…`; and where it stays literal.

HOME=/h

echo a=~ --opt=~/x x=~:~ a+=~
printf '%s\n' --prefix=~/local -Dx=~ 1x=~ _x=~ x.y=~ 'x'=~
echo a:~ :~ host:~/f
echo ~/"a b" ~"/x" ~\/ x=\~ x=a~
x=~$USER
echo "${x:0:1}"
echo ~root:~ ~:~/x ~/:~ ~nosuchuser-oslo:~ ~nosuchuser-oslo/b
echo x=~nosuchuser-oslo:~ x=a:~"" x=a:~/"q" x=~/"q" x=:~:~/y

a=b=~; echo "$a"
a=~:b=~; echo "$a"
p=$HOME:~/bin; echo "$p"
export X=~/x Y=a:~; echo "$X $Y"
declare D=~ E=:~; echo "$D $E"
f() { local L=~:~; echo "$L"; }
f

arr=(~ ~/a x=~ a:~ [5]=a:~ [6]=~:~ [7]=a=~)
echo "${arr[@]}"
declare -a darr=(~ x=~)
echo "${darr[@]}"

for i in a=~ ~; do echo "$i"; done
case a=~ in a=/h) echo "case word expands" ;; *) echo "case word literal" ;; esac
cat <<< ~
cat <<< a=~
[[ a=~ == a=/h ]] && echo "cond word expands"

echo "${u:-a:~}" ${u:-a=~} ${u:-~} "~" '~' "${u:-~}"
y=${u:-a:~}; echo "$y"
echo x=${u:-~}

echo ~0 ~+0 ~-0 ~1 ~+1 ~-1 | sed "s|$PWD|<PWD>|g"

set -o posix
echo a=~ x=a:~
export Z=~; echo "$Z"
Q=a:~; echo "$Q"
