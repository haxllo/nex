import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";

// The What's New page has two levels: an always-visible intro and collapsed
// drawers. Which lines land in which is the whole design, and it is decided
// here rather than in the document, so it is worth pinning.
const source = readFileSync(new URL("../apps/core/assets/app.js", import.meta.url), "utf8");

function harness() {
  const band = source.slice(
    source.indexOf("  function escapeHtml("),
    source.indexOf("  function postWhatsNewResize("),
  );
  const context: any = {};
  runInNewContext(`${band}\nthis.render = renderWhatsNewMarkdown;`, context);
  return (md: string, hero = "") => context.render(md, hero) as string;
}

const doc = `## Chat works where your files are

This release is about Chat. It holds a folder.

### Chat

- **A folder for the conversation**: pick one and bare names resolve.
- **One approval covers the rest**: the task continues.

### Fixes

- Quitting no longer freezes input.
`;

describe("the What's New page", () => {
  it("puts the release phrase in a heading and the intro above the drawers", () => {
    const html = harness()(doc);
    expect(html).toContain('<h2 class="wn-heading">Chat works where your files are</h2>');
    expect(html.indexOf("wn-heading")).toBeLessThan(html.indexOf("This release is about Chat"));
    expect(html.indexOf("This release is about Chat")).toBeLessThan(html.indexOf("wn-section"));
  });

  it("makes every section a closed drawer, not a flat heading", () => {
    const html = harness()(doc);
    expect(html.match(/<details class="wn-section">/g)).toHaveLength(2);
    expect(html).toContain("<span>Chat</span>");
    expect(html).toContain("<span>Fixes</span>");
    // No bare <h3>: a drawer that also renders as a heading reads twice.
    expect(html).not.toContain("<h3>");
    expect(html).not.toContain("open>");
  });

  it("keeps each bullet inside the drawer it was written under", () => {
    const html = harness()(doc);
    const chat = html.slice(html.indexOf("<span>Chat</span>"), html.indexOf("<span>Fixes</span>"));
    expect(chat).toContain("bare names resolve");
    expect(chat).not.toContain("Quitting no longer freezes input");
  });

  it("places the banner between the heading and the intro, and drops it when absent", () => {
    const withHero = harness()(doc, "data:image/webp;base64,AAA");
    expect(withHero).toContain('<img class="wn-hero"');
    expect(withHero.indexOf("wn-heading")).toBeLessThan(withHero.indexOf("wn-hero"));
    expect(withHero.indexOf("wn-hero")).toBeLessThan(withHero.indexOf("This release is about Chat"));
    expect(harness()(doc)).not.toContain("wn-hero");
  });

  it("escapes what it renders and never passes raw html through", () => {
    const html = harness()("## A <b>bold</b> claim\n\nText with <script>x</script> in it.\n");
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<b>bold</b>");
    expect(html).toContain("&lt;b&gt;");
  });

  it("treats a continuation line as the same bullet", () => {
    const html = harness()("## T\n\n- one long item\n  that continues here\n- second\n");
    expect(html.match(/whats-new-item/g)).toHaveLength(2);
    expect(html).toContain("one long item that continues here");
  });
});
