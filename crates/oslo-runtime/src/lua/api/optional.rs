//! The namespaces a build may or may not have.
//!
//! Every one is behind its cargo feature, which is why they are gathered here rather than sitting
//! among the unconditional ones: a reader asking "what does `oslo-minimal` not have?" gets one
//! file, and a config asks the same question the way `docs/features/runtime-features.md` says —
//! `if oslo.nix then … end`.

use oslo_base::value::Table;
use oslo_luavm::Host;
use oslo_shell::env::Environment;
use std::sync::{Arc, Mutex};

/// The namespaces a build may or may not have, each behind its cargo feature — so a config asks
/// whether one is there, `if oslo.nix then`, as `docs/features/runtime-features.md` says.
#[allow(unused_variables)]
pub fn install(oslo: &mut Table, host: &dyn Host, env: &Arc<Mutex<Environment>>) {
    #[cfg(feature = "direnv")]
    oslo.set_str("direnv", super::direnv::build(env));
    #[cfg(feature = "argc")]
    oslo.set_str("args", super::args::build());
    #[cfg(feature = "nix")]
    oslo.set_str("nix", super::nix::build());
    #[cfg(feature = "make")]
    oslo.set_str("make", super::make::build());
    #[cfg(feature = "watch")]
    oslo.set_str("watch", super::watch_service::build());
    #[cfg(feature = "secrets")]
    oslo.set_str("secret", super::secret::build());
    #[cfg(feature = "plugin")]
    oslo.set_str("plugin", crate::plugin::health::build());
}
