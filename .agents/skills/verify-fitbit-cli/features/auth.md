# Auth

`auth --non-interactive` writes private pending PKCE state and prints a first-party
Google sign-in link. Complete it with the returned clienthandler URL containing
#code=, using the same pending state. Verify a profile read afterward. Unit tests
cover the PKCE vector, fragment parsing, origin/path checks, legacy state checks,
and token client persistence. Live refresh needs its own actual API verification.
