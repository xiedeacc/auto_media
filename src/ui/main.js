const invoke = window.__TAURI__.core.invoke;

const els = {
  message: document.querySelector("#message"),
  state: document.querySelector("#state"),
  lastTick: document.querySelector("#last-tick"),
  recentCheck: document.querySelector("#recent-check"),
  nextWakeup: document.querySelector("#next-wakeup"),
  manualPost: document.querySelector("#manual-post"),
  runNow: document.querySelector("#run-now"),
  pauseToggle: document.querySelector("#pause-toggle"),
  logsButton: document.querySelector("#logs-button"),
  autostart: document.querySelector("#autostart"),
  records: document.querySelector("#records"),
  manualModal: document.querySelector("#manual-modal"),
  logsModal: document.querySelector("#logs-modal"),
  logsContent: document.querySelector("#logs-content"),
  selectImages: document.querySelector("#select-images"),
  imageCount: document.querySelector("#image-count"),
  imageList: document.querySelector("#image-list"),
  manualTitle: document.querySelector("#manual-title"),
  manualText: document.querySelector("#manual-text"),
  submitManual: document.querySelector("#submit-manual"),
  manualPlatforms: document.querySelectorAll('input[name="manual-platform"]'),
  platformStatuses: document.querySelectorAll("[data-platform-status]"),
  loginButtons: document.querySelectorAll("[data-login]"),
};

let paused = false;
let busy = false;
let manualImages = [];
let lastMessage = "";

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
    lastMessage = status.last_message || "";
    els.message.textContent = "";
    els.state.textContent = paused ? "paused" : status.state;
    els.lastTick.textContent = status.last_tick ? "最近状态已更新" : "-";
    els.recentCheck.textContent = fmt(status.last_tick);
    els.nextWakeup.textContent = fmt(status.next_wakeup);
    els.pauseToggle.textContent = paused ? "恢复" : "暂停";
    els.autostart.checked = Boolean(data.autostart_enabled);
    renderPlatformSessions(data.platform_sessions || []);
    renderRecords(status.recent_platform_statuses || []);
  } catch (error) {
    showError(error);
  }
}

function compactMessage(status) {
  const message = status.last_message || "";
  if (!message || message === "-") return "等待任务";
  if (status.state === "manual_publish") return "手动发文已处理，详细内容见日志";
  if (status.state === "error") return "任务异常，点击日志查看详情";
  if (message.length > 42) return `${message.slice(0, 42)}...`;
  return message;
}

function showError(error) {
    lastMessage = error?.message || String(error);
    els.message.textContent = "";
}

function openManualModal() {
  els.manualModal.classList.remove("hidden");
  els.manualTitle.focus();
}

function closeManualModal() {
  els.manualModal.classList.add("hidden");
}

async function openLogsModal() {
  els.logsModal.classList.remove("hidden");
  els.logsContent.textContent = "加载中";
  try {
    const logs = await call("get_logs", { lines: 240 });
    const manual = lastMessage ? `最近消息:\n${lastMessage}\n\n` : "";
    els.logsContent.textContent = `${manual}${logs || "暂无日志"}`;
  } catch (error) {
    els.logsContent.textContent = error.message;
  }
}

function closeLogsModal() {
  els.logsModal.classList.add("hidden");
}

function selectedManualPlatforms() {
  return Array.from(els.manualPlatforms)
    .filter((input) => input.checked)
    .map((input) => input.value);
}

function renderManualImages() {
  els.imageCount.textContent = manualImages.length
    ? `已选择 ${manualImages.length} 张图片`
    : "未选择图片，可按 Ctrl+V 粘贴";
  els.imageList.innerHTML = manualImages
    .map(
      (path, index) => `<div class="image-row">
        <span>${escapeHtml(path)}</span>
        <button type="button" data-remove-image="${index}">移除</button>
      </div>`,
    )
    .join("");

  els.imageList.querySelectorAll("[data-remove-image]").forEach((button) => {
    button.addEventListener("click", () => {
      manualImages.splice(Number(button.dataset.removeImage), 1);
      renderManualImages();
    });
  });
}

async function saveClipboardImage(file, index) {
  const buffer = await file.arrayBuffer();
  const bytes = Array.from(new Uint8Array(buffer));
  const extension = mimeToExtension(file.type);
  const fileName = file.name || `clipboard-${index}.${extension}`;
  return await call("save_pasted_image", { fileName, bytes });
}

function mimeToExtension(mime) {
  if (mime === "image/jpeg") return "jpg";
  if (mime === "image/webp") return "webp";
  return "png";
}

async function handlePaste(event) {
  if (els.manualModal.classList.contains("hidden")) {
    return;
  }

  const items = Array.from(event.clipboardData?.items || []);
  const imageItems = items.filter((item) => item.type.startsWith("image/"));
  if (!imageItems.length) {
    return;
  }

  event.preventDefault();
  try {
    const saved = [];
    for (const [index, item] of imageItems.entries()) {
      const file = item.getAsFile();
      if (file) {
        saved.push(await saveClipboardImage(file, index + 1));
      }
    }
    manualImages = Array.from(new Set([...manualImages, ...saved]));
    renderManualImages();
  } catch (error) {
    showError(error);
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
      const message = record.last_error ? "有错误，点击日志查看" : record.remote_url || "";
      return `<tr>
        <td>${escapeHtml(record.platform)}</td>
        <td>${escapeHtml(statusText(record.status))}</td>
        <td>${record.attempt_count}</td>
        <td>${escapeHtml(fmt(record.updated_at))}</td>
        <td>${escapeHtml(message)}</td>
      </tr>`;
    })
    .join("");
}

function statusText(status) {
  if (status === "success") return "发送成功";
  if (status === "failed") return "发送失败";
  if (status === "publishing") return "发送中";
  if (status === "auth_required") return "需要登录";
  if (status === "skipped_duplicate") return "已跳过";
  return status || "-";
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
    showError(error);
  } finally {
    busy = false;
    els.runNow.disabled = false;
  }
});

els.manualPost.addEventListener("click", openManualModal);
els.logsButton.addEventListener("click", openLogsModal);

document.querySelectorAll("[data-close-manual]").forEach((node) => {
  node.addEventListener("click", closeManualModal);
});

document.querySelectorAll("[data-close-logs]").forEach((node) => {
  node.addEventListener("click", closeLogsModal);
});

window.addEventListener("paste", handlePaste);

els.selectImages.addEventListener("click", async () => {
  try {
    const selected = await call("select_images");
    manualImages = Array.from(new Set([...manualImages, ...selected]));
    renderManualImages();
  } catch (error) {
    showError(error);
  }
});

els.submitManual.addEventListener("click", async () => {
  if (busy) return;
  if (!manualImages.length) {
    lastMessage = "请选择至少一张图片";
    window.alert(lastMessage);
    return;
  }
  const platforms = selectedManualPlatforms();
  if (!platforms.length) {
    lastMessage = "请选择至少一个发布平台";
    window.alert(lastMessage);
    return;
  }
  if (platforms.includes("zhihu") && els.manualText.value.trim().length < 9) {
    lastMessage = "知乎正文至少需要 9 个字";
    window.alert(lastMessage);
    return;
  }

  busy = true;
  els.submitManual.disabled = true;
  try {
    const message = await call("manual_publish", {
      title: els.manualTitle.value,
      text: els.manualText.value,
      imagePaths: manualImages,
      platforms,
    });
    lastMessage = message;
    closeManualModal();
    await refresh();
  } catch (error) {
    showError(error);
  } finally {
    busy = false;
    els.submitManual.disabled = false;
  }
});

els.pauseToggle.addEventListener("click", async () => {
  try {
    await call("set_paused", { paused: !paused });
    await refresh();
  } catch (error) {
    showError(error);
  }
});

els.autostart.addEventListener("change", async () => {
  try {
    await call("set_autostart", { enabled: els.autostart.checked });
    await refresh();
  } catch (error) {
    showError(error);
    els.autostart.checked = !els.autostart.checked;
  }
});

els.loginButtons.forEach((button) => {
  button.addEventListener("click", async () => {
    try {
      await call("login_platform", { platform: button.dataset.login });
      await refresh();
    } catch (error) {
      showError(error);
    }
  });
});

document.querySelectorAll("[data-open]").forEach((button) => {
  button.addEventListener("click", async () => {
    try {
      await call("open_dir", { kind: button.dataset.open });
    } catch (error) {
      showError(error);
    }
  });
});

refresh();
setInterval(refresh, 5000);
