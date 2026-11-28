# mode: bash
# In a UTF-8 locale a regex's POSIX classes take in every letter, as glibc's do.
export LC_ALL=C.UTF-8
t() { if eval "$1"; then echo "y $1"; else echo "n $1"; fi; }
t '[[ é =~ ^[[:alpha:]]+$ ]]'
t '[[ é1 =~ ^[[:alnum:]]+$ ]]'
t '[[ 1 =~ [[:alpha:]] ]]'
t '[[ _ =~ [[:alnum:]] ]]'
t '[[ é =~ [^[:alpha:]] ]]'
t '[[ x =~ [[:digit:][:alpha:]] ]]'
t '[[ - =~ [[:alpha:]-] ]]'
t '[[ "a b" =~ a[[:space:]]b ]]'
