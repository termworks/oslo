//! Pure scripts, on luna's functional state.
//!
//! A script whose shebang says `--func` runs here rather than on the session's
//! [`Engine`](crate::Engine): no `oslo.*`, no `io`, `os` or `print`, frozen globals, canonical
//! `pairs` order, and a compile error for any assignment to a captured variable. See
//! `luna::Lua::functional`.

use crate::{convert, failure_in, jit_mode};
use luna::{Closure, Executor, Lua, Value, Variadic};
use oslo_base::value::{LuaResult, Value as Own};

/// Run `source` in a fresh functional state, answering what it returned.
///
/// `args` is the chunk's `...`. `import(name)` reads its text through `loader`, and each name is
/// evaluated once. The results and the chunk's own globals are frozen before they come out. `jit`
/// asks for the native tier, as [`Engine::set_jit`](crate::Engine::set_jit) does.
pub fn run(
    source: &str,
    chunk: &str,
    args: &[Own],
    jit: bool,
    loader: impl Fn(&str) -> Result<Vec<u8>, String> + 'static,
) -> LuaResult<Vec<Own>> {
    let mut lua = Lua::functional();
    jit_mode(&mut lua, jit);
    let (executor, env) = lua
        .try_enter(|ctx| {
            let env = luna::functional::import_environment(ctx, loader);
            let closure = Closure::load_with_env(ctx, Some(chunk), source.as_bytes(), env)?;
            let given: Vec<Value> = args.iter().map(|v| convert::into_lua(ctx, v)).collect();
            let executor = Executor::start(ctx, closure.into(), Variadic(given));
            Ok((ctx.stash(executor), ctx.stash(env)))
        })
        .map_err(|e| failure_in(e, chunk))?;
    lua.finish(&executor)
        .map_err(|e| failure_in(luna::ExternError::from(luna::RuntimeError::new(e)), chunk))?;
    lua.try_enter(|ctx| {
        let Variadic(values): Variadic<Vec<Value>> =
            ctx.fetch(&executor).take_result(ctx).map_err(|e| {
                luna::Error::Runtime(luna::RuntimeError::new(std::io::Error::other(
                    e.to_string(),
                )))
            })??;
        for value in &values {
            luna::functional::freeze(ctx, *value);
        }
        luna::functional::freeze(ctx, ctx.fetch(&env).into());
        Ok(values
            .into_iter()
            .map(|v| convert::from_lua(ctx, v))
            .collect())
    })
    .map_err(|e| failure_in(e, chunk))
}

#[cfg(test)]
mod tests;
