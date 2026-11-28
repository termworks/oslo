# mode: bash
# In a UTF-8 locale `\u` encodes UTF-8, `%q` leaves printable characters alone, a leading quote
# reads a whole character, and `%lc`/`%ls` count characters rather than bytes.
LC_ALL=C.UTF-8
printf 'é|\U0001F600|\uD800|\n' | od -An -tx1
printf '%q|%q|%q\n' é 'aé b' "é"$'\e'
printf '%d|%x\n' "'é" "'€"
printf '%lc|%5ls|%.1ls|%-4lc|\n' é é éa é
printf '%c|%.1s|\n' é éa | od -An -tx1
echo -e 'é\U0001F600' | od -An -tx1
