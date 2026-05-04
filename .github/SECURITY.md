# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in lore, please report it responsibly
through [GitHub's private vulnerability reporting](https://github.com/timorunge/lore/security/advisories/new).

Do **not** open a public issue for security vulnerabilities.

## Scope

lore handles local files, crawls remote URLs (HTTP/HTTPS), connects to S3,
clones git repositories, reads Maildir email stores, executes shell commands
(`exec` sources), and connects to upstream MCP servers. Security-relevant
areas include:

- **SSRF protection** -- URL fetching validates targets against internal/private IP ranges.
- **Path traversal** -- archive extraction and file operations validate paths.
- **Git transport** -- only `https://`, `http://`, `ssh://`, `git://`, SCP-style (`git@host:path`), and local paths are allowed; dangerous transports (`ext::`, `fd::`) are blocked.
- **Shell command execution** -- `exec` sources run user-configured shell commands. lore does not sandbox these commands; they run with the privileges of the calling user. Only configure `exec` sources you trust.
- **MCP client** -- MCP sources connect to upstream servers via stdio subprocess or HTTP. The subprocess is launched with the calling user's privileges.
- **Credential handling** -- AWS credentials are sourced from the standard SDK chain; no credentials are stored by lore.

## Supported Versions

Only the latest release is supported with security fixes.
