# mode: bash
# `command_not_found_handle` runs in a forked child, which `BASH_SUBSHELL` does not count — except
# for the last command of a `-c` program, which bash does not fork at all.
command_not_found_handle() {
  echo "differs=$(( $$ != BASHPID )) subshell=$BASH_SUBSHELL"
}
nosuch_command_zz
(nosuch_command_zz)
nosuch_command_zz
