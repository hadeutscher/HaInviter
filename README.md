# HaInviter

Guest invitations and RSVP tracking for weddings, parties and anything else with
a guest list. One small Rust binary: an admin panel for the hosts, and a private,
good-looking invitation page for every guest.

- **Admin panel** — create events, paste in a guest list, watch the replies
  arrive, download them as CSV.
- **Per-guest links** — every invitee gets `/i/<token>`, showing their name, the
  event details and a single question to answer.
- **No accounts** — access is by unguessable link, for hosts and guests alike.
- **Small** — one binary, SQLite compiled into it, and nothing installed on top
  of the runtime image. It is meant to run on a Raspberry Pi and it does.

## How it works

One crate is compiled twice: natively for the Axum server (`--features server`)
and to WebAssembly for the browser (`--features web`). The page components, the
data types and the client/server API are therefore shared code and cannot drift
apart.

| Path | What it holds |
| --- | --- |
| `src/types.rs` | DTOs and formatting shared by both builds |
| `src/api.rs` | every server function — the whole client/server boundary |
| `src/db.rs` | SQLite schema, migrations and queries |
| `src/auth.rs` | token generation and the admin check |
| `src/audit.rs` | the append-only audit log |
| `src/export.rs` | CSV rendering |
| `src/ui/material.rs` | the Material 3 component set |
| `src/ui/invite.rs` | the guest-facing invitation page |
| `src/ui/admin.rs` | the admin panel |
| `assets/material.css` | Material 3 tokens and components |

Material 3 is implemented directly in CSS and inline SVG rather than pulled in as
a JavaScript component library: the invitation pages are opened on phones over
mobile data, and there is no web font, icon font or CDN request in the bundle.

## Running it

### Development

```bash
cargo install dioxus-cli --locked   # or: cargo binstall dioxus-cli
HAINVITER_DATA_DIR=./data dx serve
```

`dx serve` builds both halves and reloads on change. The admin link is printed to
the terminal on startup:

```
  ┌────────────────────────────────────────────────────────────
  │ HaInviter admin panel — anyone with this link is an admin:
  │
  │   http://127.0.0.1:8080/admin/1f4c…
  │
  └────────────────────────────────────────────────────────────
```

### Tests

```bash
cargo test --features server
```

### Docker

```bash
docker compose up --build
```

The published image is `ghcr.io/hadeutscher/hainviter:latest` (amd64 and arm64),
built by [`.github/workflows/docker.yml`](.github/workflows/docker.yml) on every
push to `master` and on every `v*` tag.

## Configuration

Everything is an environment variable; there is no configuration file.

| Variable | Default | Purpose |
| --- | --- | --- |
| `HAINVITER_DATA_DIR` | `/data` | Where the database, audit log and uploaded cover images live. **Must be a persistent volume.** |
| `HAINVITER_BASE_URL` | *(unset)* | Public origin, e.g. `https://invites.example.com`. Used for the invitation links in the CSV export. When unset, the admin panel falls back to the browser's own origin. |
| `HAINVITER_ADMIN_TOKEN` | *(generated)* | Fixes the admin token so the admin link survives a restart. Leave unset to get a fresh one, printed to the log, on every start. |
| `IP` | `127.0.0.1` | Listen address. The container sets `0.0.0.0`. |
| `PORT` | `8080` | Listen port. |

## Security model

Deliberately minimal, and worth understanding before pointing this at the
internet.

- **The admin link is the credential.** It is 128 random bits, generated at
  startup and printed to the log. Anyone holding it can do anything. It is never
  stored in the browser, so closing the tab ends the session, and restarting the
  server invalidates it unless `HAINVITER_ADMIN_TOKEN` is set.
- **Each guest link is a credential too**, with the same 128 bits of entropy. It
  lets that one guest read their invitation and answer it — nothing else. If a
  link leaks, reissue it from the guest list and the old one stops working.
- **Tokens are compared in constant time**, and an invalid admin token gets a 404
  rather than a 403.
- **Uploaded images are typed by their bytes**, never by the supplied file name,
  are capped at 8 MB, and are stored under a random name — an upload cannot
  overwrite anything or escape the uploads directory.
- **Serve it over HTTPS.** The tokens are in the URL path; without TLS they are
  in plain sight.
- Rate limiting is not built in. Guessing a token is not feasible, but if the
  service is public, put it behind an ingress that limits request rates.

## Not losing the guest list

Replies are the one thing here that cannot be recreated, so each one is written
to four places before the guest sees a confirmation:

1. the guest's row in the `guests` table;
2. `rsvp_log`, an append-only history that deliberately has no foreign keys —
   deleting a guest or an event never erases what they answered;
3. `audit.log` on the data volume, one JSON object per line, `fsync`ed;
4. stdout, so a reply is recoverable from container logs alone.

SQLite runs in WAL mode with `synchronous = FULL`: a power cut costs throughput,
not answers. Administrative changes (events created or deleted, guests added or
removed, links reissued, exports taken) are audited the same way.

Back up `HAINVITER_DATA_DIR`. That directory is the entire state of the system.

## Guest lists

The "Add guests" box takes one name per line:

```
Dana Levi
The Cohen family, 4
Roni & Tal, 2
Cohen, Dana
```

A trailing `, <number>` sets how many people that invitation covers; everything
else is part of the name, so `Cohen, Dana` stays one guest. Names already on the
list are skipped, so pasting an updated list is safe.

## CSV export

`Responses → Download CSV` gives one row per guest: id, name, reply, confirmed
party size, party cap, note, reply timestamp, and full invitation link. The file
begins with a UTF-8 BOM so Excel opens non-ASCII names correctly, and fields
starting with `=`, `+`, `-` or `@` are neutralised so a spreadsheet cannot treat a
guest's note as a formula.
