# mode: bash
exec 2>&1
set -v
echo a
eval 'echo x
echo y'
echo b
