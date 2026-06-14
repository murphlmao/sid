# Backlog — captured 2026-06-13 (owner request, verbatim intent)

Raw intake so nothing is lost. Each becomes its own spec before implementation.
Order: finish the in-flight **Settings-tab iteration** first, then A → B → C.

## A. SSH tab is broken (DEBUG + FIX — highest priority)
Owner could barely use the SSH area:
- Struggled to **add an SSH connection**.
- Struggled to **generate a key**. "It hardly worked at all."
- Could **not establish a connection at all**.
- Could **not import keys onto the device**.
- Test device available: `raspberrypi@10.1.1.93`, password `raspberry`.
- Deliverable owner asked for: **test + debug**, then come back with
  (1) a list of things to fix + issues noticed, and (2) a list of questions.
- → This is investigation-first. Do NOT blind-fix; reproduce against the real pi.

## B. Database tab — connect to our own config store + SQLite file ops
- Want an option to **connect to the type of database we use to store
  configuration here**, ideally **auto-added** as a ready-to-use connection in
  the Database UI tab, AND offered as an option when configuring a new database.
- **SQLite:** be able to **point at an existing file and connect**, and
  **create a new SQLite file**. (If we don't already support this, add it.)
- OPEN QUESTION to resolve in the spec: sid's config store is **redb** (a KV
  store), not SQL. The Database tab speaks SQL (Postgres / SQLite via rusqlite).
  redb cannot be queried as SQL. So "connect to the config DB" needs a decision:
  (a) the owner actually means SQLite and assumed that's what config uses, or
  (b) expose the redb store path read-only via some bridge, or (c) scope this to
  "auto-add a SQLite connection pointing at <config dir> if/when we store
  anything in SQLite." Clarify before building.

## C. System tab — edit config files + editor preference
- In the System tab's config area, owner **cannot edit the files**; wants to.
- New **Settings** option to choose how editing happens:
  - editor = `vim` | `vi` | `nano` (**nano default**), OR
  - "spawn a new terminal session that opens the file on launch."
- Spawn behavior: **`cd` into the parent directory** of the file, then open it.
- If **sudo** is required, the user supplies it in the new terminal tab (we do
  not try to escalate ourselves).

---
Status: captured. Next = verify in-flight settings-tab agents, integrate, gate;
then write specs A/B/C.
