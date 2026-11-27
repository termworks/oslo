# mode: bash
# A `[[ ]]` test spans lines wherever bash skips newlines: before and after `&&` and `||`, right
# after `[[`, after `!`, inside a group, and before `]]`.
[[ a = a
  && -n x ]] && echo before-and
[[
  a = a ]] && echo after-open
[[ -n x
]] && echo before-close
[[ a = b ||
  a = a ]] && echo after-or
[[ a = a
  || b = c
  ]] && echo both
[[ ( a = a
  ) ]] && echo group
[[ !
  a = b ]] && echo negated
[[ $HOME =~ /
]] && echo regex
if [[ -n x &&
      -n y ]]; then
  echo in-if
fi
