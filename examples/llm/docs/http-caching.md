# HTTP Caching

HTTP caching reduces latency and server load by reusing previously fetched
responses. Caches exist at multiple levels: browser, CDN, reverse proxy, and
application.

## Cache-Control

The `Cache-Control` header is the primary mechanism. Common directives:

- `max-age=N` -- response is fresh for N seconds.
- `no-cache` -- must revalidate with the origin before using.
- `no-store` -- never cache the response.
- `public` / `private` -- whether shared caches (CDNs) may store it.
- `immutable` -- the resource will never change (useful for hashed asset URLs).
- `stale-while-revalidate=N` -- serve stale while revalidating in background.

## Conditional Requests

When a cached response becomes stale, the client sends a conditional request
using `If-None-Match` (ETag) or `If-Modified-Since` (Last-Modified). If the
resource has not changed, the server responds with `304 Not Modified` and no
body, saving bandwidth.

## ETags

An ETag is an opaque identifier for a specific version of a resource:

- **Strong ETags** (`"abc123"`) -- byte-for-byte identical.
- **Weak ETags** (`W/"abc123"`) -- semantically equivalent.

Strong ETags enable range requests; weak ETags are sufficient for cache
validation.

## Vary Header

The `Vary` header tells caches which request headers affect the response. For
example, `Vary: Accept-Encoding` means gzip and non-gzip responses are cached
separately. Incorrect `Vary` headers cause cache pollution or serve wrong
content.

## Cache Invalidation

There is no reliable way to actively purge all caches. Strategies include:

- **Cache busting** -- embed a hash or version in the URL (e.g.
  `/app.3fa9b1.js`).
- **Short max-age** -- limit freshness so stale entries expire quickly.
- **Purge APIs** -- CDN-specific endpoints to invalidate by URL or tag.
