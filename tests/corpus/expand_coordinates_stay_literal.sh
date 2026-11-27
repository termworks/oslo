# mode: bash
# Stream coordinates are prompt syntax; a script reads these words as bash does.
echo x{1..}y x{..3}y x{..}y {1..} {0} {0:1}
printf '%s\n' a b c | cat - {2..} 2>/dev/null
echo "status=$?"
