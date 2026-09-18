# HaInviter

Guest invitations and RSVP tracking for weddings, parties and anything else with
a guest list. One small Rust binary: an admin panel for the hosts, and a private,
good-looking invitation page for every guest.

- **Admin panel** — create events, paste in a guest list, watch the replies
  arrive, download them as CSV.
- **Per-guest links** — every invitee gets `/i/<token>`, showing their name, the
  event details and a single question to answer.
- **No accounts** — access is by unguessable link, for hosts and guests alike.
- **English and Hebrew**, including right-to-left layout, chosen per deployment.
- **Stateless** — everything lives in PostgreSQL, so you can run as many
  replicas as you like behind one Service.

## How it works

One crate is compiled twice: natively for the Axum server (`--features server`)
and to WebAssembly for the browser (`--features web`). The page components, the
data types and the client/server API are therefore shared code and cannot drift
apart.

| Path | What it holds |
| --- | --- |
| `src/types.rs` | DTOs shared by both builds |
| `src/i18n.rs` | the locales, both string tables, and date formatting |
| `src/api.rs` | every server function — the whole client/server boundary |
| `src/db.rs` | PostgreSQL schema, migrations and queries |
| `src/auth.rs` | token generation and the admin check |
| `src/audit.rs` | the audit log |
| `src/export.rs` | CSV rendering |
| `src/bin/devdb.rs` | the embedded PostgreSQL used for local development |
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
```

You do not need PostgreSQL installed. `cargo dev` runs an embedded one and hands
its URL to whatever command follows:

```bash
cargo dev dx serve
```

The first run downloads a real PostgreSQL 18 (about 100 MB) into a directory of
its own and keeps it; later runs start in well under a second and the data from
last time is still there. `HAINVITER_DEV_DB_DIR` moves that directory;
`cargo dev` with no command prints the URL and waits, for use from another
terminal. It is [`src/bin/devdb.rs`](src/bin/devdb.rs), behind the `dev-db`
feature, and is never built for the container image.

It runs a real PostgreSQL rather than SQLite or a mock deliberately. A second
backend would mean every query written twice in two dialects, and the failure
mode of that is local development passing while production breaks.

If you would rather bring your own database, set `HAINVITER_DATABASE_URL` and run
`dx serve` directly — `docker compose up -d postgres` will start a suitable one.

The admin link is printed to the terminal on startup:

```
  ┌────────────────────────────────────────────────────────────
  │ HaInviter admin panel — anyone with this link is an admin:
  │
  │   http://127.0.0.1:8080/admin/1f4c…
  │
  └────────────────────────────────────────────────────────────
```

### Tests

The unit tests need nothing. The end-to-end test needs a **scratch** database —
the first thing it does is drop the schema, which is why it reads a variable of
its own rather than the one the application uses:

```bash
cargo dev bash -c 'HAINVITER_TEST_DATABASE_URL=$HAINVITER_DATABASE_URL cargo test --features server'
```

or point it at a database of your own:

```bash
HAINVITER_TEST_DATABASE_URL=postgres://hainviter:hainviter@localhost:5432/hainviter_test cargo test --features server
```

Without that variable the end-to-end test prints a skip notice and the rest still
run. CI always sets it, and sets `HAINVITER_REQUIRE_DB=1` so a broken database
service fails the build instead of quietly skipping.

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
| `HAINVITER_DATABASE_URL` | *(required)* | PostgreSQL connection string. `DATABASE_URL` is accepted as a fallback. The server exits at startup if it cannot connect. |
| `HAINVITER_LOCALE` | `en-US` | `en-US` or `he-IL`. Sets the language and the text direction for the whole deployment. An unrecognised tag falls back to `en-US`. |
| `HAINVITER_BASE_URL` | *(unset)* | Public origin, e.g. `https://invites.example.com`. Used for the invitation links in the CSV export. When unset, the admin panel falls back to the browser's own origin. |
| `HAINVITER_ADMIN_TOKEN` | *(generated)* | Pins the admin token. Leave it unset to have one generated on first startup and stored in the database. |
| `HAINVITER_DATA_DIR` | *(unset)* | Optional. Where to write a second copy of the audit log. Give each instance its own directory — never a shared one. |
| `IP` | `127.0.0.1` | Listen address. The container sets `0.0.0.0`. |
| `PORT` | `8080` | Listen port. |

## Running more than one instance

Nothing is kept in the process or on its local disk, so replicas need no
coordination and no shared filesystem:

- **Cover images are stored in the database** as `bytea`, not on disk, and served
  from there. That is the whole reason there is no RWX volume to provision.
- **The admin token is stored in the database.** With it unset, the first
  instance to start generates one and writes it with `ON CONFLICT DO NOTHING`;
  every instance then reads back the same value. Without that, each replica would
  print a different admin link and only one of them would work.
- **Migrations take a Postgres advisory lock**, so several instances starting at
  once is safe — exactly one applies them.
- **The audit-log file is per-instance and optional.** The complete copies are the
  database and stdout.

Postgres is the only thing left that holds state, and therefore the only thing
that needs a volume and a backup.

## Security model

Deliberately minimal, and worth understanding before pointing this at the
internet.

- **The admin link is the credential.** It is 128 random bits. Anyone holding it
  can do anything. It is never stored in the browser, so closing the tab ends the
  session.
- **Each guest link is a credential too**, with the same 128 bits of entropy. It
  lets that one guest read their invitation and answer it — nothing else. If a
  link leaks, reissue it from the guest list and the old one stops working.
- **Tokens are compared in constant time**, and an invalid admin token gets a 404
  rather than a 403.
- **Uploaded images are typed by their bytes**, never by the supplied file name,
  are capped at 8 MB, and are stored under a random id.
- **Serve it over HTTPS.** The tokens are in the URL path; without TLS they are
  in plain sight.
- The database connection does **not** use TLS: it is expected to be reached over
  a trusted network.
- Rate limiting is not built in. Guessing a token is not feasible, but if the
  service is public, put it behind an ingress that limits request rates.

## Not losing the guest list

Replies are the one thing here that cannot be recreated, so each one is written
to three places before the guest sees a confirmation, plus a fourth when a data
directory is configured:

1. the guest's row in the `guests` table;
2. `rsvp_log`, an append-only history that deliberately has no foreign keys —
   deleting a guest or an event never erases what they answered;
3. stdout, so a reply is recoverable from container logs alone;
4. `audit.log` on `HAINVITER_DATA_DIR`, one JSON object per line, `fsync`ed, when
   that variable is set.

Administrative changes (events created or deleted, guests added or removed, links
reissued, exports taken) are audited the same way.

Back up the database. It is the entire state of the system.

## Localisation

`HAINVITER_LOCALE` picks one language for the whole deployment, resolved on the
server and passed to the browser before its first render — server-rendered markup
and the hydrated page have to agree on every string.

Both string tables live in [`src/i18n.rs`](src/i18n.rs) as one struct per
language, so a missing translation is a compile error rather than a runtime hole.
Dates are assembled from the table's own month and weekday names, because chrono
only formats those in English.

For Hebrew the whole page is marked `dir="rtl"`, and the stylesheet's directional
rules are written as CSS logical properties so they flip on their own. Hebrew also
drops the letter-spacing and uppercasing the Latin type uses, and asks for a
Hebrew serif for the invitation headings.

Adding a language means adding a variant to `Locale` and one more `Strings`
table; the compiler then lists everything still missing.
