# QBWebUIHelper

If you run qBittorrent as a headless server and access it through its web interface, you've probably hit this annoyance: clicking a magnet link in your browser does nothing useful, or double-clicking a `.torrent` file opens the wrong app (or a dialog asking you to pick one). qBittorrent's built-in browser extension workaround exists, but it's fiddly to set up and doesn't always stick.

I personally need a tool for myself to fix this properly.

---

## What it does

It's a small desktop app that opens your qBittorrent web interface in its own window — no browser tab needed. More importantly, it registers itself as the system default handler for magnet links and `.torrent` files, so clicking either one sends it straight to your qBittorrent instance, no fuss.

Works on **Windows 11 and macOS 26.4+ (No Intel support)**.

---

## Why this instead of qBittorrent's own option?

qBittorrent's web interface does have a "Register to handle magnet links" option, but it only works if your WebUI is running over HTTPS — which most home and LAN setups aren't. And even if you get that working, there's no equivalent option for `.torrent` files at all, because browsers simply can't register a file type association on behalf of a website.

QBWebUIHelper sidesteps both limitations entirely. It registers at the OS level, so HTTPS isn't a requirement and `.torrent` files work just as well as magnet links.

---

## You can revert the .torrent and magnet link association

We went out of our way to make sure this app doesn't leave a mess on your system.

Every association it registers can be fully undone, a proper before-and-after snapshot. Before claiming `.torrent` files or magnet links, it saves exactly what was there before (Transmission, Deluge, whatever). One click in Settings restores everything back to how it was, precisely.

No admin password needed because I don't want to mess with privileges and keep it very simple. No digging through system settings. If you ever want to uninstall, just hit **Unregister** (Windows) or **Restore Previous Default** (macOS) first, and the app cleans up completely after itself.  So, that means there is no installation needed for Windows, it's a portable app.

---

## The basics

- Wraps your qBittorrent web interface in a native window
- Registers as the default handler for `.torrent` files and `magnet:` links
- Full undo — restores your previous defaults when you unregister
- No administrator privileges required
- Minimises to the system tray (Windows 11) or menu bar (macOS 26.4+) and stays out of your way
- Remembers window size and position between launches
- **Windows binary: under 10 MB. macOS binary: under 10 MB.**
- I didn't test Windows 10 or older version of MacOS, so it may or may not work.

---

## Setup

1. Download the .zip file for Windows or .dmg for MacOS from the [Releases](../../releases) page
2. Extract the zip file to whereever you want for Windows to place the binary or drag the .app into Application folder for Mac and launch the app
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

## New Features?

- Most likely no new features will be added becuase the main reason of this helper is to allow Windows 11 and MacOS to register the .torrent and magnet link correctly on both platforms.  Since it is a wrapper of the qBittorrent web interface, all new features should be coming from the web interface, not this Windows/MacOS wrapper.

---

## License

AGPL-3.0

---

[![Buy me a coffee](https://img.buymeacoffee.com/button-api/?text=Buy+me+a+coffee&emoji=&slug=kritblade&button_colour=5F7FFF&font_colour=ffffff&font_family=Poppins&outline_colour=000000&coffee_colour=FFDD00)](https://www.buymeacoffee.com/kritblade)