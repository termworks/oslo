# mode: bash
# extglob: the five groups in pathnames, `case`, `[[ ]]` and `${v#…}`, and `!(` as negation.
mkdir e && cd e && touch ab c x.txt y.md z.rs 'a b'
!(false) && echo a negated subshell
shopt -s extglob
echo @(ab|c) *.@(txt|md) !(*.txt|*.md)
echo ?(a)b +(a|b) x*(y) @("a b"|q)
v=ab; echo @($v|zz)
case notes.md in *.@(txt|md)) echo text;; *) echo other;; esac
case a.txt in !(*.txt)) echo not;; *) echo txt;; esac
[[ abc == !(x*) ]] && echo y1
[[ aab == +(a)b ]] && echo y2
v=aaab; echo ${v##+(a)} ${v%%?(b)} ${v//@(a|b)/-}
shopt -u extglob
p='@(ab|c)'; [[ ab == $p ]] && echo always in brackets
case ab in $p) echo matched;; *) echo literal when off;; esac
