# Security policy

## Supported versions

| Version | Supported |
|---|---|
| `master` and the 4.0 line (in preparation) | Yes |
| Latest tagged release | Yes, fixes ship in the next release |
| Older releases | No |

There is no long-term-support branch.

## Reporting a vulnerability

Please report security issues privately, not in public issues:

- Open a [private security advisory](https://github.com/helion-fpga/helion/security/advisories/new), or
- contact the owner, [@saksham-45](https://github.com/saksham-45), on GitHub.

Include the version or commit, the platform, steps to reproduce, and the
impact you expect. We will acknowledge the report, work on a fix in a
private advisory, and credit you in the release notes unless you prefer
otherwise.

## In scope

- Code execution from opening a project, source file, checkpoint, IP
  package, Tcl script or bitstream in `helion` or `helion-ide`
- Path traversal when reading sources, projects or device data (HAD), or
  when writing reports, checkpoints and archives
- Unexpected hardware actions: the Hardware Manager or `helion hw`
  programming, erasing or writing to a connected device without an explicit
  user command
- Secrets or credentials in logs, release artifacts or the repository

## Not vulnerabilities

- Quality-of-results changes (LUT counts, WNS), missing Tcl commands, or a
  design that does not fit a part
- Legal-fence refusals (for example, refusing to ingest vendor libraries)

Please do not attach vendor bitstreams or encrypted vendor IP to any report.
