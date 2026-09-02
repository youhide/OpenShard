# Login encryption: read, and deferred

What the two reference servers do about encrypting the login stream, why this
one does not, and what would have to turn up before the answer changes. The open
item this leaves is ranked in [the domain README](README.md).

## It buys obfuscation, not security

Sphere ships `sphereCrypt.ini`: a per-client-version key table for the login
stream, and separate game-stream encryption. It is a real lift and it buys
nothing — the keys are extracted from the client binary, so anyone can read the
stream. It is obfuscation, not security.

ClassicUO connects with encryption off, which is what freeshards use in
practice. So: support unencrypted first, get a client on screen, and revisit
only if a real client turns up that cannot be configured without it. Do not
mistake this for a security feature when it lands.
