# Protocol backlog

[Backlog](../../../plans/roadmap/PLAN.md) · [Roadmap](../README.md)

## Login encryption is deliberately deferred

Sphere ships `sphereCrypt.ini`: a per-client-version key table for the login
stream, and separate game-stream encryption. It is a real lift and it buys
nothing — the keys are extracted from the client binary, so anyone can read the
stream. It is obfuscation, not security.

ClassicUO connects with encryption off, which is what freeshards use in
practice. So: support unencrypted first, get a client on screen, and revisit
only if a real client turns up that cannot be configured without it. Do not
mistake this for a security feature when it lands.
