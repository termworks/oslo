# mode: bash
# A shell that is not interactive has no history for `fc`, is not a login shell for `logout`, and
# warns that `bind` changes no line editor while still keeping its table.
fc -l
echo "list rc=$?"
fc -s 2>/dev/null
echo "substitute rc=$?"
fc -n
echo "edit rc=$?"
fc -Z 2>/dev/null
echo "bad rc=$?"
logout 2>/dev/null
echo "logout rc=$?"
bind -l 2>/dev/null | head -3
bind -x '"\C-y": echo hi' 2>/dev/null
bind -X 2>/dev/null
bind -q nosuch 2>/dev/null
echo "bind rc=$?"
bind -Z 2>/dev/null
echo "bind bad rc=$?"
type -t enable help bind complete compgen compopt fc logout
