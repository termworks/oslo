# mode: bash
exec 3>/dev/null
echo <(true) <(true)
cat <(echo a) <(echo b)
