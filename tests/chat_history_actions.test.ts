import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";

// CIQ-158: the model only sees history text, never the action cards. "Open it
// again" had no path, so it scanned G:\ recursively for 43 seconds and the
// request was stopped before it could answer.
const source = readFileSync(new URL("../apps/core/assets/app.js", import.meta.url), "utf8");

type Action = { tool?: string; ok?: boolean; target?: string; summary?: string; preview?: string };
type Turn = { role: string; content: string; actions?: Action[]; incomplete?: boolean; error?: boolean };

function harness() {
  const constants = source.slice(
    source.indexOf("  const CHAT_IPC_MAX_BYTES"),
    source.indexOf("  function chatMessageFits("),
  );
  const history = source.slice(
    source.indexOf("  // One line per action a turn actually ran."),
    source.indexOf("  function renderChatContextSelection("),
  );
  const context: any = { TextEncoder, TextDecoder };
  runInNewContext(`${constants}\n${history}\nthis.buildChatHistory = buildChatHistory;`, context);
  return (turns: Turn[], message = "open it again") =>
    context.buildChatHistory(turns, message, "req-1", [], true) as Turn[];
}

const page = "G:\\vegetable-landing-page\\index.html";

describe("history carries what the previous turn did", () => {
  it("gives a follow-up the path instead of making the model search for it", () => {
    const build = harness();
    const history = build([
      { role: "user", content: "make me a landing page on G:" },
      { role: "assistant", content: "Created the page.", actions: [{ tool: "create_file", ok: true, target: page }] },
    ]);
    expect(history).toHaveLength(2);
    expect(history[1].content).toContain(page);
    expect(history[1].content).toContain("Actions already run in this reply");
  });

  it("keeps one line per path when a long file was written in parts", () => {
    const build = harness();
    const [assistant] = build([
      {
        role: "assistant",
        content: "",
        actions: [
          { tool: "create_file", ok: true, target: page },
          { tool: "create_file", ok: true, target: page },
          { tool: "create_file", ok: true, target: page },
        ],
      },
    ]);
    expect(assistant.content.split(page)).toHaveLength(2);
  });

  it("says why an action failed so the next turn does not retry it blind", () => {
    const build = harness();
    const [assistant] = build([
      {
        role: "assistant",
        content: "",
        actions: [{ tool: "create_folder", ok: false, summary: "That folder already exists" }],
      },
    ]);
    expect(assistant.content).toContain("create_folder failed: That folder already exists");
  });

  it("sends no file contents, only the outcome", () => {
    const build = harness();
    const [assistant] = build([
      {
        role: "assistant",
        content: "",
        actions: [{ tool: "create_file", ok: true, target: page, preview: "SECRET PAGE BODY" }],
      },
    ]);
    expect(assistant.content).toContain(page);
    expect(assistant.content).not.toContain("SECRET PAGE BODY");
  });

  it("keeps the action lines when the reply itself is cut to fit", () => {
    const build = harness();
    const [assistant] = build([
      {
        role: "assistant",
        content: "word ".repeat(4000),
        actions: [{ tool: "create_file", ok: true, target: page }],
      },
    ]);
    expect(assistant.content.length).toBeLessThanOrEqual(6000);
    expect(assistant.content).toContain(page);
  });

  it("still records a turn that was nothing but actions", () => {
    const build = harness();
    const [assistant] = build([
      { role: "assistant", content: "", actions: [{ tool: "open_path", ok: true, target: page }] },
    ]);
    expect(assistant.content).toContain("open_path: " + page);
  });

  it("leaves a user turn alone and skips the unfinished one", () => {
    const build = harness();
    const history = build([
      { role: "user", content: "open it again" },
      { role: "assistant", content: "", incomplete: true, actions: [{ tool: "open_path", ok: true, target: page }] },
    ]);
    expect(history).toHaveLength(1);
    expect(history[0]).toEqual({ role: "user", content: "open it again" });
  });
});
