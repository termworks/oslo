# mode: posix
eval 'echo A
fi'
echo "ev=$?"
eval 'fi'
echo never
