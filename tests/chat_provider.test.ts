import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";

const source = readFileSync(new URL("../apps/core/assets/app.js", import.meta.url), "utf8");
function harness() {
  const element = () => ({ value: "", textContent: "", disabled: false, hidden: false });
  const context: any = {
    chatSavePending: null, chatSettingsDirty: true, chatSaveButton: element(), chatRemoveKeyButton: element(),
    chatApiKeyInput: { ...element(), value: "new-secret" }, chatSaveStatus: element(),
    chatProviderInput: { value: "openai-compatible" },
    chatBaseUrlInput: { value: "https://example.test/v1" }, chatModelInput: { value: "endpoint-model" },
    posted: [], post(type: string, payload: unknown) { context.posted.push({ type, payload }); },
    setChatSettingsOpen(open: boolean) { context.settingsOpen = open; },
  };
  const save = source.slice(source.indexOf("  function saveChatProvider("), source.indexOf("  // Closed-field label:"));
  const result = source.slice(source.indexOf("    if (state.chatConfigureResult"), source.indexOf("    if (state.chatConfig) {", source.indexOf("    if (state.chatConfigureResult")));
  runInNewContext(save + "\nthis.save = saveChatProvider;", context);
  context.receive = (state: unknown) => { context.state = state; runInNewContext(result, context); };
  return context;
}

describe("provider settings acknowledgment", () => {
  it("dispatches result-only frames to the chat handler", () => {
    const dispatch = source.slice(source.indexOf("if (!Array.isArray(state.rows)"), source.indexOf("applyChatUpdate(state);", source.indexOf("if (!Array.isArray(state.rows)")));
    expect(dispatch).toContain("state.chatConfigureResult");
  });
  it("retains the typed key until success and blocks duplicate saves", () => {
    const ui = harness(); ui.save(); ui.save();
    expect(ui.posted).toHaveLength(1);
    expect(ui.chatApiKeyInput.value).toBe("new-secret");
    expect(ui.chatSaveButton.disabled).toBe(true);
    ui.receive({ chatConfigureResult: { ok: true } });
    expect(ui.chatApiKeyInput.value).toBe("");
    expect(ui.chatSaveStatus.textContent).toBe("Provider settings saved securely.");
    expect(ui.chatSaveButton.disabled).toBe(false);
    expect(ui.chatSettingsDirty).toBe(false);
  });
  it("retains the replacement and reports the specific save error", () => {
    const ui = harness(); ui.save();
    ui.receive({ chatConfigureResult: { ok: false, error: "Could not write provider settings." } });
    expect(ui.chatApiKeyInput.value).toBe("new-secret");
    expect(ui.chatSaveStatus.textContent).toBe("Could not write provider settings.");
    expect(ui.settingsOpen).toBe(true);
    expect(ui.chatApiKeyInput.disabled).toBe(false);
  });
  it("sends explicit removal without the typed secret", () => {
    const ui = harness(); ui.save(true);
    expect(ui.posted[0].payload.clearApiKey).toBe(true);
    expect(ui.posted[0].payload.apiKey).toBe("");
    ui.receive({ chatConfigureResult: { ok: true } });
    expect(ui.chatSaveStatus.textContent).toContain("Saved key removed");
  });
});
