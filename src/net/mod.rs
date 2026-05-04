/// HTTP client with SSRF protection, rate limiting, robots.txt, and caching.
pub(crate) mod client;
/// robots.txt enforcement for the HTTP client.
mod robots;
/// SSRF-safe DNS resolver that blocks private/reserved IP ranges.
mod ssrf;

pub(crate) use client::{FetchRequest, Fetcher};
