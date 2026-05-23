# QBWebUIHelper

If you run qBittorrent as a headless server and access it through its web interface, you've probably hit this annoyance: clicking a magnet link in your browser does nothing useful, or double-clicking a `.torrent` file opens the wrong app (or a dialog asking you to pick one). qBittorrent's built-in browser extension workaround exists, but it's fiddly to set up and doesn't always stick.

QBWebUIHelper fixes that properly.

---

## What it does

It's a small desktop app that opens your qBittorrent web interface in its own window — no browser tab needed. More importantly, it registers itself as the system default handler for magnet links and `.torrent` files, so clicking either one sends it straight to your qBittorrent instance, no fuss.

Works on **Windows and macOS**.

---

## Why this instead of qBittorrent's own option?

qBittorrent does have a setting to handle magnet links, but it expects the full qBittorrent desktop app to be installed locally. If you're using the web interface to talk to a remote instance (on a NAS, a server, another machine on your network), that option doesn't help you.

This app bridges that gap. It doesn't care where your qBittorrent is running — just point it at the URL, and it handles the rest.

---

## The part we're most proud of

We went out of our way to make sure this app doesn't leave a mess on your system.

Every association it registers can be fully undone — not just "we'll try our best", but a proper before-and-after snapshot. Before claiming `.torrent` files or magnet links, it saves exactly what was there before (Transmission, Deluge, whatever). One click in Settings restores everything back to how it was, precisely.

No admin password needed. No digging through system settings. If you ever want to uninstall, just hit **Unregister** (Windows) or **Restore Previous Default** (macOS) first, and the app cleans up completely after itself.

---

## The basics

- Wraps your qBittorrent web interface in a native window
- Registers as the default handler for `.torrent` files and `magnet:` links
- Full undo — restores your previous defaults when you unregister
- No administrator privileges required
- Minimises to the system tray (Windows) or menu bar (macOS) and stays out of your way
- Remembers window size and position between launches
- **Windows binary: under 10 MB. macOS binary: under 10 MB.**

---

## Setup

1. Download the installer for your platform from the [Releases](../../releases) page
2. Install and launch the app
3. On first run, enter your qBittorrent WebUI URL (e.g. `http://192.168.1.100:8080`)
4. Go to Settings → click **Register** (Windows) or **Set as Default** (macOS)
5. Follow the short on-screen prompt to confirm in your system's default apps settings

That's it. Magnet links and `.torrent` files will now route to your qBittorrent instance.

---

## Before you uninstall

**Please click Unregister / Restore Previous Default in Settings before removing the app.** This hands the file associations back to whatever you were using before. The app reminds you of this, but it's worth saying here too.

---

## Building from source

You'll need [Rust](https://rustup.rs) and [Node.js](https://nodejs.org) installed.

```bash
# Windows
buildme.bat

# macOS
./buildme.sh
```

The release binary lands in `src-tauri/target/release/`.

---

## License

AGPL-3.0
