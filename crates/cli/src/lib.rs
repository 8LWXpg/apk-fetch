//! The workspace's crates under one `apk_fetch::` namespace.

pub use fetch;
pub use provider as core;

pub mod providers {
    pub use apkcombo;
    pub use apkmirror;
    pub use apkpure;
    pub use uptodown;
}
