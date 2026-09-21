//! Bakes the system config directory into the binary at build time,
//! mirroring GNU nano's `./configure --sysconfdir=DIR` (which is how nano
//! decides where `$(sysconfdir)/nanorc` lives, typically `/etc/nanorc`).
//!
//! Cargo has no `./configure` step, so the equivalent here is an
//! environment variable read at build time:
//!
//! - unset: defaults to `/etc` (so the system nanorc is looked for at
//!   `/etc/nanorc`, same as a stock nano).
//! - `TICO_SYSCONFDIR=/some/dir`: use that directory instead.
//! - `TICO_SYSCONFDIR=` (empty): disable system-wide config lookup
//!   entirely; the built binary will never look for a system nanorc.
//!
//! Example: `TICO_SYSCONFDIR=/usr/local/etc cargo build --release`

fn main() {
    let sysconfdir = std::env::var("TICO_SYSCONFDIR").unwrap_or_else(|_| "/etc".to_string());
    println!("cargo:rustc-env=TICO_SYSCONFDIR={sysconfdir}");
    println!("cargo:rerun-if-env-changed=TICO_SYSCONFDIR");
}
