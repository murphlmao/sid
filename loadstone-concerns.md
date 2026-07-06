# Lodestone: what happened, what it risks, and why it's not acceptable

Everything in the facts section is verifiable from git history and file diffs against
our repos. The rest is my read, and I've marked it as such. A full technical breakdown
with architecture diagrams lives in the companion doc (`lodestone-analysis.md`).

## What happened

Over the July 4th weekend, while the team that owns gen4 was off, our manager and an
external contractor forked two years of our work and built a spin-off product with it.
Nobody on the owning team was told. We found it ourselves.

The facts:

- Sean initialized the repo on 2026-07-02 at 7:09 AM (commit `e47049a`). He committed
  31 times, wrote the goal doc, wrote the business pitch (`docs/Leadership.md`), and
  vendored our source in on day one. He has since framed this as Brennen's independent
  idea. The git history says that framing is false.
- Brennen authored 81 commits. His family owns one of our customers, and he has a
  history of building proprietary software to route around licensing fees. Every
  commit in the repo, Sean's included, is authored from personal Gmail accounts.
- ~19,200 of its ~27,700 lines of Rust are lifted from gen4-backend-api: ~7,500
  byte-identical, the rest lightly edited copies. The refui frontend is copied in
  wholesale. There is no crate or submodule dependency, just a hand-maintained patch
  folder (`patches/gen4/`) that had already drifted out of sync within four days.
- Full git-stripped snapshots of gen4-backend-api, refui, and core (~99k more lines)
  are vendored under `Copied/`.
- The implementation lives on `win7-lite-foundation` while `main` holds only docs. The
  branch is on origin, so technically visible; practically, it was built to not be
  noticed, and it wasn't, until we went looking.
- The API has no authentication (`Bearer no-auth` placeholder) and binds `0.0.0.0`.
  Its write path can start and end runs, change the optimal rate, and change belt
  speeds by rewriting the REF's control files directly (a from-scratch Rust port of
  the Delphi FG/BG command coordinator). To be fair, it ships read-only by default;
  the write path is enabled by a single environment variable
  (`LODESTONE_REF_ALLOW_COMMANDS=1`), and once enabled there is no authentication in
  front of it. Their own code comments say it has never been validated against a live
  REF, only against inert file dumps.

## The risks, in order of severity

**1. We handed our IP to a conflicted contractor.** The entire gen4 backend, the
frontend, our infrastructure code, and our architecture docs now sit in one portable
repo, authored from personal emails, worked on by a contractor whose family owns a
customer and who has done exactly this kind of end-run before. The repo's explicit
design goal is a standalone offline binary with no telemetry and no dependency on us.
If you wanted to build something that could walk out the door and run at a customer
site without our knowledge, this is what it would look like. I am not claiming that's
the intent. I'm saying nobody should ever have to wonder, and now we do.

**2. It's a company liability aimed at live production machinery.** This thing can
start and end runs and change belt speeds on a customer's egg line by rewriting the
REF's control files. The command channel is a from-scratch port of Mark's Delphi
coordinator that, per their own code comments, has never been validated against a
live REF, only against inert file dumps. It ships read-only by default, and I'll
credit that, but the entire write path sits behind one environment variable, and once
that variable is set there is no authentication in front of it: anyone who can reach
the port can command the machine. Our team spent the last month building
offline-first, fails-closed auth for exactly this class of deployment. If this binary
ever touches a customer site and misbehaves, whether that's a bad handshake stopping
a run mid-shift or an open port on a plant network, it's the company's name on the
incident, not Brennen's. Nobody who has been burned by this codebase would have
shipped that, and nobody who has been burned by this codebase was asked.

**3. It's a hard fork of two years of team investment.** Every bug we fix in gen4 now
has to be manually rediscovered and reapplied in a second repo, forever. Their docs
promise convergence ("full vs lite = one codebase, different composition over the same
trait boundaries," HANDOFF.md) but there is no mechanism behind the promise: no shared
crates, no dependency link, just a patch folder that already drifted once in four
days. Meanwhile the fork discards everything our platform is built on (k8s,
Timescale, ingestion, telemetry), so if it ships and works, every future deployment
decision pulls toward the fork and away from the platform. A convergence promise with
no dependency graph is a wish. It converges or it competes; their docs never engage
with that.

**4. This is the exact disease gen4 exists to cure.** Atlas, NOAH, and REF are what
you get when products are built as disconnected copies abstracting over legacy
systems. Gen4 is the two-year effort to stop doing that. Lodestone is a panic
reversion to the old pattern, and it isn't even the ground-up rebuild it presents as:
it runs *beside* a live Delphi REF and talks to it through file IPC. It cannot exist
without the legacy product running next to it. We are building a more flammable house
next to the one that's already on fire, instead of waking up the firefighters we
already employ.

## The part that makes me angry, stated plainly

The team wanted to build REF from the ground up, without the DataServer/RMM
dependency, from the very beginning. Sean overruled that and committed us to the
architecture we've been maintaining ever since: the disjoint abstraction-over-legacy
mess we've spent two years managing around. Lodestone eliminates the DataServer
dependency, deploys as a single binary, and uses local storage. This team proposed no external DataServer (Delphi) dependency and was denied by Sean.

The write path makes the double standard impossible to miss. For two years we were
not allowed to touch the Delphi side. Everything routed through Mark, the sole
maintainer, on his schedule and his hours. We couldn't fix bugs there ourselves; we
coordinated, we waited, we built workarounds in our own stack for problems we could
see and weren't permitted to solve, and we complained about it the entire time. That
was the cost of doing it "properly." Then a contractor shows up over a holiday
weekend and is handed sole control to rebuild that exact layer, the FG/BG command
coordinator, from scratch, in Rust, with no review, no coordination with Mark, and no
one checking his work against a live machine. Every constraint that was mandatory for
us was optional for him. Either those constraints were never real, or they were
waived for an outsider while being enforced on the team. Both answers are damning.

So the sequence is: he made the architecture call, we paid for it for two years, and
when he finally concluded we'd been right, he didn't bring the do-over to the team he
overruled. He gave it to an outside contractor, over a weekend, in silence, and then
called it the contractor's idea. Managers are allowed to be wrong. They are not
allowed to launder being wrong through an outsider so it never has to be acknowledged,
and they are especially not allowed to do it with the team's own code and a conflicted
contractor.

The "why weren't the maintainers in the loop" question has no good answer. A quick proof of concept
doesn't require secrecy from the owning team; a text message costs nothing. The only
reason not to send one is knowing what the answer would be. If leadership believed the
company needed a new revenue product, the move was to bring that problem to the
engineering team openly. There was no brainstorm, no brief, no incentive, no ask. Just
a fork, discovered after the fact, and a false story about whose idea it was.

## The morale cost, which may be the biggest one

The team is going to hate this, and they'll be right to. Every person on this team
has spent two years doing it the hard way: coordinating through Mark, waiting on the
Delphi bottleneck, going through review, maintaining the architecture Sean chose over
our objections. The lesson Lodestone teaches is that none of that was actually
required. It was required *of us*. The moment leadership wanted something done, the
process, the review, the coordination, and the team itself were all skipped, and the
work went to an outsider using our code.

You cannot ask people to keep doing it the hard way after showing them that. Every
future "we need to route this through Mark" or "this needs review first" will be
answered, silently or out loud, with "Lodestone didn't." The daily effort this team
puts in only makes sense if the rules are the rules; Sean demonstrated they're
negotiable for everyone except the people who follow them.

Speaking for myself: I have lost significant professional trust in Sean over this,
and it wasn't the fork that did it. It was finding out from git history what he
wouldn't say himself, and then watching him frame it as someone else's idea. Trust
in a manager is the belief that he's playing the same game he's asking you to play.
I don't currently believe that, and I don't think I'm going to be the only one.

## What's fair, so it's on the record

- A lite product for the Win7 fleet is a legitimate business idea, possibly a good
  one. I'm not asking to kill the idea.
- The weekend produced real answers: actix builds and runs on Win7 32-bit, and the REF
  flat files are readable in place. Both findings fit on an index card, and both are
  now company knowledge worth keeping.
- The copying is documented in their own provenance ledger, and the branch was pushed
  to our GitLab. It wasn't concealed in the literal sense. That doesn't help much: the
  offense isn't hiding the code, it's bypassing the people who own it.

## Defenses I expect, and the answers

**"It was just a weekend proof of concept."** A proof of concept doesn't need an external
contractor or silence toward the owning team. It also doesn't need a
business pitch document; you don't write `Leadership.md` for a throwaway.

**"Look how fast we moved."** They moved fast because 19k of 27k lines were this
team's work, copied. The genuinely new part is a 4,700-line port of an existing Delphi
component. What the speed bought is an unauthenticated write path into a production
controller and inherited gen4 bugs that are now diverging in a second repo.
Copy-pasting unreviewed code isn't velocity, it's debt with a short grace period.

**"It reuses our UI, so the frontend stays shared."** The UI doesn't exist yet:
`web/dist/` in the repo is a committed placeholder file, with refui sitting in a
frozen copy waiting to be moved in. When someone actually tries, they'll hit the
problem nobody checked: Windows 7 machines cap at Chrome 109, and refui is built on
Tailwind 4 and modern component tooling that requires newer browsers than that. It
will either break outright or render as a degraded, disjoint version of itself,
because that is not what the UI was built for. Making it genuinely work on the Win7
fleet means forking the frontend too, onto older tooling, where it diverges and looks
worse; the "shared UI" is a third fork waiting to happen, on top of the backend one.

**"It's a bridge, not a replacement; we'll converge later."** There is no convergence
mechanism, only a stated intent and a patch folder that already drifted. A real
convergence plan starts with the team and a shared-crate design; it doesn't end with
one after 27k lines are written.

**"Brennen came up with it on his own."** Commit `e47049a`, and the 31 commits after
it, and the goal doc, and the pitch doc. This isn't a judgment call; it's git log.

**"Isn't this territorial?"** I concede the idea and the market up front, and every
remedy below would apply identically if the fork had been made by someone on our own
team. What I won't concede is that the people who maintain a system are an obstacle to
be routed around in their own domain.

**"We were always going to bring it to the team once it was proven."** Deciding in
private and informing after the fact is not consultation; it's a fait accompli with a
delay. The `Leadership.md` pitch was written before any team conversation happened,
which means the plan was to sell it upward first and present the team with a done
deal. If the intent was genuinely "prove, then hand over," the handover would have
been designed in from the start: company accounts, a shared-crate layout, and a note
to the maintainers. None of that is there.

**"It was done on personal time, that's why the personal emails."** This one cuts the
other way. If it's company work, it belongs on company accounts and there's no
explanation for the Gmail authorship. If it's personal work, then two years of
company IP was copied into a personal project with an external contractor, which is
worse. There is no version of the personal-time framing that makes the repo more
defensible; pick either branch and follow it.

**"The team was busy on critical roadmap work; this was the only way to explore it
without disrupting delivery."** That's a resourcing decision, and making resourcing
decisions is Sean's day job, in the open. Nothing about exploring a lite product
required the team to stop anything: it required one message. And the "no disruption"
framing is false anyway, because the team inherits the disruption on the back end:
the fork's maintenance, the review it never had, and this entire conversation.

**"Don't blow this up, nothing has shipped, no harm done."** The harm isn't
hypothetical and it isn't about shipping. Our IP is in a portable repo under personal
accounts with a conflicted contractor; the precedent that process is optional has
been set in front of the whole team; and the trust cost is already paid. Waiting for
a shipped incident before calling something harmful is how the last twenty years of
this codebase happened.

**"This should inspire the team, look what's possible in a weekend."** What the
weekend proves is what copying two years of this team's work gets you, minus the
parts that take actual time: auth, review, validation against real hardware, and a
UI that runs on the target fleet. Presenting that as a velocity benchmark for the
team is exactly backwards; the team is the reason the weekend was possible.

**"Sean was just advising; Brennen drove it."** Sean created the repo, authored 31
commits, vendored our source in on day one, wrote the goal document, and wrote the
business pitch. Advisors don't hold the pen on the charter. Every downgrade of Sean's
role has to survive contact with the git log, and none of them do.

## What should happen

1. **Contain the repo immediately.** Revoke external access and audit what has
   already left. This part is not debatable and should not wait for a meeting.
2. **Keep the knowledge, quarantine the artifact.** The Win7 toolchain and file-format
   findings are valuable. The code is an unreviewed, unauthenticated fork that should
   not become a product by momentum. The prototype answered its question; the answer
   belongs to the company, not to the people who obtained it this way.
3. **Give the team the brief we never got.** Time-box two weeks for a team-owned
   answer to "what does gen4-ref-lite look like as a profile of the real codebase":
   one binary, SQLite storage seam, our proven device-mode auth, no cluster. The
   hardest lite-shaped problems (offline-first auth, config seams) are already solved
   on our side, some of them within the last month. Then put a maintained lite
   *profile* of the product next to an orphaned fork of it and compare honestly.
4. **Acknowledge the process failure out loud.** Not the idea, the method: a manager
   privately answering a company strategy question with outside help, using the
   team's own code, and misrepresenting his role afterward. If that isn't named and
   owned, no process fix matters, because the lesson everyone learns is that the way
   to get your idea built here is to go around the team that would have to maintain
   it.

I'm angry, and I think the anger is proportionate to what the history shows. The facts
are checkable by anyone with repo access, and if any of them are wrong I want to know.

## Quick reference

**Facts**
- Sean created the repo (`e47049a`, 2026-07-02 7:09 AM), 31 commits, wrote Goal.txt
  and the Leadership.md business pitch, vendored our source on day one.
- Brennen: 81 commits, family owns a customer, history of routing around licensing.
- All 112 commits from personal Gmail accounts.
- ~19.2k of ~27.7k Rust lines lifted from gen4-backend-api (~7.5k byte-identical);
  refui copied wholesale; ~99k more lines vendored under `Copied/`.
- No dependency link to gen4; the `patches/gen4/` back-port ledger drifted within
  four days.
- Write path can start/end runs, set optimal rate, change belt speeds. Read-only by
  default, but one env var enables it and there is no auth behind it. Never validated
  against a live REF (their own comments).
- The "shared" UI is a placeholder file; refui's tooling (Tailwind 4) won't run
  properly on Win7's Chrome 109 ceiling.

**Arguments**
- The idea is fine; the execution and the method are the problem. Concede the idea
  first, then nothing else needs conceding.
- The four-day speed is our two years, copied. The new part is one Delphi port.
- Convergence with no shared crates and no dependency is a wish; it converges or it
  competes.
- Lodestone is the Atlas/NOAH pattern gen4 exists to kill: another copy abstracting
  over a live legacy REF it can't exist without.
- The team was denied exactly this: no DataServer dependency, single binary, local
  storage. Sean denied it, then did it with an outsider.
- Two years of "route it through Mark" discipline, waived in a weekend for a
  contractor rebuilding Mark's own layer with no review and no Mark.
- Morale: the rules were only ever enforced on the people who followed them. Every
  future "this needs review" gets answered with "Lodestone didn't."
- Liability: if it misbehaves on a plant floor, it's the company's name on the
  incident.

**Counters to expected defenses**
- "Weekend proof of concept" → doesn't need a contractor, silence, or a business
  pitch doc.
- "Look how fast" → 19k of 27k lines were ours; the speed bought no auth and no live
  validation.
- "We'll converge later" → no mechanism, ledger already drifted; real plans start
  with shared crates, not end with them.
- "Brennen's idea" → git log.
- "We'd have told you once proven" → pitch-upward-first is a fait accompli, not
  consultation.
- "Personal time" → then company IP is in a personal project; both branches are
  worse.
- "Didn't want to disrupt the team" → one message; the team inherits the disruption
  anyway.
- "No harm, nothing shipped" → IP exposure, precedent, and trust are already spent.
- "Be inspired" → the team is the reason the weekend was possible.
- "Sean just advised" → advisors don't write the charter.

**Asks**
1. Revoke external access; audit what left.
2. Keep the knowledge (Win7 toolchain, file formats); quarantine the artifact.
3. Two-week team-owned answer: gen4-ref-lite as a profile of the real codebase (one
   binary, SQLite seam, our device-mode auth, no cluster). Compare honestly.
4. Name and own the process failure, out loud, or the lesson everyone learns is to
   go around the team.
