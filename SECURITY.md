# Security

## Report a vulnerability

Use GitHub private vulnerability reporting. Open the repository page and
select "Report a vulnerability" under the Security tab.

Do not open a public issue for a vulnerability.

## What to include

- The version or commit you tested
- The Linux distribution and kernel version
- The steps to reproduce the problem
- The effect of the problem, if known

## Response

The maintainers acknowledge the report within 5 business days. They keep
the reporter informed of the fix status. The fix is released as a normal
commit unless the problem needs an embargo.

## Scope

The engine does not validate untrusted data at the application layer. It
validates packet headers for bounds safety. Run the engine behind a trusted
boundary for production use.
