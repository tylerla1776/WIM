# Connecting WIM to eBay — plain-English setup

This is everything you need to do on eBay's side so WIM's Live features work. You do not need to write any code. Most of this is one-time, done on eBay's websites. Work top to bottom.

There are **two different eBay sites** you'll use, and it's easy to mix them up:
- **eBay Seller Hub / your normal eBay account** (ebay.com) — where your shipping, returns, and payment settings live.
- **eBay Developer site** (developer.ebay.com) — where you get the "keys" that let WIM talk to eBay.

---

## Part A — Your seller account (so eBay will allow listings)

You said eBay already knows your ship-from address — good, that's one piece. To actually publish listings through an app, eBay also wants **business policies** turned on. Here's how to confirm/finish that.

### A1. Turn on Business Policies
1. Sign in at **ebay.com**.
2. Go to **Account settings → Business policies** (direct link: ebay.com/bp/policy-settings).
3. If you see an **"Opt in"** button, click it. (If it shows you existing policies, you're already opted in.)

### A2. Make sure you have one of each policy
On that same Business Policies page, confirm you have at least one of each:
- **Payment policy** (usually auto-created — just confirm it exists).
- **Shipping policy** (your shipping options + who pays).
- **Return policy** (returns accepted or not, and the window).

If any is missing, click **Create policy**, fill the simple form, and save. You only need one of each to start.

### A3. Confirm your ship-from location
- Still in seller settings, make sure your **shipping/return address** is set (you mentioned this is done).

That's the whole seller side. You don't need to do anything technical here — these are normal seller settings, and WIM will use them automatically when it publishes (in the publishing update).

---

## Part B — Developer keys (so WIM can connect)

You may have already done some of this earlier. This is just to confirm it's complete and correct.

### B1. Create the free developer account (once)
1. Go to **developer.ebay.com**, click **Sign in**, use your normal eBay login.
2. If prompted, **Join** the developer program (free).

### B2. Get your keys
1. Go to **My Account → Application Keysets**.
2. You'll have two environments:
   - **Sandbox** = a fake practice eBay (safe for testing — nothing is real).
   - **Production** = the real eBay (real listings, real money).
3. For each environment you'll see an **App ID (Client ID)**, **Dev ID**, and **Cert ID (Client Secret)**.

> Start with **Sandbox** while you're testing WIM's new buttons. Switch to **Production** when you're ready to touch real listings.

### B3. Get your RuName (the redirect name)
WIM can sign you in directly — you no longer have to manually generate a token on eBay's site. But eBay requires the redirect address ("Your auth accepted URL") to be a secure **https://** address — it will reject `http://localhost` outright. WIM gets around this with a tiny pass-through page hosted on GitHub Pages that immediately forwards eBay's response to WIM running on your computer.

**One-time setup (you, not me, since it's your GitHub account):**
1. On github.com, open the **WIM** repo → **Settings → Pages**.
2. Under **Build and deployment → Source**, choose **Deploy from a branch**.
3. Branch: **main**, folder: **/docs**. Save.
4. Wait a minute or two, then confirm `https://tylerla1776.github.io/WIM/ebay-callback.html` loads (it'll just say "Couldn't reach WIM automatically" if you open it directly without WIM running and without a code in the URL — that's expected and fine).

**Then on the eBay developer site:**
1. Open your keyset → **User Tokens**. Note the **RuName** shown there — it looks like `Your_Name-YourApp-WIM-abcde`.
2. Click into that RuName's settings and set **"Your auth accepted URL"** to:
   `https://tylerla1776.github.io/WIM/ebay-callback.html`
3. Save it. (You only do this once per environment — Sandbox and Production each have their own RuName.)

### B4. Connect WIM to eBay
1. Open WIM → switch to **Live** mode → **Configuration → eBay Connection**.
2. Choose **Sandbox** (or Production).
3. Paste your **App ID**, **Cert ID**, and **RuName**.
4. Click **Connect with eBay**. Your browser opens eBay's real sign-in page — sign in and click **Agree**.
5. eBay redirects to the GitHub Pages page, which immediately bounces you back to WIM running on your computer. Switch back to WIM — within a few seconds it shows **"Connected — WIM got its own token directly from eBay and saved it."** Nothing to copy or paste, as long as WIM is open when you click Agree.

**If the automatic bounce-back doesn't work** (firewall, browser settings, etc.), the GitHub Pages page shows a fallback after a few seconds with a "Try again" link and a text box you can copy — paste that into WIM if asked, or just try the link again with WIM open.

**Prefer the old manual method entirely?** You can still get a token the old way (developer site → User Tokens → "Get a Token from eBay via Your Application" → copy the Refresh Token) and paste it into the **User Token** field, then press **Test current keys**. Both methods write to the same place, so you can mix and match.

---

## Part C — Using the new v2.2.4 buttons

### "🔍 Find on eBay" (on an item's page, Live mode)
1. Open an item, make sure it has a **Category** and **Sub Category** chosen.
2. Click **🔍 Find on eBay**.
3. The first time, it asks eBay which **category** best matches and shows a few choices — pick the closest one. (It remembers this on the sub category, so you only pick once per sub category.)
4. WIM then pulls eBay's **listing fields** for that category (Author, Genre, Studio, etc.), with eBay's suggested values in dropdowns.
5. Edit anything, then **Accept**. The fields are saved to the item *and* to the sub category (so the next item in that sub category already has the right fields).

### "Pull Active Listings from eBay" (Manage Listings / Live Inventory)
1. Click the button.
2. WIM asks eBay for your active inventory and matches each one to a WIM item **by SKU**, updating title, price, quantity, and status.
3. It tells you how many matched, and lists any eBay SKUs it couldn't find in WIM.

---

## What still needs eBay's extra permission (not your fault, just how eBay works)

- **Field structure always works** (the list of fields for a category). 
- **Auto-filling the *values*** from a known product (e.g. eBay already knows this exact book's author) can require eBay to grant your app **catalog access**. If values don't prefill, the fields still appear and you type them in — nothing is broken. If you want, you can request Catalog/Buy API access from the developer site later; I'll guide you when we get there.

---

## If a button shows an error

WIM shows you eBay's actual response. The usual ones:
- **"invalid_grant" / 401** — the Refresh Token expired or you mixed Sandbox keys with Production (or vice-versa). Redo **B3** for the right environment and re-paste.
- **"Insufficient permissions" / scope error** — your keyset/token doesn't include the needed access; regenerate the User Token (B3) and try again.
- **Nothing happens / "set up your eBay keys first"** — you're either not in the desktop app, or the keys aren't saved+tested yet (B4).

Copy the error text to me and I'll tell you exactly which setting to fix.
