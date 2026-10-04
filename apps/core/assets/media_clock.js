(function (root) {
  function shownPosition(anchor, now, lastPushAt, lastPushPos, stallHold) {
    if (anchor.status !== "playing") return anchor.pos;
    if (stallHold !== null) return stallHold;
    if (now - lastPushAt > 4000) return lastPushPos;
    return anchor.pos + Math.max(0, now - anchor.at) / 1000;
  }

  function resumeFromHold(anchor, held, pushed, now) {
    return { ...anchor, pos: Math.max(held, pushed), at: now };
  }

  root.NexMediaClock = { shownPosition, resumeFromHold };
})(globalThis);
