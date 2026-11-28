//! Which plugin's file is being loaded, while it is being loaded.
//!
//! # What it is for
//!
//! Two things ask *who is running this*: `oslo.plugin.test` attributes a registered test to the
//! plugin that registered it, and `oslo.secret` decides which store a handle may reach. Both need
//! the same answer and it is not one Lua can be trusted to supply — a plugin naming itself would be
//! a plugin naming somebody else.
//!
//! # What it is not
//!
//! **It is true while a plugin's file is being evaluated, and false everywhere else.** A hook
//! handler, a completion callback or a timer registered by a plugin runs later, with the slot
//! empty, and is indistinguishable from the same call made by a config or at the prompt. So this
//! answers *who acquired a handle*, which is attribution, and never *who is using one*, which
//! would be a sandbox. `docs/features/secrets.md` says so in those words.

use std::cell::RefCell;

/// A plugin, while its file runs.
#[derive(Debug, Clone)]
pub struct Loading {
    pub plugin: String,
    /// The user's secrets the config granted it, in the order the grant lists them.
    pub secrets: Vec<String>,
}

thread_local! {
    static LOADING: RefCell<Option<Loading>> = const { RefCell::new(None) };
}

/// Who is loading, if anybody.
pub fn current() -> Option<Loading> {
    LOADING.with(|slot| slot.borrow().clone())
}

/// Attribute everything `body` does to `plugin`.
///
/// Restores whatever was there rather than clearing, so a plugin loaded from inside another
/// plugin's load does not leave the slot empty for the rest of the outer one.
pub fn while_loading<T>(plugin: &str, secrets: &[String], body: impl FnOnce() -> T) -> T {
    let held = LOADING.with(|slot| {
        slot.borrow_mut().replace(Loading {
            plugin: plugin.to_string(),
            secrets: secrets.to_vec(),
        })
    });
    let outcome = body();
    LOADING.with(|slot| *slot.borrow_mut() = held);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **What a registration is attributed to.** `oslo.plugin.test`'s first argument names the
    /// *test*, and `oslo.secret`'s handle has no argument naming anybody, so both have to learn the
    /// plugin from *when* the call happened. Outside a load there is nobody, which is the answer
    /// that makes a hook indistinguishable from the prompt — deliberately, and documented.
    #[test]
    fn it_is_true_only_while_a_file_is_loading() {
        assert!(current().is_none());
        while_loading("outer", &["gh-token".to_string()], || {
            let held = current().expect("something is loading");
            assert_eq!(held.plugin, "outer");
            assert_eq!(held.secrets, ["gh-token"]);

            // A plugin loaded from inside another's load restores the outer one rather than
            // clearing it, or the rest of the outer file would run attributed to nobody.
            while_loading("inner", &[], || {
                assert_eq!(current().expect("inner").plugin, "inner");
            });
            assert_eq!(current().expect("outer again").plugin, "outer");
        });
        assert!(current().is_none(), "and nothing afterwards");
    }
}
