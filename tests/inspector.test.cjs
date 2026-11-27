const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const assets = join(__dirname, "../assets/propertyInspector");
const source = readFileSync(join(assets, "inspector.js"), "utf8");
const markup = readFileSync(join(assets, "index.html"), "utf8");

class Element extends EventTarget {
  value = "";
  dataset = {};
  options = [];
  classes = new Set();
  classList = {
    toggle: (name, on) => (on ? this.classes.add(name) : this.classes.delete(name)),
  };
  replaceChildren(...options) {
    this.options = options;
  }
  append(option) {
    this.options.push(option);
  }
  get selectedOptions() {
    return this.options.filter((option) => option.value === this.value);
  }
}

class Option {
  constructor(text, value) {
    this.text = text;
    this.value = value;
    this.dataset = {};
  }
}

function inspector(settings = {}) {
  const controls = {};
  for (const [tag] of markup.matchAll(/<[\w-]+\s+[^>]*\bid="[^"]+"[^>]*>/g)) {
    const attributes = Object.fromEntries(
      [...tag.matchAll(/([\w-]+)="([^"]*)"/g)].map(([, key, value]) => [key, value]),
    );
    controls[attributes.id] = Object.assign(new Element(), {
      min: attributes.min,
      max: attributes.max,
      defaultValue: attributes.value || "",
    });
  }
  const sent = [];
  let socket;
  class WebSocket extends EventTarget {
    static OPEN = 1;
    constructor() {
      super();
      socket = this;
      this.readyState = 0;
    }
    send(message) {
      assert.equal(this.readyState, WebSocket.OPEN);
      sent.push(JSON.parse(message));
    }
  }

  const timers = new Map();
  let timerId = 0;
  const inspectorPage = vm.createContext({
    document: { getElementById: (id) => controls[id] },
    Option,
    WebSocket,
    setTimeout(callback) {
      timers.set(++timerId, callback);
      return timerId;
    },
    clearTimeout(id) {
      timers.delete(id);
    },
  });
  vm.runInContext(source, inspectorPage);
  inspectorPage.connectElgatoStreamDeckSocket(
    1234,
    "inspector",
    "registerPropertyInspector",
    "{}",
    JSON.stringify({ action: "action", context: "key", payload: { settings } }),
  );
  return {
    controls,
    sent,
    open() {
      socket.readyState = 1;
      socket.dispatchEvent(new Event("open"));
    },
    close() {
      socket.readyState = 3;
      socket.dispatchEvent(new Event("close"));
    },
    accounts(accounts) {
      socket.dispatchEvent(
        new MessageEvent("message", {
          data: JSON.stringify({
            event: "sendToPropertyInspector",
            payload: { event: "accountsDiscovered", accounts },
          }),
        }),
      );
    },
    dispatch(id, event, value) {
      if (value !== undefined) controls[id].value = value;
      controls[id].dispatchEvent(new Event(event));
    },
    flushTimers() {
      for (const [id, callback] of timers) {
        timers.delete(id);
        callback();
      }
    },
  };
}

test("discovery starts on connection and releases the busy indicator on disconnect", () => {
  const ui = inspector();
  ui.dispatch("discover", "click");
  assert.deepEqual(ui.sent, []);
  assert.equal(ui.controls.discover.classes.has("is-busy"), false);

  ui.open();
  assert.deepEqual(ui.sent, [
    { event: "registerPropertyInspector", uuid: "inspector" },
    {
      action: "action",
      context: "key",
      event: "sendToPlugin",
      payload: { event: "discoverAccounts", codexExecutable: "codex" },
    },
  ]);
  assert.equal(ui.controls.discover.classes.has("is-busy"), true);
  ui.close();
  ui.dispatch("discover", "click");
  ui.dispatch("refresh", "click");
  assert.equal(ui.controls.discover.classes.has("is-busy"), false);
  assert.equal(ui.controls["status-title"].textContent, "Disconnected");
  assert.equal(ui.sent.length, 2);
});

test("edits during discovery queue one scan using the latest executable", () => {
  const ui = inspector();
  ui.open();
  ui.dispatch("codex-executable", "change", "/new/codex");
  ui.dispatch("discover", "click");
  assert.equal(ui.sent.length, 3);
  assert.equal(ui.sent[2].payload.codexExecutable, "/new/codex");

  ui.accounts([{ codexHome: "/old/home" }]);
  assert.equal(
    ui.controls.account.options.some((option) => option.value === "/old/home"),
    false,
  );
  assert.equal(ui.sent.length, 4);
  assert.deepEqual(ui.sent[3].payload, {
    event: "discoverAccounts",
    codexExecutable: "/new/codex",
  });

  ui.accounts([{ codexHome: "/new/home" }]);
  assert.equal(ui.controls.account.options[1].value, "/new/home");
  assert.equal(ui.controls.discover.classes.has("is-busy"), false);
  assert.equal(ui.sent.length, 4);
});

test("an immediate refresh saves the latest edit once, before requesting fresh data", () => {
  const ui = inspector();
  ui.open();
  ui.dispatch("label", "input", "A");
  ui.dispatch("label", "input", "AB");
  ui.dispatch("refresh", "click");
  ui.flushTimers();
  assert.deepEqual(ui.sent.slice(2), [
    {
      context: "key",
      event: "setSettings",
      payload: { codexHome: "", label: "AB", refreshMinutes: 5, codexExecutable: "codex" },
    },
    { context: "key", action: "action", event: "sendToPlugin", payload: { event: "refreshNow" } },
  ]);
});

test("discovery preserves saved labels across plan changes and retains manual homes", () => {
  const ui = inspector({ codexHome: "/manual" });
  ui.open();
  ui.accounts([{ codexHome: "/work", planType: "plus" }]);
  assert.equal(ui.controls.account.selectedOptions[0].text, "Manual — /manual");
  ui.dispatch("account", "change", "/work");
  assert.equal(ui.sent.at(-1).payload.label, "PLUS");
  ui.dispatch("discover", "click");
  const sentBeforeRediscovery = ui.sent.length;
  ui.accounts([{ codexHome: "/work", planType: "pro" }, { codexHome: "/personal" }]);
  assert.equal(ui.controls.label.value, "PLUS");
  assert.equal(ui.sent.length, sentBeforeRediscovery);

  ui.dispatch("label", "input", "Team");
  ui.flushTimers();
  ui.dispatch("account", "change", "/personal");
  assert.equal(ui.sent.at(-1).payload.label, "Team");
});
