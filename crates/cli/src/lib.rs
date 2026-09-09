//! The workspace's crates under one `apk_fetch::` namespace.

pub use contract;
pub use fetch;

pub mod providers {
    pub use apkcombo;
    pub use apkmirror;
    pub use apkpure;
}
