# PRD — To Own: from a list to a shopping list

**Status:** Draft · **Version:** 2.0 · **Applies to:** Unsub 0.5

To Own is the list of songs your streaming history names that you don't own a copy of. This
describes six changes that turn it from a list you scroll past into a list you act on.

---

## The short version

To Own is accurate and almost useless. It knows what you're missing; it doesn't help you decide what
to do about it.

These six features give it an order, a price, a sense of completion, a way out of the app, a memory,
and a bit more honesty. None of them requires a new source of music, an account, or an API key.

## The problem

**It forgets the one thing that says what you care about.** Unsub already counts how many times
you've played each song you don't own — then uses the number only to decide whether the song belongs
on the list at all, and throws it away. A song you've played forty times and one you played twice sit
next to each other, sorted alphabetically.

**Every album costs four clicks and four tabs.** Each row offers four generic search links. You
click one, land on a website, and start typing the artist name yourself. For an album, there's an
optional lookup that finds real store pages, but it works one album at a time and nothing ever walks
the list — so most albums never get resolved.

**It never notices when you act on it.** Buy an album, import it, and its songs stay on To Own
forever. A list that drifts from reality stops being trusted, and once you stop trusting it you stop
opening it.

## What we're not doing

- **No new sources of music.** No torrents, no trackers, no download search. Prices and links only;
  the app never fetches music from a store.
- **No buying inside the app.** Links open in your browser. Unsub never touches payment details.
- **No accounts, keys, or sign-ins.** Everything comes from public catalogues that need none.
- **No telemetry.** The app reports nothing about you, and these features add nothing to that.
- **No price tracking.** Prices are fetched and shown. They are not watched, charted, or alerted on.

## Who it's for

**The returning listener** has a few hundred songs on the list from a decade of streaming and wants
to know which twelve to buy first. *Show me what I actually miss.*

**The album completer** owns eleven of fourteen tracks and doesn't want eleven separate top-ups.
*Tell me I'm three short, and sell me the album.*

**The list-maker** wants to take the whole thing somewhere — a record shop, a spreadsheet, a friend.
*Let me get this list out of here.*

---

## 1. Rank by what you'd miss

Carry the play count through to the list, and let the list be ordered by it.

Unsub already has the numbers. Play history imported from Spotify, YouTube Music or ListenBrainz
includes how often each song was played and when; library exports carry their own play counts. All of
it is read today and none of it reaches the list.

The default order becomes **most played**. Two other orders are offered: **recently played**, for
people who care about what they've been listening to lately, and plain **A–Z**, for people who want
the list they have now.

### What you'll see

- Each row shows how many times you played it, and when you last did.
- The list opens sorted by play count, highest first.
- A song whose play count is unknown — something only a playlist named, never the history — shows no
  number at all rather than "0", and sorts to the bottom of its group.
- The choice of order sticks while the view is open.

## 2. Real prices and real links

Resolve the whole list at once and show what things cost, instead of four search boxes.

The catalogue lookups behind this already work in Unsub: the album lookup that finds store pages
exists today, and the same Apple catalogue the podcast directory uses can return an album's price and
a direct link to it. Both are public, both need no key, and both are already rate-limited and cached
in this codebase.

The difference is that they get applied to the whole list rather than one album at a time, and the
answers are kept so the second visit is free.

### What you'll see

- Instead of four generic buttons, an album shows where it can actually be bought: the store, the
  price, and a link straight to that album.
- An album nobody has priced still shows the old search links rather than an empty space.
- Prices carry their currency, and are remembered — revisiting the list doesn't re-ask.
- If a catalogue is unreachable, the list still works. Prices go quiet; nothing breaks.

## 3. Complete the album

Read a partly-owned album as one thing: "9 of 14 tracks, 5 to go."

Right now a half-owned album is nine rows in one place and five in another, with nothing connecting
them. Grouping them and counting what's missing turns a scattered list into a handful of decisions.

Stores sell albums, not fractions, so completing an album is the same action as buying it — which is
why this pairs with real prices.

### What you'll see

- An album row says how complete it is, and lists exactly which tracks are missing.
- Where the album's total track count isn't known, the row simply counts what's missing and shows no
  fraction.
- An album you own none of looks exactly as it does today.

## 4. Take the list with you

Export To Own as a spreadsheet, a Markdown file, or a printable page.

A shopping list you can't carry isn't much of a shopping list. The export includes titles, artists,
albums, play counts, last played, and any prices already found.

### What you'll see

- Three formats: CSV for a spreadsheet, Markdown for notes, and a printable page for paper.
- The file reflects the order you're looking at, so "most played" exports as most played.
- An empty list exports cleanly rather than producing a broken file.

*Note:* an M3U playlist was considered and dropped. A playlist of files you don't have can't play;
it would be a broken file pretending to be a shopping list.

## 5. Remove what you buy

To Own holds what you don't own. Once you own something, it is deleted from the list — not ticked,
not struck through, not moved to a "purchased" pile. It's gone.

Buying an album currently leaves its songs on To Own indefinitely. Removal happens after a purchase,
after a folder is added, and after an import — so the list reflects the library as it is, not as it
was.

There is no completed state to manage and nothing to un-tick later. The single exception is
deliberate: a song is only removed when it's certainly the same recording. A doubtful match keeps its
row, because quietly deleting a song from a working list is worse than an extra row.

### What you'll see

- Buy an album, import it, and its songs are removed from To Own outright — no restart, no tick, no
  strikethrough.
- The list only ever contains missing songs. Nothing lingers as "done".
- A song that only loosely resembles something you own stays put.
- Delete a song from the library and it reappears, because your history still names it.

## 6. Show the near-miss

When a song on the list looks like something you already own but might not be, say so.

The matching Unsub does today is graded, and the loosest grade counts as owned — so a song you don't
really have can vanish from the list entirely. A live version matching a studio title is the common
case. That's the wrong kind of silence: it hides something you might want to buy.

### What you'll see

- A loosely matched row says "Probably yours — a different recording?" and shows what it matched.
- Exact matches look exactly as they do now, with no badge and no noise.
- The badge is a note, not a reclassification: totals, filters and sorting don't change.
- Clicking it shows the track it matched, so you can judge for yourself.

---

## Where the information comes from

Two public catalogues, both already used elsewhere in Unsub, neither needing an account:

| For | From | Key needed |
|---|---|---|
| Store pages for an album | MusicBrainz | None |
| Price, currency, direct album link | Apple's public catalogue | None |

Everything else — play counts, album completeness, the list itself — is computed from data already
on your computer. Most of these features make no network requests at all.

Answers are cached locally, so the list stays useful without a connection once it's been priced
once.

## What ships first

**First:** features 1, 4, 5, 6, and the counting half of 3. These are entirely local — no network, no
new dependency, no privacy surface. They are the largest change in how the list feels, for the
smallest amount of risk.

**Second:** feature 2, and with it the buy-the-album action in feature 3. This is the only part that
depends on someone else's service, so it comes after the list is already worth opening.

## Open questions

1. **Default order** — most played, or keep A–Z and let people opt in?
2. **Which storefront's prices** — the system's region, or a choice made on first run?
3. **When prices load** — quietly in the background as the view opens, or behind a "price my list"
   button?
4. **Does a near-miss deserve its own filter**, or is a badge enough?

## How we'll know it worked

No telemetry is added, so these are things worth asking about rather than measuring:

- Do people stop describing To Own as something they ignore?
- Do the lists get shorter over time, because the app now notices purchases?
- Does anyone report buying something twice — the thing feature 6 exists to prevent?
