# B3 user test: a first game by someone new to box-engine

Status: **protocol ready, not run yet** (2026-11-25). The behaviours plan ([BEHAVIOURS.md](../BEHAVIOURS.md), B3) asks for one person who has not used box-engine to go from install to an exported game with only the guides, and for every place they got stuck to be written down here.

## Who

Someone who has not used box-engine, ideally has never made a 3D game, and is comfortable installing a Mac app. Not an engine developer. One session; a second person later is welcome.

## Setup (by the facilitator, before the session)

1. Build the release: `scripts/release/package-macos.sh`; check `scripts/release/clean-machine-test.sh` passes on the same zip.
2. Give the tester the `.dmg` (or `.zip`) and nothing else; open [getting-started.md](../guide/getting-started.md) and [first-game.md](../guide/first-game.md) in a browser for them.
3. A Mac without Rust or Xcode tools is best (a fresh user account is enough).
4. Screen recording on, with the tester's agreement. Start a timer.

## Script (read to the tester)

> "This is a tool for making 3D games. Please install it and make the small game the 'Your first game' page describes, then export it so you could send it to a friend. Read the pages as you would on your own. Think aloud: say what you expect, what surprises you, and when you are unsure. I will not help unless you are stuck for five minutes. There are no wrong moves; we are testing the tool, not you."

## What to watch for (facilitator)

- Each step of the guide: did they find the menu or button the guide names? How long did it take?
- Every hesitation over 30 seconds, every wrong click that cost time, every time they reread the guide.
- Anything they expected to work by dragging or clicking in the viewport that did not.
- Words they did not understand (Inspector, preset, component, collider, navmesh…).
- Whether Play, Stop and the win screen were clear; whether they knew where the game was saved.
- Any error, toast or warning they saw, and whether they understood it.
- Whether the exported app opened on the first try (Gatekeeper prompt: did they know what to do?).

Help only after five minutes stuck, and note exactly what you said.

## After the session (5 minutes, ask)

1. What was the hardest part?
2. What did you expect to be there that was not?
3. On a scale of 1–5, how likely would you make a second game with it, and why?
4. What would you make next?

## Results

| Field | Value |
|---|---|
| Date, tester (no names: "tester A") | |
| Mac and macOS version | |
| Install to first Play | min |
| First Play to win screen | min |
| Win to exported app running | min |
| Total | min |
| Times helped | |
| Likelihood of a second game (1–5) | |

### Where they got stuck

| # | Guide step | What happened | Time lost | Severity (blocked / slowed / confused) | Fix |
|---|---|---|---|---|---|
| 1 | | | | | |

### Quotes

-

### Follow-up

Issues found here go into [DX_REVIEW.md](../DX_REVIEW.md)'s table with the ID prefix U (U1, U2…), and the fixes are listed in this file's table.
