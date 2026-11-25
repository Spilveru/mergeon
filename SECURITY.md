# Security Policy

## Supported Versions

Only the latest release of Mergeon receives security updates.

| Version | Supported |
| ------- | --------- |
| Latest release (`0.14.x`+) | Yes |
| Older releases | No |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, please report them privately using GitHub Security Advisories:

- <https://github.com/Spilveru/mergeon/security/advisories/new>

When submitting a report, please include:

- A clear description of the vulnerability and its potential impact
- Step-by-step reproduction instructions or a minimal proof of concept
- Affected platform(s) (Linux, Windows, macOS) and Mergeon / Git versions
- Any suggested fix or mitigation (optional)

## Response Timeline

- **Acknowledgement:** within 72 hours
- **Triage & assessment:** within 7 days
- **Fix & coordinated disclosure:** as soon as a verified patch and release are ready

## Security Architecture Notes

- Mergeon invokes the system `git` binary directly without an intermediate shell (`GIT_TERMINAL_PROMPT=0`, `--no-ext-diff`, argument guards).
- HTTP credentials and GitHub tokens are passed via ephemeral `0600` files wiped and deleted on process exit — never via `argv` or environment variables exposed to hooks.
- Stored GitHub tokens are encrypted with AES-256-GCM using a master key kept in the OS keyring (`keyring`). If no keyring is available, Mergeon refuses to persist tokens in plain text silently and keeps sessions in memory.
- Automatic updates require HTTPS (`https_only`) and Ed25519 signature verification (`latest*.yml.sig`) plus SHA-512 verification of downloaded installers.
