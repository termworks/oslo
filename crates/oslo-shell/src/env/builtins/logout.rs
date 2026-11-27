//! `logout [n]` — `exit`, for a login shell only.

use crate::env::origin_now;
use crate::env::scope::Environment;
use oslo_base::error::Result;

pub fn builtin_logout(env: &mut Environment, args: &[String]) -> Result<i32> {
    if !env.shell_name.starts_with('-') {
        eprintln!("{}logout: not login shell: use `exit'", origin_now());
        return Ok(1);
    }
    let mut argv = args.to_vec();
    if let Some(first) = argv.first_mut() {
        *first = "exit".to_string();
    }
    super::builtin_exit(env, &argv)
}
