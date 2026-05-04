pub mod archive;
pub(crate) mod exec;
pub(crate) mod feed;
pub mod file;
pub(crate) mod git;
pub(crate) mod maildir;
#[cfg(feature = "mcp")]
pub(crate) mod mcp;
#[cfg(feature = "s3")]
pub(crate) mod s3;
pub(crate) mod sitemap;
pub(crate) mod youtube;

/// Truncate `urls` to at most `cap` entries, warning if it was over the limit.
pub(crate) fn cap_urls(urls: &mut Vec<String>, cap: usize, label: &str) {
    if urls.len() > cap {
        tracing::warn!(total = urls.len(), cap, "{label} exceeds cap, truncating");
        urls.truncate(cap);
    }
}
