# mode: bash
# every POSIX class in a =~ regex takes glibc's Unicode set under UTF-8, and bytes under C
export LC_ALL=C.UTF-8
classes=(alpha upper lower digit alnum xdigit space blank cntrl graph print punct)
show() {
  local ch line hex
  for hex in "$@"; do
    printf -v ch "\\U$hex"
    line=$hex
    for c in "${classes[@]}"; do
      re="^[[:$c:]]\$"
      [[ $ch =~ $re ]] && line+=" $c"
    done
    echo "$line"
  done
}
show 00000041 000000e9 000000c9 000000df 000001c5 00001f88 0000a72f 00002102 \
  000024d0 00002170 00000345 00000660 00002028 000000a0 00002007 00001680 \
  00003000 00000085 000000ad 00000378 0000e000 00000300 000020ac 00004e00 \
  0001f600 0001d400 000002b0 000002c6 00000149
[[ x =~ [[:foo:]] ]]; echo "unknown class: $?"
[[ Éa =~ ^[[:upper:]][[:lower:]]$ ]] && echo "case pair"
shopt -s nocasematch
[[ é =~ ^[[:upper:]]$ ]] && echo "nocasematch folds"
shopt -u nocasematch
LC_ALL=C
[[ é =~ ^[[:alpha:]]+$ ]]; echo "C alpha: $?"
[[ é =~ ^[^[:alpha:]][^[:punct:]]$ ]]; echo "C bytes: $?"
[[ x =~ [[:foo:]] ]]; echo "C unknown class: $?"
