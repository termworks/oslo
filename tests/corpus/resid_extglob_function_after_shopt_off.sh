# mode: bash
shopt -s extglob
f() {
  case $1 in @(a|b)) echo match;; *) echo no;; esac
  [[ $1 == +(a) ]] && echo cond
  touch aa bb
  echo +(a)
  x=aab; echo "${x//+(a)/X}"
}
f a
shopt -u extglob
f a
f c
shopt -s extglob; eval 'echo @(b)b'
shopt -u extglob
shopt -s extglob; echo @(a)
echo never
