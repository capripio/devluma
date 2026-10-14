<div align="center">

<img src="src-tauri/icons/128x128.png" alt="Devluma logo" width="96" height="96">

# Devluma — Free, Open-Source Laragon Alternative for Windows

**A modern local development environment for Windows: PHP, Nginx, Apache, MariaDB, PostgreSQL,
MongoDB, Node.js, Python and Mailpit — with per-project versions, local `.test` domains and
trusted HTTPS, in one portable app.**

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Platform: Windows 10/11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D6.svg)](#requirements)
[![Built with Tauri 2](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB.svg)](https://tauri.app)
[![Latest release](https://img.shields.io/github/v/release/capripio/devluma?include_prereleases)](https://github.com/capripio/devluma/releases)

[Download](https://github.com/capripio/devluma/releases/latest) ·
[Features](#features) ·
[Laragon comparison](#devluma-vs-laragon-xampp-and-wamp) ·
[Build from source](#build-from-source) ·
[FAQ](#faq)

</div>

---

## What is Devluma?

Devluma is a **free and open-source alternative to Laragon** (and to XAMPP and WampServer) for
PHP, Laravel, WordPress, Node.js and Python developers on Windows. It installs official builds
of your web servers, databases and language runtimes, runs them as local services, and gives
every project its own local domain with HTTPS — **without changing your system `PATH`, without
taking over Windows services, and without running as administrator.**

Like Laragon, Devluma is **portable**: unzip it anywhere and everything it downloads or creates
stays in that folder. Unlike most WAMP stacks, every install is **checksum-verified**, every
config change is **validated and reversible**, and services only ever listen on `127.0.0.1`.

## Features

### Runtimes and version switching
- **Multiple PHP versions** side by side (8.2, 8.3, 8.4, 8.5) with a managed `php.ini` and
  extension toggles.
- **Node.js** (22 LTS, 24 LTS) and **Python** (3.14, 3.15), including project `.venv` support.
- **Per-project version pinning**: pin PHP 8.2 for a legacy app and PHP 8.5 for a new one.
  Pins never affect other projects or the global default.
- **Project terminal** that opens with the right runtime versions on `PATH` — your system
  `PATH` is never modified. Choose PowerShell, Command Prompt or **Cmder**.

### Web servers and databases
- **Nginx** and **Apache httpd**, with PHP served through FastCGI.
- **MariaDB** (MySQL-compatible), **PostgreSQL** and **MongoDB**, each with its own data folder.
- **Mailpit** to catch outgoing email from your apps during development.
- One-click **start, stop and restart**, service logs, and clear port-conflict reports (Devluma
  tells you which program holds a port — it never kills it).

### Local domains and HTTPS
- **Automatic virtual hosts**: serve any project folder at a local domain like
  `myapp.test`.
- **Safe hosts file editing**: only Devluma's own marked block is changed, you see the exact
  diff first, and administrator rights are requested only for that one write.
- **Local HTTPS certificates** from a private certificate authority that is name-constrained to
  local domains (`.test`, `.localhost`, …), so it can never be used for real websites. Trusting
  it is a separate, explicit step.
- **ngrok tunnels** to share a local site publicly, using your own ngrok account.

### Safety and reliability
- **Verified downloads**: packages come only from official publishers and are checked with
  SHA-256 before anything is installed. A failed install leaves nothing behind.
- **Database backups and restores** (logical dumps or data-folder snapshots), with a safety
  backup before every restore.
- **Every action is journaled**, so an interrupted operation is detected and explained on the
  next start.
- **Signed updates** and an exportable, secret-free diagnostics report.
- **Portable or installed**: a per-user installer (no admin needed) or a zip you can carry on a
  USB drive.

## Devluma vs Laragon, XAMPP and WAMP

Looking for a **Laragon alternative**? Devluma covers the same everyday workflow — quick
installs, multiple PHP versions, pretty URLs and HTTPS — with a stronger focus on safety and
transparency:

| | Devluma |
|---|---|
| License | Free and open source (GPL-3.0) |
| Portable | Yes — one self-contained folder, or a per-user installer |
| Multiple PHP versions | Yes, plus per-project pins |
| Web servers | Nginx and Apache |
| Databases | MariaDB, PostgreSQL, MongoDB |
| Other runtimes | Node.js, Python |
| Local domains | `*.test` with a managed hosts-file block |
| HTTPS | Name-constrained local CA, trusted only when you choose |
| System `PATH` | Never modified |
| Runs as admin | Never; elevation only for the hosts-file write |
| Download verification | SHA-256 against the publisher's checksums |
| Email testing | Mailpit |
| Sharing | ngrok tunnels |

## Download

Get the latest version from the **[Releases page](https://github.com/capripio/devluma/releases/latest)**:

- `Devluma_<version>_x64-setup.exe` — per-user installer (no administrator rights needed).
- `Devluma_<version>_x64-portable.zip` — portable edition: unzip anywhere writable (not
  `C:\Program Files`) and run `devluma.exe`.

### Requirements
- Windows 10 or Windows 11 (64-bit).
- Microsoft Edge WebView2 Runtime (already included in Windows 11 and up-to-date Windows 10;
  the installer fetches it if missing).

## Quick start

1. Run Devluma. The first-run setup asks where to keep your tools and data.
2. Open **Runtimes & Packages** and install PHP, a web server (Nginx or Apache) and a
   database — plus Node.js or Python if you need them.
3. Open **Services**, add the web server and database, and start them.
4. Add your project folder under **Projects**, then create a site such as `myapp.test` under
   **Sites & Domains**.
5. Optionally create a certificate under **SSL Certificates** and enable HTTPS for the site.

## Build from source

Requirements: Windows 10/11, [Rust](https://rustup.rs) (version in `rust-toolchain.toml`),
[Node.js](https://nodejs.org) with [pnpm](https://pnpm.io), and the Tauri CLI:

```sh
cargo install tauri-cli --version "^2"
pnpm install
cargo tauri dev        # run the app in development mode
cargo tauri build      # release build + installer
```

Checks:

```sh
pnpm typecheck && pnpm lint && pnpm format:check && pnpm test
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

**Tech stack:** [Tauri 2](https://tauri.app), Rust, React 19, TypeScript, Vite, Tailwind CSS v4,
SQLite.

## FAQ

**Is Devluma a drop-in replacement for Laragon?**
It covers the same core workflow — PHP/Laravel/WordPress development with Nginx or Apache,
MySQL-compatible MariaDB, pretty local URLs and HTTPS. Projects stay where they are; Devluma
never moves or deletes your project files.

**Does Devluma include MySQL?**
It ships MariaDB, a MySQL-compatible database that works with Laravel, WordPress and other PHP
apps out of the box.

**Why are Redis and Memcached missing?**
Their publishers don't provide official Windows builds, and Devluma only installs software
from official sources.

**Does it change my system PATH or Windows services?**
No. Runtimes are added to `PATH` only inside Devluma's project terminals, and Devluma never
installs or takes over Windows services.

**Are the HTTPS certificates publicly trusted?**
No. They are for local development only. The local certificate authority is limited to local
domain names and is trusted on your machine only after you confirm it.

## License

Devluma is free software licensed under the [GNU General Public License v3.0](LICENSE).
Third-party components are listed in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) and
[`sbom/devluma.cdx.json`](sbom/devluma.cdx.json). Packages that Devluma downloads (PHP, Nginx,
Apache, MariaDB, PostgreSQL, MongoDB, Node.js, Python, Mailpit) are separate projects under
their own licenses.

<!-- Keywords: Laragon alternative, XAMPP alternative, WAMP alternative, WampServer alternative,
local development environment Windows, PHP development environment, local web server Windows,
multiple PHP versions Windows, Laravel local development, WordPress local development, Nginx
Windows, MariaDB Windows, local HTTPS, .test domains -->
