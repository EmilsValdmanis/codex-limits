"use strict";

let socket;
let context;
let action;
let settings = {
  codexHome: "",
  label: "",
  refreshMinutes: 5,
  codexExecutable: "codex",
};
let saveTimer;
let lastSuggestedLabel = "";

const elements = {};

function connectElgatoStreamDeckSocket(
  inPort,
  inPropertyInspectorUUID,
  inRegisterEvent,
  _inInfo,
  inActionInfo,
) {
  const actionInfo = JSON.parse(inActionInfo);
  context = actionInfo.context;
  action = actionInfo.action;
  settings = { ...settings, ...(actionInfo.payload.settings || {}) };

  bindElements();
  populateSettings();
  bindEvents();

  socket = new WebSocket(`ws://localhost:${inPort}`);
  socket.addEventListener("open", () => {
    socket.send(JSON.stringify({ event: inRegisterEvent, uuid: inPropertyInspectorUUID }));
    elements.connectionLed.classList.add("is-online");
    elements.connectionLed.title = "Connected";
    requestAccounts();
  });
  socket.addEventListener("message", handleMessage);
  socket.addEventListener("close", () => {
    elements.connectionLed.classList.remove("is-online");
    setStatus("error", "Disconnected", "OpenDeck closed the inspector connection");
  });
}

function bindElements() {
  elements.account = document.getElementById("account");
  elements.label = document.getElementById("label");
  elements.refreshMinutes = document.getElementById("refresh-minutes");
  elements.codexHome = document.getElementById("codex-home");
  elements.codexExecutable = document.getElementById("codex-executable");
  elements.discover = document.getElementById("discover");
  elements.refresh = document.getElementById("refresh");
  elements.connectionLed = document.getElementById("connection-led");
  elements.statusDot = document.getElementById("status-dot");
  elements.statusTitle = document.getElementById("status-title");
  elements.statusDetail = document.getElementById("status-detail");
}

function populateSettings() {
  elements.label.value = settings.label || "";
  elements.refreshMinutes.value = clampRefresh(settings.refreshMinutes);
  elements.codexHome.value = settings.codexHome || "";
  elements.codexExecutable.value = settings.codexExecutable || "codex";
}

function bindEvents() {
  elements.account.addEventListener("change", () => {
    if (!elements.account.value) return;
    const previousHome = elements.codexHome.value;
    const canSuggestLabel =
      !elements.label.value ||
      elements.label.value === labelFromHome(previousHome) ||
      elements.label.value === lastSuggestedLabel;
    elements.codexHome.value = elements.account.value;
    lastSuggestedLabel = defaultTileName(
      elements.account.selectedOptions[0]?.dataset.planType,
      elements.account.value,
    );
    if (canSuggestLabel) {
      elements.label.value = lastSuggestedLabel;
    }
    saveSettings();
  });

  elements.label.addEventListener("input", scheduleSave);
  elements.refreshMinutes.addEventListener("change", saveSettings);
  elements.codexHome.addEventListener("change", () => {
    selectCurrentHome();
    saveSettings();
  });
  elements.codexExecutable.addEventListener("change", () => {
    saveSettings();
    requestAccounts();
  });
  elements.discover.addEventListener("click", requestAccounts);
  elements.refresh.addEventListener("click", () => {
    saveSettings();
    sendToPlugin({ event: "refreshNow" });
    setStatus("refreshing", "Refreshing", "Starting a fresh Codex app-server request");
  });
}

function handleMessage(event) {
  let message;
  try {
    message = JSON.parse(event.data);
  } catch {
    return;
  }
  if (message.event !== "sendToPropertyInspector") return;

  const payload = message.payload || {};
  if (payload.event === "accountsDiscovered") {
    renderAccounts(payload.accounts || []);
  } else if (payload.event === "status") {
    const detail = payload.refreshedAt
      ? `${payload.message} · ${formatTimestamp(payload.refreshedAt)}`
      : payload.message;
    setStatus(payload.state, statusHeading(payload.state), detail);
  }
}

function renderAccounts(accounts) {
  const selected = elements.codexHome.value;
  elements.account.replaceChildren();

  if (!accounts.length) {
    elements.account.append(new Option("No Codex homes found", ""));
  } else {
    elements.account.append(new Option("Choose an account…", ""));
    for (const account of accounts) {
      const directory = labelFromHome(account.codexHome);
      const identity = account.email || (account.signedIn ? "Signed in" : "Signed out");
      const plan = account.planType ? ` · ${account.planType}` : "";
      const option = new Option(`${directory} — ${identity}${plan}`, account.codexHome);
      option.dataset.planType = account.planType || "";
      elements.account.append(option);
    }
  }

  if (selected && !accounts.some((account) => account.codexHome === selected)) {
    elements.account.append(new Option(`Manual — ${selected}`, selected));
  }
  selectCurrentHome();
  const current = accounts.find((account) => account.codexHome === selected);
  if (current) {
    lastSuggestedLabel = defaultTileName(current.planType, current.codexHome);
    if (
      !elements.label.value ||
      elements.label.value === labelFromHome(selected)
    ) {
      elements.label.value = lastSuggestedLabel;
      saveSettings();
    }
  }
  elements.discover.classList.remove("is-busy");
}

function selectCurrentHome() {
  const home = elements.codexHome.value;
  elements.account.value = [...elements.account.options].some(
    (option) => option.value === home,
  )
    ? home
    : "";
}

function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(saveSettings, 180);
}

function saveSettings() {
  if (!socket || socket.readyState !== WebSocket.OPEN) return;
  settings = {
    codexHome: elements.codexHome.value.trim(),
    label: elements.label.value.trim(),
    refreshMinutes: clampRefresh(elements.refreshMinutes.value),
    codexExecutable: elements.codexExecutable.value.trim() || "codex",
  };
  elements.refreshMinutes.value = settings.refreshMinutes;
  socket.send(
    JSON.stringify({
      event: "setSettings",
      context,
      payload: settings,
    }),
  );
}

function requestAccounts() {
  if (elements.discover.classList.contains("is-busy")) return;
  elements.discover.classList.add("is-busy");
  sendToPlugin({ event: "discoverAccounts" });
}

function sendToPlugin(payload) {
  if (!socket || socket.readyState !== WebSocket.OPEN) return;
  socket.send(
    JSON.stringify({
      action,
      event: "sendToPlugin",
      context,
      payload,
    }),
  );
}

function setStatus(state, title, detail) {
  elements.statusDot.dataset.state = state;
  elements.statusTitle.textContent = title;
  elements.statusDetail.textContent = detail || "";
}

function statusHeading(state) {
  return (
    {
      ready: "Up to date",
      refreshing: "Refreshing",
      stale: "Showing stale data",
      error: "Needs attention",
      unconfigured: "Not configured",
      loading: "Loading",
    }[state] || "Status"
  );
}

function labelFromHome(home) {
  return (home || "")
    .replace(/\/+$/, "")
    .split("/")
    .pop()
    .replace(/^\./, "");
}

function defaultTileName(planType, home) {
  return (planType || labelFromHome(home) || "CL").toUpperCase();
}

function clampRefresh(value) {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed)) return 5;
  return Math.min(1440, Math.max(1, parsed));
}

function formatTimestamp(seconds) {
  const date = new Date(seconds * 1000);
  return `updated ${date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}
