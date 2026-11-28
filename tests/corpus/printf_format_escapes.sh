# mode: bash
# The format's own escapes: `\NNN` is at most three octal digits after the backslash, the quote
# escapes lose their backslash, `\c` is literal, and `\u` outside a UTF-8 locale stays a C99
# escape.
printf '\101|\0101|\33[0m|\12|\777|\7' | od -An -tx1
printf '\x41\x4142|\E\e|\"\?\'"'"'|\a\v\f|\z\n' | od -An -c
printf 'a\cb\n'
printf '\%d|abc\\\n' 5
printf 'end\'; echo
printf 'é|\U0001F600|\u41|\U00000042|\u00\n' | od -An -c
printf 'x\x' 2>/dev/null; echo " rc=$?"
printf 'x\u' 2>/dev/null; echo " rc=$?"
