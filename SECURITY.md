# Security Policy

## Supported Versions

Only the latest minor release line receives security updates.

| Version | Supported |
| ------- | --------- |
| 0.1.x   | Yes       |
| < 0.1   | No        |

## Reporting a Vulnerability

**Please do not open public GitHub issues for security vulnerabilities.**

Preferred channel:

- [GitHub Security Advisories](https://github.com/lee-to/ai-tester/security/advisories/new) — private disclosure, tracked alongside the repo.

Alternate channel:

- Email: `thecutcode@gmail.com` with the subject line `[ai-tester security]`.

Please include:

- A description of the issue and the potential impact.
- Steps to reproduce, or a proof-of-concept.
- The affected version and platform.
- Whether the issue is already public.

## Response Timeline

- Acknowledgement within **3 business days**.
- Initial assessment within **7 business days**.
- Fix and coordinated disclosure timeline agreed with the reporter.

We credit reporters in release notes by default. Let us know if you prefer to remain anonymous.

## Scope

In scope:

- The `@lee-to/ai-tester` npm package and anything it ships (`bin/`, `dist/`).
- The scenario schema and its handling of user-supplied YAML / filesystem paths.
- The sandbox lifecycle (`src/sandbox/`) — particularly anything that could escape the sandbox or leak host filesystem state.

Out of scope:

- Vulnerabilities in third-party runtimes (`claude` CLI, `codex` CLI) or their SDKs — please report to the respective upstream projects.
- Social-engineering or physical-access attacks.
- Denial of service from running extremely large fixtures or adversarial scenarios (this is a developer tool; resource limits are the user's responsibility).
