# Security policy

## Reporting a vulnerability

Please report security issues privately, not in a public issue or pull request.

- Use GitHub's private vulnerability reporting: the **Security** tab of the repository, then **Report a
  vulnerability**.
- Or write to `SECURITY-CONTACT-TO-BE-FILLED-IN` <!-- placeholder: add the project's security contact before publishing -->.

Include the affected crate and version (or commit), what an attacker controls (a file, a packet, a script), and if you
can, an input that reproduces the problem. We aim to acknowledge a report within a week and to agree on a disclosure
date with you once a fix is ready.

## Scope

torque-rs reads input it does not control: script sources and compiled scripts, shapes, maps, images, fonts and sounds
from servers and download sites, and network packets from any peer. In any crate that reads such input, these count as
security issues:

- a panic, an infinite loop or a stack overflow caused by input;
- an allocation or amount of work out of proportion to the size of the input;
- a script or a peer reaching a file, a socket or a process it was not given;
- a remote peer making the server or a client misbehave beyond the rules of the protocol.

## Supported versions

The project has not had a release yet. Fixes go to the `main` branch; once releases exist, the latest release
receives security fixes.
