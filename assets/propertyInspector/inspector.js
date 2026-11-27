"use strict";

function connectElgatoStreamDeckSocket(port, uuid, registerEvent, _info, actionInfoJson) {
  const { action, context, payload } = JSON.parse(actionInfoJson);
  const socket = new WebSocket(`ws://localhost:${port}`);
  const account = document.getElementById("account");
  const label = document.getElementById("label");
  const refreshMinutes = document.getElementById("refresh-minutes");
  const codexHome = document.getElementById("codex-home");
  const codexExecutable = document.getElementById("codex-executable");
  const discover = document.getElementById("discover");

  let saveTimer;
  let suggestedLabel = "";
  let discovery = "idle";
  const settings = payload.settings || {};
  label.value = settings.label || "";
  codexHome.value = settings.codexHome || "";
  codexExecutable.value = settings.codexExecutable || codexExecutable.defaultValue;
  refreshMinutes.value = interval(settings.refreshMinutes);

  function interval(value) {
    const parsed = Number.parseInt(value, 10);
    if (!Number.isFinite(parsed)) return Number(refreshMinutes.defaultValue);
    return Math.min(Number(refreshMinutes.max), Math.max(Number(refreshMinutes.min), parsed));
  }

  function executable() {
    return codexExecutable.value.trim() || codexExecutable.defaultValue;
  }

  function send(message) {
    if (socket.readyState !== WebSocket.OPEN) return false;
    socket.send(JSON.stringify(message));
    return true;
  }

  function command(payload) {
    return send({ action, context, event: "sendToPlugin", payload });
  }

  function saveSettings() {
    clearTimeout(saveTimer);
    const minutes = interval(refreshMinutes.value);
    refreshMinutes.value = minutes;
    send({
      event: "setSettings",
      context,
      payload: {
        codexHome: codexHome.value.trim(),
        label: label.value.trim(),
        refreshMinutes: minutes,
        codexExecutable: executable(),
      },
    });
  }

  function setDiscovery(state) {
    discovery = state;
    discover.classList.toggle("is-busy", state !== "idle");
  }

  function requestAccounts() {
    if (discovery !== "idle") {
      setDiscovery("queued");
    } else if (command({ event: "discoverAccounts", codexExecutable: executable() })) {
      setDiscovery("pending");
    }
  }

  function suggestLabel(home, planType, replaceableSuggestion) {
    const previous = label.value;
    const automatic =
      !previous || previous === labelFromHome(codexHome.value) || previous === replaceableSuggestion;
    suggestedLabel = (planType || labelFromHome(home) || "CL").toUpperCase();
    if (automatic) label.value = suggestedLabel;
    return label.value !== previous;
  }

  function selectHome() {
    const home = codexHome.value;
    account.value = [...account.options].some((option) => option.value === home) ? home : "";
  }

  function renderAccounts(accounts) {
    const selected = codexHome.value;
    account.replaceChildren(
      new Option(accounts.length ? "Choose an account…" : "No Codex homes found", ""),
    );
    for (const entry of accounts) {
      const directory = labelFromHome(entry.codexHome);
      const identity = entry.email || (entry.signedIn ? "Signed in" : "Signed out");
      const plan = entry.planType ? ` · ${entry.planType}` : "";
      const option = new Option(`${directory} — ${identity}${plan}`, entry.codexHome);
      option.dataset.planType = entry.planType || "";
      account.append(option);
    }

    const current = accounts.find((entry) => entry.codexHome === selected);
    if (current) {
      if (suggestLabel(selected, current.planType)) saveSettings();
    } else if (selected) {
      account.append(new Option(`Manual — ${selected}`, selected));
    }
    selectHome();
  }

  account.addEventListener("change", () => {
    if (!account.value) return;
    suggestLabel(account.value, account.selectedOptions[0]?.dataset.planType, suggestedLabel);
    codexHome.value = account.value;
    saveSettings();
  });
  label.addEventListener("input", () => {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(saveSettings, 180);
  });
  refreshMinutes.addEventListener("change", saveSettings);
  codexHome.addEventListener("change", () => {
    selectHome();
    saveSettings();
  });
  codexExecutable.addEventListener("change", () => {
    saveSettings();
    requestAccounts();
  });
  discover.addEventListener("click", requestAccounts);
  document.getElementById("refresh").addEventListener("click", () => {
    saveSettings();
    if (command({ event: "refreshNow" })) {
      setStatus("refreshing", "Refreshing", "Starting a fresh Codex app-server request");
    }
  });

  socket.addEventListener("open", () => {
    send({ event: registerEvent, uuid });
    requestAccounts();
  });
  socket.addEventListener("close", () => {
    clearTimeout(saveTimer);
    setDiscovery("idle");
    setStatus("error", "Disconnected", "OpenDeck closed the inspector connection");
  });
  socket.addEventListener("message", (event) => {
    let message;
    try {
      message = JSON.parse(event.data);
    } catch {
      return;
    }
    if (message?.event !== "sendToPropertyInspector") return;

    const payload = message.payload || {};
    if (payload.event === "accountsDiscovered") {
      const queued = discovery === "queued";
      setDiscovery("idle");
      if (queued) requestAccounts();
      else renderAccounts(payload.accounts || []);
    } else if (payload.event === "status") {
      const detail = payload.refreshedAt
        ? `${payload.message} · ${formatTimestamp(payload.refreshedAt)}`
        : payload.message;
      setStatus(payload.state, statusHeading(payload.state), detail);
    }
  });
}

function setStatus(state, title, detail) {
  document.getElementById("status-dot").dataset.state = state;
  document.getElementById("status-title").textContent = title;
  document.getElementById("status-detail").textContent = detail || "";
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
  return home.trim().replace(/\/+$/, "").split("/").pop().replace(/^\.+/, "");
}

function formatTimestamp(seconds) {
  const date = new Date(seconds * 1000);
  return `updated ${date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}
