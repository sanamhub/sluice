# 0036. A page in the browser drives the local daemon, and never carries a transfer

**Status:** Proposed **Date:** 2026-11-12 **Relates to:** ADR 0018, ADR 0034, SPEC section 11

## Context

A tab negotiates TLS through the browser's own stack, and no browser offers `X25519MLKEM768` to a
page's own QUIC code. A native peer refuses every connection without it (ADR 0018), so a tab that
fetches a blob itself is refused by every released sender. The proof that a tab can fetch a blob
stands, and it shows that a tab cannot be a peer today.

People still want a window with buttons: pick files, send them, see the ticket as a QR code, paste a
ticket, watch progress, find the received files. None of that needs the tab to be a peer. The
daemon is a peer, it negotiates the post-quantum group, and the control API already names every
action and event such a window needs.

## Decision

1. **`sluice daemon --ui` serves one page and a JSON form of the control API over HTTP on a
   loopback address only.** It binds `127.0.0.1` on a port the system picks, never `0.0.0.0` or a
   routable address, and it closes a connection whose peer address is not loopback.
2. **The page never carries file bytes.** The section 11 rule holds unchanged: the page asks the
   daemon to send a path under a root or to receive into one, and it watches events. It picks files
   from a listing of names under the daemon's roots, never through the browser's file picker,
   because a picked file reaches a page as bytes and never as a path.
3. **A token guards every request.** The daemon makes 32 random bytes when it starts.
   - `sluice ui` opens the page with the token in the URL fragment, which a browser never sends to
     a server.
   - The page moves the token to session storage, clears the fragment, and sends it in an
     `Authorization` header on every call.
   - A request without the token, or with a wrong one, is refused, and the comparison runs in
     constant time.
4. **The daemon refuses another site's page.**
   - It never sends a CORS header, so a cross-origin call that carries the header fails its
     preflight.
   - It refuses a request whose `Host` is not `127.0.0.1:<port>` or `localhost:<port>`. That stops
     a rebound name.
   - It refuses a request whose `Origin`, when present, is not the page's own origin.
5. **The page is static and strict.**
   - The daemon embeds it in the binary at build time, and it loads nothing from elsewhere.
   - Content-Security-Policy: `default-src 'self'; connect-src 'self'; img-src 'self' data:;
     frame-ancestors 'none'; base-uri 'none'; form-action 'none'`.
   - `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, and no third-party font,
     script or analytics.
6. **SPEC section 11 changes in place**, since it is not released. A loopback HTTP listener is
   allowed without TLS only under points 1, 3 and 4. Every other TCP listener still requires TLS and
   a token.
7. **The page meets WCAG 2.2 at level AA.** A person who uses a keyboard alone, a screen reader,
   a high zoom or a phone does the same things as anyone else:
   - every control is reached and used from the keyboard, the focus is always visible, and a
     control keeps the focus while the numbers beside it change;
   - every control and every progress bar has a name, and the page has one first-level heading
     and landmarks for its parts;
   - what changes without the user's action is said through a status region: the end of a
     transfer, a copied ticket, a send being prepared. Progress is not announced;
   - text has a contrast of at least 4.5 to 1 in the light and the dark theme, the placeholder
     included;
   - the page reads at 320 CSS pixels wide with no sideways scroll, and it stops its motion for
     a user who asks for less.
   A test checks the parts of this that the page's own text can show. A change to the page is
   also checked in a browser, in both themes and at a phone's width.

## Alternatives

- **The tab as a classical peer.** ADR 0018 refuses it, and an exception for browsers weakens every
  receive for the sake of one client.
- **A page on sluicedrop.com calling the local daemon.** That lets a remote origin control a local
  process, and it runs into browser rules on private network access. Refused.
- **TLS on loopback.** A self-signed certificate puts a warning in front of every user and trains
  them to click through it. The token and the origin checks give the same protection against other
  pages without that habit.
- **A desktop shell (Tauri).** It needs a native build per platform and an update channel. A page
  the daemon already serves costs neither.

## Consequences

- One code path moves bytes, the native one, with post-quantum TLS, and the window adds no transport.
- The window needs Sluice installed. A browser with nothing installed still cannot send or receive,
  and the wasm crate stays a research path until browsers offer the post-quantum group.
- A process running as the same user can read the token from the daemon's memory or from the
  browser profile. That is the threat model's existing line for the local socket, and this decision
  does not move it.
- The name listing shows a page the file names under each root. The token keeps other pages out of
  it, and the roots keep it to folders the user chose.
