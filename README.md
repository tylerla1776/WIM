# WIM — Desktop App (Tauri) — Setup & Update Guide

This turns WIM into a real Windows application: its own icon, its own window, **no browser, no address bar**, unlimited local storage, the ability to **update itself from GitHub**, and a **direct, live connection to eBay** (no broker needed).

You do not need to know how to code to follow this. You will copy a few files and run a few commands exactly as written. Take it one numbered step at a time.

The app you run is built from the same WIM you already use. The screen `src/index.html` **is** WIM — when I send you an update, that's usually the only file that changes.

---

## What's in this folder

```
wim-tauri/
├─ package.json                  ← lists the build tool
├─ README.md                     ← this guide
├─ latest.json.example           ← template for the auto-update manifest
├─ .github/workflows/release.yml ← builds + publishes releases automatically
├─ src/
│   └─ index.html                ← WIM itself (the whole app)
└─ src-tauri/
    ├─ Cargo.toml                ← lists the Rust pieces
    ├─ build.rs
    ├─ tauri.conf.json           ← app name, window, updater settings  ★ you edit this
    ├─ capabilities/default.json
    ├─ icons/                    ← the WIM icon, ready to use
    └─ src/main.rs               ← connects WIM to eBay + checks for updates
```

---

## Part 1 — One-time setup on your PC (about 30–40 minutes)

You install three things once. After that, building WIM is two commands.

### 1.1 Install Node.js
- Go to **https://nodejs.org** and download the **LTS** version.
- Run the installer, click Next through it, keep the defaults.

### 1.2 Install Rust
- Go to **https://www.rust-lang.org/tools/install** and download **rustup-init.exe** (64-bit).
- Run it. When it asks, type **1** and press Enter for the standard install.
- Let it finish, then **close and reopen** any terminal windows.

### 1.3 Install the Windows build tools
Tauri needs Microsoft's C++ build tools (this is a Windows requirement, not WIM's).
- Go to **https://visualstudio.microsoft.com/visual-cpp-build-tools/** and download **Build Tools for Visual Studio**.
- Run it. In the installer, tick **"Desktop development with C++"**, then Install. (This download is large; let it finish.)
- WebView2 (the engine that draws the app) is already on Windows 10/11, so there's nothing else to install.

> Mac/Linux is also supported, but these instructions assume Windows since that's what you use.

### 1.4 Put the project somewhere simple
- Move this whole `wim-tauri` folder to an easy location, e.g. `C:\WIM\wim-tauri`.

### 1.5 Install the project's pieces
- Open the **Start menu**, type **"Terminal"** (or "PowerShell"), and open it.
- Go into the folder by typing this and pressing Enter (adjust the path if you put it elsewhere):
  ```
  cd C:\WIM\wim-tauri
  ```
- Then:
  ```
  npm install
  ```
  This downloads the build tool. It only needs to be done once.

---

## Part 2 — Run WIM as an app (to try it)

From inside the `wim-tauri` folder, run:
```
npm run tauri dev
```
The first run compiles everything and can take **5–15 minutes** — that's normal, only the first time. A WIM window will open. This is the live app; close the window to stop it.

> Your login and data are the same WIM you know (user **64**, password **1234** the first time).

---

## Part 3 — Build the installer (the thing you double-click)

When you're happy and want a real installed app:
```
npm run tauri build
```
When it finishes, the installer is here:
```
wim-tauri\src-tauri\target\release\bundle\nsis\WIM_2.2.0_x64-setup.exe
```
Double-click that to install WIM like any normal program. It gets a Start-menu entry and the WIM icon.

You can stop here if you don't care about auto-updates yet — the app already works, stores data, and (after Part 5) talks to eBay. Auto-update (Part 4) is optional but recommended so I can push fixes to you easily.

---

## Part 4 — Auto-update from GitHub (recommended)

This is what lets the app **update itself** when I send you a new version, instead of you reinstalling each time.

### 4.1 Make a signing key (once)
Updates must be digitally signed so the app trusts them. Create your key:
```
npm run tauri signer generate -- -w wim-update.key
```
This prints a **public key** and writes a **private key** file (`wim-update.key`).
- **Copy the public key** it shows (a long line of characters).
- **Keep the private key file safe and private.** Never share it or commit it to GitHub. (Anyone with it could push fake updates.)

### 4.2 Put the public key in the config
Open `src-tauri/tauri.conf.json` in Notepad and:
- Replace `PASTE_YOUR_TAURI_PUBLIC_KEY_HERE` with the public key from 4.1.
- Replace `GITHUB_USER` and `GITHUB_REPO` (both places they appear) with your GitHub username and the repository name you'll make in 4.3.

### 4.3 Put the project on GitHub (once)
- Make a free account at **https://github.com** if you don't have one.
- Click **New repository**, name it (e.g. `wim`), keep it **Private** if you like, and create it.
- Upload this `wim-tauri` folder to it. The simplest no-terminal way: on the new repo page click **"uploading an existing file"** and drag the folder contents in. (Or, if you installed Git, use the commands GitHub shows you.)

### 4.4 Add your signing secrets to GitHub (once)
So GitHub can sign builds for you:
- In your repo: **Settings → Secrets and variables → Actions → New repository secret**.
- Add **TAURI_SIGNING_PRIVATE_KEY** = the entire contents of your `wim-update.key` file (open it in Notepad, copy everything).
- Add **TAURI_SIGNING_PRIVATE_KEY_PASSWORD** = the password you chose when generating the key (if you left it blank, add the secret with an empty value).

### 4.5 Publish a version
The included workflow (`.github/workflows/release.yml`) does the building, signing, and publishing for you whenever you push a **tag**. To release version 2.2.0:
- If you have Git installed, in the project folder run:
  ```
  git tag v2.2.0
  git push origin v2.2.0
  ```
- GitHub will build the installer, sign it, create a **Release**, and attach **`latest.json`** automatically.

From then on, **installed copies of WIM check that release on startup and update themselves silently.** That's the whole point: you ship once, everyone stays current.

> If you'd rather not use the automated workflow, you can build locally (Part 3), then create a Release on GitHub by hand and upload the `-setup.exe`, its `.sig` file, and a `latest.json` based on `latest.json.example` (paste the `.sig` contents into the `signature` field and fix the version + file name).

### How I send you updates after this is set up
Most updates are just a new `src/index.html` (the WIM screen) — occasionally also `src-tauri/src/main.rs`. The routine is:
1. I send you the new file(s); you replace them in the project folder.
2. Bump the version number in **three** places to the same value (e.g. `2.2.1`): `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`. (Also update the `APP_VERSION` line near the top of `src/index.html` if I haven't already.)
3. Push a new tag: `git tag v2.2.1` then `git push origin v2.2.1`.
4. Everyone's WIM auto-updates next time they open it.

---

## Part 5 — Connect WIM to eBay (live)

The desktop build talks to eBay **directly** — this is the big advantage over the browser version.

### 5.1 Get your eBay developer keys (free)
1. Go to **https://developer.ebay.com** and sign in with your normal eBay account, then click **Join** to create the free developer account.
2. Open **My Account → Application Keysets**. Create a **Sandbox** keyset first (Sandbox is a free practice eBay so you can't accidentally post real listings). You'll get an **App ID (Client ID)**, a **Dev ID**, and a **Cert ID (Client Secret)**.
3. Click **User Tokens → "Get a Token from eBay via Your Application"**, sign in, and accept. Copy the **Refresh Token** it gives you.

### 5.2 Enter them in WIM
- Open WIM → **Configuration → eBay Connection**.
- Choose **Sandbox** while testing.
- Paste the **App ID**, **Cert ID**, and **Refresh Token**. Leave Marketplace as `EBAY_US`.
- Click **Save**, then **Test**. You should see **"Connected to eBay — token refreshed successfully."**
- When you're ready for real listings, create a **Production** keyset the same way, paste those keys, and switch the dropdown to **Production**.

### 5.3 Use it
- Open any active item → click **List on eBay** in the item's toolbar.
- You'll see the exact data WIM will send. Click **Create / Update eBay Inventory Item** to send it live to your eBay inventory.
- The item's History tab records that it was pushed to eBay.

### What this first iteration does and doesn't do (honest version)
- **Does:** authenticate to eBay securely and create/update the *inventory item* (the product record) in your eBay account. That's the foundation every listing is built on, and it's a real live API call.
- **Doesn't yet:** turn that inventory item into a *published live listing*. eBay requires an **offer** (price/quantity/listing details) plus your **business policies** (payment, shipping, returns) and a **merchant location**, which you set up once in your eBay seller account. Once you've created those, the next iteration adds two more buttons — **Create Offer** and **Publish** — to finish a listing end-to-end. Tell me when your eBay policies are set and I'll wire those in.

---

## Reverting to the browser version (2.1.6 / 2.1.7)

Nothing here touches your old setup. If you decide the standalone app isn't ready, just keep using the single **WIM.html** file in your browser as before. Your data is independent. You can run both; they each keep their own data.

---

## Troubleshooting

- **"npm is not recognized"** — Node didn't finish installing or the terminal was open before install. Close every terminal, reopen one, try again.
- **"cargo/rustc not found"** — same thing for Rust: reopen the terminal after installing rustup.
- **The first `tauri dev` or `tauri build` takes forever** — expected the first time (it compiles a lot). Later runs are fast.
- **A build error mentions "link.exe" or "C++"** — the Visual Studio C++ build tools (step 1.3) aren't installed or didn't include the C++ workload. Re-run that installer and tick **Desktop development with C++**.
- **A crate version error** — run `cargo update` inside `src-tauri`, then build again.
- **eBay says "invalid_grant" or 401** — the Refresh Token expired or the keys don't match the environment (Sandbox keys with Production selected, or vice-versa). Regenerate the User Token and re-paste.
- **Auto-update doesn't trigger** — the version in `tauri.conf.json` of the *new* release must be higher than the installed one, and `latest.json` must be reachable at the endpoint URL. Check that the GitHub Release published and the URL in `tauri.conf.json` matches your username/repo.

---

## Quick reference

| Task | Command (run inside the `wim-tauri` folder) |
|------|---------------------------------------------|
| Install project pieces (once) | `npm install` |
| Run the app to try it | `npm run tauri dev` |
| Build the installer | `npm run tauri build` |
| Make a signing key (once) | `npm run tauri signer generate -- -w wim-update.key` |
| Publish a release | `git tag vX.Y.Z` then `git push origin vX.Y.Z` |
