//! Umbrella facade: the workspace's crates under one `apk_fetch::` namespace.
//! The leaf crates refer to each other by bare name (`provider`, `fetch`);
//! outside code goes through here.

pub use fetch;
pub use provider as core;

pub mod providers {
    pub use apkcombo;
    pub use apkmirror;
    pub use apkpure;
    pub use uptodown;
}
