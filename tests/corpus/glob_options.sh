# mode: bash
# The glob options scripts switch on — nullglob, dotglob, nocaseglob, nocasematch — and GLOBIGNORE,
# which also turns dotglob on while it is set. oslo refused every one of these with an error and
# kept the default, so a script counting `x=(*.log)` saw one element for an empty directory.
export LC_ALL=C.UTF-8
touch a1 b1 ABC .h c.txt d.txt f.md
mkdir sub
set -- zz*
echo "$#"
shopt -s nullglob
set -- zz*
echo "$#"
x=(zz*)
echo "${#x[@]}"
shopt -u nullglob
shopt -s dotglob
printf '[%s]\n' *
shopt -u dotglob
shopt -s nocaseglob
printf '[%s]\n' a*
printf '[%s]\n' *.TXT
shopt -u nocaseglob
shopt -s nocasematch
case ABC in abc) echo y1 ;; *) echo n1 ;; esac
[[ ABC == a* ]] && echo y2 || echo n2
[[ ABC =~ ^a ]] && echo y3 || echo n3
shopt -u nocasematch
[[ ABC == a* ]] && echo y4 || echo n4
GLOBIGNORE='a*'
printf '[%s]\n' [ab]*
printf '[%s]\n' *
GLOBIGNORE='*.txt:*.md'
printf '[%s]\n' [c-f]*
unset GLOBIGNORE
printf '[%s]\n' *
