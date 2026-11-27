# mode: bash
# A `command_not_found_handle` function answers for a missing command, in a subshell, with the
# command and its arguments as its own.
command_not_found_handle() { x=changed; echo "handled: $1 $# ${*:2}"; return 42; }
x=kept
nosuch_command_zz a b
echo "status $? x=$x"
command_not_found_handle() { :; }
nosuch_command_zz
echo "status $?"
unset -f command_not_found_handle
nosuch_command_zz 2>/dev/null
echo "status $?"
