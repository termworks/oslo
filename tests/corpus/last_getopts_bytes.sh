# mode: bash
# getopts reads option letters as bytes, so a multibyte letter is several options
show() { printf '%s|%s|%s\n' "$1" "$2" "$3" | od -An -tx1; }
while getopts 'é' o -é; do show "$o" "${OPTARG-unset}" "$OPTIND"; done
OPTIND=1
while getopts ':a' o -éa; do show "$o" "${OPTARG-unset}" "$OPTIND"; done
OPTIND=1
while getopts 'x:' o -xé -x é; do show "$o" "$OPTARG" "$OPTIND"; done
OPTIND=1
while getopts ':é:' o -é; do show "$o" "${OPTARG-unset}" "$OPTIND"; done
OPTIND=1
getopts 'a' o -é 2>/dev/null
show "$o" "${OPTARG-unset}" "$OPTIND"
