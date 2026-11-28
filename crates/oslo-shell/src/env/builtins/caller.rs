//! `caller` — where the currently executing shell function or sourced file was called from.
//!
//! The idiom it serves is a stack trace out of an `ERR` or `EXIT` trap:
//! `i=0; while caller $i; do i=$((i+1)); done`. That loop terminates on the exit status: 1 as soon
//! as the frame asked for does not exist, 0 while it does.
//!
//! It reads the same stack bash publishes as `FUNCNAME`, `BASH_SOURCE` and `BASH_LINENO`,
//! innermost first: frame `n` prints `BASH_LINENO[n]`, `FUNCNAME[n+1]` and `BASH_SOURCE[n+1]`.

use crate::env::scope::Environment;
use oslo_base::error::Result;

/// `caller [expr]`.
pub fn builtin_caller(env: &mut Environment, args: &[String]) -> Result<i32> {
    // Innermost first, as the three arrays are.
    let frames: Vec<_> = env.frames().iter().rev().collect();
    if frames.is_empty() {
        return Ok(1);
    }

    let Some(operand) = args.get(1) else {
        let source = frames.get(1).map_or("NULL", |frame| frame.file.as_str());
        println!("{} {}", frames[0].line, source);
        return Ok(0);
    };

    let Ok(index) = operand.trim().parse::<usize>() else {
        crate::env::complain(
            args,
            operand,
            &format!("caller: {operand}: invalid number"),
            "not a frame number",
            Some("caller: usage: caller [expr]"),
        );
        return Ok(2);
    };
    let (Some(frame), Some(outer)) = (frames.get(index), frames.get(index + 1)) else {
        return Ok(1);
    };
    println!("{} {} {}", frame.line, outer.name, outer.file);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::builtin_caller;
    use crate::env::Environment;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    /// Outside every frame there is nothing to report, and the status is what says so.
    #[test]
    fn outside_a_frame_it_fails_quietly() {
        let mut env = Environment::new();
        assert_eq!(builtin_caller(&mut env, &argv(&["caller"])).unwrap(), 1);
        assert_eq!(
            builtin_caller(&mut env, &argv(&["caller", "0"])).unwrap(),
            1
        );
    }

    /// A bare `caller` answers for any frame; `caller n` needs a frame beyond the one it names.
    #[test]
    fn a_frame_is_reported_for_each_active_call() {
        let mut env = Environment::new();
        env.enter_function_named("outer").unwrap();
        assert_eq!(builtin_caller(&mut env, &argv(&["caller"])).unwrap(), 0);
        assert_eq!(
            builtin_caller(&mut env, &argv(&["caller", "0"])).unwrap(),
            1
        );

        env.enter_function_named("inner").unwrap();
        assert_eq!(
            builtin_caller(&mut env, &argv(&["caller", "0"])).unwrap(),
            0
        );
        assert_eq!(
            builtin_caller(&mut env, &argv(&["caller", "1"])).unwrap(),
            1
        );

        env.exit_function();
        env.exit_function();
        assert_eq!(builtin_caller(&mut env, &argv(&["caller"])).unwrap(), 1);
    }

    #[test]
    fn a_non_numeric_frame_is_a_usage_error() {
        let mut env = Environment::new();
        env.enter_function_named("f").unwrap();
        assert_eq!(
            builtin_caller(&mut env, &argv(&["caller", "x"])).unwrap(),
            2
        );
        env.exit_function();
    }
}
