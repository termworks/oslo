# mode: bash
# `echo -e` decodes each argument on its own: `\E` is escape, `\u`/`\U` are code points (a C99
# escape outside a UTF-8 locale), only `\0NNN` is octal, and `\c` ends the output and the newline.
echo -e 'a\E[0m\e|' | od -An -c
echo -e '\u41\U00000042|\u|\U|é|\U0001F600'
echo -e "\\'" '\"' '\?' '\101' '\0101' '\01234' '\q' 'a\'
echo -e 'a' 'b\c' 'c'; echo after
echo -e '\x41\x4\xZ' | od -An -c
echo -ne 'a\nb'; echo
echo -n -e 'x\tz'; echo
