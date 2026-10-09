# Security Policy

## Reporting a vulnerability

**Please do not report security problems in public issues.** Use GitHub's
private vulnerability reporting instead:

<https://github.com/glima/mare-player/security/advisories/new>

Only you and the maintainer can see the report. Please include:

- the version (shown in **Settings → About**, or the release / commit you built),
  and whether you run the panel applet or standalone `mare-player`;
- what an attacker can do, and what they need first (another local process,
  a crafted link, a hostile network, …);
- steps or a proof of concept to reproduce it.

Never include your own TIDAL session, tokens, or keyring contents. Redact them
from any logs you attach.

### What happens next

Maré Player has a single maintainer, and these timelines reflect that:

- **Acknowledgement within 14 days** of your report.
- An assessment, and a fix or mitigation plan, as soon as the problem is
  understood. Fixes ship in a new release.
- **Coordinated disclosure:** the advisory is published when the fix is
  released, or after **90 days** if no fix is ready by then, unless we agree
  on another date. Reporters are credited in the advisory unless they prefer
  not to be.

### Supported versions

Only the **latest release** receives security fixes. The project is pre-1.0,
so fixes are not backported to older releases.

### Scope

In scope: anything in this repository, including how Maré stores and uses your
TIDAL session, its sign-in callback and D-Bus interfaces, its parsing of TIDAL
responses, its logs, and the release pipeline that builds and signs the
packages.

Please report elsewhere:

- the TIDAL service itself, to TIDAL;
- [`tidlers`](https://codeberg.org/tomkoid/tidlers), the TIDAL API library,
  [libcosmic](https://github.com/pop-os/libcosmic) or GStreamer, to those
  projects (but tell us too if Maré is affected, so we can ship the fix).

## Security model

Maré Player is a desktop client: it runs as your user, opens no network
ports, and talks only to TIDAL. This section describes what it protects and
what it assumes.

### Your TIDAL session

- **Sign-in** uses OAuth with PKCE. The session is stored in your desktop's
  Secret Service keyring (service `cosmic-applet-mare`), never in the config
  directory, the cache, or the logs.
- **The sign-in callback.** When Maré handles `tidal://` links, the browser
  starts a second `cosmic-applet-mare` process with the callback URI. That
  process forwards the URI to the running instance over the session D-Bus
  (`io.github.cosmic_applet_mare.Login`) and exits. Only the URI's `code`
  parameter is used, and it is useless without the PKCE verifier that exists
  only in the running process's memory.
- **The OAuth client.** Maré signs in as one of TIDAL's first-party clients
  (see [`client_identity.rs`](src/tidal/client_identity.rs)). Its secret lives
  in `tidlers` and is never copied into this repository.
- **Signing out** in Settings deletes the session from the keyring.

### Network surface

No inbound listeners: Maré opens no TCP or UDP port. All outbound traffic is
HTTPS:

| Host | Purpose |
| --- | --- |
| `login.tidal.com`, `auth.tidal.com` | Sign-in and token refresh |
| `api.tidal.com`, `openapi.tidal.com` | Catalogue, collection, playback info |
| `resources.tidal.com` | Artwork. Artwork URLs that are not `https://` are refused. |
| Stream URLs returned by TIDAL | Audio and video playback, through GStreamer |
| `ec.tidal.com` | Playback events, as TIDAL's own clients report them |

**Share** builds a song.link address locally and hands it to your browser.
Maré itself does not contact song.link.

### Local surface

- **D-Bus (session bus).** MPRIS, for media keys and the panel's media
  controls; `OpenUri` accepts TIDAL links only. The sign-in callback service
  above. Both are reachable by other processes running as your user.
- **Video window.** Popped-out music videos play in `mare-video-window`, a
  child process the applet starts and controls over a pipe.
- **Files.** `~/.cache/cosmic-applet-mare/cache.db` holds cached API
  responses, artwork and your play history. Settings live in COSMIC's config
  store. Neither contains credentials.
- **Logs** go to the systemd journal (through `cosmic-panel` for the applet).
  Stream URLs are logged without their token-bearing query strings, and HTTP
  errors are summarised without URLs or response bodies.

### Threat model

Designed to protect against:

- remote attackers: there is nothing to connect to;
- network attackers between you and TIDAL: everything is HTTPS, and plain-HTTP
  artwork links are refused;
- malformed or hostile API responses crashing the app: parsing is fallible
  throughout, production code may not `unwrap`, `expect`, `panic!` or index
  out of bounds (enforced by clippy), and the lyrics parser is fuzzed;
- credentials leaking through files, logs or the repository.

Assumed trusted:

- **other processes running as your user.** They can read an unlocked
  keyring, call the D-Bus interfaces, and read the cache. A desktop session
  offers no boundary between them.
- **TIDAL** for the content it serves, and your **desktop's Secret Service**
  for storing the session;
- **physical access** to the machine.

### If you think your session is compromised

1. Sign out in **Settings**, or remove the stored session directly:
   `secret-tool clear service cosmic-applet-mare`.
2. Change your TIDAL password.
3. Optionally clear the cache: `rm -r ~/.cache/cosmic-applet-mare`.
4. Report how it happened, as described above.

## Supply chain and releases

- **Code.** Rust with no `unsafe` in this repository. `just check` runs clippy
  with warnings as errors, `cargo-deny` licence checks and `cargo audit`.
  CodeQL and [OpenSSF Scorecard](https://scorecard.dev/viewer/?uri=github.com/glima/mare-player)
  run in CI.
- **Dependencies.** `Cargo.lock` is committed, and Dependabot proposes Cargo
  and GitHub Actions updates.
- **CI.** Workflows default to read-only tokens and grant write access per job.
  Actions are pinned by commit SHA. The one exception, the SLSA generator, is
  pinned by tag, because its verifier derives the builder identity from it.
- **Releases.** Every release ships a `SHA256SUMS` file and two provenance
  attestations covering every package: GitHub build provenance
  (`provenance.sigstore`) and SLSA level 3 provenance (`multiple.intoto.jsonl`),
  both signed keylessly with Sigstore. The Fedora COPR packages are built from
  the release's source RPMs.

### Verifying a download

```sh
# Check the file against the published checksums
sha256sum --check --ignore-missing SHA256SUMS

# GitHub build provenance (needs the GitHub CLI)
gh attestation verify mare-player-x86_64.tar.gz --repo glima/mare-player

# SLSA provenance (needs slsa-verifier)
slsa-verifier verify-artifact mare-player-x86_64.tar.gz \
  --provenance-path multiple.intoto.jsonl \
  --source-uri github.com/glima/mare-player \
  --source-tag v0.3.6
```

Replace the file name and tag with the ones you downloaded.
