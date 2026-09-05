const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const source = readFileSync(
  join(__dirname, "../assets/propertyInspector/inspector.js"),
  "utf8",
);

function inspector() {
  const sent = [];
  const rendered = [];
  const classes = new Set();
  const timers = new Map();
  let timerId = 0;
  const socket = {
    readyState: 1,
    send: (message) => sent.push(JSON.parse(message)),
  };
  const context = vm.createContext({
    WebSocket: { OPEN: 1 },
    testSocket: socket,
    testElements: {
      discover: {
        classList: {
          contains: (name) => classes.has(name),
          add: (name) => classes.add(name),
          remove: (name) => classes.delete(name),
        },
      },
      codexHome: { value: "/test/home" },
      label: { value: "TEST" },
      refreshMinutes: { value: "5" },
      codexExecutable: { value: "codex" },
    },
    setTimeout: (callback) => {
      timers.set(++timerId, callback);
      return timerId;
    },
    clearTimeout: (id) => timers.delete(id),
  });
  vm.runInContext(source, context);
  vm.runInContext("socket = testSocket; Object.assign(elements, testElements);", context);
  context.renderAccounts = (accounts) => rendered.push(accounts);
  return { context, socket, sent, rendered, classes, timers };
}

function discoveryResponse(context, home) {
  context.handleMessage({
    data: JSON.stringify({
      event: "sendToPropertyInspector",
      payload: {
        event: "accountsDiscovered",
        accounts: [{ codexHome: home }],
      },
    }),
  });
}

test("disconnected discovery does not leave the button busy", () => {
  const { context, socket, sent, classes } = inspector();
  socket.readyState = 3;
  context.requestAccounts();
  assert.equal(sent.length, 0);
  assert.equal(classes.has("is-busy"), false);

  socket.readyState = 1;
  context.requestAccounts();
  assert.equal(sent.length, 1);
  assert.equal(classes.has("is-busy"), true);
});

test("discovery changes queue one fresh scan and discard the outdated result", () => {
  const { context, sent, rendered, classes } = inspector();
  context.requestAccounts();
  context.testElements.codexExecutable.value = "/new/codex";
  context.saveSettings();
  context.requestAccounts();
  context.requestAccounts();
  assert.equal(sent.length, 2);
  assert.equal(sent[1].payload.codexExecutable, "/new/codex");

  discoveryResponse(context, "/old/home");
  assert.equal(rendered.length, 0);
  assert.equal(sent.length, 3);
  assert.equal(sent[2].payload.event, "discoverAccounts");
  assert.equal(classes.has("is-busy"), true);

  discoveryResponse(context, "/new/home");
  assert.equal(rendered.length, 1);
  assert.equal(rendered[0][0].codexHome, "/new/home");
  assert.equal(classes.has("is-busy"), false);
});

test("an immediate save cancels a pending debounced save", () => {
  const { context, sent, timers } = inspector();
  context.scheduleSave();
  context.scheduleSave();
  assert.equal(timers.size, 1);
  context.saveSettings();
  assert.equal(timers.size, 0);
  assert.equal(sent.length, 1);
  assert.equal(sent[0].event, "setSettings");
});
