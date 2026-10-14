# Security policy

HW Glance is a hobby project maintained by one person in spare time. I take security reports seriously, but replies are not instant.

## Supported versions

Only the latest release gets fixes. If you run an older one, update first.

## Reporting a vulnerability

Please report privately, not in a public issue:

1. Open the repository's **Security** tab.
2. Choose **Report a vulnerability**.

Or use this link: <https://github.com/Raed-Alghamdi/hwglance/security/advisories/new>

Please include:

- what you found and why it matters,
- the version (`hwglance --version`) and how you installed it,
- steps to reproduce, or a small proof of concept,
- how you would like to be credited, if at all.

## What happens next

- I aim to acknowledge your report within about a week.
- I confirm the problem, fix it, and publish a patch release. Complex fixes can take a few weeks.
- The fix is announced as a GitHub security advisory. I credit you if you want that.

## What is in scope

- The root helper (`hwglance-helper`): it is the only part that runs as root.
- The collector socket in `$XDG_RUNTIME_DIR`, which the window and the bar widget connect to.
- Ending or killing processes through polkit.
- Parsing of the config file and of data read from `/sys`, `/proc` and the helper.

## What is out of scope

- Hardware damage. HW Glance only reads sensors and never writes fan, clock, power or other hardware settings, so a fault like that is not a security issue here.
- Problems that need an attacker who already runs code as your user or as root.
- Vulnerabilities in dependencies with no effect on HW Glance. Dependency advisories are checked automatically.
