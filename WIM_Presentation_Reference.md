# WIM — Warehouse Inventory Management
### Reference content for presentation / slideshow build

---

## 1. What WIM Is, in One Paragraph

WIM is a custom-built desktop application that runs the full lifecycle of a warehouse resale operation — from an item arriving in inventory, through being listed on eBay, sold, shipped, and completed — with real-time eBay integration, a genuine multi-user permissions system, cross-device cloud sync, and built-in accuracy and communication tools for a team. It replaced an Excel/VBA workbook prototype and has grown into a full Tauri desktop application (Rust backend, single-file frontend) with real separate operating-system windows, live eBay API integration, and a real cloud backend.

---

## 2. How the App Works — The Core Flow

**1. Item enters inventory** (Warehouse mode) — added individually or in bulk, assigned a permanent category-based Item Number and a physical warehouse slot location (row/column, e.g. "B.4.3").

**2. Item gets listed** — either built and sent for supervisor review ("Send Listing") or listed directly to eBay by authorized staff ("List on eBay"), with real eBay category mapping, multi-photo management, and a Featured/Secondary photo system.

**3. Item sells** — WIM automatically detects the sale via eBay's real order data (polled every 5 minutes while in Live mode), pulls it out of active inventory, and starts it through the Fulfillment workflow.

**4. Fulfillment** — Recently Sold → Shipping (Ready to Ship / Shipped, with real eBay order status updates and tracking numbers) → Completed, with automatic aging and an eventual move to the permanent Catalog record.

**5. Accuracy & communication run alongside the whole thing** — every correction to an already-listed item is tracked and attributed; a team messaging widget keeps Warehouse and Listing staff in sync without leaving the app.

Two operating modes run throughout: **Warehouse mode** (physical inventory, slotting, receiving) and **Live mode** (eBay-facing: listings, orders, messages, reviews) — with a five-level permission system controlling exactly what each person can see and do in each.

---

## 3. Complete Feature List

### Core Inventory Management
- Category-based permanent Item Numbering, separate from physical warehouse SKU/slot
- Physical slot-based location system (row.column.placement), with automatic and manual slot assignment
- Bulk add, bulk adjust, and category/vendor/condition maintenance
- Real item statuses: Listed, Created, Active, Sold, Inactive — not just a simple sold/unsold flag
- Multi-photo management per item: a real Featured/Secondary system, thumbnail strip, multi-select upload with duplicate detection
- Print Queue for physical warehouse labels (SL-100 layout, iframe-based printing)

### eBay Integration (Real, Live API — Not Simulated)
- Real OAuth connection to eBay's production API
- Live listing creation and updates through eBay's Inventory API
- Bulk import of existing eBay listings into WIM, with automatic category mapping and quantity-aware item creation
- Real eBay order pulls (Fulfillment API) — automatic detection of sales every 5 minutes
- Real shipping status updates pushed back to eBay, with tracking numbers
- Real eBay seller feedback (Positive/Neutral/Negative, matching eBay's actual system — not a star rating)
- Real, live eBay API rate-limit monitoring across every API WIM uses
- Full eBay buyer messaging integration (read and reply from inside WIM)
- eBay marketplace account-deletion compliance — a real webhook listener that automatically purges buyer data when eBay reports an account deleted

### Fulfillment Workflow
- Recently Sold, Shipping (Ready to Ship / Shipped), and Completed stages, each a real, distinct screen
- Automatic sale detection, automatic aging, and an admin-configurable auto-complete window (since eBay itself has no "delivered" signal to rely on)
- Packing slip printing with real buyer shipping information
- Recently Listed view — everything listed in the last 7 days, with one-click access to modify

### Fulfillment Accuracy (Quality Control)
- Every correction made to an already-listed item is automatically tracked: what changed, when, and by whom — attributed to whoever originally listed it
- A full report (Supervisor and up): date-range filtering, per-user violation summaries, drill-down into exactly what was wrong, repeat-violation flagging
- A dashboard widget summarizing recent violations and top offenders
- Per-viewer "unread" tracking, so nothing gets missed silently

### Permissions & Security
- Five-level supervisory permission system (Basic, Warehouse, Product Lister, Supervisor, Admin), fully customizable per level and per individual user
- Individual permission overrides that survive a level change
- Same-level-or-lower management rules — a Supervisor can't touch an Admin account
- Change Password, Modify Users, and (placeholder pending real infrastructure) Force Log Out

### Cloud Sync & Multi-Device
- Real Supabase-backed cloud database, shared across every device on the team
- Automatic device-trust cloud login — every staff member gets real cloud access the moment they log in locally, with zero setup outside the app
- Real conflict detection on cloud writes (no silent overwrites of someone else's changes)
- Device registry with remote pause/rename, and both team-wide and per-device sync pause controls
- Automated daily cloud backups to OneDrive with rolling retention, plus a 7-day fail-safe if automation ever silently breaks
- A guided, safe data migration tool (preview before running, safe to re-run, never duplicates)

### Reporting
- Aging Inventory, Item Report, Active Summary, Item Rank, Listing Efficiency, Listing Report
- Records — a full searchable transaction history (adds, sells, adjustments) with date-range, description, SKU, and listing-status search
- A real, customizable dashboard with charts (with genuine Y-axis scale and hover tooltips), tiles, and rank lists

### Communication
- Full eBay buyer messaging (read, reply, unread tracking) from inside WIM
- The Agenda Widget — real-time, cross-device internal team messaging (Warehouse / Listing / All), pinned to every dashboard, with role-based visibility and admin moderation

### Demonstration & Testing
- A full Simulation Mode with named, switchable demo profiles — generates months of realistic, believable inventory, sales, feedback, and message history in one click, entirely separate from real data and instantly reversible

### Reliability & Operations
- Automatic daily backups with verified-success tracking and a real fail-safe if automation breaks silently
- Low cloud-storage warnings before space actually runs out
- An in-app update mechanism with a real installer-based release pipeline

---

## 4. Strengths of the Application

- **Built on real, live data — not a mockup.** Every eBay integration (listings, orders, messages, feedback, category mapping) talks to eBay's actual production API. Nothing in the day-to-day workflow is simulated.
- **Genuinely cross-device.** This isn't a spreadsheet or a single-computer tool — the whole team works from a shared, real-time-synced source of truth, from any device.
- **Real accountability built in.** The Fulfillment Accuracy system means listing mistakes are automatically tracked and attributed — not something a manager has to notice and remember on their own.
- **A real permissions system, not a light switch.** Five levels, fully customizable, with individual overrides — the app can grow with a team of any size without becoming unmanageable.
- **Resilient by design.** Automatic backups, fail-safes for when automation silently breaks, and conflict detection that prevents one person's work from silently overwriting another's.
- **Demo-ready on demand.** Simulation Mode means the app can be shown off convincingly — with realistic months of history — without touching a single piece of real inventory data.
- **Purpose-built for exactly this business,** not a generic inventory tool bent to fit — every screen reflects how this specific warehouse-to-eBay operation actually works.

---

## 5. General Pros / Business Value

- Reduces listing errors and gives real visibility into who's making them, so coaching is based on data, not guesswork
- Cuts the time spent manually cross-checking eBay against warehouse records — the two are now the same system
- Makes onboarding new staff faster: permissions, structure, and workflow are already defined and enforced by the app itself
- Removes single points of failure — data lives in the cloud, is backed up automatically, and isn't trapped on one person's computer
- Internal communication happens where the work already happens, instead of over text or a separate chat app
- Scales from one person to a full team without re-architecting anything

---

## 6. Future Opportunities

- **Full cloud migration of every screen** — the schema and migration tooling already exist; the remaining work is wiring each individual screen to read/write live from the cloud instead of local storage, which would make the app fully real-time across every device, not just the pieces already migrated (accounts, compliance, Agenda messaging).
- **Real eBay-generated shipping labels** — currently WIM prints a packing slip on your own printer; eBay's actual label-purchasing API is a separate, whitelist-only approval that could be pursued if worth the investment.
- **Real carrier delivery tracking** (UPS/FedEx APIs) — would replace the current day-based auto-complete estimate with genuine "this package actually arrived" detection.
- **A dedicated in-app staff management screen** — right now, adding a new cloud-linked staff member outside of local login still involves some direct database work; a real screen would remove that entirely.
- **Print preview and label layout overhaul** — a planned pass at making label printing more flexible and visually configurable.
- **Real cross-device "who's logged in right now" tracking** — would unlock a genuine Force Log Out capability, currently a placeholder.

---

## 7. Suggested Slide Flow

1. Title / what WIM is (Section 1)
2. The core workflow, visually (Section 2 — this is a natural diagram slide)
3. Feature overview — pick 4-6 categories from Section 3, not the full list
4. Strengths (Section 4)
5. Business value / pros (Section 5)
6. What's next (Section 6)
7. Closing / questions
