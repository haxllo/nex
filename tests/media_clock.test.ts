import { describe, expect, it } from "vitest";
import "../apps/core/assets/media_clock.js";

const { shownPosition, resumeFromHold } = (globalThis as typeof globalThis & {
  NexMediaClock: {
    shownPosition: (
      anchor: { status: string; pos: number; at: number },
      now: number,
      lastPushAt: number,
      lastPushPos: number,
      stallHold: number | null,
    ) => number;
    resumeFromHold: (
      anchor: { key: string; status: string; pos: number; at: number },
      held: number,
      pushed: number,
      now: number,
    ) => { key: string; status: string; pos: number; at: number };
  };
}).NexMediaClock;

describe("media progress clock", () => {
  it("keeps advancing across regular same-track pushes", () => {
    const anchor = { status: "playing", pos: 20, at: 0 };
    let lastPushAt = 0;
    let lastPushPos = 20;
    let previous = 20;

    for (let now = 500; now <= 10_000; now += 500) {
      const position = shownPosition(anchor, now, lastPushAt, lastPushPos, null);
      expect(position).toBeGreaterThanOrEqual(previous);
      lastPushAt = now;
      lastPushPos = position;
      previous = position;
    }

    expect(previous).toBe(30);
  });

  it("freezes at the last extrapolated position after pushes stop", () => {
    const anchor = { status: "playing", pos: 20, at: 0 };
    expect(shownPosition(anchor, 7_001, 3_000, 23, null)).toBe(23);
    expect(shownPosition(anchor, 9_000, 3_000, 23, null)).toBe(23);
  });

  it("uses the running clock through a status transition before pausing", () => {
    const anchor = { status: "playing", pos: 20, at: 0 };
    const visibleAtPause = shownPosition(anchor, 5_000, 4_500, 24.5, null);
    expect(visibleAtPause).toBe(25);
    anchor.status = "paused";
    anchor.pos = visibleAtPause;
    expect(shownPosition(anchor, 8_000, 5_000, 25, null)).toBe(25);
  });

  it("rebases from the held position when playback recovers from a stall", () => {
    const resumed = resumeFromHold(
      { key: "track", status: "playing", pos: 100, at: 0 },
      103,
      104,
      6_000,
    );
    expect(resumed).toEqual({ key: "track", status: "playing", pos: 104, at: 6_000 });
    expect(shownPosition(resumed, 6_500, 6_000, 104, null)).toBe(104.5);
  });

  it("does not regress when a fresh sample resumes behind frozen progress", () => {
    const resumed = resumeFromHold(
      { key: "track", status: "playing", pos: 100, at: 0 },
      103,
      102.5,
      6_000,
    );
    expect(resumed.pos).toBe(103);
    expect(shownPosition(resumed, 6_500, 6_000, 103, null)).toBe(103.5);
  });
});
