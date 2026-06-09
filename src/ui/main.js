const invoke = window.__TAURI__.core.invoke;

const els = {
  message: document.querySelector("#message"),
  state: document.querySelector("#state"),
  lastTick: document.querySelector("#last-tick"),
  nextWakeup: document.querySelector("#next-wakeup"),
  runNow: document.querySelector("#run-now"),
  pauseToggle: document.querySelector("#pause-toggle"),
  autostart: document.querySelector("#autostart"),
  records: document.querySelector("#records"),
  platformStatuses: document.querySelectorAll("[data-platform-status]"),
  loginButtons: document.querySelectorAll("[data-login]"),
};

let paused = false;
let busy = false;

function fmt(value) {
  if (!value) return "-";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

async function call(command, args = {}) {
  return await invoke(command, args);
}

async function refresh() {
  try {
    const data = await call("get_status");
    const status = data.status;
    paused = status.paused;
    els.message.textContent = status.last_message || "-";
    els.state.textContent = paused ? "paused" : status.state;
    els.lastTick.textContent = fmt(status.last_tick);
    els.nextWakeup.textContent = fmt(status.next_wakeup);
    els.pauseToggle.textContent = paused ? "恢复" : "暂停";
    els.autostart.checked = Boolean(data.autostart_enabled);
    renderPlatformSessions(data.platform_sessions || []);
    renderRecords(status.recent_platform_statuses || []);
  } catch (error) {
    els.message.textContent = error.message;
  }
}

function renderPlatformSessions(sessions) {
  const byPlatform = new Map(sessions.map((session) => [session.platform, session]));
  els.platformStatuses.forEach((node) => {
    const platform = node.dataset.platformStatus;
    const session = byPlatform.get(platform);
    const label = session?.label || "未启用";
    node.textContent = label;
    node.className = `platform-status ${statusClass(label)}`;
  });
  els.loginButtons.forEach((button) => {
    const session = byPlatform.get(button.dataset.login);
    button.textContent = session?.label === "已登录" ? "重新登录" : "登录";
  });
}

function statusClass(label) {
  if (label === "已登录") return "ok";
  if (label === "需确认") return "pending";
  if (label === "未登录") return "missing";
  return "unknown";
}

function renderRecords(records) {
  if (!records.length) {
    els.records.innerHTML = '<tr><td colspan="5">暂无任务</td></tr>';
    return;
  }

  els.records.innerHTML = records
    .map((record) => {
      const message = record.last_error || record.remote_url || "";
      return `<tr>
        <td>${escapeHtml(record.platform)}</td>
        <td>${escapeHtml(record.status)}</td>
        <td>${record.attempt_count}</td>
        <td>${escapeHtml(fmt(record.updated_at))}</td>
        <td>${escapeHtml(message)}</td>
      </tr>`;
    })
    .join("");
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

els.runNow.addEventListener("click", async () => {
  if (busy) return;
  busy = true;
  els.runNow.disabled = true;
  try {
    await call("run_now");
    await refresh();
  } catch (error) {
    els.message.textContent = error.message;
  } finally {
    busy = false;
    els.runNow.disabled = false;
  }
});

els.pauseToggle.addEventListener("click", async () => {
  try {
    await call("set_paused", { paused: !paused });
    await refresh();
  } catch (error) {
    els.message.textContent = error.message;
  }
});

els.autostart.addEventListener("change", async () => {
  try {
    await call("set_autostart", { enabled: els.autostart.checked });
    await refresh();
  } catch (error) {
    els.message.textContent = error.message;
    els.autostart.checked = !els.autostart.checked;
  }
});

els.loginButtons.forEach((button) => {
  button.addEventListener("click", async () => {
    try {
      await call("login_platform", { platform: button.dataset.login });
      await refresh();
    } catch (error) {
      els.message.textContent = error.message;
    }
  });
});

document.querySelectorAll("[data-open]").forEach((button) => {
  button.addEventListener("click", async () => {
    try {
      await call("open_dir", { kind: button.dataset.open });
    } catch (error) {
      els.message.textContent = error.message;
    }
  });
});

refresh();
setInterval(refresh, 5000);
